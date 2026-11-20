use crate::app::{AppConfig, AppEvent};
use crate::config::manager as cfgmgr;
use crate::config::mihomo as renderer;
use crate::config::model::AppProfile;
use crate::core::{api::MihomoApi, process};
use crate::error::AppResult;
use anyhow::anyhow;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

#[derive(Clone)]
pub struct CoreManager {
    inner: Arc<CoreInner>,
}

struct CoreInner {
    config_path: PathBuf,
    elevated: bool,
    child: Mutex<Option<tokio::process::Child>>,
    applied: Mutex<Option<AppliedConfig>>,
    event_tx: tokio::sync::broadcast::Sender<AppEvent>,
    last_fatal: Arc<std::sync::Mutex<Option<String>>>,
    started_at: std::sync::Mutex<Option<Instant>>,
    logs_task: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
struct AppliedConfig {
    proxies: Vec<Value>,
    main: Value,
    selected_node: String,
    port: u16,
    secret: String,
    tun_enabled: bool,
    system_proxy: bool,
}

impl CoreManager {
    pub fn new(
        config_path: PathBuf,
        event_tx: tokio::sync::broadcast::Sender<AppEvent>,
    ) -> Self {
        Self {
            inner: Arc::new(CoreInner {
                config_path,
                elevated: crate::system::admin::is_elevated(),
                child: Mutex::new(None),
                applied: Mutex::new(None),
                event_tx,
                last_fatal: Arc::new(std::sync::Mutex::new(None)),
                started_at: std::sync::Mutex::new(None),
                logs_task: Mutex::new(None),
            }),
        }
    }

    pub async fn start(&self, profile: &AppProfile, app_cfg: &AppConfig) -> AppResult<()> {
        let runtime_cfg = self.runtime_config(app_cfg);
        let app_cfg = &runtime_cfg;
        if self.is_running().await {
            return Ok(());
        }
        if app_cfg.system_proxy {
            crate::system::sysproxy::disable()?;
        }
        check_listener_port(app_cfg.clash_api_port, 0)?;
        check_listener_port(app_cfg.proxy_port, 0)?;

        let home = self.home_dir();
        let provider_path = self.provider_path();
        let (config, provider) = render_files(profile, app_cfg)?;
        let old_config = std::fs::read(&self.inner.config_path).ok();
        let old_provider = std::fs::read(&provider_path).ok();
        if let Err(error) = self.write_files(&config, &provider) {
            restore_file(&self.inner.config_path, old_config.as_deref())?;
            restore_file(&provider_path, old_provider.as_deref())?;
            return Err(error);
        }

        let binary = Self::resolve_core_binary(&app_cfg.core_binary, &home);
        if !binary.exists() {
            restore_file(&self.inner.config_path, old_config.as_deref())?;
            restore_file(&provider_path, old_provider.as_deref())?;
            return Err(anyhow!(
                "未找到 Mihomo 核心程序 (mihomo.exe)。请确认 mihomo.exe 与 yap-xfish.exe 位于同一目录，或在设置中指定正确路径。实际查找路径: {}",
                binary.display()
            )
            .into());
        }
        if let Err(error) = process::validate(&binary, &home, &self.inner.config_path).await {
            restore_file(&self.inner.config_path, old_config.as_deref())?;
            restore_file(&provider_path, old_provider.as_deref())?;
            return Err(error);
        }

        let child = match self.spawn_captured(&binary, &home).await {
            Ok(child) => child,
            Err(error) => {
                restore_file(&self.inner.config_path, old_config.as_deref())?;
                restore_file(&provider_path, old_provider.as_deref())?;
                return Err(error);
            }
        };
        let pid = child.id();
        *self.inner.child.lock().await = Some(child);
        self.remember_applied(profile, app_cfg).await?;

        let api = MihomoApi::new(app_cfg.clash_api_port, app_cfg.api_secret.clone())?;
        let Some(version) = self.wait_for_api(&api, Duration::from_secs(8)).await else {
            let fatal = self.take_last_fatal().unwrap_or_else(|| "未收到核心日志".into());
            let _ = self.stop().await;
            restore_file(&self.inner.config_path, old_config.as_deref())?;
            restore_file(&provider_path, old_provider.as_deref())?;
            return Err(anyhow!("Mihomo 未能启动控制 API: {fatal}").into());
        };

        let selected = selected_proxy(profile);
        if let Err(error) = api.select_proxy(renderer::NODE_GROUP, selected).await {
            let _ = self.stop().await;
            restore_file(&self.inner.config_path, old_config.as_deref())?;
            restore_file(&provider_path, old_provider.as_deref())?;
            return Err(error.into());
        }
        if app_cfg.system_proxy {
            if let Err(error) = crate::system::sysproxy::enable(app_cfg.proxy_port) {
                let _ = self.stop().await;
                return Err(error.into());
            }
        }

        self.start_log_monitor(api).await;
        let _ = self.inner.event_tx.send(AppEvent::Log {
            level: "info".into(),
            source: "core".into(),
            message: format!("Mihomo 已启动: version={version} pid={}", pid.unwrap_or_default()),
        });
        Ok(())
    }

