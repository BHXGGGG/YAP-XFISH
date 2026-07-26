//! 进程内累计流量 + 瞬时速率：轮询 Clash `GET /connections` 的 totals，按增量累加。
//!
//! - `/traffic` 在 sing-box 上是 NDJSON 流，不适合一次性 JSON 解析。
//! - `/connections` 的 uploadTotal/downloadTotal 在 **每次启动 sing-box 后从 0 计**。
//! - 本 poller 用「相对上次采样的增量」写入累计，并在 INTERVAL=1s 时把增量当作 B/s。
//! - **start/stop/restart 均不清零累计**；停止时速率置 0。
//! - 连接明细不在此轮询：仅管理页打开时由 `GET /api/connections` 按需拉取。

use crate::app::{AppEvent, AppState};
use crate::core::traffic::TrafficTotals;
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

        let mut last_core_up: Option<u64> = None;
        let mut last_core_down: Option<u64> = None;
        let mut was_running = false;
        // 每 30 次 tick（约 30s）把会话增量并入今日历史，供侧栏「流量统计」使用。
        let mut history_ticks: u32 = 0;

        let mut ticker = tokio::time::interval(INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let running = state.status.read().await.running;
            if !running {
                last_core_up = None;
                last_core_down = None;
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

            let (port, secret) = {
                let cfg = state.config.read().await;
                (cfg.clash_api_port, cfg.api_secret.clone())
            };
            let Some(tot) = fetch_totals(&client, port, &secret).await else {
                continue;
            };

            let du = delta(last_core_up, tot.up);
            let dd = delta(last_core_down, tot.down);
            last_core_up = Some(tot.up);
            last_core_down = Some(tot.down);

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

fn delta(last: Option<u64>, now: u64) -> u64 {
    match last {
        None => now,
        Some(prev) if now >= prev => now - prev,
        Some(_) => now,
    }
}

async fn fetch_totals(
    client: &reqwest::Client,
    port: u16,
    secret: &str,
) -> Option<TrafficTotals> {
    let url = format!("http://127.0.0.1:{port}/connections");
    let mut req = client.get(&url);
    if !secret.is_empty() {
        req = req.header("Authorization", format!("Bearer {secret}"));
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<TrafficTotals>().await.ok()
}
