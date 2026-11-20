use axum::{
    extract::{Path, State},
    Json,
};
use std::sync::Arc;

use crate::app::{gen_id, AppConfig, AppEvent, AppState};
use crate::config::model::{AppProfile, ProxyMode, RouteRule};
use crate::error::{AppError, AppResult};
use crate::subscription::model::{Subscription, UpdateInterval, UpdateStatus};

#[derive(serde::Serialize)]
pub struct StatusResp {
    pub running: bool,
    pub mode: ProxyMode,
    pub current_node: Option<String>,
    /// 当前节点显示名（与节点列表 `name` 一致；找不到时回退为 id）。
    pub current_node_name: Option<String>,
    pub traffic_up: u64,
    pub traffic_down: u64,
    /// 上行瞬时速率 B/s（最近一拍；停止时为 0）。
    pub up_rate: u64,
    /// 下行瞬时速率 B/s。
    pub down_rate: u64,
    pub node_count: usize,
    /// 当前进程是否以管理员身份运行（TUN 需要）。
    pub elevated: bool,
    /// 当前进程工作集内存（MB），用于空闲态内存剖析。
    pub mem_mb: f32,
    /// 系统代理是否启用。
    pub system_proxy: bool,
    /// TUN 是否启用（配置项；实际生效还依赖提权）。
    pub enable_tun: bool,
}

pub async fn status(State(state): State<Arc<AppState>>) -> AppResult<Json<StatusResp>> {
    let (stored_running, mode, current_node, traffic_up, traffic_down, up_rate, down_rate) = {
        let st = state.status.read().await;
        (
            st.running,
            st.mode,
            st.current_node.clone(),
            st.traffic_up,
            st.traffic_down,
            st.up_rate,
            st.down_rate,
        )
    };
    let mem = crate::system::mem::memory_info();
    let cfg = state.config.read().await.clone();
    let running = stored_running && state.core.version(&cfg).await.is_some();
    // 把 current_node（id）解析成节点列表中的 name，仪表盘显示名称与节点页一致。
    let current_node_name = {
        let p = state.profile.read().await;
        current_node.as_ref().map(|id| {
            p.nodes
                .iter()
                .find(|n| &n.id == id)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| id.clone())
        })
    };
    Ok(Json(StatusResp {
        running,
        mode,
        current_node,
        current_node_name,
        traffic_up,
        traffic_down,
        up_rate,
        down_rate,
        node_count: state.visible_profile().await.nodes.len(),
        elevated: crate::system::admin::is_elevated(),
        mem_mb: (mem.working_set_bytes as f32) / (1024.0 * 1024.0),
        system_proxy: cfg.system_proxy,
        enable_tun: cfg.enable_tun,
    }))
}

pub async fn get_config(State(state): State<Arc<AppState>>) -> AppResult<Json<AppConfig>> {
    let cfg = state.config.read().await;
    Ok(Json(cfg.clone()))
}

pub async fn get_profile(State(state): State<Arc<AppState>>) -> AppResult<Json<AppProfile>> {
    // 仅返回启用订阅 + 手动节点，停用订阅的节点不展示。
    Ok(Json(state.visible_profile().await))
}

#[derive(serde::Deserialize)]
pub struct SelectReq {
    pub node_id: String,
}

pub async fn select_node(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SelectReq>,
) -> AppResult<Json<serde_json::Value>> {
    let _mutation = state.mutation.lock().await;
    let previous = {
        let mut p = state.profile.write().await;
        if !p.nodes.iter().any(|n| n.id == req.node_id) {
            return Err(AppError(anyhow::anyhow!("节点不存在: {}", req.node_id)));
        }
        // 找到目标节点的 name 用于日志
        let node_name = p
            .nodes
            .iter()
            .find(|n| n.id == req.node_id)
            .map(|n| n.name.clone())
            .unwrap_or_default();
        let prev = p.selected_node.clone();
        p.selected_node = Some(req.node_id.clone());
        crate::config::manager::save_profile(&state.data_dir, &p)?;
        state.log(
            "info",
            format!(
                "切换节点: {} -> {} ({})",
                prev.as_deref().unwrap_or("无"),
                req.node_id,
                node_name
            ),
        );
        prev
    };
    if state.core.is_running().await {
        let cfg = state.config.read().await.clone();
        if let Err(error) = state.core.select_node(&req.node_id, &cfg).await {
            let mut p = state.profile.write().await;
            p.selected_node = previous.clone();
            let _ = crate::config::manager::save_profile(&state.data_dir, &p);
            return Err(AppError(anyhow::anyhow!("切换 Mihomo 节点失败: {error}")));
        }
    }
    {
        let mut st = state.status.write().await;
        st.current_node = Some(req.node_id.clone());
        state.emit(AppEvent::Status {
            running: st.running,
            mode: st.mode,
            current_node: st.current_node.clone(),
        });
    }
    state.broadcast_profile().await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(serde::Deserialize)]
