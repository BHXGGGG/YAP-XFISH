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

/// 首次启动 / 未配置场景的判定：数据目录里没有订阅，或订阅列表为空。
/// 启动时会基于此决定是否自动打开浏览器到管理面板。
pub fn has_no_subscriptions(data_dir: &Path) -> bool {
    match load_subscriptions(data_dir) {
        Ok(list) => list.is_empty(),
        Err(_) => true,
    }
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
    state.log_with(
        "sub",
        "info",
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
        Ok((r, meta)) => {
            state.log_with("sub", "info", format!("下载成功: {} 字节", r.len()));
            apply_meta(state, &sub_id, meta).await;
            r
        }
        Err(e) => {
            let msg = format!("下载失败: {e}");
            state.log_with("sub", "error", msg.clone());
            record(state, &sub_id, UpdateStatus::Failed, 0, &msg).await;
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
            return Err(AppError(anyhow::anyhow!(msg)));
        }
        Err(e) => {
            let msg = format!("解析失败: {e}");
            state.log_with("sub", "error", msg.clone());
            record(state, &sub_id, UpdateStatus::Failed, 0, &msg).await;
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
    // 4.5 订阅内去重：某些机场会在同一 (kind, server, port) 上塞多个"占位"节点
    // （如公告 / 套餐到期 / 备用 等共用假地址），它们的 name 不同但身份相同，
    // 算出的 id 也会完全相同——sing-box 渲染时 `duplicate outbound/endpoint tag`
    // FATAL 拒绝启动。这里给后续重复身份补 "-1" "-2" … 后缀，让它们成为独立节点；
    // 真实节点（身份唯一）保持原 id，便于迁移期 id 仍然稳定。
    {
        let mut seen: std::collections::HashMap<(String, String, u16), u32> =
            std::collections::HashMap::new();
        for n in new_nodes.iter_mut() {
            let key = (n.kind.clone(), n.server.clone(), n.port);
            let counter = seen.entry(key).or_insert(0);
            if *counter > 0 {
                n.id.push('-');
                n.id.push_str(&counter.to_string());
            }
            *counter += 1;
        }
    }
    // 解析到的实际节点数（无论订阅启用与否都要回写到 node_count）
    let parsed_count = new_nodes.len();

    emit(
        state,
        &sub_id,
        "updating",
        60,
        &format!("合并 {} 个节点", parsed_count),
        None,
        None,
    );

    // 5. 合并进 profile：先快照，再移除旧节点、追加新节点、保证选中有效。
    // 保留旧节点上的本地 detour（链式前置），避免订阅刷新冲掉用户配置的链路。
    let snapshot = {
        let mut p = state.profile.write().await;
        let snap = p.clone();
        let old_detours: std::collections::HashMap<String, String> = p
            .nodes
            .iter()
            .filter(|n| n.subscription_id.as_deref() == Some(id))
            .filter_map(|n| {
                n.detour
                    .as_ref()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .map(|d| (n.id.clone(), d))
            })
            .collect();
        p.nodes.retain(|n| n.subscription_id.as_deref() != Some(id));
        for n in new_nodes.iter_mut() {
            if let Some(d) = old_detours.get(&n.id) {
                // 目标可能仍是本订阅内其它节点，或其它订阅/手动节点；存在性在 render 时再校验
                n.detour = Some(d.clone());
            }
        }
        p.nodes.append(&mut new_nodes);
        ensure_selection(&mut p);
        if let Err(e) = cfgmgr::save_profile(&state.data_dir, &p) {
            // 持久化失败：回滚内存并上报
            *p = snap;
            let msg = format!("保存配置失败: {e}");
            record(state, &sub_id, UpdateStatus::Failed, 0, &msg).await;
            return Err(AppError(e.into()));
        }
        snap
    };

    emit(state, &sub_id, "updating", 80, "应用配置并重启核心", None, None);

    // 6. 应用：核心在运行时 reload（带配置回滚），未运行时仅写配置待下次启动。
    // 注意：运行时配置用 visible_profile（排除停用订阅节点），但 node_count 必须
    // 用解析到的实际节点数 / 完整 profile 统计——否则「全部更新」停用订阅时会写成 0。
    let app_cfg = state.config.read().await.clone();
    let profile_now = state.visible_profile().await;
    let running = state.core.is_running().await;

    // reload_safe 内部会判断：只有节点连接参数 / 规则等实质变化才重启核心。
    let apply_result = if running {
        state.core.reload_safe(&profile_now, &app_cfg).await
    } else {
        state.core.write_config_only(&profile_now, &app_cfg).await
    };

    match apply_result {
        Ok(()) => {
            // 优先按完整 profile 统计（含停用订阅节点）；兜底用解析数。
            let cnt = {
                let p = state.profile.read().await;
                let c = profile_count_for(&sub_id, &p);
                if c > 0 {
                    c
                } else {
                    parsed_count
                }
            };
            record(state, &sub_id, UpdateStatus::Success, cnt, "更新成功").await;
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
        next_retry_at: None,
    });
}

async fn record(state: &AppState, id: &str, status: UpdateStatus, count: usize, msg: &str) {
    let mut subs = state.subscriptions.write().await;
    let Some(s) = subs.iter_mut().find(|s| s.id == id) else { return };
    let now = chrono::Local::now();
    let success = status == UpdateStatus::Success;
    s.last_status = status;
    s.last_message = msg.to_string();
    if success {
        s.node_count = count;
        s.last_updated = Some(now.to_rfc3339());
        s.next_retry_at = None;
    } else {
        s.next_retry_at = Some((now + chrono::Duration::minutes(5)).to_rfc3339());
    }
    let event = AppEvent::Subscription {
        id: id.to_string(),
        status: if success { "success" } else { "failed" }.to_string(),
        progress: 100,
        message: msg.to_string(),
        node_count: Some(s.node_count),
        last_updated: s.last_updated.clone(),
        next_retry_at: s.next_retry_at.clone(),
    };
    let _ = save_subscriptions(&state.data_dir, &subs);
    drop(subs);
    state.emit(event);
}

/// 写入订阅响应头信息：自动名字替换为机场名，记录流量 / 到期时间。
async fn apply_meta(state: &AppState, id: &str, meta: fetcher::SubMeta) {
    let mut subs = state.subscriptions.write().await;
    let Some(s) = subs.iter_mut().find(|s| s.id == id) else { return };
    let mut changed = false;
    if s.name_auto {
        if let Some(name) = meta.name {
            state.log_with("sub", "info", format!("订阅名字已自动设为: {name}"));
            s.name = name;
            s.name_auto = false;
            changed = true;
        }
    }
    if meta.userinfo.is_some() && s.userinfo != meta.userinfo {
        s.userinfo = meta.userinfo;
        changed = true;
    }
    if changed {
        let _ = save_subscriptions(&state.data_dir, &subs);
        // 名字 / 流量变化推给前端（单条进度事件不带这些字段）
        state.emit(AppEvent::SubscriptionsRefresh { subscriptions: subs.clone() });
    }
}

fn profile_count_for(id: &str, profile: &AppProfile) -> usize {
    profile
        .nodes
        .iter()
        .filter(|n| n.subscription_id.as_deref() == Some(id))
        .count()
}

/// 选中节点失效时回退到第一个节点；无任何节点则置空（渲染时落入 direct）。
/// 注意：节点集合为空时不要"回退到 nodes.first()"，否则会从残留节点里挑一个
/// 看似"自动"继续使用旧订阅的节点；这里统一保持 None。
fn ensure_selection(p: &mut AppProfile) {
    if p.nodes.is_empty() {
        p.selected_node = None;
        return;
    }
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
///
/// 注意：hash key 包含 `name`，但**重复更新时 name 通常不变**（机场只是偶尔在
/// 节点名里塞剩余流量/到期等可变字段，导致订阅刷新后 id 漂移）。本函数在调用
/// 之前由「订阅内冲突去重」负责把同一 (kind, server, port) 的多个节点补上不同
/// 序号后缀（参见 `update_subscription_inner`），因此正常情况下 name 改变也
/// 不会让 id 漂移；本函数只承担最后一道身份计算 + 兜底。
fn stable_node_id(sub_id: &str, n: &Node) -> String {
    let mut h = DefaultHasher::new();
    let key = format!("{}-{}-{}-{}", n.kind, n.server, n.port, n.name);
    key.hash(&mut h);
    format!("{}-{:x}", sub_id, h.finish())
}

#[cfg(test)]
mod tests;