    pub async fn stop(&self) -> AppResult<()> {
        let applied = self.inner.applied.lock().await.clone();
        let proxy_cleanup = if applied.as_ref().is_some_and(|applied| applied.system_proxy) {
            crate::system::sysproxy::disable()
        } else {
            Ok(())
        };
        let mut cleanup_error: Option<anyhow::Error> = None;
        if let Some(applied) = &applied {
            if applied.tun_enabled {
                match MihomoApi::new(applied.port, applied.secret.clone()) {
                    Ok(api) => {
                        if let Err(error) = self.disable_tun_before_stop(&api).await {
                            cleanup_error = Some(error.into());
                        }
                    }
                    Err(error) => cleanup_error = Some(error),
                }
            }
        }

        self.stop_log_monitor().await;
        if let Some(mut child) = self.inner.child.lock().await.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        *self.inner.applied.lock().await = None;
        *self.inner.started_at.lock().unwrap_or_else(|e| e.into_inner()) = None;
        proxy_cleanup?;

        if let Some(error) = cleanup_error {
            self.log("warn", format!("关闭 Mihomo TUN 时 API 清理失败: {error}"));
            return Err(error.into());
        }
        Ok(())
    }

    pub async fn restart(&self, profile: &AppProfile, app_cfg: &AppConfig) -> AppResult<()> {
        let was_running = self.is_running().await;
        self.stop().await?;
        self.start(profile, app_cfg).await?;
        if was_running {
            notify_core_restarted();
        }
        Ok(())
    }

    pub async fn reload(&self, profile: &AppProfile, app_cfg: &AppConfig) -> AppResult<()> {
        self.reload_safe(profile, app_cfg).await
    }

    /// Reload configuration through the controller so TUN changes close the old device first.
    pub async fn reload_safe(&self, profile: &AppProfile, app_cfg: &AppConfig) -> AppResult<()> {
        let runtime_cfg = self.runtime_config(app_cfg);
        let app_cfg = &runtime_cfg;
        if !self.is_running().await {
            return self.write_config_only(profile, app_cfg).await;
        }

        let old_applied = self.inner.applied.lock().await.clone();
        if let Some(applied) = &old_applied {
            if applied.port != app_cfg.clash_api_port || applied.secret != app_cfg.api_secret {
                return Err(anyhow!("Mihomo 控制端口或密钥变化需要重启核心").into());
            }
            if applied.main == renderer::render(profile, app_cfg)? && applied.proxies == profile_proxies(profile)? {
                if applied.selected_node != selected_proxy(profile) {
                    self.select_node(selected_proxy(profile), app_cfg).await?;
                }
                sync_system_proxy(app_cfg, applied.system_proxy)?;
                self.remember_applied(profile, app_cfg).await?;
                return Ok(());
            }
            check_listener_port(app_cfg.proxy_port, applied.main["mixed-port"].as_u64().unwrap_or_default() as u16)?;
        }

        let provider_path = self.provider_path();
        let old_config = std::fs::read(&self.inner.config_path).ok();
        let old_provider = std::fs::read(&provider_path).ok();
        let (config, provider) = render_files(profile, app_cfg)?;
        cfgmgr::backup_config(&self.inner.config_path)?;
        if let Err(error) = self.write_files(&config, &provider) {
            restore_file(&self.inner.config_path, old_config.as_deref())?;
            restore_file(&provider_path, old_provider.as_deref())?;
            return Err(error);
        }

        let home = self.home_dir();
        let binary = Self::resolve_core_binary(&app_cfg.core_binary, &home);
        if !binary.exists() {
            restore_file(&self.inner.config_path, old_config.as_deref())?;
            restore_file(&provider_path, old_provider.as_deref())?;
            return Err(anyhow!("未找到 Mihomo 核心程序: {}", binary.display()).into());
        }
        if let Err(error) = process::validate(&binary, &home, &self.inner.config_path).await {
            restore_file(&self.inner.config_path, old_config.as_deref())?;
            restore_file(&provider_path, old_provider.as_deref())?;
            return Err(error);
        }

        let old_api = old_applied
            .as_ref()
            .and_then(|applied| MihomoApi::new(applied.port, applied.secret.clone()).ok())
            .unwrap_or(MihomoApi::new(
                app_cfg.clash_api_port,
                app_cfg.api_secret.clone(),
            )?);
        let result = async {
            old_api.reload_config(&self.inner.config_path).await?;
            let version = self.wait_for_api(&old_api, Duration::from_secs(8)).await
                .ok_or_else(|| anyhow!("Mihomo 配置重载后控制 API 未恢复"))?;
            old_api.select_proxy(renderer::NODE_GROUP, selected_proxy(profile)).await?;
            sync_system_proxy(app_cfg, old_applied.as_ref().is_some_and(|applied| applied.system_proxy))?;
            Ok::<_, anyhow::Error>(version)
        }.await;
        let version = match result {
            Ok(version) => version,
            Err(error) => {
                restore_file(&self.inner.config_path, old_config.as_deref())?;
                restore_file(&provider_path, old_provider.as_deref())?;
                let restored = async {
                    old_api.reload_config(&self.inner.config_path).await?;
                    if let Some(applied) = &old_applied {
                        old_api.select_proxy(renderer::NODE_GROUP, &applied.selected_node).await?;
                    }
                    Ok::<_, anyhow::Error>(())
                }.await;
                if let Err(restore_error) = restored {
                    return Err(anyhow!("重载失败: {error}；恢复旧配置失败: {restore_error}").into());
                }
                return Err(error.into());
            }
        };
        self.remember_applied(profile, app_cfg).await?;
        self.start_log_monitor(old_api).await;
        self.log("info", format!("Mihomo 配置已重载，核心进程保持运行: {version}"));
        Ok(())
    }