pub struct SetDetourReq {
    /// 前置节点 id；空字符串或 null 表示清除链式代理。
    pub detour: Option<String>,
}

/// 为节点设置链式前置代理（Mihomo `dialer-proxy`）。
pub async fn set_node_detour(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<SetDetourReq>,
) -> AppResult<Json<serde_json::Value>> {
    let _mutation = state.mutation.lock().await;
    let previous = state.profile.read().await.clone();
    let detour_raw = req
        .detour
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    {
        let mut p = state.profile.write().await;
        let exit = p
            .nodes
            .iter()
            .find(|n| n.id == id)
            .cloned()
            .ok_or_else(|| AppError(anyhow::anyhow!("节点不存在: {}", id)))?;

        if let Some(ref d) = detour_raw {
            if d == &id {
                return Err(AppError(anyhow::anyhow!("前置代理不能是自身")));
            }
            let entry = p
                .nodes
                .iter()
                .find(|n| &n.id == d)
                .ok_or_else(|| AppError(anyhow::anyhow!("前置节点不存在: {}", d)))?;

            // UDP/QUIC 出口经 detour 拨号在多数前置上会直接超时（实测 hy2 链式）。
            let exit_k = exit.kind.to_ascii_lowercase();
            if matches!(
                exit_k.as_str(),
                "hysteria" | "hysteria2" | "hy2" | "tuic" | "wireguard" | "wg"
            ) {
                return Err(AppError(anyhow::anyhow!(
                    "出口「{}」是 {}（UDP/QUIC），无法经前置链式拨号。请换 VLESS/VMess/Trojan/SS 等 TCP 出口，或不要给该节点设前置。",
                    exit.name,
                    exit.kind
                )));
            }

            if !crate::config::mihomo::detour_is_safe(&p.nodes, &id, d) {
                return Err(AppError(anyhow::anyhow!(
                    "设置该前置会形成代理环，已拒绝"
                )));
            }

            let _ = entry; // 已校验存在
        }

        let name = exit.name.clone();
        if let Some(n) = p.nodes.iter_mut().find(|n| n.id == id) {
            n.detour = Some(detour_raw.clone().unwrap_or_default());
            if let Some(extra) = n.extra.as_mut().and_then(serde_json::Value::as_object_mut) {
                extra.remove("dialer-proxy");
                extra.remove("detour");
            }
        }
        crate::config::manager::save_profile(&state.data_dir, &p)?;
        let detour_label = detour_raw
            .as_ref()
            .and_then(|d| {
                p.nodes
                    .iter()
                    .find(|n| &n.id == d)
                    .map(|n| format!("{} ({})", n.name, d))
            })
            .unwrap_or_else(|| "无".into());
        state.log(
            "info",
            format!("设置链式代理: {} → 前置={}", name, detour_label),
        );
    }

    if let Err(error) = regen_config(&state).await {
        *state.profile.write().await = previous.clone();
        crate::config::manager::save_profile(&state.data_dir, &previous)?;
        return Err(error);
    }
    state.broadcast_profile().await;
    Ok(Json(serde_json::json!({
        "ok": true,
        "id": id,
        "detour": detour_raw,
    })))
}

