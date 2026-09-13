//! 节点测速调度：限制同时运行的任务，完成一个就立即推送并补充下一个。
mod probe;

use crate::app::{AppEvent, AppState};
use crate::config::model::{AppProfile, Node};
use crate::error::{AppError, AppResult};
use probe::ProbeContext;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::task::JoinSet;

/// 与设置页范围一致；默认 50，避免把用户设置静默压到 16。
const MAX_CONCURRENCY: usize = 200;

struct NodeClaims {
    shared: Arc<Mutex<HashSet<String>>>,
    ids: HashSet<String>,
}

impl NodeClaims {
    fn acquire(state: &AppState, targets: &[Node]) -> Self {
        let shared = state.latency_in_flight.clone();
        let ids = {
            let mut active = shared.lock().unwrap_or_else(|e| e.into_inner());
            targets
                .iter()
                .filter(|n| active.insert(n.id.clone()))
                .map(|n| n.id.clone())
                .collect()
        };
        Self { shared, ids }
    }

    fn release(&mut self, id: &str) {
        self.ids.remove(id);
        self.shared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }
}

impl Drop for NodeClaims {
    fn drop(&mut self) {
        let mut active = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        for id in &self.ids {
            active.remove(id);
        }
    }
}

/// 一次请求的配置快照和节点集合。丢弃或取消任务时自动释放测速占用。
pub struct LatencyBatch {
    profile: AppProfile,
    targets: Vec<Node>,
    claims: NodeClaims,
}

/// None = 全部可见节点；Some([]) = 不测速；显式 ID 必须属于当前可见节点。
/// 同一节点正在测速时复用已有任务的结果，避免重复点击启动更多核心/请求。
pub async fn prepare(state: &AppState, node_ids: Option<Vec<String>>) -> AppResult<LatencyBatch> {
    let profile = state.visible_profile().await;
    let mut targets = select_targets(&profile, node_ids.as_deref())?;
    let claims = NodeClaims::acquire(state, &targets);
    targets.retain(|n| claims.ids.contains(&n.id));
    Ok(LatencyBatch {
        profile,
        targets,
        claims,
    })
}

fn select_targets(profile: &AppProfile, ids: Option<&[String]>) -> AppResult<Vec<Node>> {
    let Some(ids) = ids else {
        return Ok(profile.nodes.clone());
    };
    let mut seen = HashSet::new();
    let mut targets = Vec::new();
    for id in ids {
        let node = profile
            .nodes
            .iter()
            .find(|n| &n.id == id)
            .ok_or_else(|| AppError(anyhow::anyhow!("节点不存在或所属订阅未启用: {id}")))?;
        if seen.insert(id) {
            targets.push(node.clone());
        }
    }
    Ok(targets)
}

#[derive(Default)]
struct BatchStats {
    completed: usize,
    successful: usize,
    first_result_ms: Option<u128>,
}

impl LatencyBatch {
    pub fn len(&self) -> usize {
        self.targets.len()
    }

    pub async fn run(mut self, state: &AppState) -> AppResult<()> {
        if self.targets.is_empty() {
            return Ok(());
        }
        let cfg = state.config.read().await.clone();
        let concurrency = cfg.latency_concurrency.clamp(1, MAX_CONCURRENCY);
        let started = Instant::now();
        state.log_with(
            "latency",
            "info",
            format!(
                "开始测速: 节点={} 并发={} 超时={}ms",
                self.targets.len(),
                concurrency,
                cfg.latency_timeout
            ),
        );

        let mut context = ProbeContext::prepare(state, &self.profile, &self.targets).await;
        state.log_with("latency", "info", format!("测速方式: {}", context.label()));
        let stats = self
            .run_probes(
                state,
                &context,
                &cfg.latency_test_url,
                cfg.latency_timeout,
                concurrency,
                started,
            )
            .await;
        context.shutdown().await;

        // 结果已推送；整批只落盘一次，磁盘写入不阻塞首个结果。
        {
            let profile = state.profile.read().await;
            crate::config::manager::save_profile(&state.data_dir, &profile)?;
        }
        state.log_with(
            "latency",
            "info",
            format!(
                "测速完成: 首个结果={}ms 总耗时={}ms 成功={} 失败={}",
                stats.first_result_ms.unwrap_or_default(),
                started.elapsed().as_millis(),
                stats.successful,
                stats.completed - stats.successful,
            ),
        );
        Ok(())
    }

    async fn run_probes(
        &mut self,
        state: &AppState,
        context: &ProbeContext,
        url: &str,
        timeout_ms: u64,
        concurrency: usize,
        started: Instant,
    ) -> BatchStats {
        let mut pending = std::mem::take(&mut self.targets).into_iter();
        let mut tasks = JoinSet::new();
        let mut stats = BatchStats::default();

        // 只提交一个并发窗口，立即进入结果接收循环。
        // 不要先 acquire permit 直到提交完全部任务，那会把已完成结果留在队列里。
        for node in pending.by_ref().take(concurrency.clamp(1, MAX_CONCURRENCY)) {
            spawn_probe(&mut tasks, context, node, url, timeout_ms);
        }
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok((id, latency)) => {
                    stats.completed += 1;
                    stats.successful += usize::from(latency.is_some());
                    stats
                        .first_result_ms
                        .get_or_insert_with(|| started.elapsed().as_millis());
                    {
                        let mut profile = state.profile.write().await;
                        if let Some(node) = profile.nodes.iter_mut().find(|n| n.id == id) {
                            node.latency = latency;
                            node.latency_status =
                                Some(if latency.is_some() { "ok" } else { "timeout" }.to_string());
                        }
                    }
                    self.claims.release(&id);
                    emit(state, &id, latency);
                }
                Err(e) => state.log_with("latency", "error", format!("测速任务异常: {e}")),
            }
            if let Some(node) = pending.next() {
                spawn_probe(&mut tasks, context, node, url, timeout_ms);
            }
        }
        stats
    }
}

fn spawn_probe(
    tasks: &mut JoinSet<(String, Option<u32>)>,
    context: &ProbeContext,
    node: Node,
    url: &str,
    timeout_ms: u64,
) {
    let method = context.method_for(&node);
    let url = url.to_string();
    tasks.spawn(async move {
        let latency = method.measure(&node, &url, timeout_ms).await;
        (node.id, latency)
    });
}

fn emit(state: &AppState, id: &str, latency: Option<u32>) {
    let message = match latency {
        Some(ms) => format!("{ms} ms"),
        None => "超时/不可达".to_string(),
    };
    state.emit(AppEvent::Latency {
        id: id.to_string(),
        latency,
        message: message.clone(),
    });
    let level = if latency.is_some() { "info" } else { "warn" };
    state.log_with("latency", level, format!("节点测速 {id}: {message}"));
}

#[cfg(test)]
mod tests;
