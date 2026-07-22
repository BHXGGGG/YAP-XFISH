use crate::config::model::{AppProfile, ProxyMode};
use crate::core::CoreManager;
use crate::error::AppResult;
use crate::subscription::model::Subscription;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, RwLock};

/// 应用级配置（端口、内核路径、数据目录等），持久化到 app_config.json。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Web 管理面板端口（默认 9527）。
    #[serde(default = "default_web_port")]
    pub web_port: u16,
    /// 核心程序路径；缺省时（旧配置/部分配置）回退为 exe 同目录 sing-box.exe。
    #[serde(default = "default_core_binary")]
    pub core_binary: PathBuf,
    /// 数据目录（展示用；运行时实际目录由 default_data_dir 决定）。缺省为空。
    #[serde(default)]
    pub data_dir: PathBuf,
    /// Clash API（控制面板）端口（默认 9999）。
    #[serde(default = "default_clash_api_port")]
    pub clash_api_port: u16,
    /// 代理入站端口（SOCKS5/HTTP 混合入口，供系统/浏览器代理设置填写）。
    /// 与 clash_api_port（控制面板端口）不同；默认 10020。
    #[serde(default = "default_proxy_port")]
    pub proxy_port: u16,
    /// Clash API 密钥；缺省为空（空时由 load_or_init 自动生成随机值）。
    #[serde(default)]
    pub api_secret: String,
    #[serde(default)]
    pub enable_tun: bool,
    #[serde(default)]
    pub autostart: bool,
    /// 延迟测试：测试 URL（多为可访问的轻量地址，如 gstatic generate_204）。
    #[serde(default = "default_latency_url")]
    pub latency_test_url: String,
    /// 延迟测试：并发数（同时探测的节点数）。
    #[serde(default = "default_latency_concurrency")]
    pub latency_concurrency: usize,
    /// 延迟测试：单次超时（毫秒）。
    #[serde(default = "default_latency_timeout")]
    pub latency_timeout: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        let data_dir = default_data_dir();
        // 自包含分发：核心默认与 exe 同目录，免去用户手动放置
        let core_binary = exe_dir().join("sing-box.exe");
        AppConfig {
            web_port: 9527,
            core_binary,
            data_dir: data_dir.clone(),
            clash_api_port: 9999,
            proxy_port: default_proxy_port(),
            api_secret: random_secret(),
            enable_tun: false,
            autostart: false,
            latency_test_url: default_latency_url(),
            latency_concurrency: default_latency_concurrency(),
            latency_timeout: default_latency_timeout(),
        }
    }
}

fn default_proxy_port() -> u16 {
    10020
}

fn default_web_port() -> u16 {
    9527
}

fn default_clash_api_port() -> u16 {
    9999
}

fn default_core_binary() -> PathBuf {
    exe_dir().join("sing-box.exe")
}

fn default_latency_url() -> String {
    "https://www.gstatic.com/generate_204".to_string()
}

fn default_latency_concurrency() -> usize {
    50
}

fn default_latency_timeout() -> u64 {
    5000
}

fn random_secret() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:016x}", n % 0xFFFF_FFFF_FFFF_FFFF)
}

/// 运行时状态（内存态，不持久化）。通过 WS 推送给浏览器。
#[derive(Debug, Clone, Default)]
pub struct RuntimeStatus {
    pub running: bool,
    pub mode: ProxyMode,
    pub current_node: Option<String>,
    pub traffic_up: u64,
    pub traffic_down: u64,
}

/// 通过 WebSocket 广播给所有已连接浏览器的事件。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppEvent {
    Status {
        running: bool,
        mode: ProxyMode,
        current_node: Option<String>,
    },
    Traffic {
        up: u64,
        down: u64,
    },
    Log {
        level: String,
        /// 日志来源：`core`（核心子进程）/`app`（后端业务）/`sub`（订阅）/`latency`/`config`/`http`/`net`。
        /// 前端按来源染色 / 分组，便于排查问题。
        #[serde(default)]
        source: String,
        message: String,
    },
    Subscription {
        id: String,
        status: String,
        progress: u8,
        message: String,
        /// 更新完成后的节点数（进行中为 None）。
        #[serde(default)]
        node_count: Option<usize>,
        /// 更新成功后的时间（RFC3339；进行中/失败为 None）。
        #[serde(default)]
        last_updated: Option<String>,
    },
    /// 节点延迟测试结果（id 为节点 ID，latency 为毫秒，None 表示超时/不可达）。
    Latency {
        id: String,
        latency: Option<u32>,
        message: String,
    },
    /// 配置模型整体更新（节点 / 选中节点 / 规则 / 模式）。前端据此实时刷新，无需手动刷新页面。
    Profile {
        profile: AppProfile,
    },
    /// 订阅列表整体更新（增删/批量操作后由后端推送，避免前端缓存与后端不一致）。
    SubscriptionsRefresh {
        subscriptions: Vec<Subscription>,
    },
}

/// 全局共享状态。所有字段均放在 Arc 内，AppState 本身可 Clone 以塞进 Axum State。
#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<AppConfig>>,
    pub profile: Arc<RwLock<AppProfile>>,
    pub status: Arc<RwLock<RuntimeStatus>>,
    pub core: Arc<CoreManager>,
    pub event_tx: broadcast::Sender<AppEvent>,
    pub subscriptions: Arc<RwLock<Vec<Subscription>>>,
    pub data_dir: PathBuf,
    /// 正在更新的订阅 ID 集合，防止定时调度与手动触发并发重复更新同一订阅。
    pub updating: Arc<Mutex<HashSet<String>>>,
}