pub async fn set_mode(
    State(state): State<Arc<AppState>>,
    Json(mode): Json<ProxyMode>,
) -> AppResult<Json<serde_json::Value>> {
    let _mutation = state.mutation.lock().await;
    let previous = {
        let mut p = state.profile.write().await;
        let prev = format!("{:?}", p.mode).to_lowercase();
        let previous = p.mode;
        p.mode = mode;
        crate::config::manager::save_profile(&state.data_dir, &p)?;
        let new_label = format!("{:?}", mode).to_lowercase();
        state.log("info", format!("切换代理模式: {} -> {}", prev, new_label));
        previous
    };
    if let Err(error) = regen_config(&state).await {
        let mut profile = state.profile.write().await;
        profile.mode = previous;
        crate::config::manager::save_profile(&state.data_dir, &profile)?;
        return Err(AppError(anyhow::anyhow!("切换 Mihomo 模式失败: {error}")));
    }
    let mut status = state.status.write().await;
    status.mode = mode;
    state.emit(AppEvent::Status {
        running: status.running,
        mode,
        current_node: status.current_node.clone(),
    });
    drop(status);
    state.broadcast_profile().await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn core_start(State(state): State<Arc<AppState>>) -> AppResult<Json<serde_json::Value>> {
    let _mutation = state.mutation.lock().await;
    let p = state.visible_profile().await;
    let cfg = state.config.read().await;
    let node_name = p
        .selected_node
        .as_ref()
        .and_then(|id| p.nodes.iter().find(|n| &n.id == id).map(|n| n.name.clone()));
    let elevated = crate::system::admin::is_elevated();
    state.log_with(
        "core",
        "info",
        format!(
            "启动核心: 模式={:?} 节点={} TUN={} 提权={} 节点数={}",
            p.mode,
            node_name.as_deref().unwrap_or("无"),
            cfg.enable_tun,
            elevated,
            p.nodes.len()
        ),
    );
    match state.core.start(&p, &cfg).await {
        Ok(_) => {
            {
                let mut st = state.status.write().await;
                st.running = true;
                st.mode = p.mode;
                st.current_node = p.selected_node.clone();
            }
            // 上下行累计跨 start/stop/restart 保留（poller 按 core 增量累加）
            state.emit(AppEvent::Status {
                running: true,
                mode: p.mode,
                current_node: p.selected_node.clone(),
            });
            state.log_with("core", "info", "Mihomo 核心已启动");
            Ok(Json(serde_json::json!({ "ok": true })))
        }
        Err(e) => {
            let mut st = state.status.write().await;
            st.running = false;
            state.log_with("core", "error", format!("核心启动失败: {e}"));
            Err(e)
        }
    }
}

pub async fn core_stop(State(state): State<Arc<AppState>>) -> AppResult<Json<serde_json::Value>> {
    let _mutation = state.mutation.lock().await;
    state.log_with("core", "info", "正在停止核心…");
    let stop_result = state.core.stop().await;
    {
        let mut st = state.status.write().await;
        st.running = false;
        // 保留 traffic_up / traffic_down：停止后仪表盘继续显示本会话累计
    }
    {
        let st = state.status.read().await;
        state.emit(AppEvent::Status {
            running: false,
            mode: st.mode.clone(),
            current_node: st.current_node.clone(),
        });
    }
    state.log_with("core", "info", "Mihomo 核心已停止");
    crate::core::traffic_history::flush().await;
    stop_result?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn core_restart(
    State(state): State<Arc<AppState>>,
) -> AppResult<Json<serde_json::Value>> {
    let _mutation = state.mutation.lock().await;
    let p = state.visible_profile().await;
    let cfg = state.config.read().await;
    state.log_with("core", "info", "正在重启核心…");
    state.core.restart(&p, &cfg).await?;
    {
        let mut st = state.status.write().await;
        st.running = true;
    }
    // 上下行累计跨 restart 保留
    state.emit(AppEvent::Status {
        running: true,
        mode: p.mode,
        current_node: p.selected_node.clone(),
    });
    state.log_with("core", "info", "Mihomo 核心已重启");
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---------- 节点延迟测试（阶段 6） ----------

pub async fn test_node_latency(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    start_latency_batch(state, Some(vec![id])).await
}

#[derive(Default, serde::Deserialize)]
pub struct TestLatencyReq {
    #[serde(default)]
    pub node_ids: Option<Vec<String>>,
}

pub async fn test_all_latency(
    State(state): State<Arc<AppState>>,
    body: axum::body::Bytes,
) -> AppResult<Json<serde_json::Value>> {
    // 保留旧客户端的空 body 调用；新客户端按当前订阅页签传入节点 ID。
    let req: TestLatencyReq = if body.is_empty() {
        TestLatencyReq::default()
    } else {
        serde_json::from_slice(&body)?
    };
    start_latency_batch(state, req.node_ids).await
}

async fn start_latency_batch(
    state: Arc<AppState>,
    node_ids: Option<Vec<String>>,
) -> AppResult<Json<serde_json::Value>> {
    let batch = crate::latency::prepare(&state, node_ids).await?;
    let count = batch.len();
    tokio::spawn(async move {
        if let Err(e) = batch.run(&state).await {
            state.log_with("latency", "error", format!("测速失败: {e}"));
        }
    });
    Ok(Json(serde_json::json!({ "ok": true, "async": true, "count": count })))
}

// ---------- 订阅（阶段 5：持久化 + 调度 + 解析 + 自动更新） ----------

pub async fn list_subscriptions(
    State(state): State<Arc<AppState>>,
) -> AppResult<Json<Vec<Subscription>>> {
    let subs = state.subscriptions.read().await;
    Ok(Json(subs.clone()))
}

#[derive(serde::Deserialize)]
pub struct AddSubscriptionReq {
    /// 订阅显示名；留空时后端按 URL 规则自动生成（去掉 http(s):// 后前 7 个字符）。
    #[serde(default)]
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub interval: UpdateInterval,
    /// 添加时是否启用。缺省 false：避免用户已有启用订阅时新增的
    /// 第二/第三个订阅被自动启用；用户可在列表里手动开启。
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub user_agent: Option<String>,
}

/// 订阅名未填写时的默认值：去掉 URL 前导 `https://` / `http://` 后取前 7 个字符。
/// 剩余不足 7 个字符则取全部；去掉协议后为空则回退为 `"订阅"`。
fn default_subscription_name_from_url(url: &str) -> String {
    let rest = url
        .trim()
        .strip_prefix("https://")
        .or_else(|| url.trim().strip_prefix("http://"))
        .unwrap_or_else(|| url.trim());
    let rest = rest.trim_start_matches('/');
    let name: String = rest.chars().take(7).collect();
    if name.is_empty() {
        "订阅".to_string()
    } else {
        name
    }
}

pub async fn add_subscription(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddSubscriptionReq>,
) -> AppResult<Json<Subscription>> {
    let _mutation = state.mutation.lock().await;
    crate::subscription::scheduler::validate_interval(&req.interval)?;
    // 名字留空：先按 URL 生成占位名，首次更新时换成订阅响应头里的机场名。
    let name_auto = req.name.trim().is_empty();
    let name = if name_auto {
        default_subscription_name_from_url(&req.url)
    } else {
        req.name.trim().to_string()
    };
    let sub = Subscription {
        id: gen_id(),
        name,
        url: req.url,
        interval: req.interval,
        enabled: req.enabled,
        user_agent: req.user_agent,
        last_updated: None,
        next_retry_at: None,
        last_status: UpdateStatus::Idle,
        last_message: String::new(),
        node_count: 0,
        name_auto,
        userinfo: None,
    };
    {
        let mut subs = state.subscriptions.write().await;
        subs.push(sub.clone());
        crate::subscription::manager::save_subscriptions(&state.data_dir, &subs)?;
    }
    state.log_with(
        "sub",
        "info",
        format!(
            "添加订阅: name={} url={} enabled={} interval={:?}",
            sub.name, sub.url, sub.enabled, sub.interval
        ),
    );
    // 广播新列表，避免前端缓存与后端不一致。
    {
        let subs = state.subscriptions.read().await;
        state.emit(AppEvent::SubscriptionsRefresh {
            subscriptions: subs.clone(),
        });
    }
    Ok(Json(sub))
}

pub async fn delete_subscription(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let _mutation = state.mutation.lock().await;
    let subscriptions_before = state.subscriptions.read().await.clone();
    let profile_before = state.profile.read().await.clone();
    let removed = {
        let mut subs = state.subscriptions.write().await;
        let removed = subs.iter().find(|s| s.id == id).cloned();
        subs.retain(|s| s.id != id);
        crate::subscription::manager::save_subscriptions(&state.data_dir, &subs)?;
        removed
    };
    let removed_count = {
        let mut p = state.profile.write().await;
        let before = p.nodes.len();
        p.nodes.retain(|n| n.subscription_id.as_deref() != Some(id.as_str()));
        let removed = before - p.nodes.len();
        if let Some(sel) = &p.selected_node {
            if !p.nodes.iter().any(|n| &n.id == sel) {
                p.selected_node = None;
            }
        }
        crate::config::manager::save_profile(&state.data_dir, &p)?;
        removed
    };
    let visible = state.visible_profile().await;
    let cfg = state.config.read().await.clone();
    if let Err(error) = state.core.update_provider(&visible, &cfg).await {
        *state.subscriptions.write().await = subscriptions_before.clone();
        *state.profile.write().await = profile_before.clone();
        let _ = crate::subscription::manager::save_subscriptions(
            &state.data_dir,
            &subscriptions_before,
        );
        let _ = crate::config::manager::save_profile(&state.data_dir, &profile_before);
        let restored = state.visible_profile().await;
        let _ = state.core.update_provider(&restored, &cfg).await;
        return Err(AppError(anyhow::anyhow!(
            "刷新 Mihomo Provider 失败，已恢复订阅和节点数据: {error}"
        )));
    }
    sync_visible_selection(&state).await?;
    state.log_with("sub", "info",
        format!(
            "删除订阅: name={} 移除节点数={}",
            removed.as_ref().map(|s| s.name.as_str()).unwrap_or("?"),
            removed_count
        ),
    );
    state.broadcast_profile().await;
    {
        let subs = state.subscriptions.read().await;
        state.emit(AppEvent::SubscriptionsRefresh { subscriptions: subs.clone() });
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

/// 手动触发单个订阅更新（异步执行，进度通过 WebSocket 推送）。
pub async fn update_subscription_now(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    {
        let subs = state.subscriptions.read().await;
        if !subs.iter().any(|s| s.id == id) {
            return Err(AppError(anyhow::anyhow!("订阅不存在: {}", id)));
        }
    }
    let st = state.clone();
    tokio::spawn(async move {
        let _ = crate::subscription::manager::update_subscription(&st, &id).await;
    });
    Ok(Json(serde_json::json!({ "ok": true, "async": true })))
}

/// 手动触发「所有已启用订阅」更新。
pub async fn update_all_subscriptions(
    State(state): State<Arc<AppState>>,
) -> AppResult<Json<serde_json::Value>> {
    let st = state.clone();
    tokio::spawn(async move {
        let _ = crate::subscription::manager::update_all_enabled(&st).await;
    });
    Ok(Json(serde_json::json!({ "ok": true, "async": true })))
}

#[derive(serde::Deserialize)]
pub struct UpdateSubscriptionReq {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub interval: Option<UpdateInterval>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub user_agent: Option<String>,
}

/// 更新订阅设置（名称 / URL / 周期 / 启用 / UA）。
pub async fn update_subscription_settings(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateSubscriptionReq>,
) -> AppResult<Json<Subscription>> {
    let _mutation = state.mutation.lock().await;
    if let Some(interval) = &req.interval {
        crate::subscription::scheduler::validate_interval(interval)?;
    }
    let (updated, previous, enabled_changed) = {
        let mut subs = state.subscriptions.write().await;
        let sub = match subs.iter_mut().find(|s| s.id == id) {
            Some(s) => s,
            None => return Err(AppError(anyhow::anyhow!("订阅不存在: {}", id))),
        };
        let previous = sub.clone();
        let was_enabled = sub.enabled;
        if let Some(v) = req.name {
            if v != sub.name {
                sub.name_auto = false;
            }
            sub.name = v;
        }
        if let Some(v) = req.url {
            sub.url = v;
        }
        if let Some(v) = req.interval {
            sub.interval = v;
        }
        if let Some(v) = req.enabled {
            sub.enabled = v;
        }
        if let Some(v) = req.user_agent {
            sub.user_agent = Some(v);
        }
        let out = sub.clone();
        let enabled_changed = was_enabled != sub.enabled;
        crate::subscription::manager::save_subscriptions(&state.data_dir, &subs)?;
        (out, previous, enabled_changed)
    };
    if enabled_changed {
        let visible = state.visible_profile().await;
        let cfg = state.config.read().await.clone();
        if let Err(error) = state.core.update_provider(&visible, &cfg).await {
            {
                let mut subs = state.subscriptions.write().await;
                if let Some(sub) = subs.iter_mut().find(|sub| sub.id == id) {
                    *sub = previous.clone();
                }
                let _ = crate::subscription::manager::save_subscriptions(&state.data_dir, &subs);
            }
            let restored = state.visible_profile().await;
            let _ = state.core.update_provider(&restored, &cfg).await;
            return Err(AppError(anyhow::anyhow!(
                "刷新 Mihomo Provider 失败，已恢复订阅设置: {error}"
            )));
        }
        sync_visible_selection(&state).await?;
    }
    // 启用/停用变化会立即改变可见节点并刷新运行中的 Provider。
    state.broadcast_profile().await;
    {
        let subs = state.subscriptions.read().await;
        state.emit(AppEvent::SubscriptionsRefresh { subscriptions: subs.clone() });
    }
    Ok(Json(updated))
}

async fn sync_visible_selection(state: &Arc<AppState>) -> AppResult<crate::config::model::AppProfile> {
    let visible = state.visible_profile().await;
    let selection_changed = {
        let mut profile = state.profile.write().await;
        if profile.selected_node != visible.selected_node {
            profile.selected_node = visible.selected_node.clone();
            crate::config::manager::save_profile(&state.data_dir, &profile)?;
            true
        } else {
            false
        }
    };
    if selection_changed {
        let mut status = state.status.write().await;
        status.current_node = visible.selected_node.clone();
        state.emit(AppEvent::Status {
            running: status.running,
            mode: status.mode,
            current_node: status.current_node.clone(),
        });
    }
    Ok(visible)
}

// ---------- 规则 ----------

pub async fn list_rules(
    State(state): State<Arc<AppState>>,
) -> AppResult<Json<Vec<RouteRule>>> {
    let p = state.profile.read().await;
    Ok(Json(p.rules.clone()))
}

/// 返回常用规则预设（阶段 6：便于前端一键添加典型分流规则）。
pub async fn list_rule_presets(
    State(_state): State<Arc<AppState>>,
) -> AppResult<Json<Vec<RouteRule>>> {
    Ok(Json(crate::rules::presets()))
}

/// 规则变更后重载 Mihomo 配置；核心未运行时只更新待启动文件。
async fn regen_config(state: &Arc<AppState>) -> AppResult<()> {
    let cfg = state.config.read().await.clone();
    let p = state.visible_profile().await;
    state.core.reload_safe(&p, &cfg).await
}

#[derive(serde::Deserialize)]
pub struct AddRuleReq {
    pub name: String,
    pub r#type: String,
    pub payload: String,
    pub outbound: String,
}

pub async fn add_rule(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddRuleReq>,
) -> AppResult<Json<RouteRule>> {
    let _mutation = state.mutation.lock().await;
    let r#type = req.r#type.trim().to_string();
    let mut payload = req.payload.trim().to_string();
    let outbound = req.outbound.trim().to_string();
    if r#type.is_empty() || payload.is_empty() || outbound.is_empty() {
        return Err(AppError(anyhow::anyhow!(
            "规则类型 / 匹配内容 / 出站不能为空"
        )));
    }
    // IP 规则：去掉端口、补前缀长度，保存规范形式；非法地址直接给出明确提示
    if matches!(r#type.to_ascii_lowercase().as_str(), "ip_cidr" | "ip") {
        payload = crate::config::mihomo::normalize_ip_cidr(&payload).ok_or_else(|| {
            AppError(anyhow::anyhow!(
                "无效的 IP / CIDR：{payload}。请填 1.2.3.4、1.2.3.0/24 或 IPv6 地址；端口请另建「端口」规则。"
            ))
        })?;
    }
    // 预检：能否渲染为合法 Mihomo 规则，避免将无效规则写入 profile。
    let probe = crate::config::model::RouteRule {
        id: "probe".into(),
        name: req.name.clone(),
        r#type: r#type.clone(),
        payload: payload.clone(),
        outbound: outbound.clone(),
    };
    let rendered = {
        use crate::config::model::{AppProfile, ProxyMode};
        let mut p = AppProfile::default();
        p.mode = ProxyMode::Global;
        p.rules.push(probe.clone());
        // 用当前可见节点保证 outbound tag 存在于配置里（direct/block 始终有）
        let vis = state.visible_profile().await;
        p.nodes = vis.nodes;
        p.selected_node = vis.selected_node;
        let cfg = state.config.read().await.clone();
        crate::config::mihomo::render(&p, &cfg)?
    };
    let user_n = rendered
        .get("rules")
        .and_then(|a| a.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    if user_n <= 1 {
        return Err(AppError(anyhow::anyhow!(
            "无法识别的规则：type={} payload={}。域名请填 host（如 ip.skk.moe），不要写完整 URL 前缀以外无法解析的内容；或改用 domain_suffix。",
            r#type,
            payload
        )));
    }

    let mut rule = RouteRule {
        id: gen_id(),
        name: if req.name.trim().is_empty() {
            payload.clone()
        } else {
            req.name
        },
        r#type,
        payload,
        outbound,
    };
    let rules_before = {
        let mut p = state.profile.write().await;
        let before = p.rules.clone();
        if let Some(existing) = p
            .rules
            .iter_mut()
            .find(|r| r.r#type == rule.r#type && r.payload.eq_ignore_ascii_case(&rule.payload))
        {
            // 同类型同内容已存在：只改出站，不重复添加（扩展里重复点击也安全）
            existing.outbound = rule.outbound.clone();
            rule = existing.clone();
        } else if crate::config::mihomo::is_rule_set_rule(&rule) {
            p.rules.push(rule.clone());
        } else {
            // 精确规则排在 GeoSite/GeoIP 前面，否则会先被「国内直连」命中而不生效
            let at = p
                .rules
                .iter()
                .position(crate::config::mihomo::is_rule_set_rule)
                .unwrap_or(p.rules.len());
            p.rules.insert(at, rule.clone());
        }
        crate::config::manager::save_profile(&state.data_dir, &p)?;
        state.log(
            "info",
            format!(
                "添加路由规则: [{}] {} → {}",
                rule.r#type, rule.payload, rule.outbound
            ),
        );
        before
    };
    // 规则错误会导致核心拒绝配置，这里用严格 reload 并把错误返回前端
    let cfg = state.config.read().await.clone();
    let p = state.visible_profile().await;
    if state.core.is_running().await {
        if let Err(e) = state.core.reload(&p, &cfg).await {
            // 回滚到添加前的规则列表，避免配置一直坏
            {
                let mut pr = state.profile.write().await;
                pr.rules = rules_before;
                let _ = crate::config::manager::save_profile(&state.data_dir, &pr);
            }
            return Err(AppError(anyhow::anyhow!(
                "规则已拒绝（核心重载失败）: {e}"
            )));
        }
    } else if let Err(e) = state.core.write_config_only(&p, &cfg).await {
        let mut pr = state.profile.write().await;
        pr.rules = rules_before;
        let _ = crate::config::manager::save_profile(&state.data_dir, &pr);
        return Err(AppError(anyhow::anyhow!("写入配置失败: {e}")));
    }
    state.broadcast_profile().await;
    Ok(Json(rule))
}

pub async fn delete_rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    let _mutation = state.mutation.lock().await;
    let previous = state.profile.read().await.rules.clone();
    {
        let mut p = state.profile.write().await;
        p.rules.retain(|r| r.id != id);
        crate::config::manager::save_profile(&state.data_dir, &p)?;
    }
    if let Err(error) = regen_config(&state).await {
        let mut profile = state.profile.write().await;
        profile.rules = previous;
        crate::config::manager::save_profile(&state.data_dir, &profile)?;
        return Err(error);
    }
    state.broadcast_profile().await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---------- 配置更新 ----------

pub async fn update_config(
    State(state): State<Arc<AppState>>,
    Json(cfg): Json<AppConfig>,
) -> AppResult<Json<serde_json::Value>> {
    state.apply_config(cfg.clone()).await?;
    state.log(
        "info",
        format!(
            "保存设置: TUN={} DNS接管={} stack={} mtu={} 系统代理={} 自启={} 代理端口={} Web端口={} MihomoAPI端口={}",
            cfg.enable_tun,
            cfg.tun_dns_mode,
            cfg.tun_stack,
            cfg.tun_mtu,
            cfg.system_proxy,
            cfg.autostart,
            cfg.proxy_port,
            cfg.web_port,
            cfg.clash_api_port
        ),
    );
    // 开启 TUN 但未提权时提示：Mihomo 需要管理员权限才能创建虚拟网卡。
    if cfg.enable_tun && !crate::system::admin::is_elevated() {
        state.log(
            "warn",
            "已开启 TUN，但当前未以管理员身份运行，Mihomo 无法创建虚拟网卡。请通过托盘菜单「以管理员身份运行」重新启动。",
        );
    }
    Ok(Json(serde_json::json!({
        "ok": true,
        "elevated": crate::system::admin::is_elevated(),
        "system_proxy": cfg.system_proxy,
        "enable_tun": cfg.enable_tun,
    })))
}

// ---------- 管理员提权 / 调试 ----------

/// 以管理员身份重新启动自身（触发 UAC）。提权成功时后台会退出并由新实例接管。
pub async fn admin_elevate(
    State(state): State<Arc<AppState>>,
) -> AppResult<Json<serde_json::Value>> {
    let _mutation = state.mutation.lock().await;
    if crate::system::admin::is_elevated() {
        return Ok(Json(serde_json::json!({ "ok": true, "already_elevated": true })));
    }
    // 兜底：提权前把内存中的最新 config 同步落盘，确保新实例启动时读到用户
    // 想要的状态（如 enable_tun=true）。即使前端守卫路径漏写盘也不影响。
    {
        let cfg = state.config.read().await;
        let _ = crate::config::manager::save_app_config(&state.data_dir, &cfg);
    }
    let ok = crate::system::admin::elevate_and_restart();
    if ok {
        // 提权实例已启动，本实例退出（让出单实例互斥锁已在内部完成）。
        let _ = state.core.stop().await;
        std::process::exit(0);
    }
    Ok(Json(serde_json::json!({
        "ok": false,
        "message": "提权失败或被取消",
    })))
}

/// 返回当前进程内存占用（调试 / 空闲态内存剖析）。
pub async fn mem_debug(
    State(_state): State<Arc<AppState>>,
) -> AppResult<Json<serde_json::Value>> {
    let m = crate::system::mem::memory_info();
    Ok(Json(serde_json::json!({
        "working_set_mb": (m.working_set_bytes as f64) / (1024.0 * 1024.0),
        "private_mb": (m.private_bytes as f64) / (1024.0 * 1024.0),
    })))
}


// ---------- 连接明细（按需；仅管理页打开时由前端轮询，后台不常驻） ----------

/// Return the Mihomo `GET /connections` payload used by the existing connections view.
/// 不在 traffic_poller 中解析连接列表，避免无人观看时浪费 CPU/内存。
pub async fn list_connections(
    State(state): State<Arc<AppState>>,
) -> AppResult<Json<serde_json::Value>> {
    let running = state.status.read().await.running;
    if !running {
        return Ok(Json(serde_json::json!({
            "connections": [],
            "downloadTotal": 0,
            "uploadTotal": 0,
            "memory": 0,
        })));
    }
    let (port, secret) = {
        let cfg = state.config.read().await;
        (cfg.clash_api_port, cfg.api_secret.clone())
    };
    let api = crate::core::api::MihomoApi::new(port, secret)?;
    let v = api
        .connections()
        .await
        .map_err(|e| AppError(anyhow::anyhow!("Mihomo 连接列表请求失败: {e}")))?;
    Ok(Json(v))
}

/// 断开连接：id 为 None 时断开全部。
async fn close_connections_inner(state: &AppState, id: Option<&str>) -> AppResult<()> {
    if !state.status.read().await.running {
        return Ok(());
    }
    let (port, secret) = {
        let cfg = state.config.read().await;
        (cfg.clash_api_port, cfg.api_secret.clone())
    };
    crate::core::api::MihomoApi::new(port, secret)?
        .close_connection(id)
        .await
        .map_err(|e| AppError(anyhow::anyhow!("断开 Mihomo 连接失败: {e}")))
}

pub async fn close_all_connections(
    State(state): State<Arc<AppState>>,
) -> AppResult<Json<serde_json::Value>> {
    close_connections_inner(&state, None).await?;
    state.log("info", "已断开全部连接");
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn close_connection(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    close_connections_inner(&state, Some(&id)).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---------- 版本号 ----------

/// 返回当前构建版本（从 Cargo.toml 编译期注入）。
pub async fn get_version() -> AppResult<Json<serde_json::Value>> {
    let version = env!("CARGO_PKG_VERSION");
    Ok(Json(serde_json::json!({ "version": version })))
}

/// 流量历史：最近 N 天（默认 30）每日上行/下行。
pub async fn traffic_daily(
    State(state): State<Arc<AppState>>,
) -> AppResult<Json<serde_json::Value>> {
    // 每次都先 flush 当前累计，让今日数据新
    crate::core::traffic_history::record_tick(&state).await;
    let days = crate::core::traffic_history::last_n_days(
        crate::core::traffic_history::DAILY_HISTORY_DAYS,
    )
    .await;
    let (today, today_up, today_down) =
        crate::core::traffic_history::snapshot_today().await;
    Ok(Json(serde_json::json!({
        "today": today,
        "today_up": today_up,
        "today_down": today_down,
        "days": days,
    })))
}

#[cfg(test)]
mod tests;
