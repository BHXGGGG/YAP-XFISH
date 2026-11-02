//! 仪表盘信息：运行时长 / 内网 IP / 内存、出口 IP 与 IP 质量、服务延迟。
//!
//! 出口 IP 的做法与 FlClash 相同：请求经本机代理端口（sing-box mixed 入站）发出，
//! 走当前模式与选中节点，查到的就是实际出口。IP 质量再用该 IP 查公开情报接口，
//! 判定规则照搬 FlClash（lib/common/ip_quality.dart、lib/models/ip_quality.dart）。

use crate::app::AppState;
use crate::error::{AppError, AppResult};
use axum::extract::{Query, State};
use axum::Json;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const EXIT_TTL: Duration = Duration::from_secs(5 * 60);
const QUALITY_TTL: Duration = Duration::from_secs(24 * 3600);

// ---------------- 运行信息 ----------------

pub async fn info(State(state): State<Arc<AppState>>) -> AppResult<Json<serde_json::Value>> {
    let core = state.core.uptime_and_pid().await;
    let core_mem = core
        .and_then(|(_, pid)| pid)
        .map(crate::system::mem::process_working_set)
        .unwrap_or(0);
    let app_mem = crate::system::mem::memory_info().working_set_bytes;
    let lan_ip = tokio::task::spawn_blocking(crate::system::netinfo::lan_ipv4)
        .await
        .ok()
        .flatten()
        .map(|ip| ip.to_string());
    let proxy_port = state.config.read().await.proxy_port;
    Ok(Json(serde_json::json!({
        "uptime_secs": core.map(|(d, _)| d.as_secs()),
        "lan_ip": lan_ip,
        "proxy_port": proxy_port,
        "app_mem": app_mem,
        "core_mem": core_mem,
    })))
}

// ---------------- 出口 IP + 质量 ----------------

#[derive(Debug, Clone, Serialize, Default)]
pub struct ExitIp {
    pub ip: String,
    pub country: Option<String>,
    pub org: Option<String>,
    /// residential / mobile / business / hosting
    pub ip_type: Option<String>,
    /// good / normal / risky
    pub quality: Option<String>,
    /// 类型是否只是「未标记为机房 / 移动」推断出来的
    pub inferred: bool,
}

#[derive(Debug, Clone)]
struct Quality {
    org: Option<String>,
    ip_type: String,
    inferred: bool,
    risky: bool,
}

static EXIT_CACHE: Lazy<Mutex<HashMap<String, (Instant, ExitIp)>>> = Lazy::new(Default::default);
static QUALITY_CACHE: Lazy<Mutex<HashMap<String, (Instant, Quality)>>> = Lazy::new(Default::default);

#[derive(Deserialize)]
pub struct ExitQuery {
    #[serde(default)]
    refresh: bool,
}

pub async fn exit_ip(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ExitQuery>,
) -> AppResult<Json<Option<ExitIp>>> {
    if !state.status.read().await.running {
        return Ok(Json(None));
    }
    let (port, key) = {
        let p = state.profile.read().await;
        let port = state.config.read().await.proxy_port;
        (port, format!("{:?}|{}", p.mode, p.selected_node.clone().unwrap_or_default()))
    };
    if !q.refresh {
        if let Some((t, v)) = EXIT_CACHE.lock().await.get(&key) {
            if t.elapsed() < EXIT_TTL {
                return Ok(Json(Some(v.clone())));
            }
        }
    }
    let client = proxy_client(port, Duration::from_secs(10))?;
    let Some((ip, country)) = lookup_exit(&client).await else {
        state.log_with("app", "warn", "出口 IP 查询失败：节点未在超时内响应");
        return Err(AppError(anyhow::anyhow!("出口 IP 查询失败（当前节点可能不可用）")));
    };

    let mut out = ExitIp { ip: ip.clone(), country, ..Default::default() };
    if let Some(qv) = quality_for(&client, &ip, q.refresh).await {
        out.org = qv.org.clone();
        out.quality = Some(level(&qv).into());
        out.ip_type = Some(qv.ip_type.clone());
        out.inferred = qv.inferred;
    }
    EXIT_CACHE.lock().await.insert(key, (Instant::now(), out.clone()));
    Ok(Json(Some(out)))
}

fn proxy_client(port: u16, timeout: Duration) -> AppResult<reqwest::Client> {
    let proxy = reqwest::Proxy::all(format!("socks5h://127.0.0.1:{port}"))
        .map_err(|e| AppError(anyhow::anyhow!(e)))?;
    reqwest::Client::builder()
        .proxy(proxy)
        .timeout(timeout)
        .build()
        .map_err(|e| AppError(anyhow::anyhow!(e)))
}

/// 两个来源并发查询，任一成功即用，避免前一个占满超时后另一个没时间。
async fn lookup_exit(client: &reqwest::Client) -> Option<(String, Option<String>)> {
    let (trace, ipinfo) = tokio::join!(from_cloudflare(client), from_ipinfo(client));
    // Cloudflare trace 还给出国家代码，优先用它
    trace.or(ipinfo)
}

async fn from_cloudflare(client: &reqwest::Client) -> Option<(String, Option<String>)> {
    let body = client
        .get("https://www.cloudflare.com/cdn-cgi/trace")
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()?;
    parse_trace(&body)
}

async fn from_ipinfo(client: &reqwest::Client) -> Option<(String, Option<String>)> {
    let j: serde_json::Value = client.get("https://ipinfo.io/json").send().await.ok()?.json().await.ok()?;
    let ip = j.get("ip")?.as_str()?.to_string();
    Some((ip, j.get("country").and_then(|c| c.as_str()).map(String::from)))
}

