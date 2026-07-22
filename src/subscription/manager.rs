use crate::app::{AppEvent, AppState};
use crate::config::manager as cfgmgr;
use crate::config::model::{AppProfile, Node};
use crate::error::{AppError, AppResult};
use crate::subscription::fetcher;
use crate::subscription::model::{Subscription, UpdateInterval, UpdateStatus};
use crate::subscription::parser;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;

// ---------------- 持久化 ----------------

const SUBS_FILE: &str = "subscriptions.json";

fn subs_file(data_dir: &Path) -> std::path::PathBuf {
    data_dir.join(SUBS_FILE)
}

pub fn load_subscriptions(data_dir: &Path) -> AppResult<Vec<Subscription>> {
    let p = subs_file(data_dir);
    if p.exists() {
        let s = std::fs::read_to_string(&p)?;
        let v: Vec<Subscription> = serde_json::from_str(&s)?;
        Ok(v)
    } else {
        Ok(Vec::new())
    }
}

pub fn save_subscriptions(data_dir: &Path, subs: &[Subscription]) -> AppResult<()> {
    let p = subs_file(data_dir);
    let s = serde_json::to_string_pretty(subs)?;
    std::fs::write(&p, s)?;
    Ok(())
}

// ---------------- 并发去重 ----------------

async fn try_begin(state: &AppState, id: &str) -> bool {
    let mut set = state.updating.lock().await;
    if set.contains(id) {
        false
    } else {
        set.insert(id.to_string());
        true
    }
}

async fn end(state: &AppState, id: &str) {
    let mut set = state.updating.lock().await;
    set.remove(id);
}

// ---------------- 公共入口 ----------------

/// 更新单个订阅：下载 → 解析 → 校验 → 合并 → 应用（reload 或仅写配置）→ 失败回滚。
/// 通过 WebSocket 推送进度事件（status 字段为 updating/success/failed）。
pub async fn update_subscription(state: &AppState, id: &str) -> AppResult<()> {
    if !try_begin(state, id).await {
        // 同一订阅正在更新，跳过避免并发
        return Ok(());
    }
    let result = update_subscription_inner(state, id).await;
    end(state, id).await;
    result
}

/// 更新所有「非手动」的订阅（含启用与停用）。供"全部更新"按钮 / 启动时调用。
/// 用户的预期："全部更新"就是真的"全部"，不管是否启用——只是把已经停用的
/// 节点更新到最新，停用状态本身保持不变（不自动启用）。
pub async fn update_all_enabled(state: &AppState) -> AppResult<()> {
    let ids: Vec<String> = {
        let subs = state.subscriptions.read().await;
        subs.iter()
            .filter(|s| s.interval != UpdateInterval::Manual)
            .map(|s| s.id.clone())
            .collect()
    };
    state.log_with("sub", "info",
        format!("触发全部更新: 共 {} 个订阅（含停用）", ids.len()),
    );
    for id in ids {
        let _ = update_subscription(state, &id).await;
    }
    Ok(())
}