    pub async fn write_config_only(
        &self,
        profile: &AppProfile,
        app_cfg: &AppConfig,
    ) -> AppResult<()> {
        let runtime_cfg = self.runtime_config(app_cfg);
        let app_cfg = &runtime_cfg;
        let (config, provider) = render_files(profile, app_cfg)?;
        let old_config = std::fs::read(&self.inner.config_path).ok();
        let old_provider = std::fs::read(self.provider_path()).ok();
        cfgmgr::backup_config(&self.inner.config_path)?;
        let result = async {
            self.write_files(&config, &provider)?;
            let home = self.home_dir();
            let binary = Self::resolve_core_binary(&app_cfg.core_binary, &home);
            if binary.exists() {
                process::validate(&binary, &home, &self.inner.config_path).await?;
            }
            Ok(())
        }.await;
        if let Err(error) = result {
            restore_file(&self.inner.config_path, old_config.as_deref())?;
            restore_file(&self.provider_path(), old_provider.as_deref())?;
            return Err(error);
        }
        Ok(())
    }

    /// Refresh the local file provider in place; the Mihomo process is not restarted.
    pub async fn update_provider(
        &self,
        profile: &AppProfile,
        app_cfg: &AppConfig,
    ) -> AppResult<()> {
        let runtime_cfg = self.runtime_config(app_cfg);
        let app_cfg = &runtime_cfg;
        let running = self.is_running().await;
        if !running {
            return self.write_config_only(profile, app_cfg).await;
        }
        let rendered_provider = renderer::render_provider(profile)?;
        let has_proxies = rendered_provider["proxies"]
            .as_array()
            .is_some_and(|proxies| !proxies.is_empty());
        let applied = self.inner.applied.lock().await.clone();
        let Some(applied) = applied else {
            return self.reload_safe(profile, app_cfg).await;
        };
        if applied.main != renderer::render(profile, app_cfg)? {
            return self.reload_safe(profile, app_cfg).await;
        }

        let provider_path = self.provider_path();
        let previous = std::fs::read(&provider_path).ok();
        let provider = serde_yaml::to_string(&rendered_provider)?;
        if let Some(parent) = provider_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Err(error) = std::fs::write(&provider_path, provider) {
            restore_file(&provider_path, previous.as_deref())?;
            return Err(error.into());
        }

        if running {
            let api = self.applied_api(app_cfg).await?;
            let selected = selected_proxy(profile);
            let update = async {
                if has_proxies {
                    api.update_provider(renderer::NODE_PROVIDER).await?;
                }
                api.select_proxy(renderer::NODE_GROUP, selected).await
            }
            .await;
            if let Err(error) = update {
                restore_file(&provider_path, previous.as_deref())?;
                let restored = async {
                    if has_proxies {
                        api.update_provider(renderer::NODE_PROVIDER).await?;
                    }
                    api.select_proxy(renderer::NODE_GROUP, &applied.selected_node).await
                }.await;
                if let Err(restore_error) = restored {
                    return Err(anyhow!("刷新节点失败: {error}；恢复旧节点失败: {restore_error}").into());
                }
                return Err(error.into());
            }
            self.remember_applied(profile, app_cfg).await?;
        }
        Ok(())
    }

    pub async fn select_node(&self, node_id: &str, app_cfg: &AppConfig) -> AppResult<()> {
        let applied = self.inner.applied.lock().await.clone();
        let (port, secret) = applied
            .map(|applied| (applied.port, applied.secret))
            .unwrap_or((app_cfg.clash_api_port, app_cfg.api_secret.clone()));
        MihomoApi::new(port, secret)?
            .select_proxy(renderer::NODE_GROUP, node_id)
            .await?;
        if let Some(applied) = self.inner.applied.lock().await.as_mut() {
            applied.selected_node = node_id.to_string();
        }
        Ok(())
    }

    pub async fn version(&self, app_cfg: &AppConfig) -> Option<String> {
        if !self.is_running().await {
            return None;
        }
        let applied = self.inner.applied.lock().await.clone();
        let api = match applied {
            Some(applied) => MihomoApi::new(applied.port, applied.secret).ok()?,
            None => MihomoApi::new(app_cfg.clash_api_port, app_cfg.api_secret.clone()).ok()?,
        };
        api.version().await.ok()
    }

    pub async fn is_running(&self) -> bool {
        let exited = {
            let mut child = self.inner.child.lock().await;
            match child.as_mut().map(|child| child.try_wait()) {
                Some(Ok(Some(_))) => {
                    *child = None;
                    true
                }
                Some(Ok(None)) | Some(Err(_)) => false,
                None => return false,
            }
        };
        if exited {
            self.stop_log_monitor().await;
            let applied = self.inner.applied.lock().await.take();
            if applied.is_some_and(|applied| applied.system_proxy) {
                if let Err(error) = crate::system::sysproxy::disable() {
                    self.log("error", format!("核心退出后关闭系统代理失败: {error}"));
                }
            }
            *self.inner.started_at.lock().unwrap_or_else(|e| e.into_inner()) = None;
            false
        } else {
            true
        }
    }