pub fn parse_trace(body: &str) -> Option<(String, Option<String>)> {
    let mut ip = None;
    let mut loc = None;
    for line in body.lines() {
        if let Some(v) = line.strip_prefix("ip=") {
            ip = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("loc=") {
            loc = Some(v.trim().to_string()).filter(|s| !s.is_empty() && s != "XX");
        }
    }
    ip.filter(|s| !s.is_empty()).map(|ip| (ip, loc))
}

async fn quality_for(client: &reqwest::Client, ip: &str, refresh: bool) -> Option<Quality> {
    if !refresh {
        if let Some((t, v)) = QUALITY_CACHE.lock().await.get(ip) {
            if t.elapsed() < QUALITY_TTL {
                return Some(v.clone());
            }
        }
    }
    // 同时问两家，优先「明确给出类型」的结果；都只是推断时取先到的
    let (a, b) = tokio::join!(from_ipquery(client, ip), from_ip_api(client, ip));
    let pick = match (a, b) {
        (Some(x), _) if !x.inferred => Some(x),
        (_, Some(y)) if !y.inferred => Some(y),
        (x, y) => x.or(y),
    }?;
    QUALITY_CACHE.lock().await.insert(ip.to_string(), (Instant::now(), pick.clone()));
    Some(pick)
}

async fn from_ipquery(client: &reqwest::Client, ip: &str) -> Option<Quality> {
    let j: serde_json::Value = client
        .get(format!("https://api.ipquery.io/{ip}?format=json"))
        .send().await.ok()?.json().await.ok()?;
    let risk = j.get("risk")?;
    let isp = j.get("isp");
    let flag = |k: &str| risk.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    let org = isp
        .and_then(|i| i.get("org").or_else(|| i.get("isp")))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    Some(classify(flag("is_datacenter"), flag("is_mobile"), org, flag("is_tor")))
}

/// ip-api.com 免费版只提供 HTTP。
async fn from_ip_api(client: &reqwest::Client, ip: &str) -> Option<Quality> {
    let j: serde_json::Value = client
        .get(format!("http://ip-api.com/json/{ip}?fields=status,isp,org,mobile,hosting"))
        .send().await.ok()?.json().await.ok()?;
    if j.get("status")?.as_str()? != "success" {
        return None;
    }
    let flag = |k: &str| j.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    let org = ["org", "isp"]
        .iter()
        .find_map(|k| j.get(*k).and_then(|v| v.as_str()).filter(|s| !s.is_empty()))
        .map(String::from);
    Some(classify(flag("hosting"), flag("mobile"), org, false))
}

/// 机房 > 移动 > 其余按住宅推断（来源没说是机房也没说是移动）。
fn classify(hosting: bool, mobile: bool, org: Option<String>, risky: bool) -> Quality {
    let (ip_type, inferred) = if hosting {
        ("hosting", false)
    } else if mobile {
        ("mobile", false)
    } else {
        ("residential", true)
    };
    Quality { org, ip_type: ip_type.into(), inferred, risky }
}

/// Tor / 滥用 → 风险；住宅 / 移动 / 企业 → 优；机房 → 一般。
fn level(q: &Quality) -> &'static str {
    if q.risky {
        "risky"
    } else if q.ip_type == "hosting" {
        "normal"
    } else {
        "good"
    }
}

// ---------------- 服务延迟 ----------------

const SERVICES: &[(&str, &str)] = &[
    ("google", "https://www.google.com/generate_204"),
    ("github", "https://github.com"),
];

pub async fn services(State(state): State<Arc<AppState>>) -> AppResult<Json<serde_json::Value>> {
    if !state.status.read().await.running {
        return Ok(Json(serde_json::json!([])));
    }
    let port = state.config.read().await.proxy_port;
    // 各服务并发测；每个用新客户端，测的是完整的「连接 + TLS + 首个响应」耗时
    let handles: Vec<_> = SERVICES
        .iter()
        .map(|(name, url)| {
            tokio::spawn(async move {
                let ms = match proxy_client(port, Duration::from_secs(6)) {
                    Ok(c) => {
                        let t = Instant::now();
                        c.head(*url).send().await.ok().map(|_| t.elapsed().as_millis() as u64)
                    }
                    Err(_) => None,
                };
                serde_json::json!({ "name": name, "ms": ms })
            })
        })
        .collect();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(v) = h.await {
            out.push(v);
        }
    }
    Ok(Json(serde_json::Value::Array(out)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cloudflare_trace() {
        let body = "fl=1\nh=www.cloudflare.com\nip=104.251.122.9\nts=1\nloc=US\nwarp=off\n";
        assert_eq!(parse_trace(body), Some(("104.251.122.9".into(), Some("US".into()))));
        assert_eq!(parse_trace("loc=US\n"), None);
    }

    #[test]
    fn classifies_like_flclash() {
        let q = classify(true, false, None, false);
        assert_eq!((q.ip_type.as_str(), level(&q)), ("hosting", "normal"));
        let q = classify(false, true, None, false);
        assert_eq!((q.ip_type.as_str(), level(&q), q.inferred), ("mobile", "good", false));
        let q = classify(false, false, None, false);
        assert_eq!((q.ip_type.as_str(), level(&q), q.inferred), ("residential", "good", true));
        assert_eq!(level(&classify(false, false, None, true)), "risky");
    }
}