async fn update_subscription_inner(state: &AppState, id: &str) -> AppResult<()> {
    // 1. 取出订阅基本信息（只读，尽快释放锁）
    let (sub_url, sub_ua, sub_id) = {
        let subs = state.subscriptions.read().await;
        match subs.iter().find(|s| s.id == id) {
            Some(s) => (s.url.clone(), s.user_agent.clone(), s.id.clone()),
            None => return Err(AppError(anyhow::anyhow!("订阅不存在: {}", id))),
        }
    };

    emit(state, &sub_id, "updating", 10, "开始下载订阅", None, None);
    state.log_with("sub", "info", format!("开始下载订阅: {}", sub_url));

    // 2. 下载
    let raw = match fetcher::fetch_raw(&sub_url, sub_ua.as_deref()).await {
        Ok(r) => {
            state.log_with("sub", "info", format!("下载成功: {} 字节", r.len()));
            r
        }
        Err(e) => {
            let msg = format!("下载失败: {e}");
            state.log_with("sub", "error", msg.clone());
            record(state, &sub_id, UpdateStatus::Failed, 0, &msg).await;
            emit(state, &sub_id, "failed", 100, &msg, Some(0), None);
            return Err(AppError(anyhow::anyhow!(msg)));
        }
    };

    emit(state, &sub_id, "updating", 40, "解析节点列表", None, None);

    // 3. 解析 + 校验
    let parsed = match parser::parse(&raw) {
        Ok(nodes) if !nodes.is_empty() => {
            state.log_with("sub", "info", format!("解析成功: {} 个节点", nodes.len()));
            nodes
        }
        Ok(_) => {
            let msg = "订阅解析结果为空".to_string();
            state.log_with("sub", "error", msg.clone());
            record(state, &sub_id, UpdateStatus::Failed, 0, &msg).await;
            emit(state, &sub_id, "failed", 100, &msg, Some(0), None);
            return Err(AppError(anyhow::anyhow!(msg)));
        }
        Err(e) => {
            let msg = format!("解析失败: {e}");
            state.log_with("sub", "error", msg.clone());
            record(state, &sub_id, UpdateStatus::Failed, 0, &msg).await;
            emit(state, &sub_id, "failed", 100, &msg, Some(0), None);
            return Err(AppError(anyhow::anyhow!(msg)));
        }
    };

    // 4. 生成稳定节点 ID 并打上订阅标记（保证重复更新时选中节点稳定）
    let mut new_nodes: Vec<Node> = parsed
        .into_iter()
        .map(|mut n| {
            n.id = stable_node_id(&sub_id, &n);
            n.subscription_id = Some(sub_id.clone());
            n
        })
        .collect();

    emit(
        state,
        &sub_id,
        "updating",
        60,
        &format!("合并 {} 个节点", new_nodes.len()),
        None,
        None,
    );

    // 5. 合并进 profile：先快照，再移除旧节点、追加新节点、保证选中有效
    let snapshot = {
        let mut p = state.profile.write().await;
        let snap = p.clone();
        p.nodes.retain(|n| n.subscription_id.as_deref() != Some(id));
        p.nodes.append(&mut new_nodes);
        ensure_selection(&mut p);
        if let Err(e) = cfgmgr::save_profile(&state.data_dir, &p) {
            // 持久化失败：回滚内存并上报
            *p = snap;
            let msg = format!("保存配置失败: {e}");
            record(state, &sub_id, UpdateStatus::Failed, 0, &msg).await;
            emit(state, &sub_id, "failed", 100, &msg, Some(0), None);
            return Err(AppError(e.into()));
        }
        snap
    };

    emit(state, &sub_id, "updating", 80, "应用配置并重启核心", None, None);

    // 6. 应用：核心在运行时 reload（带配置回滚），未运行时仅写配置待下次启动
    let app_cfg = state.config.read().await.clone();
    let profile_now = state.visible_profile().await;
    let running = state.core.is_running().await;

    let apply_result = if running {
        state.core.reload_safe(&profile_now, &app_cfg).await
    } else {
        state.core.write_config_only(&profile_now, &app_cfg).await
    };

    match apply_result {
        Ok(()) => {
            let cnt = profile_count_for(&sub_id, &profile_now);
            record(state, &sub_id, UpdateStatus::Success, cnt, "更新成功").await;
            emit(state, &sub_id, "success", 100, "更新成功", Some(cnt), Some(chrono::Local::now().to_rfc3339()));
            // 实时推送最新节点列表，前端无需刷新即可看到订阅拉取的节点。
            state.broadcast_profile().await;
            Ok(())
        }
        Err(e) => {
            // 回滚 profile 到快照（内存 + 磁盘）
            {
                let mut p = state.profile.write().await;
                *p = snapshot;
                let _ = cfgmgr::save_profile(&state.data_dir, &p);
            }
            let msg = format!("应用失败: {e}");
            record(state, &sub_id, UpdateStatus::Failed, 0, &msg).await;
            emit(state, &sub_id, "failed", 100, &msg, Some(0), None);
            Err(e)
        }
    }
}

// ---------------- 辅助 ----------------

fn emit(
    state: &AppState,
    id: &str,
    status: &str,
    progress: u8,
    message: &str,
    node_count: Option<usize>,
    last_updated: Option<String>,
) {
    state.emit(AppEvent::Subscription {
        id: id.to_string(),
        status: status.to_string(),
        progress,
        message: message.to_string(),
        node_count,
        last_updated,
    });
}

async fn record(state: &AppState, id: &str, status: UpdateStatus, count: usize, msg: &str) {
    {
        let mut subs = state.subscriptions.write().await;
        if let Some(s) = subs.iter_mut().find(|s| s.id == id) {
            s.last_status = status.clone();
            s.last_message = msg.to_string();
            s.node_count = count;
            if status == UpdateStatus::Success {
                s.last_updated = Some(chrono::Local::now().to_rfc3339());
            }
        }
        let _ = save_subscriptions(&state.data_dir, &subs);
    }
}

fn profile_count_for(id: &str, profile: &AppProfile) -> usize {
    profile.nodes.iter().filter(|n| n.subscription_id.as_deref() == Some(id)).count()
}

/// 选中节点失效时回退到第一个节点；无任何节点则置空（渲染时落入 direct）。
fn ensure_selection(p: &mut AppProfile) {
    let valid = p
        .selected_node
        .as_ref()
        .map(|id| p.nodes.iter().any(|n| &n.id == id))
        .unwrap_or(false);
    if !valid {
        p.selected_node = p.nodes.first().map(|n| n.id.clone());
    }
}

/// 由「订阅 ID + 节点身份」生成稳定的节点 ID，使重复更新时选中不丢。
fn stable_node_id(sub_id: &str, n: &Node) -> String {
    let mut h = DefaultHasher::new();
    let key = format!("{}-{}-{}-{}", n.kind, n.server, n.port, n.name);
    key.hash(&mut h);
    format!("{}-{:x}", sub_id, h.finish())
}