impl AppState {
    pub fn new(config: AppConfig, profile: AppProfile, data_dir: PathBuf) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let current_node = profile.selected_node.clone();
        let mode = profile.mode;
        let core = CoreManager::new(
            config.core_binary.clone(),
            data_dir.join("config.json"),
            event_tx.clone(),
        );
        // 加载已持久化的订阅（阶段 5：重启后台后订阅不丢失）
        let subscriptions = crate::subscription::manager::load_subscriptions(&data_dir)
            .unwrap_or_default();
        AppState {
            config: Arc::new(RwLock::new(config)),
            profile: Arc::new(RwLock::new(profile)),
            status: Arc::new(RwLock::new(RuntimeStatus {
                mode,
                current_node,
                ..Default::default()
            })),
            core: Arc::new(core),
            event_tx,
            subscriptions: Arc::new(RwLock::new(subscriptions)),
            data_dir,
            updating: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// 广播一个事件给所有浏览器。
    pub fn emit(&self, ev: AppEvent) {
        let _ = self.event_tx.send(ev);
    }

    /// 便捷的日志广播：source 默认为 "app"。
    pub fn log(&self, level: &str, message: impl Into<String>) {
        self.log_with("app", level, message);
    }

    /// 带来源的日志广播。来源用于前端染色 / 分组（core / app / sub / latency / config / http / net）。
    pub fn log_with(&self, source: &str, level: &str, message: impl Into<String>) {
        let _ = self.event_tx.send(AppEvent::Log {
            level: level.to_string(),
            source: source.to_string(),
            message: message.into(),
        });
    }

    /// 广播最新配置模型（节点 / 选中节点 / 规则 / 模式），让前端实时刷新。
    /// 仅推送「启用订阅 + 手动节点」，停用订阅的节点不展示（其数据仍保留在持久化 profile 中）。
    pub async fn broadcast_profile(&self) {
        let p = self.visible_profile().await;
        self.emit(AppEvent::Profile { profile: p });
    }

    /// 返回仅含「启用订阅的节点 + 手动节点（无 subscription_id）」的 profile。
    /// 停用订阅的节点被排除出渲染与节点列表，但其数据保留在持久化 profile 中，
    /// 重新启用订阅（或重新更新）即可恢复。若当前选中节点属于停用订阅，则回退到
    /// 第一个可见节点（无可见节点则置空，渲染时落入 direct）。
    pub async fn visible_profile(&self) -> AppProfile {
        let p = self.profile.read().await;
        let subs = self.subscriptions.read().await;
        let enabled: std::collections::HashSet<String> = subs
            .iter()
            .filter(|s| s.enabled)
            .map(|s| s.id.clone())
            .collect();
        let mut vis = p.clone();
        vis.nodes.retain(|n| match &n.subscription_id {
            Some(sid) => enabled.contains(sid),
            None => true,
        });
        let valid = vis
            .selected_node
            .as_ref()
            .map(|id| vis.nodes.iter().any(|n| &n.id == id))
            .unwrap_or(false);
        if !valid {
            vis.selected_node = vis.nodes.first().map(|n| n.id.clone());
        }
        vis
    }
}

/// 生成进程内唯一 ID（订阅、规则等）。
pub fn gen_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static C: AtomicU64 = AtomicU64::new(0);
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{}", n, C.fetch_add(1, Ordering::Relaxed))
}

/// 默认数据目录（便携）：与 exe 同目录的 `data` 子目录。
/// 所有运行时产生的文件（app_config.json / profile.json / subscriptions.json / config.json / 日志等）
/// 都落在这一目录，删除整个程序文件夹即可清除全部数据。
/// 可用环境变量 `PROXY_RS_DATA_DIR` 覆盖（主要用于隔离冒烟测试），优先级最高。
pub fn default_data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("PROXY_RS_DATA_DIR") {
        return PathBuf::from(d);
    }
    exe_dir().join("data")
}

/// 首次使用便携 data 目录时，若旧版 `%LOCALAPPDATA%\Proxy` 存在且本目录尚未初始化，
/// 则一次性复制（复制、不删除）过来，避免已有订阅/配置丢失。
/// 仅在未用 `PROXY_RS_DATA_DIR` 覆盖、且旧目录确实含 app_config.json 时触发。
pub fn maybe_migrate_legacy(data_dir: &std::path::Path) {
    if std::env::var_os("PROXY_RS_DATA_DIR").is_some() {
        return; // 测试/自定义目录不触发迁移
    }
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        return;
    };
    let legacy = PathBuf::from(local).join("Proxy");
    if !legacy.join("app_config.json").exists() {
        return; // 旧目录无数据，无需迁移
    }
    if data_dir.join("app_config.json").exists() {
        return; // 已迁移或已有数据，跳过
    }
    if let Err(e) = copy_dir_all(&legacy, data_dir) {
        eprintln!("[proxy] 迁移旧数据失败（已忽略，将使用空配置）: {e}");
    }
}

/// 递归复制目录（用于一次性迁移旧数据）。
fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let dest = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_all(&path, &dest)?;
        } else {
            std::fs::copy(&path, &dest)?;
        }
    }
    Ok(())
}

/// 当前 exe 所在目录。自包含分发时使用它来定位同包内的 sing-box 核心，
/// 使 proxy-rs.exe 与 sing-box.exe 放在同一目录即可直接运行，无需额外配置。
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// 加载或初始化配置与配置模型。
pub fn load_or_init(data_dir: &std::path::Path) -> AppResult<(AppConfig, AppProfile)> {
    let config = crate::config::manager::load_app_config(data_dir)?;
    let profile = crate::config::manager::load_profile(data_dir)?;
    Ok((config, profile))
}
