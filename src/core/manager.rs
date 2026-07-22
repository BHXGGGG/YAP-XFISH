use crate::app::{AppConfig, AppEvent};
use crate::config::model::AppProfile;
use crate::config::manager as cfgmgr;
use crate::config::render;
use crate::core::process;
use crate::error::AppResult;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::sync::Mutex;

/// 管理 sing-box 核心子进程的生命周期。
pub struct CoreManager {
    inner: Arc<CoreInner>,
}

struct CoreInner {
    binary: PathBuf,
    config_path: PathBuf,
    child: Mutex<Option<tokio::process::Child>>,
    /// 用于把核心的 stdout/stderr 转发为日志事件（网页日志面板可见）。
    event_tx: tokio::sync::broadcast::Sender<AppEvent>,
}

impl CoreManager {
    pub fn new(
        binary: PathBuf,
        config_path: PathBuf,
        event_tx: tokio::sync::broadcast::Sender<AppEvent>,
    ) -> Self {
        CoreManager {
            inner: Arc::new(CoreInner {
                binary,
                config_path,
                child: Mutex::new(None),
                event_tx,
            }),
        }
    }

    /// 解析实际使用的 sing-box 核心路径。
    ///
    /// 自包含分发优先：sing-box.exe 与 yap-xfish.exe 同目录（或同目录 `core/`）。
    /// 这样移动整个程序文件夹时，核心路径自动跟随新位置，不会停留在旧绝对路径导致报错。
    /// 其次使用配置中指定的路径（相对路径按 exe 目录解析），最后回退到 `<data_dir>/core/`。
    fn resolve_core_binary(configured: &PathBuf, data_dir: &PathBuf) -> PathBuf {
        // 1. exe 同目录（自包含分发，优先 —— 保证随文件夹移动而移动）
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let a = dir.join("sing-box.exe");
                if a.exists() {
                    return a;
                }
                let b = dir.join("core").join("sing-box.exe");
                if b.exists() {
                    return b;
                }
            }
        }
        // 2. 配置中显式指定的路径（相对路径按 exe 目录解析；绝对路径若仍存在则可用）
        let configured_resolved = if configured.is_absolute() {
            configured.clone()
        } else {
            crate::app::exe_dir().join(configured)
        };
        if configured_resolved.exists() {
            return configured_resolved;
        }
        // 3. data_dir/core 回退
        let c = data_dir.join("core").join("sing-box.exe");
        if c.exists() {
            return c;
        }
        configured.clone()
    }

    pub async fn is_running(&self) -> bool {
        let guard = self.inner.child.lock().await;
        match &*guard {
            Some(child) => child.id().is_some(),
            None => false,
        }
    }

    /// 渲染配置并启动核心。若核心已运行则跳过。
    pub async fn start(&self, profile: &AppProfile, app_cfg: &AppConfig) -> AppResult<()> {
        if self.is_running().await {
            return Ok(());
        }
        // 备份旧配置（订阅更新回滚也复用此函数）
        let _ = cfgmgr::backup_config(&self.inner.config_path);

        // 渲染并写入新配置
        let rendered = render::render(profile, app_cfg);
        let s = serde_json::to_string_pretty(&rendered)?;
        if let Some(parent) = self.inner.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.inner.config_path, s)?;

        // 解析实际使用的核心路径：优先配置路径，回退到 exe 同目录/子目录/data_dir。
        // 这样自包含分发包（sing-box.exe 与 yap-xfish.exe 同目录）无需用户手动放置即可运行。
        let data_dir = self
            .inner
            .config_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();
        let binary = Self::resolve_core_binary(&app_cfg.core_binary, &data_dir);

        if !binary.exists() {
            return Err(anyhow::anyhow!(
                "未找到 sing-box 核心程序 (sing-box.exe)。\n请确认 sing-box.exe 已与 yap-xfish.exe 位于同一目录，或在设置中指定正确路径。\n实际查找路径: {}",
                binary.display()
            )
            .into());
        }

        // 启动前记录一条日志，便于网页日志面板观察启动动作。
        let _ = self.inner.event_tx.send(AppEvent::Log {
            level: "info".into(),
            source: "core".into(),
            message: format!(
                "正在启动 sing-box 核心… binary={} config={}",
                binary.display(),
                self.inner.config_path.display()
            ),
        });

        let child = self.spawn_captured(&binary).await?;
        *self.inner.child.lock().await = Some(child);
        Ok(())
    }

    pub async fn stop(&self) -> AppResult<()> {
        let mut guard = self.inner.child.lock().await;
        if let Some(mut child) = guard.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
        Ok(())
    }

    pub async fn restart(&self, profile: &AppProfile, app_cfg: &AppConfig) -> AppResult<()> {
        self.stop().await?;
        self.start(profile, app_cfg).await?;
        Ok(())
    }

    /// 配置变更后的热重载：停止旧进程并按新 profile 重启。
    pub async fn reload(&self, profile: &AppProfile, app_cfg: &AppConfig) -> AppResult<()> {
        self.restart(profile, app_cfg).await
    }

    /// 仅渲染并写入 config.json（备份旧配置），不启动核心。
    /// 用于核心未运行时刷新订阅：下次手动启动即生效，不会意外拉起代理。
    pub async fn write_config_only(&self, profile: &AppProfile, app_cfg: &AppConfig) -> AppResult<()> {
        let _ = cfgmgr::backup_config(&self.inner.config_path);
        let rendered = render::render(profile, app_cfg);
        let s = serde_json::to_string_pretty(&rendered)?;
        if let Some(parent) = self.inner.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.inner.config_path, s)?;
        Ok(())
    }

    /// 带回滚的 reload：备份旧配置 → 渲染写入 → 检查内核 → 重启。
    /// 若内核缺失或启动失败，自动恢复备份的 config.json 并返回错误（调用方据此回滚 profile）。
    pub async fn reload_safe(&self, profile: &AppProfile, app_cfg: &AppConfig) -> AppResult<()> {
        let backup = cfgmgr::backup_config(&self.inner.config_path).ok().flatten();

        let rendered = render::render(profile, app_cfg);
        let s = serde_json::to_string_pretty(&rendered)?;
        if let Some(parent) = self.inner.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.inner.config_path, &s)?;

        if !self.inner.binary.exists() {
            if let Some(b) = &backup {
                let _ = std::fs::copy(b, &self.inner.config_path);
            }
            return Err(anyhow::anyhow!(
                "未找到 sing-box 核心程序: {}。请放置到该路径，或在设置中修改 core_binary。",
                self.inner.binary.display()
            )
            .into());
        }

        // 停掉旧进程
        self.stop().await?;

        match self.spawn_captured(&self.inner.binary).await {
            Ok(child) => {
                *self.inner.child.lock().await = Some(child);
                Ok(())
            }
            Err(e) => {
                // 启动失败：恢复备份配置
                if let Some(b) = &backup {
                    let _ = std::fs::copy(b, &self.inner.config_path);
                }
                Err(e)
            }
        }
    }

    /// 拉起核心并捕获其 stdout/stderr 转发为日志事件（网页日志面板可见）。
    async fn spawn_captured(&self, binary: &Path) -> AppResult<tokio::process::Child> {
        let mut child = process::spawn(binary, &self.inner.config_path).await?;
        if let Some(out) = child.stdout.take() {
            spawn_log_reader(out, self.inner.event_tx.clone());
        }
        if let Some(err) = child.stderr.take() {
            spawn_log_reader(err, self.inner.event_tx.clone());
        }
        Ok(child)
    }

    #[allow(dead_code)]
    pub fn config_path(&self) -> &PathBuf {
        &self.inner.config_path
    }
}

/// 将核心子进程的 stdout/stderr 逐行转发为 AppEvent::Log 事件（网页日志面板可见）。
/// 持续排空管道，避免缓冲区满导致核心阻塞。
fn spawn_log_reader(
    stream: impl AsyncRead + Unpin + Send + 'static,
    tx: tokio::sync::broadcast::Sender<AppEvent>,
) {
    tokio::spawn(async move {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let l = line.trim_end_matches(['\r', '\n']).to_string();
                    if !l.is_empty() {
                        let _ = tx.send(AppEvent::Log {
                            level: log_level_of(&l),
                            source: "core".into(),
                            message: l,
                        });
                    }
                }
                Err(_) => break,
            }
        }
    });
}

/// 根据核心日志行内容推断日志级别（sing-box 日志形如 `FATAL[...]` / `INFO ...` / `WARN ...`）。
fn log_level_of(line: &str) -> String {
    let u = line.to_uppercase();
    if u.contains("FATAL") || u.contains("ERROR") || u.contains("PANIC") {
        "error".to_string()
    } else if u.contains("WARN") {
        "warn".to_string()
    } else {
        "info".to_string()
    }
}
