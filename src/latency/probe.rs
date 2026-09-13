//! 探测连接：优先复用运行核心的 Clash API，配置不一致或核心未运行时使用临时核心。
use crate::app::AppState;
use crate::config::model::{AppProfile, Node};
use crate::config::render;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};

fn needs_core(node: &Node) -> bool {
    !matches!(node.kind.as_str(), "socks" | "socks5" | "http" | "https")
        || node.detour.as_ref().is_some_and(|d| !d.trim().is_empty())
}

pub(super) struct ProbeContext {
    api: Option<ClashApi>,
    tags: HashMap<String, String>,
    temporary: Option<TemporaryCore>,
}

impl ProbeContext {
    pub(super) async fn prepare(state: &AppState, profile: &AppProfile, targets: &[Node]) -> Self {
        let protocol: Vec<Node> = targets.iter().filter(|n| needs_core(n)).cloned().collect();
        if protocol.is_empty() {
            return Self {
                api: None,
                tags: HashMap::new(),
                temporary: None,
            };
        }

        // CoreManager 只在出站配置与已启动实例一致时提供 API 地址。
        // 还要核对实际 tag：导入 sing-box JSON 时 extra.tag 可能与内部节点 ID 不同。
        if let Some((port, secret)) = state.core.probe_api(profile).await {
            if let Some(api) = ClashApi::new(port, secret) {
                let tags: HashMap<String, String> = protocol
                    .iter()
                    .map(|node| {
                        let outbound = render::node_to_outbound_in_profile(node, profile);
                        let tag = outbound["tag"].as_str().unwrap_or(&node.id).to_string();
                        (node.id.clone(), tag)
                    })
                    .collect();
                if let Some(available) = api.tags(Duration::from_millis(350)).await {
                    if tags.values().all(|tag| available.contains(tag)) {
                        return Self {
                            api: Some(api),
                            tags,
                            temporary: None,
                        };
                    }
                }
            }
        }

        let nodes = render::expand_with_detours(&profile.nodes, &protocol);
        let temporary = TemporaryCore::start(state, &nodes).await;
        let api = temporary.as_ref().map(|core| core.api.clone());
        if api.is_none() {
            state.log_with(
                "latency",
                "warn",
                "测速核心不可用，请检查核心路径及节点配置",
            );
        }
        let tags = protocol
            .iter()
            .map(|n| (n.id.clone(), n.id.clone()))
            .collect();
        Self {
            api,
            tags,
            temporary,
        }
    }

    pub(super) fn label(&self) -> &'static str {
        if self.temporary.is_some() {
            "临时 sing-box + Clash delay API"
        } else if self.api.is_some() {
            "复用运行中的 sing-box + Clash delay API"
        } else {
            "HTTP/SOCKS 出口请求（协议节点需可用核心）"
        }
    }

    pub(super) fn method_for(&self, node: &Node) -> ProbeMethod {
        if !needs_core(node) {
            return ProbeMethod::Proxy;
        }
        match (self.api.as_ref(), self.tags.get(&node.id)) {
            (Some(api), Some(tag)) => ProbeMethod::Clash(api.clone(), tag.clone()),
            _ => ProbeMethod::Unavailable,
        }
    }

    pub(super) async fn shutdown(&mut self) {
        if let Some(core) = self.temporary.take() {
            core.shutdown().await;
        }
    }

    #[cfg(test)]
    pub(super) fn for_api(port: u16, secret: &str, nodes: &[Node]) -> Self {
        Self {
            api: ClashApi::new(port, secret.to_string()),
            tags: nodes.iter().map(|n| (n.id.clone(), n.id.clone())).collect(),
            temporary: None,
        }
    }

    #[cfg(test)]
    pub(super) fn temporary_path(&self) -> Option<PathBuf> {
        self.temporary.as_ref().map(|core| core.tmp_dir.clone())
    }
}

pub(super) enum ProbeMethod {
    Clash(ClashApi, String),
    Proxy,
    Unavailable,
}

impl ProbeMethod {
    pub(super) async fn measure(self, node: &Node, url: &str, timeout_ms: u64) -> Option<u32> {
        match self {
            Self::Clash(api, tag) => api.delay(&tag, url, timeout_ms).await,
            Self::Proxy => proxy_measure(node, url, Duration::from_millis(timeout_ms.max(1))).await,
            Self::Unavailable => None,
        }
    }
}

/// 每批共享一个 reqwest Client，复用本地 API 的连接池。
#[derive(Clone)]
pub(super) struct ClashApi {
    client: reqwest::Client,
    base: String,
    secret: String,
}

impl ClashApi {
    fn new(port: u16, secret: String) -> Option<Self> {
        Some(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .connect_timeout(Duration::from_millis(500))
                .build()
                .ok()?,
            base: format!("http://127.0.0.1:{port}"),
            secret,
        })
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        let request = self.client.get(format!("{}{path}", self.base));
        if self.secret.is_empty() {
            request
        } else {
            request.bearer_auth(&self.secret)
        }
    }

    async fn tags(&self, timeout: Duration) -> Option<HashSet<String>> {
        let response = self.get("/proxies").timeout(timeout).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let value: serde_json::Value = response.json().await.ok()?;
        Some(value.get("proxies")?.as_object()?.keys().cloned().collect())
    }

    async fn delay(&self, tag: &str, url: &str, timeout_ms: u64) -> Option<u32> {
        let timeout_ms = timeout_ms.max(500);
        let response = self
            .get(&format!("/proxies/{}/delay", encode_tag(tag)))
            .query(&[
                ("timeout", timeout_ms.to_string()),
                ("url", url.to_string()),
            ])
            // 额外 500ms 留给本地 API 返回超时响应，不再逐节点多等待 1.5s。
            .timeout(Duration::from_millis(timeout_ms.saturating_add(500)))
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let value: serde_json::Value = response.json().await.ok()?;
        value
            .get("delay")?
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0)
    }
}