    /// The latency path may reuse the running API only when its proxy definitions match.
    pub async fn probe_api(&self, profile: &AppProfile) -> Option<(u16, String)> {
        if !self.is_running().await {
            return None;
        }
        let proxies = profile_proxies(profile).ok()?;
        let applied = self.inner.applied.lock().await;
        let applied = applied.as_ref()?;
        (applied.proxies == proxies).then(|| (applied.port, applied.secret.clone()))
    }

    pub async fn uptime_and_pid(&self) -> Option<(Duration, Option<u32>)> {
        let child = self.inner.child.lock().await;
        let pid = child.as_ref()?.id();
        let started = (*self.inner.started_at.lock().unwrap_or_else(|e| e.into_inner()))?;
        Some((started.elapsed(), pid))
    }

    pub fn take_last_fatal(&self) -> Option<String> {
        self.inner
            .last_fatal
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    fn runtime_config(&self, cfg: &AppConfig) -> AppConfig {
        let mut cfg = cfg.clone();
        cfg.enable_tun &= self.inner.elevated;
        cfg
    }

    fn home_dir(&self) -> PathBuf {
        self.inner
            .config_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(crate::app::default_data_dir)
    }

    fn provider_path(&self) -> PathBuf {
        self.home_dir().join(renderer::NODE_PROVIDER_PATH)
    }

    pub(crate) fn resolve_core_binary(configured: &Path, home: &Path) -> PathBuf {
        let executable_dir = crate::app::exe_dir();
        let configured = if configured.is_absolute() {
            configured.to_path_buf()
        } else {
            executable_dir.join(configured)
        };
        if configured.exists() {
            return configured;
        }
        for candidate in [
            executable_dir.join("mihomo.exe"),
            executable_dir.join("core").join("mihomo.exe"),
        ] {
            if candidate.exists() {
                return candidate;
            }
        }
        let fallback = home.join("core").join("mihomo.exe");
        if fallback.exists() {
            return fallback;
        }
        configured
    }

    async fn spawn_captured(&self, binary: &Path, home: &Path) -> AppResult<tokio::process::Child> {
        *self.inner.last_fatal.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.inner.started_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        let mut child = process::spawn(binary, home, &self.inner.config_path).await?;
        if let Some(stdout) = child.stdout.take() {
            spawn_error_reader(stdout, self.inner.last_fatal.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            spawn_error_reader(stderr, self.inner.last_fatal.clone());
        }
        Ok(child)
    }

    async fn wait_for_api(&self, api: &MihomoApi, timeout: Duration) -> Option<String> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if !self.is_running().await {
                return None;
            }
            if let Ok(Ok(version)) = tokio::time::timeout(Duration::from_millis(600), api.version()).await {
                return Some(version);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        None
    }

    async fn remember_applied(&self, profile: &AppProfile, cfg: &AppConfig) -> AppResult<()> {
        let proxies = profile_proxies(profile)?;
        *self.inner.applied.lock().await = Some(AppliedConfig {
            proxies,
            main: renderer::render(profile, cfg)?,
            selected_node: selected_proxy(profile).to_string(),
            port: cfg.clash_api_port,
            secret: cfg.api_secret.clone(),
            tun_enabled: cfg.enable_tun,
            system_proxy: cfg.system_proxy,
        });
        Ok(())
    }

    async fn start_log_monitor(&self, api: MihomoApi) {
        self.stop_log_monitor().await;
        let manager = self.clone();
        let tx = self.inner.event_tx.clone();
        let task = tokio::spawn(async move {
            loop {
                if !manager.is_running().await {
                    break;
                }
                let _ = api.stream_logs(tx.clone()).await;
                if !manager.is_running().await {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
        *self.inner.logs_task.lock().await = Some(task);
    }

    async fn stop_log_monitor(&self) {
        if let Some(task) = self.inner.logs_task.lock().await.take() {
            task.abort();
        }
    }

    async fn disable_tun_before_stop(&self, api: &MihomoApi) -> AppResult<()> {
        let text = std::fs::read_to_string(&self.inner.config_path)?;
        let mut config: Value = serde_yaml::from_str(&text)?;
        if !disable_tun_value(&mut config) {
            return Ok(());
        }
        let original = text.into_bytes();
        cfgmgr::backup_config(&self.inner.config_path)?;
        std::fs::write(&self.inner.config_path, serde_yaml::to_string(&config)?)?;
        if let Err(error) = api.reload_config(&self.inner.config_path).await {
            restore_file(&self.inner.config_path, Some(&original))?;
            return Err(error.into());
        }
        Ok(())
    }

    fn write_files(&self, config: &str, provider: &str) -> AppResult<()> {
        let provider_path = self.provider_path();
        if let Some(parent) = provider_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir_all(self.home_dir())?;
        std::fs::write(&provider_path, provider)?;
        std::fs::write(&self.inner.config_path, config)?;
        Ok(())
    }

    async fn applied_api(&self, fallback: &AppConfig) -> AppResult<MihomoApi> {
        let applied = self.inner.applied.lock().await.clone();
        match applied {
            Some(applied) => Ok(MihomoApi::new(applied.port, applied.secret)?),
            None => Ok(MihomoApi::new(fallback.clash_api_port, fallback.api_secret.clone())?),
        }
    }

    fn log(&self, level: &str, message: String) {
        let _ = self.inner.event_tx.send(AppEvent::Log {
            level: level.into(),
            source: "core".into(),
            message,
        });
    }
}

fn check_listener_port(port: u16, previous: u16) -> AppResult<()> {
    if port == 0 {
        return Err(anyhow!("代理端口和控制 API 端口不能为 0").into());
    }
    if port != previous {
        std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .map_err(|error| anyhow!("端口 {port} 无法监听，请更换端口: {error}"))?;
    }
    Ok(())
}

fn sync_system_proxy(cfg: &AppConfig, was_enabled: bool) -> anyhow::Result<()> {
    if cfg.system_proxy {
        crate::system::sysproxy::enable(cfg.proxy_port)
    } else if was_enabled {
        crate::system::sysproxy::disable()
    } else {
        Ok(())
    }
}

fn render_files(profile: &AppProfile, app_cfg: &AppConfig) -> AppResult<(String, String)> {
    let main = serde_yaml::to_string(&renderer::render(profile, app_cfg)?)?;
    let provider = serde_yaml::to_string(&renderer::render_provider(profile)?)?;
    Ok((main, provider))
}

fn restore_file(path: &Path, contents: Option<&[u8]>) -> AppResult<()> {
    match contents {
        Some(contents) => std::fs::write(path, contents)?,
        None if path.exists() => std::fs::remove_file(path)?,
        None => {}
    }
    Ok(())
}

fn disable_tun_value(config: &mut Value) -> bool {
    let Some(tun) = config.get_mut("tun").and_then(Value::as_object_mut) else {
        return false;
    };
    if tun.get("enable").and_then(Value::as_bool) != Some(true) {
        return false;
    }
    tun.insert("enable".into(), json!(false));
    tun.insert("dns-mode".into(), json!("native"));
    tun.insert("strict-route".into(), json!(false));
    tun.remove("dns-hijack");
    true
}

fn selected_proxy(profile: &AppProfile) -> &str {
    profile.selected_node.as_deref()
        .filter(|id| profile.nodes.iter().any(|node| node.id == *id))
        .unwrap_or("DIRECT")
}

fn profile_proxies(profile: &AppProfile) -> AppResult<Vec<Value>> {
    Ok(renderer::render_provider(profile)?["proxies"]
        .as_array()
        .cloned()
        .unwrap_or_default())
}

fn spawn_error_reader(stream: impl AsyncRead + Unpin + Send + 'static, last_fatal: Arc<std::sync::Mutex<Option<String>>>) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let value = line.trim_end_matches(['\r', '\n']);
                    let upper = value.to_ascii_uppercase();
                    if upper.contains("FATAL") || upper.contains("PANIC") {
                        *last_fatal.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(strip_ansi(value));
                    }
                }
                Err(_) => break,
            }
        }
    });
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for x in chars.by_ref() {
                if ('@'..='~').contains(&x) {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn notify_core_restarted() {
    crate::system::notify::toast("YAP-XFISH：Mihomo 核心已重启", "代理核心已重新启动。");
}

/// FATAL 是否为配置错误，而非网络错误。
pub fn is_config_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    let network = [
        "timeout",
        "timed out",
        "connection",
        "connectex",
        "refused",
        "wsarecv",
        "dial",
        "handshake",
        "no such host",
        "eof",
    ];
    !network.iter().any(|needle| message.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, server: &str) -> crate::config::model::Node {
        crate::config::model::Node {
            id: id.into(),
            name: id.into(),
            kind: "shadowsocks".into(),
            server: server.into(),
            port: 443,
            cipher: Some("aes-128-gcm".into()),
            password: Some("secret".into()),
            ..Default::default()
        }
    }

    #[test]
    fn provider_comparison_ignores_latency_and_ui_selection() {
        let mut before = AppProfile {
            nodes: vec![node("n1", "one.example")],
            selected_node: Some("n1".into()),
            ..Default::default()
        };
        let applied = profile_proxies(&before).unwrap();
        before.selected_node = None;
        before.nodes[0].latency = Some(24);
        assert_eq!(applied, profile_proxies(&before).unwrap());
    }

    #[test]
    fn provider_comparison_detects_connection_changes() {
        let before = AppProfile {
            nodes: vec![node("n1", "one.example")],
            ..Default::default()
        };
        let after = AppProfile {
            nodes: vec![node("n1", "two.example")],
            ..Default::default()
        };
        assert_ne!(
            profile_proxies(&before).unwrap(),
            profile_proxies(&after).unwrap()
        );
    }

    #[test]
    fn shutdown_config_disables_tun_and_dns_hijack() {
        let mut config = json!({
            "tun": {
                "enable": true,
                "dns-mode": "hijack",
                "strict-route": true,
                "dns-hijack": ["any:53"]
            }
        });
        assert!(disable_tun_value(&mut config));
        assert_eq!(config["tun"]["enable"], false);
        assert_eq!(config["tun"]["dns-mode"], "native");
        assert_eq!(config["tun"]["strict-route"], false);
        assert!(config["tun"].get("dns-hijack").is_none());
    }

    #[test]
    fn classifies_network_failures_separately() {
        assert!(!is_config_error("dial tcp: i/o timeout"));
        assert!(is_config_error("failed to parse proxy provider entry"));
    }

    async fn check_pending_tun(binary: Option<PathBuf>) {
        let dir = std::env::temp_dir().join(format!("mihomo-pending-tun-{}", crate::app::gen_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let port = || std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let cfg = AppConfig { data_dir:dir.clone(), core_binary:binary.clone().unwrap_or_else(|| dir.join("missing-mihomo.exe")),
            proxy_port:port(), clash_api_port:port(), web_port:port(), system_proxy:false, enable_tun:false, ..Default::default() };
        let mut profile = AppProfile { nodes:vec![node("n1", "127.0.0.1")], selected_node:Some("n1".into()), ..Default::default() };
        let mut state = crate::app::AppState::new(cfg.clone(), profile.clone(), dir.clone());
        Arc::get_mut(&mut Arc::get_mut(&mut state.core).unwrap().inner).unwrap().elevated = false;
        let assert_tun_disabled = || {
            let main: Value = serde_yaml::from_str(&std::fs::read_to_string(dir.join("mihomo.yaml")).unwrap()).unwrap();
            assert_eq!(main["tun"]["enable"], false);
        };
        let result = async {
            let mut pending = cfg;
            pending.enable_tun = true;
            state.apply_config(pending.clone()).await?;
            assert_tun_disabled();
            let saved: AppConfig = serde_json::from_slice(&std::fs::read(dir.join("app_config.json"))?)?;
            anyhow::ensure!(saved.enable_tun && state.config.read().await.enable_tun);
            if binary.is_some() {
                state.core.start(&profile, &pending).await?;
                assert_tun_disabled();
            }
            profile.mode = crate::config::model::ProxyMode::Direct;
            state.core.reload_safe(&profile, &pending).await?;
            assert_tun_disabled();
            profile.nodes.push(node("n2", "127.0.0.2"));
            state.core.update_provider(&profile, &pending).await?;
            assert_tun_disabled();
            if binary.is_some() {
                let api = MihomoApi::new(pending.clash_api_port, pending.api_secret.clone())?;
                anyhow::ensure!(api.get_json("/configs").await?["tun"]["enable"] == false);
                anyhow::ensure!(!state.core.inner.applied.lock().await.as_ref().unwrap().tun_enabled);
                state.core.stop().await?;
                state.core.start(&profile, &pending).await?;
                assert_tun_disabled();
            }
            Ok::<_, anyhow::Error>(())
        }.await;
        let stopped = state.core.stop().await;
        std::fs::remove_dir_all(&dir).unwrap();
        result.unwrap();
        stopped.unwrap();
    }

    #[tokio::test]
    async fn pending_tun_stays_disabled_in_all_generated_configs() {
        check_pending_tun(None).await;
    }

    #[tokio::test]
    #[ignore = "requires MIHOMO_TEST_BINARY; simulates an unelevated app without enabling TUN"]
    async fn pending_tun_stays_disabled_across_live_core_operations() {
        check_pending_tun(Some(std::env::var_os("MIHOMO_TEST_BINARY").unwrap().into())).await;
    }

    #[tokio::test]
    #[ignore = "requires MIHOMO_TEST_BINARY pointing to a Mihomo executable"]
    async fn failed_reload_restores_files_and_the_active_selection() {
        let dir = std::env::temp_dir().join(format!("mihomo-rollback-test-{}", crate::app::gen_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let port = || std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let cfg = AppConfig { data_dir:dir.clone(), core_binary:std::env::var_os("MIHOMO_TEST_BINARY").unwrap().into(),
            proxy_port:port(), clash_api_port:port(), system_proxy:false, enable_tun:false, ..Default::default() };
        let mut profile = AppProfile { nodes:vec![node("n1", "127.0.0.1"), node("n2", "127.0.0.2")], selected_node:Some("n1".into()), ..Default::default() };
        let state = crate::app::AppState::new(cfg.clone(), profile.clone(), dir.clone());
        let result = async {
            state.core.start(&profile, &cfg).await?;
            state.core.select_node("n2", &cfg).await?;
            profile.selected_node = Some("n2".into());
            let pid = state.core.uptime_and_pid().await.and_then(|(_, pid)| pid);
            let config_before = std::fs::read(dir.join("mihomo.yaml"))?;
            let provider_before = std::fs::read(state.core.provider_path())?;
            let api = MihomoApi::new(cfg.clash_api_port, cfg.api_secret.clone())?;
            let mut invalid = AppProfile { nodes:vec![node("n3", "127.0.0.3")], selected_node:Some("n3".into()), mode:crate::config::model::ProxyMode::Direct, ..Default::default() };
            invalid.nodes[0].cipher = Some("not-a-cipher".into());
            anyhow::ensure!(state.core.reload_safe(&invalid, &cfg).await.is_err());
            anyhow::ensure!(std::fs::read(dir.join("mihomo.yaml"))? == config_before);
            anyhow::ensure!(std::fs::read(state.core.provider_path())? == provider_before);
            anyhow::ensure!(api.get_json("/proxies/YAP-PROXY").await?["now"] == "n2", "failed reload changed the active proxy");

            invalid.mode = profile.mode;
            anyhow::ensure!(state.core.update_provider(&invalid, &cfg).await.is_err());
            anyhow::ensure!(api.get_json("/proxies/YAP-PROXY").await?["now"] == "n2");
            anyhow::ensure!(std::fs::read(state.core.provider_path())? == provider_before);
            profile.selected_node = Some("n1".into());
            state.core.reload_safe(&profile, &cfg).await?;
            anyhow::ensure!(api.get_json("/proxies/YAP-PROXY").await?["now"] == "n1");
            anyhow::ensure!(state.core.uptime_and_pid().await.and_then(|(_, pid)| pid) == pid);
            Ok::<_, anyhow::Error>(())
        }.await;
        let stopped = state.core.stop().await;
        std::fs::remove_dir_all(&dir).unwrap();
        result.unwrap();
        stopped.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires MIHOMO_TEST_BINARY pointing to a Mihomo executable"]
    async fn provider_rules_chains_and_healthchecks_use_live_nodes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use crate::config::model::{Node, ProxyMode, RouteRule};
        let dir = std::env::temp_dir().join(format!("mihomo-chain-test-{}", crate::app::gen_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let front_port = listener.local_addr().unwrap().port();
        let (calls, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let server = tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let calls = calls.clone();
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    loop {
                        let Ok(byte) = stream.read_u8().await else { return };
                        request.push(byte);
                        if request.ends_with(b"\r\n\r\n") {
                            let text = String::from_utf8_lossy(&request).to_string();
                            let _ = calls.send(text.clone());
                            if text.starts_with("CONNECT ") {
                                if stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.is_err() { return; }
                                request.clear();
                            } else {
                                tokio::time::sleep(Duration::from_millis(10)).await;
                                let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n").await;
                                return;
                            }
                        }
                    }
                });
            }
        });
        let port = || std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let cfg = AppConfig {
            data_dir: dir.clone(), core_binary: std::env::var_os("MIHOMO_TEST_BINARY").unwrap().into(),
            proxy_port: port(), clash_api_port: port(), system_proxy: false, enable_tun: false,
            ..Default::default()
        };
        let front = Node { id: "front".into(), name: "front".into(), kind: "http".into(), server: "127.0.0.1".into(), port: front_port, ..Default::default() };
        let exit = Node { id: "exit".into(), name: "exit".into(), kind: "http".into(), server: "127.0.0.2".into(), port: 23456, detour: Some("front".into()), ..Default::default() };
        let mut profile = AppProfile { nodes: vec![front, exit], rules: vec![RouteRule {
            id: "ports".into(), name: "ports".into(), r#type: "port".into(), payload: "18080,18081".into(), outbound: "exit".into(),
        }], selected_node: None, mode: ProxyMode::Global };
        let state = crate::app::AppState::new(cfg.clone(), profile.clone(), dir.clone());
        let result = async {
            state.core.start(&profile, &cfg).await?;
            let pid = state.core.uptime_and_pid().await.and_then(|(_, pid)| pid);
            let client = reqwest::Client::builder().no_proxy().proxy(reqwest::Proxy::http(format!("http://127.0.0.1:{}", cfg.proxy_port))?).timeout(Duration::from_secs(3)).build()?;
            for mode in [ProxyMode::Global, ProxyMode::Direct] {
                profile.mode = mode;
                state.core.reload_safe(&profile, &cfg).await?;
                for target_port in [18080, 18081] {
                    let response = client.get(format!("http://127.0.0.1:{target_port}/probe")).send().await?;
                    anyhow::ensure!(response.status() == 204, "rule/chain failed in {mode:?}: {}", response.status());
                }
            }
            let api = MihomoApi::new(cfg.clash_api_port, cfg.api_secret.clone())?;
            anyhow::ensure!(api.provider_proxy_names(renderer::NODE_PROVIDER).await?.contains(&"exit".into()));
            anyhow::ensure!(api.provider_delay(renderer::NODE_PROVIDER, "exit", "http://127.0.0.1:18080/probe", 2000).await.is_some());
            let mut connected = false;
            while let Ok(request) = requests.try_recv() {
                connected |= request.starts_with("CONNECT 127.0.0.2:23456 ");
            }
            anyhow::ensure!(connected, "front proxy did not receive the chain CONNECT");
            anyhow::ensure!(state.core.uptime_and_pid().await.and_then(|(_, pid)| pid) == pid);
            Ok::<_, anyhow::Error>(())
        }.await;
        let stopped = state.core.stop().await;
        server.abort();
        let _ = server.await;
        std::fs::remove_dir_all(&dir).unwrap();
        result.unwrap();
        stopped.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires MIHOMO_TEST_BINARY pointing to a Mihomo executable"]
    async fn provider_refresh_and_config_reload_keep_the_core_process() {
        let binary = PathBuf::from(std::env::var_os("MIHOMO_TEST_BINARY").expect("MIHOMO_TEST_BINARY"));
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("mihomo-api-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let port = || {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);
            port
        };
        let config = AppConfig {
            core_binary: binary,
            data_dir: dir.clone(),
            clash_api_port: port(),
            proxy_port: port(),
            api_secret: String::new(),
            enable_tun: false,
            ..Default::default()
        };
        let profile = AppProfile {
            mode: crate::config::model::ProxyMode::Global,
            ..Default::default()
        };
        let state = crate::app::AppState::new(config.clone(), profile.clone(), dir.clone());
        let mut events = state.event_tx.subscribe();
        let result = async {
            state.core.start(&profile, &config).await?;
            let initial_pid = state
                .core
                .uptime_and_pid()
                .await
                .and_then(|(_, pid)| pid)
                .ok_or_else(|| anyhow::anyhow!("Mihomo PID unavailable after start"))?;
            let version = state
                .core
                .version(&config)
                .await
                .ok_or_else(|| anyhow::anyhow!("Mihomo version unavailable after start"))?;
            anyhow::ensure!(version.starts_with("v1."));
            let connections = MihomoApi::new(config.clash_api_port, config.api_secret.clone())?
                .connections()
                .await?;
            anyhow::ensure!(
                connections
                    .get("connections")
                    .is_some_and(|value| value.is_array() || value.is_null()),
                "unexpected Mihomo /connections response: {connections}"
            );
            let log_seen = tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if let Ok(AppEvent::Log { source, .. }) = events.recv().await {
                        if source == "core" {
                            break;
                        }
                    }
                }
            })
            .await;
            anyhow::ensure!(log_seen.is_ok(), "Mihomo controller logs were not forwarded");

            state.core.update_provider(&profile, &config).await?;
            let mut updated = profile.clone();
            updated.nodes.push(node("n1", "127.0.0.1"));
            updated.selected_node = Some("n1".into());
            updated.nodes.push(node("n2", "127.0.0.2"));
            state.core.update_provider(&updated, &config).await?;
            anyhow::ensure!(
                state.core.uptime_and_pid().await.and_then(|(_, pid)| pid) == Some(initial_pid)
            );
            let api = MihomoApi::new(config.clash_api_port, config.api_secret.clone())?;
            let provider = api.get_json("/providers/proxies/YAP-NODES").await?;
            anyhow::ensure!(provider["proxies"].as_array().unwrap().iter().all(|proxy| proxy["udp"] == true));
            let options = api.get_json("/proxies/YAP-PROXY").await?;
            let options = options
                .get("all")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow::anyhow!("Mihomo proxy group response has no option list"))?;
            anyhow::ensure!(options.iter().any(|name| name == "n1"));
            anyhow::ensure!(options.iter().any(|name| name == "n2"));

            updated.nodes[0].server = "127.0.0.3".into();
            state.core.update_provider(&updated, &config).await?;
            anyhow::ensure!(
                state.core.uptime_and_pid().await.and_then(|(_, pid)| pid) == Some(initial_pid)
            );

            updated.nodes[0].extra = Some(json!({
                "type": "shadowsocks", "server_port": 1080,
                "method": "aes-256-gcm", "password": "test",
                "plugin": "v2ray-plugin", "plugin_opts": "mode=websocket;host=cdn.example;tls"
            }));
            state.core.update_provider(&updated, &config).await?;
            let provider_before = std::fs::read(state.core.provider_path())?;
            let mut invalid = updated.clone();
            invalid.nodes[0].extra = None;
            invalid.nodes[0].cipher = Some("not-a-cipher".into());
            let error = state.core.update_provider(&invalid, &config).await.unwrap_err().to_string();
            anyhow::ensure!(error.contains("503") && error.contains("not-a-cipher"), "{error}");
            anyhow::ensure!(std::fs::read(state.core.provider_path())? == provider_before);
            anyhow::ensure!(api.provider_proxy_names(renderer::NODE_PROVIDER).await?.contains(&"n1".into()));
            anyhow::ensure!(state.core.uptime_and_pid().await.and_then(|(_, pid)| pid) == Some(initial_pid));

            updated.mode = crate::config::model::ProxyMode::Direct;
            state.core.reload_safe(&updated, &config).await?;
            anyhow::ensure!(
                state.core.uptime_and_pid().await.and_then(|(_, pid)| pid) == Some(initial_pid)
            );
            anyhow::ensure!(state.core.probe_api(&updated).await.is_some());
            let mut empty = updated.clone();
            empty.nodes.clear();
            empty.selected_node = None;
            state.core.update_provider(&empty, &config).await?;
            anyhow::ensure!(
                state.core.uptime_and_pid().await.and_then(|(_, pid)| pid) == Some(initial_pid)
            );
            let options = api.get_json("/proxies/YAP-PROXY").await?;
            let options = options
                .get("all")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow::anyhow!("Mihomo proxy group response has no option list"))?;
            anyhow::ensure!(!options.iter().any(|name| name == "n1" || name == "n2"));

            *state.profile.write().await = empty;
            state.status.write().await.running = true;
            let mut changed = config.clone();
            changed.clash_api_port = port();
            changed.api_secret = "new-test-secret".into();
            state.apply_config(changed.clone()).await?;
            let new_api = MihomoApi::new(changed.clash_api_port, changed.api_secret.clone())?;
            new_api.version().await?;
            anyhow::ensure!(api.version().await.is_err(), "old controller must stop listening");
            anyhow::ensure!(MihomoApi::new(changed.clash_api_port, String::new())?.version().await.is_err());

            let occupied = std::net::TcpListener::bind("127.0.0.1:0")?;
            let mut invalid = changed.clone();
            invalid.clash_api_port = occupied.local_addr()?.port();
            anyhow::ensure!(state.apply_config(invalid).await.is_err());
            anyhow::ensure!(state.config.read().await.clash_api_port == changed.clash_api_port);
            let persisted: AppConfig = serde_json::from_slice(&std::fs::read(dir.join("app_config.json"))?)?;
            anyhow::ensure!(persisted.clash_api_port == changed.clash_api_port && persisted.api_secret == changed.api_secret);
            new_api.version().await?;
            Ok::<_, anyhow::Error>(())
        }
        .await;

        let stop_result = state.core.stop().await;
        std::fs::remove_dir_all(dir).unwrap();
        result.unwrap();
        stop_result.unwrap();
    }
}
