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
    let st = state.status.read().await;
    let mem = crate::system::mem::memory_info();
    let cfg = state.config.read().await;
    // 把 current_node（id）解析成节点列表中的 name，仪表盘显示名称与节点页一致。
    let current_node_name = {
        let p = state.profile.read().await;
        st.current_node.as_ref().map(|id| {
            p.nodes
                .iter()
                .find(|n| &n.id == id)
                .map(|n| n.name.clone())
                .unwrap_or_else(|| id.clone())
        })
    };
    Ok(Json(StatusResp {
        running: st.running,
        mode: st.mode.clone(),
        current_node: st.current_node.clone(),
        current_node_name,
        traffic_up: st.traffic_up,
        traffic_down: st.traffic_down,
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
    {
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
    }
    {
        let mut st = state.status.write().await;
        st.current_node = Some(req.node_id.clone());
    }
    {
        let st = state.status.read().await;
        state.emit(AppEvent::Status {
            running: st.running,
            mode: st.mode.clone(),
            current_node: Some(req.node_id.clone()),
        });
    }
    // 实时推送最新配置模型，前端无需刷新即可看到选中高亮。
    state.broadcast_profile().await;
    if state.core.is_running().await {
        let p = state.visible_profile().await;
        let cfg = state.config.read().await;
        state.core.reload(&p, &cfg).await?;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn set_mode(
    State(state): State<Arc<AppState>>,
    Json(mode): Json<ProxyMode>,
) -> AppResult<Json<serde_json::Value>> {
    {
        let mut p = state.profile.write().await;
        let prev = format!("{:?}", p.mode).to_lowercase();
        p.mode = mode;
        crate::config::manager::save_profile(&state.data_dir, &p)?;
        let new_label = format!("{:?}", mode).to_lowercase();
        state.log("info", format!("切换代理模式: {} -> {}", prev, new_label));
    }
    {
        let st = state.status.read().await;
        state.emit(AppEvent::Status {
            running: st.running,
            mode,
            current_node: st.current_node.clone(),
        });
    }
    state.broadcast_profile().await;
    if state.core.is_running().await {
        let p = state.visible_profile().await;
        let cfg = state.config.read().await;
        state.core.reload(&p, &cfg).await?;
    }
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn core_start(State(state): State<Arc<AppState>>) -> AppResult<Json<serde_json::Value>> {
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
            state.emit(AppEvent::Status {
                running: true,
                mode: p.mode,
                current_node: p.selected_node.clone(),
            });
            state.log_with("core", "info", "sing-box 核心已启动");
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
    state.log_with("core", "info", "正在停止核心…");
    state.core.stop().await?;
    {
        let mut st = state.status.write().await;
        st.running = false;
    }
    {
        let st = state.status.read().await;
        state.emit(AppEvent::Status {
            running: false,
            mode: st.mode.clone(),
            current_node: st.current_node.clone(),
        });
    }
    state.log_with("core", "info", "sing-box 核心已停止");
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn core_restart(
    State(state): State<Arc<AppState>>,
) -> AppResult<Json<serde_json::Value>> {
    let p = state.visible_profile().await;
    let cfg = state.config.read().await;
    state.log_with("core", "info", "正在重启核心…");
    state.core.restart(&p, &cfg).await?;
    {
        let mut st = state.status.write().await;
        st.running = true;
    }
    state.emit(AppEvent::Status {
        running: true,
        mode: p.mode,
        current_node: p.selected_node.clone(),
    });
    state.log_with("core", "info", "sing-box 核心已重启");
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---------- 节点延迟测试（阶段 6） ----------

pub async fn test_node_latency(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    {
        let p = state.profile.read().await;
        if !p.nodes.iter().any(|n| n.id == id) {
            return Err(AppError(anyhow::anyhow!("节点不存在: {}", id)));
        }
    }
    let st = state.clone();
    tokio::spawn(async move {
        let _ = crate::latency::test_one(&st, &id).await;
    });
    Ok(Json(serde_json::json!({ "ok": true, "async": true })))
}

pub async fn test_all_latency(
    State(state): State<Arc<AppState>>,
) -> AppResult<Json<serde_json::Value>> {
    let st = state.clone();
    tokio::spawn(async move {
        let _ = crate::latency::test_all(&st).await;
    });
    Ok(Json(serde_json::json!({ "ok": true, "async": true })))
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
    let name = {
        let n = req.name.trim();
        if n.is_empty() {
            default_subscription_name_from_url(&req.url)
        } else {
            n.to_string()
        }
    };
    let sub = Subscription {
        id: gen_id(),
        name,
        url: req.url,
        interval: req.interval,
        enabled: req.enabled,
        user_agent: req.user_agent,
        last_updated: None,
        last_status: UpdateStatus::Idle,
        last_message: String::new(),
        node_count: 0,
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
    let updated = {
        let mut subs = state.subscriptions.write().await;
        let sub = match subs.iter_mut().find(|s| s.id == id) {
            Some(s) => s,
            None => return Err(AppError(anyhow::anyhow!("订阅不存在: {}", id))),
        };
        if let Some(v) = req.name {
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
        crate::subscription::manager::save_subscriptions(&state.data_dir, &subs)?;
        out
    };
    // 启用/停用变化会立即改变「可见节点」集合，广播最新配置模型让前端节点列表实时刷新，无需刷新网页。
    state.broadcast_profile().await;
    {
        let subs = state.subscriptions.read().await;
        state.emit(AppEvent::SubscriptionsRefresh { subscriptions: subs.clone() });
    }
    Ok(Json(updated))
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

/// 规则变更后重新生成核心配置：核心运行时 reload（带回滚），未运行时仅写 config.json。
/// best-effort：sing-box 缺失等错误不阻断接口（profile 已持久化）。
async fn regen_config(state: &Arc<AppState>) {
    let cfg = state.config.read().await.clone();
    let p = state.visible_profile().await;
    if state.core.is_running().await {
        let _ = state.core.reload_safe(&p, &cfg).await;
    } else {
        let _ = state.core.write_config_only(&p, &cfg).await;
    }
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
    let rule = RouteRule {
        id: gen_id(),
        name: req.name,
        r#type: req.r#type,
        payload: req.payload,
        outbound: req.outbound,
    };
    {
        let mut p = state.profile.write().await;
        p.rules.push(rule.clone());
        crate::config::manager::save_profile(&state.data_dir, &p)?;
    }
    regen_config(&state).await;
    state.broadcast_profile().await;
    Ok(Json(rule))
}

pub async fn delete_rule(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> AppResult<Json<serde_json::Value>> {
    {
        let mut p = state.profile.write().await;
        p.rules.retain(|r| r.id != id);
        crate::config::manager::save_profile(&state.data_dir, &p)?;
    }
    regen_config(&state).await;
    state.broadcast_profile().await;
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ---------- 配置更新 ----------

pub async fn update_config(
    State(state): State<Arc<AppState>>,
    Json(cfg): Json<AppConfig>,
) -> AppResult<Json<serde_json::Value>> {
    let (tun_changed, autostart_changed, sysproxy_changed, proxy_port_changed, web_port_changed) = {
        let cur = state.config.read().await;
        (
            cur.enable_tun != cfg.enable_tun,
            cur.autostart != cfg.autostart,
            cur.system_proxy != cfg.system_proxy || cur.proxy_port != cfg.proxy_port,
            cur.proxy_port != cfg.proxy_port,
            cur.web_port != cfg.web_port,
        )
    };
    {
        let mut c = state.config.write().await;
        *c = cfg.clone();
    }
    crate::config::manager::save_app_config(&state.data_dir, &cfg)?;
    state.log(
        "info",
        format!(
            "保存设置: TUN={} 系统代理={} 自启={} 代理端口={} Web端口={} ClashAPI端口={}",
            cfg.enable_tun,
            cfg.system_proxy,
            cfg.autostart,
            cfg.proxy_port,
            cfg.web_port,
            cfg.clash_api_port
        ),
    );
    if web_port_changed {
        state.log(
            "warn",
            format!("Web 端口变更为 {}，需要重启后台生效", cfg.web_port),
        );
    }
    // 开机启动状态变化则同步注册表（HKCU Run），托盘菜单与设置页两个入口保持一致。
    if autostart_changed {
        match crate::system::autostart::set_autostart(cfg.autostart) {
            Ok(_) => {
                let msg = if cfg.autostart {
                    "已启用开机启动"
                } else {
                    "已关闭开机启动"
                };
                state.log_with("config", "info", msg);
            }
            Err(e) => state.log_with("config", "error", format!("开机启动设置失败: {e}")),
        };
    }
    // 系统代理开关 / 端口变化 → 同步 WinINET。
    if sysproxy_changed {
        let r = if cfg.system_proxy {
            crate::system::sysproxy::enable(cfg.proxy_port)
        } else {
            crate::system::sysproxy::disable()
        };
        match r {
            Ok(_) => {
                let msg = if cfg.system_proxy {
                    format!("已启用系统代理 → 127.0.0.1:{}", cfg.proxy_port)
                } else {
                    "已关闭系统代理".into()
                };
                state.log_with("config", "info", msg);
            }
            Err(e) => state.log_with("config", "error", format!("系统代理设置失败: {e}")),
        }
    }
    // 开启 TUN 但未提权时提示：sing-box 需要管理员权限才能创建虚拟网卡。
    if tun_changed && cfg.enable_tun && !crate::system::admin::is_elevated() {
        state.log(
            "warn",
            "已开启 TUN，但当前未以管理员身份运行，sing-box 无法创建虚拟网卡。请通过托盘菜单「以管理员身份运行」重新启动。",
        );
    }
    // 端口/协议等变更后重建核心配置：核心运行时 reload 立即生效，未运行时仅写 config.json。
    // proxy_port 变化也走这条路径（sysproxy 已同步端口，核心 inbound 也需更新）。
    let _ = proxy_port_changed;
    regen_config(&state).await;
    // 广播配置变更，前端右上角指示灯 / 设置勾选可即时刷新。
    state.emit(AppEvent::Config {
        system_proxy: cfg.system_proxy,
        enable_tun: cfg.enable_tun,
    });
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
    if crate::system::admin::is_elevated() {
        return Ok(Json(serde_json::json!({ "ok": true, "already_elevated": true })));
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
