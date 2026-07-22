use crate::app::{AppEvent, AppState};
use crate::config::model::Node;
use crate::config::render;
use crate::error::{AppError, AppResult};

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::Mutex;

/// 测量单个节点的延迟（毫秒）。
///
/// 协议节点（vmess/vless/trojan/ss/hysteria2 等）参考 UIF
/// (https://github.com/UIforFreedom/UIF) 的做法：
///   1. 生成临时 sing-box 配置：所有待测 outbound + experimental.clash_api
///   2. 起一个**临时** core 进程（测完 kill）
///   3. 请求 Clash API：`GET /proxies/{tag}/delay?timeout=...&url=...`
///   4. 读返回 JSON 的 `delay` 字段
///
/// socks5/http 通用代理节点仍直接用 reqwest 走节点出口测 URL。
/// 失败/超时返回 `None`（前端显示「不可达」）。
pub async fn measure(
    state: &AppState,
    node: &Node,
    test_url: &str,
    timeout_ms: u64,
) -> Option<u32> {
    let timeout = Duration::from_millis(timeout_ms.max(1));
    match node.kind.as_str() {
        "socks" | "socks5" | "http" | "https" => proxy_measure(node, test_url, timeout).await,
        _ => {
            // 单节点场景：临时 core + 单 tag delay
            let session = UifProbeSession::start(state, std::slice::from_ref(node)).await?;
            let lat = session.delay(&node.id, test_url, timeout_ms).await;
            session.shutdown().await;
            lat
        }
    }
}

async fn proxy_measure(node: &Node, test_url: &str, timeout: Duration) -> Option<u32> {
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
    let start = Instant::now();
    let r = tokio::time::timeout(timeout, client.get(test_url).send()).await;
    match r {
        Ok(Ok(_)) => Some(start.elapsed().as_millis() as u32),
        _ => None,
    }
}

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

/// UIF 风格：一批节点共享一个临时 sing-box + Clash API 会话。
struct UifProbeSession {
    child: Mutex<Option<tokio::process::Child>>,
    api_base: String,
    tmp_dir: PathBuf,
    #[allow(dead_code)]
    cfg_path: PathBuf,
}

impl UifProbeSession {
    /// 启动临时 core。nodes 全部作为 outbound 注入；clash_api 监听随机端口。
    async fn start(state: &AppState, nodes: &[Node]) -> Option<Self> {
        if nodes.is_empty() {
            return None;
        }
        let binary = resolve_singbox_binary(state);
        if !binary.exists() {
            return None;
        }

        let api_port = pick_free_port()?;
        let mut outbounds: Vec<serde_json::Value> = nodes
            .iter()
            .map(|n| {
                let mut ob = render::node_to_outbound(n);
                if let Some(obj) = ob.as_object_mut() {
                    obj.insert("tag".into(), serde_json::json!(n.id));
                }
                ob
            })
            .collect();
        outbounds.push(serde_json::json!({ "type": "direct", "tag": "direct" }));

        // 对齐 UIF MutipleTemplate：临时 core 只开 clash_api，不需要 inbound。
        // route.final 用 direct；delay API 会按 proxy tag 指定出站。
        let cfg = serde_json::json!({
            "log": { "level": "error", "timestamp": true },
            "dns": {
                "servers": [
                    { "type": "udp", "tag": "local", "server": "223.5.5.5" }
                ],
                "final": "local",
                "strategy": "prefer_ipv4"
            },
            "outbounds": outbounds,
            "route": {
                "final": "direct",
                "auto_detect_interface": true,
                "default_domain_resolver": { "server": "local" }
            },
            "experimental": {
                "clash_api": {
                    "external_controller": format!("127.0.0.1:{api_port}"),
                    "secret": ""
                }
            }
        });

        let tmp_dir = std::env::temp_dir().join(format!(
            "yap-xfish-uif-probe-{}-{}",
            std::process::id(),
            api_port
        ));
        let _ = std::fs::create_dir_all(&tmp_dir);
        let cfg_path = tmp_dir.join("probe.json");
        if std::fs::write(
            &cfg_path,
            serde_json::to_vec_pretty(&cfg).unwrap_or_default(),
        )
        .is_err()
        {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return None;
        }

        let mut child = match {
                    let mut cmd = tokio::process::Command::new(&binary);
                    cmd.arg("run")
                        .arg("-c")
                        .arg(&cfg_path)
                        .stdout(Stdio::null())
                        .stderr(Stdio::piped())
                        .stdin(Stdio::null())
                        .kill_on_drop(true);
                    // 测速临时 core 也禁止弹黑框（与 core::process 一致）。
                    #[cfg(windows)]
                    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
                    cmd.spawn()
                } {
                    Ok(c) => c,
                    Err(_) => {
                        let _ = std::fs::remove_dir_all(&tmp_dir);
                        return None;
                    }
                };

        // 吞掉 stderr，避免管道阻塞
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(_)) = lines.next_line().await {}
            });
        }

        // 等 clash_api 起来（最多约 3s）
        let api_base = format!("http://127.0.0.1:{api_port}");
        let ready = wait_api_ready(&api_base, Duration::from_secs(3)).await;
        if !ready {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return None;
        }

        Some(Self {
            child: Mutex::new(Some(child)),
            api_base,
            tmp_dir,
            cfg_path,
        })
    }

    /// 调用 Clash API `/proxies/{tag}/delay`（与 UIF TestMultipleNode 一致）。
    async fn delay(&self, tag: &str, test_url: &str, timeout_ms: u64) -> Option<u32> {
        let timeout_ms = timeout_ms.max(500);
        // tag 可能含特殊字符；UIF 用 encodeURIComponent，这里做同样处理
        let encoded = urlencoding_minimal(tag);
        let url = format!(
            "{}/proxies/{}/delay?timeout={}&url={}",
            self.api_base,
            encoded,
            timeout_ms,
            // url 参数本身也需要编码
            urlencoding_minimal(test_url)
        );
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(timeout_ms + 1500))
            .no_proxy()
            .build()
            .ok()?;
        let resp = client.get(&url).send().await.ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let v: serde_json::Value = resp.json().await.ok()?;
        // sing-box / Clash Meta 返回 {"delay": N}；0 表示失败
        let d = v.get("delay").and_then(|x| x.as_u64()).unwrap_or(0);
        if d == 0 {
            None
        } else {
            Some(d as u32)
        }
    }

    async fn shutdown(self) {
        if let Some(mut child) = self.child.lock().await.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        let _ = std::fs::remove_dir_all(&self.tmp_dir);
    }
}

