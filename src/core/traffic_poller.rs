//! 进程内累计流量 + 瞬时速率：每秒轮询 Clash `GET /connections`，**只统计走代理的连接**。
//!
//! - `/traffic` 在 sing-box 上是 NDJSON 流，不适合一次性 JSON 解析。
//! - 按连接 id 记住上次累计字节，出口为 direct / block 的连接不计入（见 `traffic.rs`）。
//! - 本 poller 用「相对上次采样的增量」写入累计，并在 INTERVAL=1s 时把增量当作 B/s。
//! - **start/stop/restart 均不清零累计**；停止时速率置 0。
//! - 连接明细不在此轮询：仅管理页打开时由 `GET /api/connections` 按需拉取。

use crate::app::{AppEvent, AppState};
use crate::core::traffic::{ConnectionsSnapshot, ProxyTrafficCounter};
use std::sync::Arc;
use std::time::Duration;

const INTERVAL: Duration = Duration::from_secs(1);

/// 启动长期后台任务（只应调用一次）。
pub fn start(state: Arc<AppState>) {
    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_millis(800))
            .no_proxy()
            .build()
        {
            Ok(c) => c,
            Err(_) => return,
        };

        // 只统计走代理的连接；直连流量不计入速度和累计
        let mut counter = ProxyTrafficCounter::default();
        let mut was_running = false;
        // 核心进程连续多少拍不在了；reload/TUN 回退期间会短暂没有子进程，3 拍（约 3s）才判定意外退出。
        let mut dead_ticks: u32 = 0;
        // 每 30 次 tick（约 30s）把会话增量并入今日历史，供侧栏「流量统计」使用。
        let mut history_ticks: u32 = 0;

        let mut ticker = tokio::time::interval(INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let running = state.status.read().await.running;
            if !running {
                counter.reset();
                if was_running {
                    // 停止瞬间：速率归零，累计保留；只在边沿 emit，避免空转刷 WS
                    {
                        let mut st = state.status.write().await;
                        st.up_rate = 0;
                        st.down_rate = 0;
                    }
                    let (up, down) = {
                        let st = state.status.read().await;
                        (st.traffic_up, st.traffic_down)
                    };
                    state.emit(AppEvent::Traffic {
                        up,
                        down,
                        up_rate: 0,
                        down_rate: 0,
                    });
                    // 停止时立刻落盘，避免进程异常退出丢今日数据。
                    crate::core::traffic_history::record_tick(&state).await;
                    history_ticks = 0;
                    was_running = false;
                }
                continue;
            }
            was_running = true;

            if state.core.is_running().await {
                dead_ticks = 0;
            } else {
                dead_ticks += 1;
                if dead_ticks >= 3 {
                    dead_ticks = 0;
                    on_core_crashed(&state).await;
                    continue;
                }
            }

            let (port, secret) = {
                let cfg = state.config.read().await;
                (cfg.clash_api_port, cfg.api_secret.clone())
            };
            let Some(snap) = fetch_connections(&client, port, &secret).await else {
                continue;
            };
            let (du, dd) = counter.tick(&snap);

            let (up, down) = {
                let mut st = state.status.write().await;
                st.traffic_up = st.traffic_up.saturating_add(du);
                st.traffic_down = st.traffic_down.saturating_add(dd);
                // INTERVAL=1s → 本秒字节 ≈ B/s
                st.up_rate = du;
                st.down_rate = dd;
                (st.traffic_up, st.traffic_down)
            };
            state.emit(AppEvent::Traffic {
                up,
                down,
                up_rate: du,
                down_rate: dd,
            });

            history_ticks = history_ticks.wrapping_add(1);
            if history_ticks >= 30 {
                history_ticks = 0;
                crate::core::traffic_history::record_tick(&state).await;
            }
        }
    });
}

/// 核心意外退出：状态置停止 → 关闭系统代理（否则系统仍指向无人监听的端口，断网）
/// → 配置错误时弹窗提示。
async fn on_core_crashed(state: &AppState) {
    let (mode, current_node) = {
        let mut st = state.status.write().await;
        st.running = false;
        (st.mode.clone(), st.current_node.clone())
    };
    let fatal = state.core.take_last_fatal();
    state.log_with(
        "core",
        "error",
        format!(
            "sing-box 核心意外退出，状态已置为停止。原因: {}",
            fatal.as_deref().unwrap_or("未捕获到 FATAL 输出")
        ),
    );
    state.emit(AppEvent::Status { running: false, mode, current_node });

    // 同步关闭系统代理开关并持久化，界面开关与实际状态保持一致；恢复后用户手动重新打开。
    let sysproxy_cfg = {
        let mut c = state.config.write().await;
        if c.system_proxy {
            c.system_proxy = false;
            let _ = crate::config::manager::save_app_config(&state.data_dir, &c);
            Some(c.enable_tun)
        } else {
            None
        }
    };
    if let Some(enable_tun) = sysproxy_cfg {
        match crate::system::sysproxy::disable() {
            Ok(_) => state.log_with("config", "warn", "核心已退出，已自动关闭系统代理"),
            Err(e) => state.log_with("config", "error", format!("自动关闭系统代理失败: {e}")),
        }
        state.emit(AppEvent::Config { system_proxy: false, enable_tun });
    }

    if let Some(f) = fatal.filter(|f| crate::core::manager::is_config_error(f)) {
        crate::system::msgbox::show_error(
            "YAP-XFISH — 核心因配置错误退出",
            &format!(
                "sing-box 核心因配置错误退出，代理已停止，系统代理已关闭。\n\n{f}\n\n请检查节点 / 规则 / 设置后在面板中重新启动。"
            ),
        );
    }
}

async fn fetch_connections(
    client: &reqwest::Client,
    port: u16,
    secret: &str,
) -> Option<ConnectionsSnapshot> {
    let url = format!("http://127.0.0.1:{port}/connections");
    let mut req = client.get(&url);
    if !secret.is_empty() {
        req = req.header("Authorization", format!("Bearer {secret}"));
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<ConnectionsSnapshot>().await.ok()
}
