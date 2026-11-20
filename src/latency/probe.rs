//! 探测连接：优先复用运行核心的 Mihomo API，配置不一致或核心未运行时使用临时核心。
use crate::app::AppState;
use crate::config::mihomo;
use crate::config::model::{AppProfile, Node};
use crate::core::api::MihomoApi;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};

fn needs_core(node: &Node) -> bool {
    let http_proxy = matches!(node.kind.as_str(), "http" | "https");
    let http_options = node.username.is_some()
        || node.password.is_some()
        || node.tls == Some(true)
        || node.extra.as_ref().is_some_and(|extra| {
            extra.get("username").is_some()
                || extra.get("password").is_some()
                || extra.get("tls").is_some()
        });
    !matches!(node.kind.as_str(), "socks" | "socks5" | "http" | "https")
        || (http_proxy && http_options)
        || node.detour().is_some()
}

pub(super) struct ProbeContext {
    api: Option<MihomoApi>,
    provider: bool,
    tags: HashMap<String, String>,
    temporary: Option<TemporaryCore>,
}

impl ProbeContext {
    pub(super) async fn prepare(state: &AppState, profile: &AppProfile, targets: &[Node]) -> Self {
        let protocol: Vec<Node> = targets.iter().filter(|n| needs_core(n)).cloned().collect();
        if protocol.is_empty() {
            return Self {
                api: None,
                provider: false,
                tags: HashMap::new(),
                temporary: None,
            };
        }

        // Reuse the controller only when the currently applied provider matches the profile.
        if let Some((port, secret)) = state.core.probe_api(profile).await {
            if let Ok(api) = MihomoApi::new(port, secret) {
                let tags: HashMap<String, String> = protocol
                    .iter()
                    .map(|node| (node.id.clone(), node.id.clone()))
                    .collect();
                if let Ok(available) = api.provider_proxy_names(mihomo::NODE_PROVIDER).await {
                    if tags.values().all(|tag| available.contains(tag)) {
                        return Self {
                            api: Some(api),
                            provider: true,
                            tags,
                            temporary: None,
                        };
                    }
                }
            }
        }

        let nodes = mihomo::expand_with_detours(&profile.nodes, &protocol);
        let temporary = TemporaryCore::start(state, &nodes).await;
        let api = temporary.as_ref().map(|core| core.api.clone());
        if api.is_none() {
            state.log_with(
                "latency",
                "warn",
                "测速核心不可用，请检查 Mihomo 路径及节点配置",
            );
        }
        let tags = protocol
            .iter()
            .map(|n| (n.id.clone(), n.id.clone()))
            .collect();
        Self {
            api,
            provider: false,
            tags,
            temporary,
        }
    }

    pub(super) async fn prepare_existing(
        state: &AppState,
        profile: &AppProfile,
        targets: &[Node],
    ) -> Option<Self> {
        let protocol: Vec<Node> = targets.iter().filter(|n| needs_core(n)).cloned().collect();
        if protocol.is_empty() {
            return Some(Self {
                api: None,
                provider: false,
                tags: HashMap::new(),
                temporary: None,
            });
        }

        let (port, secret) = state.core.probe_api(profile).await?;
        let api = MihomoApi::new(port, secret).ok()?;
        let tags: HashMap<String, String> = protocol
            .iter()
            .map(|node| (node.id.clone(), node.id.clone()))
            .collect();
        let available = api.provider_proxy_names(mihomo::NODE_PROVIDER).await.ok()?;
        if !tags.values().all(|tag| available.contains(tag)) {
            return None;
        }
        Some(Self {
            api: Some(api),
            provider: true,
            tags,
            temporary: None,
        })
    }

