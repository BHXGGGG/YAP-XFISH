use crate::app::{AppEvent, AppState};
use crate::config::model::Node;
use crate::error::{AppError, AppResult};

use std::sync::Arc;
use std::time::Duration;

/// 测量单个节点的延迟（毫秒）。
///
/// - 对 socks5/socks/http/https 节点：以其为**代理**，向 `latency_test_url` 发 HTTP 请求，
///   测量完整往返时延（与 Clash/v2rayN 的「网站访问测试」一致，真正走节点出口）。
/// - 对其余类型（vmess/vless/trojan/shadowsocks/hysteria2 等，节点 server:port 不是通用代理）：
///   退化为 TCP 连接探测（仅验证传输层可达性），超时即不可达。
/// - 超时或失败返回 `None`，前端据此显示「超时/不可达」。
pub async fn measure(node: &Node, test_url: &str, timeout_ms: u64) -> Option<u32> {
    let timeout = Duration::from_millis(timeout_ms.max(1));
    match node.kind.as_str() {
        "socks" | "socks5" | "http" | "https" => proxy_measure(node, test_url, timeout).await,
        _ => tcp_measure(&node.server, node.port, timeout).await,
    }
}

/// 通过节点代理访问测试 URL（URL Test）。
async fn proxy_measure(node: &Node, test_url: &str, timeout: Duration) -> Option<u32> {
    // socks 系列用 socks5h：让 DNS 在远端（节点侧）解析，避免本地 DNS 泄漏/误判。
    let scheme = if node.kind == "http" || node.kind == "https" {
        "http"
    } else {
        "socks5h"
    };
    let proxy_str = format!("{scheme}://{}:{}", node.server, node.port);
    let mut proxy = reqwest::Proxy::all(&proxy_str).ok()?;
    if let Some(pass) = &node.password {
        let user = extra_username(node).unwrap_or_default();
        proxy = proxy.basic_auth(&user, pass);
    }
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .proxy(proxy)
        .build()
        .ok()?;
    let start = std::time::Instant::now();
    // 只要请求完成（任意状态码）即视为可达，返回耗时；超时/连接失败返回 None。
    let r = tokio::time::timeout(timeout, client.get(test_url).send()).await;
    match r {
        Ok(Ok(_)) => Some(start.elapsed().as_millis() as u32),
        _ => None,
    }
}

/// 从节点的 extra 中提取 socks5 用户名（sing-box outbound 经 extra 透传）。
fn extra_username(node: &Node) -> Option<String> {
    if let Some(extra) = &node.extra {
        if let Some(obj) = extra.as_object() {
            for k in ["username", "user"] {
                if let Some(v) = obj.get(k).and_then(|x| x.as_str()) {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

/// 退化的 TCP 连接探测（非通用代理节点）。
async fn tcp_measure(server: &str, port: u16, timeout: Duration) -> Option<u32> {
    let start = std::time::Instant::now();
    let r = tokio::time::timeout(timeout, tokio::net::TcpStream::connect((server, port))).await;
    match r {
        Ok(Ok(_)) => Some(start.elapsed().as_millis() as u32),
        _ => None,
    }
}

/// 测试单个节点（按 id）：探测 → 回写 profile.latency + latency_status → 持久化 → 推 WS。
pub async fn test_one(state: &AppState, id: &str) -> AppResult<()> {
    let node = {
        let p = state.profile.read().await;
        p.nodes
            .iter()
            .find(|n| n.id == id)
            .cloned()
            .ok_or_else(|| AppError(anyhow::anyhow!("节点不存在: {}", id)))?
    };
    let (url, timeout) = {
        let c = state.config.read().await;
        (c.latency_test_url.clone(), c.latency_timeout)
    };
    let latency = measure(&node, &url, timeout).await;
    write_back(state, id, latency).await;
    emit(state, id, latency);
    Ok(())
}

/// 并发测试所有节点：受 `latency_concurrency` 限制并发，探测 → 回写 → 逐个推 WS。
pub async fn test_all(state: &AppState) -> AppResult<()> {
    let (targets, url, concurrency, timeout) = {
        let p = state.profile.read().await;
        let targets: Vec<Node> = p.nodes.clone();
        let c = state.config.read().await;
        (
            targets,
            c.latency_test_url.clone(),
            c.latency_concurrency.max(1),
            c.latency_timeout,
        )
    };
    if targets.is_empty() {
        state.log_with("latency", "info", "测试全部延迟: 无节点，跳过");
        return Ok(());
    }
    state.log_with(
        "latency",
        "info",
        format!(
            "开始测试全部延迟: 节点数={} 并发={} 超时={}ms URL={}",
            targets.len(),
            concurrency,
            timeout,
            url
        ),
    );
    let started = std::time::Instant::now();

    let sem = Arc::new(tokio::sync::Semaphore::new(concurrency));
    let mut set = tokio::task::JoinSet::new();
    for node in targets {
        let url = url.clone();
        // 受并发数限制：前 concurrency 个立即拿到 permit，其余阻塞直到有节点测完释放。
        let permit = sem
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| AppError(anyhow::anyhow!(e)))?;
        set.spawn(async move {
            let lat = measure(&node, &url, timeout).await;
            drop(permit);
            (node.id.clone(), lat)
        });
    }

    let mut results: Vec<(String, Option<u32>)> = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok((id, lat)) = joined {
            results.push((id, lat));
        }
    }

    // 一次性回写并落盘
    {
        let mut p = state.profile.write().await;
        for (id, lat) in &results {
            if let Some(n) = p.nodes.iter_mut().find(|n| &n.id == id) {
                n.latency = *lat;
                n.latency_status = Some(match lat {
                    Some(_) => "ok".to_string(),
                    None => "timeout".to_string(),
                });
            }
        }
        let _ = crate::config::manager::save_profile(&state.data_dir, &p);
    }

    // 逐个推送（前端逐条刷新）
    for (id, lat) in &results {
        emit(state, id, *lat);
    }
    let ok = results.iter().filter(|(_, l)| l.is_some()).count();
    let failed = results.len() - ok;
    state.log_with(
        "latency",
        "info",
        format!(
            "全部测速完成: 耗时={}ms ok={} 失败={}",
            started.elapsed().as_millis(),
            ok,
            failed
        ),
    );
    Ok(())
}

async fn write_back(state: &AppState, id: &str, latency: Option<u32>) {
    let mut p = state.profile.write().await;
    if let Some(n) = p.nodes.iter_mut().find(|n| &n.id == id) {
        n.latency = latency;
        n.latency_status = Some(match latency {
            Some(_) => "ok".to_string(),
            None => "timeout".to_string(),
        });
    }
    let _ = crate::config::manager::save_profile(&state.data_dir, &p);
}

fn emit(state: &AppState, id: &str, latency: Option<u32>) {
    let message = match latency {
        Some(ms) => format!("{} ms", ms),
        None => "超时/不可达".to_string(),
    };
    state.emit(AppEvent::Latency {
        id: id.to_string(),
        latency,
        message: message.clone(),
    });
    // 同时广播一条带来源的 info/warn 日志，让「实时日志」面板能直观看到
    // 每次测速的耗时 / 失败原因，不用再到 latency 事件流里看。
    let level = if latency.is_some() { "info" } else { "warn" };
    state.log_with("latency", level, format!("节点测速 {}: {}", id, message));
}