async fn wait_api_ready(api_base: &str, overall: Duration) -> bool {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_millis(400))
        .no_proxy()
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let deadline = Instant::now() + overall;
    let url = format!("{api_base}/proxies");
    while Instant::now() < deadline {
        if let Ok(r) = client.get(&url).send().await {
            if r.status().is_success() {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

fn pick_free_port() -> Option<u16> {
    let s = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
    let p = s.local_addr().ok()?.port();
    drop(s);
    Some(p)
}

/// 极简 URL path/query 编码（只编码非 unreserved 字符）。
fn urlencoding_minimal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn resolve_singbox_binary(state: &AppState) -> PathBuf {
    let configured = state
        .config
        .try_read()
        .map(|c| c.core_binary.clone())
        .unwrap_or_default();
    if !configured.as_os_str().is_empty() {
        let abs = if configured.is_absolute() {
            configured.clone()
        } else if let Ok(exe) = std::env::current_exe() {
            exe.parent()
                .map(|p| p.join(&configured))
                .unwrap_or(configured.clone())
        } else {
            configured.clone()
        };
        if abs.exists() {
            return abs;
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in ["sing-box.exe", "sing-box"] {
                let p = dir.join(name);
                if p.exists() {
                    return p;
                }
            }
            let p = dir.join("core").join("sing-box.exe");
            if p.exists() {
                return p;
            }
        }
    }
    state.data_dir.join("..").join("sing-box.exe")
}

/// 测试单个节点。
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
    let latency = measure(state, &node, &url, timeout).await;
    write_back(state, id, latency).await;
    emit(state, id, latency);
    Ok(())
}

/// 并发测试全部节点。
///
/// UIF 风格：协议节点共享一个临时 core 会话，并发打 `/proxies/{tag}/delay`；
/// 通用代理节点仍各自 reqwest。
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

    // 拆分：协议节点走 UIF session；socks/http 走 proxy_measure
    let mut protocol: Vec<Node> = Vec::new();
    let mut plain: Vec<Node> = Vec::new();
    for n in targets {
        match n.kind.as_str() {
            "socks" | "socks5" | "http" | "https" => plain.push(n),
            _ => protocol.push(n),
        }
    }

    // UIF 一次起 core 塞全部 outbound；并发 delay 请求数仍限制
    let concurrency = concurrency.min(16);
    state.log_with(
        "latency",
        "info",
        format!(
            "开始测试全部延迟(UIF): 协议节点={} 代理节点={} 并发={} 超时={}ms URL={}",
            protocol.len(),
            plain.len(),
            concurrency,
            timeout,
            url
        ),
    );
    let started = Instant::now();
    let mut results: Vec<(String, Option<u32>)> = Vec::new();

    // 1) 协议节点：一个临时 core
    if !protocol.is_empty() {
        if let Some(session) = UifProbeSession::start(state, &protocol).await {
            let session = Arc::new(session);
            let sem = Arc::new(tokio::sync::Semaphore::new(concurrency));
            let mut set = tokio::task::JoinSet::new();
            for node in protocol {
                let url = url.clone();
                let sess = session.clone();
                let permit = sem
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|e| AppError(anyhow::anyhow!(e)))?;
                let timeout_ms = timeout;
                set.spawn(async move {
                    let lat = sess.delay(&node.id, &url, timeout_ms).await;
                    drop(permit);
                    (node.id.clone(), lat)
                });
            }
            while let Some(joined) = set.join_next().await {
                if let Ok(pair) = joined {
                    results.push(pair);
                }
            }
            // 拆 Arc 关停
            if let Ok(sess) = Arc::try_unwrap(session) {
                sess.shutdown().await;
            }
        } else {
            // core 起不来：全部记失败
            state.log_with(
                "latency",
                "warn",
                "UIF 临时 core 启动失败，协议节点测速全部标记不可达",
            );
            for n in protocol {
                results.push((n.id, None));
            }
        }
    }

    // 2) 通用代理节点
    if !plain.is_empty() {
        let sem = Arc::new(tokio::sync::Semaphore::new(concurrency));
        let mut set = tokio::task::JoinSet::new();
        for node in plain {
            let url = url.clone();
            let permit = sem
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| AppError(anyhow::anyhow!(e)))?;
            set.spawn(async move {
                let lat = proxy_measure(&node, &url, Duration::from_millis(timeout)).await;
                drop(permit);
                (node.id.clone(), lat)
            });
        }
        while let Some(joined) = set.join_next().await {
            if let Ok(pair) = joined {
                results.push(pair);
            }
        }
    }

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
    let level = if latency.is_some() { "info" } else { "warn" };
    state.log_with("latency", level, format!("节点测速 {}: {}", id, message));
}