async fn proxy_measure(node: &Node, url: &str, timeout: Duration) -> Option<u32> {
    let scheme = if matches!(node.kind.as_str(), "http" | "https") {
        "http"
    } else {
        "socks5h"
    };
    let server = if node.server.contains(':') && !node.server.starts_with('[') {
        format!("[{}]", node.server)
    } else {
        node.server.clone()
    };
    let mut proxy = reqwest::Proxy::all(format!("{scheme}://{server}:{}", node.port)).ok()?;
    if let Some(password) = &node.password {
        let username = node
            .extra
            .as_ref()
            .and_then(|e| e.get("username").or_else(|| e.get("user")))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        proxy = proxy.basic_auth(username, password);
    }
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .proxy(proxy)
        .build()
        .ok()?;
    let started = Instant::now();
    let response = tokio::time::timeout(timeout, client.get(url).send()).await;
    response.ok()?.ok()?;
    Some(started.elapsed().as_millis().min(u32::MAX as u128) as u32)
}

struct TemporaryCore {
    child: Option<tokio::process::Child>,
    api: ClashApi,
    tmp_dir: PathBuf,
}

impl TemporaryCore {
    async fn start(state: &AppState, nodes: &[Node]) -> Option<Self> {
        let binary = resolve_singbox_binary(state).await;
        if !binary.exists() || nodes.is_empty() {
            return None;
        }
        let port = pick_free_port()?;
        let api = ClashApi::new(port, String::new())?;
        let profile = AppProfile {
            nodes: nodes.to_vec(),
            ..Default::default()
        };
        let mut outbounds: Vec<serde_json::Value> = nodes
            .iter()
            .map(|node| {
                let mut outbound = render::node_to_outbound_in_profile(node, &profile);
                outbound["tag"] = serde_json::json!(node.id);
                outbound
            })
            .collect();
        outbounds.push(serde_json::json!({ "type": "direct", "tag": "direct" }));
        let config = serde_json::json!({
            "log": { "level": "error", "timestamp": true },
            "dns": {
                "servers": [{ "type": "udp", "tag": "local", "server": "223.5.5.5" }],
                "final": "local",
                "strategy": "prefer_ipv4"
            },
            "outbounds": outbounds,
            "route": {
                "final": "direct",
                "auto_detect_interface": true,
                "default_domain_resolver": { "server": "local" }
            },
            "experimental": { "clash_api": {
                "external_controller": format!("127.0.0.1:{port}"),
                "secret": ""
            }}
        });
        let tmp_dir = std::env::temp_dir().join(format!(
            "yap-xfish-probe-{}-{}",
            std::process::id(),
            crate::app::gen_id()
        ));
        std::fs::create_dir_all(&tmp_dir).ok()?;
        let path = tmp_dir.join("probe.json");
        if std::fs::write(&path, serde_json::to_vec(&config).ok()?).is_err() {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return None;
        }
        let mut command = tokio::process::Command::new(binary);
        command
            .arg("run")
            .arg("-c")
            .arg(&path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(_) => {
                let _ = std::fs::remove_dir_all(&tmp_dir);
                return None;
            }
        };
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(_)) = lines.next_line().await {}
            });
        }
        let mut session = Self {
            child: Some(child),
            api,
            tmp_dir,
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if session.child.as_mut()?.try_wait().ok()?.is_some() {
                break;
            }
            if session.api.tags(Duration::from_millis(200)).await.is_some() {
                return Some(session);
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        session.shutdown().await;
        None
    }

    async fn shutdown(mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        let _ = std::fs::remove_dir_all(&self.tmp_dir);
    }
}

impl Drop for TemporaryCore {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let path = self.tmp_dir.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = child.kill().await;
                let _ = child.wait().await;
                let _ = tokio::fs::remove_dir_all(path).await;
            });
        } else {
            let _ = child.start_kill();
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

async fn resolve_singbox_binary(state: &AppState) -> PathBuf {
    let configured = state.config.read().await.core_binary.clone();
    let exe_dir = crate::app::exe_dir();
    for path in [
        exe_dir.join("sing-box.exe"),
        exe_dir.join("core").join("sing-box.exe"),
    ] {
        if path.exists() {
            return path;
        }
    }
    let configured = if configured.is_absolute() {
        configured
    } else {
        exe_dir.join(configured)
    };
    if configured.exists() {
        return configured;
    }
    let data_core = state.data_dir.join("core").join("sing-box.exe");
    if data_core.exists() {
        data_core
    } else {
        state.data_dir.join("..").join("sing-box.exe")
    }
}

fn pick_free_port() -> Option<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
    Some(listener.local_addr().ok()?.port())
}

fn encode_tag(tag: &str) -> String {
    let mut encoded = String::new();
    for byte in tag.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}