    pub(super) fn label(&self) -> &'static str {
        if self.temporary.is_some() {
            "临时 Mihomo + delay API"
        } else if self.api.is_some() {
            "复用运行中的 Mihomo + delay API"
        } else {
            "HTTP/SOCKS 出口请求（协议节点需可用核心）"
        }
    }

    pub(super) fn method_for(&self, node: &Node) -> ProbeMethod {
        if !needs_core(node) {
            return ProbeMethod::Proxy;
        }
        match (self.api.as_ref(), self.tags.get(&node.id)) {
            (Some(api), Some(tag)) if self.provider => ProbeMethod::Provider(api.clone(), tag.clone()),
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
            api: Some(MihomoApi::new(port, secret.to_string()).unwrap()),
            provider: false,
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
    Clash(MihomoApi, String),
    Provider(MihomoApi, String),
    Proxy,
    Unavailable,
}

impl ProbeMethod {
    pub(super) async fn measure(self, node: &Node, url: &str, timeout_ms: u64) -> Option<u32> {
        match self {
            Self::Clash(api, tag) => api.delay(&tag, url, timeout_ms).await,
            Self::Provider(api, tag) => api.provider_delay(mihomo::NODE_PROVIDER, &tag, url, timeout_ms).await,
            Self::Proxy => proxy_measure(node, url, Duration::from_millis(timeout_ms.max(1))).await,
            Self::Unavailable => None,
        }
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
    let username = node.username.as_deref().or_else(|| {
        node.extra
            .as_ref()
            .and_then(|e| e.get("username").or_else(|| e.get("user")))
            .and_then(|v| v.as_str())
    });
    let password = node.password.as_deref().or_else(|| {
        node.extra
            .as_ref()
            .and_then(|e| e.get("password"))
            .and_then(|v| v.as_str())
    });
    if let Some(password) = password {
        let username = username.unwrap_or_default();
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
    api: MihomoApi,
    tmp_dir: PathBuf,
}

impl TemporaryCore {
    async fn start(state: &AppState, nodes: &[Node]) -> Option<Self> {
        let configured = state.config.read().await.core_binary.clone();
        let binary = crate::core::CoreManager::resolve_core_binary(&configured, &state.data_dir);
        if !binary.exists() || nodes.is_empty() {
            return None;
        }
        let port = pick_free_port()?;
        let mixed_port = pick_free_port()?;
        let api = MihomoApi::new(port, String::new()).ok()?;
        let profile = AppProfile {
            nodes: nodes.to_vec(),
            ..Default::default()
        };
        let proxies: Vec<serde_json::Value> = nodes
            .iter()
            .map(|node| mihomo::node_to_proxy(node, &profile))
            .collect::<anyhow::Result<_>>()
            .ok()?;
        let mut proxy_names: Vec<serde_json::Value> = nodes
            .iter()
            .map(|node| serde_json::json!(node.id))
            .collect();
        proxy_names.push(serde_json::json!("DIRECT"));
        let config = serde_json::json!({
            "mixed-port": mixed_port,
            "allow-lan": false,
            "bind-address": "127.0.0.1",
            "mode": "global",
            "log-level": "error",
            "external-controller": format!("127.0.0.1:{port}"),
            "find-process-mode": "off",
            "dns": {
                "enable": true,
                "ipv6": true,
                "enhanced-mode": "redir-host",
                "default-nameserver": ["223.5.5.5"],
                "nameserver": ["223.5.5.5"]
            },
            "proxies": proxies,
            "proxy-groups": [
                { "name": "YAP-PROXY", "type": "select", "proxies": proxy_names },
                { "name": "GLOBAL", "type": "select", "proxies": ["YAP-PROXY"] }
            ],
            "rules": ["MATCH,YAP-PROXY"]
        });
        let tmp_dir = state.data_dir.join("tmp").join(format!(
            "yap-xfish-probe-{}-{}",
            std::process::id(),
            crate::app::gen_id()
        ));
        std::fs::create_dir_all(&tmp_dir).ok()?;
        let path = tmp_dir.join("probe.yaml");
        if std::fs::write(&path, serde_yaml::to_string(&config).ok()?).is_err() {
            let _ = std::fs::remove_dir_all(&tmp_dir);
            return None;
        }
        let mut command = tokio::process::Command::new(binary);
        command
            .arg("-d")
            .arg(&tmp_dir)
            .arg("-f")
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
            if session.api.proxy_names().await.is_ok() {
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

fn pick_free_port() -> Option<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
    Some(listener.local_addr().ok()?.port())
}
