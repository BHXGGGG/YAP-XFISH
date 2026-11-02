use crate::app::{AppConfig, AppEvent};
use crate::config::model::AppProfile;
use crate::config::manager as cfgmgr;
use crate::config::render;
use crate::core::process;
use crate::error::AppResult;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
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
    /// 只记录已交给运行核心的出站配置，测速时避免复用尚未应用的旧节点。
    probe_config: Mutex<Option<ProbeConfig>>,
    /// 用于把核心的 stdout/stderr 转发为日志事件（网页日志面板可见）。
    event_tx: tokio::sync::broadcast::Sender<AppEvent>,
    /// 最近一条 FATAL/panic 输出（已去 ANSI），核心意外退出时用来判断原因。
    last_fatal: Arc<std::sync::Mutex<Option<String>>>,
    /// 当前核心进程的启动时刻（仪表盘「运行时长」）；停止时清空。
    started_at: std::sync::Mutex<Option<std::time::Instant>>,
}

struct ProbeConfig {
    outbounds_fingerprint: u64,
    port: u16,
    secret: String,
}

/// 运行中的核心被替换时弹系统通知：重启期间 TUN 会短暂断开，
/// 这一瞬间新建的连接可能绕过代理直连并被长期复用（出口 IP 变成本机 IP）。
fn notify_core_restarted() {
    crate::system::notify::toast(
        "YAP-XFISH：sing-box 核心已重启",
        "TUN 刚刚短暂中断，重启期间新建的连接可能走了直连。如出口 IP 异常，请重开相关应用的连接。",
    );
}

/// 不参与「节点内容」比较的出站类型（它们的 tag 是固定名字，不随订阅变化）。
const FIXED_OUTBOUND_TYPES: &[&str] = &["direct", "block", "dns", "selector", "urltest"];

/// 把配置规范化后比较：节点 tag 替换成其连接参数本身，outbounds 按内容排序。
/// 节点 id（= tag）由名字参与计算，信息节点改名就会换 tag；订阅合并还会改变节点顺序。
/// 这两种变化不影响任何连接，因此视为等价。
fn same_effective_config(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    canonical_config(a) == canonical_config(b)
}

fn canonical_config(cfg: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    let mut cfg = cfg.clone();
    let mut tag_map = std::collections::HashMap::new();
    if let Some(outs) = cfg.get_mut("outbounds").and_then(|v| v.as_array_mut()) {
        for o in outs.iter_mut() {
            let ty = o.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if FIXED_OUTBOUND_TYPES.contains(&ty) {
                continue;
            }
            let Some(obj) = o.as_object_mut() else { continue };
            if let Some(Value::String(tag)) = obj.remove("tag") {
                tag_map.insert(tag, format!("node:{}", Value::Object(obj.clone())));
            }
        }
    }
    fn replace_tags(v: &mut Value, map: &std::collections::HashMap<String, String>) {
        match v {
            Value::String(s) => {
                if let Some(c) = map.get(s.as_str()) {
                    *s = c.clone();
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| replace_tags(x, map)),
            Value::Object(o) => o.values_mut().for_each(|x| replace_tags(x, map)),
            _ => {}
        }
    }
    replace_tags(&mut cfg, &tag_map);
    if let Some(outs) = cfg.get_mut("outbounds").and_then(|v| v.as_array_mut()) {
        outs.sort_by_cached_key(|o| o.to_string());
    }
    cfg
}

fn outbounds_fingerprint(profile: &AppProfile) -> u64 {
    let mut hash = DefaultHasher::new();
    for node in &profile.nodes {
        render::node_to_outbound_in_profile(node, profile)
            .to_string()
            .hash(&mut hash);
    }
    hash.finish()
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
                probe_config: Mutex::new(None),
                event_tx,
                last_fatal: Arc::new(std::sync::Mutex::new(None)),
                started_at: std::sync::Mutex::new(None),
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

    /// 用 try_wait 判活：tokio 的 child.id() 在进程退出但未被 wait 前仍返回 Some，
    /// 会把已 FATAL 退出的核心误判为运行中。已退出则顺手清掉 child。
    pub async fn is_running(&self) -> bool {
        let mut guard = self.inner.child.lock().await;
        match guard.as_mut().map(|c| c.try_wait()) {
            Some(Ok(Some(_))) => {
                *guard = None;
                false
            }
            Some(_) => true,
            None => false,
        }
    }

    /// 当前节点配置与已启动核心一致时，允许测速复用其 Clash API。
    /// 返回实际启动时的端口和密钥，不受尚未成功应用的设置变更影响。
    pub async fn probe_api(&self, profile: &AppProfile) -> Option<(u16, String)> {
        if !self.is_running().await {
            return None;
        }
        let fingerprint = outbounds_fingerprint(profile);
        let applied = self.inner.probe_config.lock().await;
        let applied = applied.as_ref()?;
        (applied.outbounds_fingerprint == fingerprint)
            .then(|| (applied.port, applied.secret.clone()))
    }

    async fn remember_probe_config(&self, profile: &AppProfile, cfg: &AppConfig) {
        *self.inner.probe_config.lock().await = Some(ProbeConfig {
            outbounds_fingerprint: outbounds_fingerprint(profile),
            port: cfg.clash_api_port,
            secret: cfg.api_secret.clone(),
        });
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

        // 配置错误时 sing-box 会立刻 FATAL 退出（例如非法 DNS detour），
        // 若这里仍返回 Ok，UI 会显示 running=true，但 9999/连接列表全挂。
        // 等 400ms 探测一次：已退出则清 child 并返回错误。
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        {
            let mut guard = self.inner.child.lock().await;
            if let Some(c) = guard.as_mut() {
                match c.try_wait() {
                    Ok(Some(status)) => {
                        *guard = None;
                        return Err(anyhow::anyhow!(
                            "sing-box 启动后立即退出（{status}）。请查看 data/logs/sing-box.log 与 yap-xfish.log 中的 FATAL 行。"
                        )
                        .into());
                    }
                    Ok(None) => {}
                    Err(e) => {
                        *guard = None;
                        return Err(anyhow::anyhow!("探测 sing-box 进程状态失败: {e}").into());
                    }
                }
            }
        }

        self.remember_probe_config(profile, app_cfg).await;

        // gvisor 启动失败回退：若启用 TUN，且 sing-box 在 1.5s 内已死亡，
        // 重写 config.json 中 TUN 的 stack=system 并重启一次。
        if app_cfg.enable_tun {
            let event_tx = self.inner.event_tx.clone();
            let child_arc = Arc::new(self.inner.clone());
            let binary2 = binary.clone();
            let config_path2 = self.inner.config_path.clone();
            tokio::spawn(async move {
                // 等待 1.5s 看子进程是否还活着
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                let alive = {
                    let mut guard = child_arc.child.lock().await;
                    if let Some(c) = guard.as_mut() {
                        // try_wait 不会阻塞
                        match c.try_wait() {
                            Ok(Some(_status)) => false, // 已退出
                            Ok(None) => true,           // 还活着
                            Err(_) => true,              // 出错视为活着
                        }
                    } else {
                        false
                    }
                };
                if alive {
                    return; // 启动成功，无需回退
                }
                // 已退出：尝试重写 stack=system 重启一次
                let _ = event_tx.send(AppEvent::Log {
                    level: "warn".into(),
                    source: "core".into(),
                    message: "TUN gvisor 栈启动失败，尝试回退到 system 栈".to_string(),
                });
                if let Ok(text) = std::fs::read_to_string(&config_path2) {
                    if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&text) {
                        if let Some(inbounds) =
                            v.get_mut("inbounds").and_then(|x| x.as_array_mut())
                        {
                            for ib in inbounds.iter_mut() {
                                if ib.get("type").and_then(|t| t.as_str())
                                    == Some("tun")
                                {
                                    ib.as_object_mut().map(|m| {
                                        m.insert("stack".into(),
                                            serde_json::Value::String("system".into()));
                                    });
                                }
                            }
                        }
                        if let Ok(s) = serde_json::to_string_pretty(&v) {
                            let _ = std::fs::write(&config_path2, s);
                        }
                    }
                }
                // 重启
                match process::spawn(&binary2, &config_path2).await {
                    Ok(mut child2) => {
                        if let Some(out) = child2.stdout.take() {
                            spawn_log_reader(out, event_tx.clone(), child_arc.last_fatal.clone());
                        }
                        if let Some(err) = child2.stderr.take() {
                            spawn_log_reader(err, event_tx.clone(), child_arc.last_fatal.clone());
                        }
                        // 替换 child
                        let mut guard = child_arc.child.lock().await;
                        *guard = Some(child2);
                        let _ = event_tx.send(AppEvent::Log {
                            level: "info".into(),
                            source: "core".into(),
                            message: "已用 system 栈重启".to_string(),
                        });
                        notify_core_restarted();
                    }
                    Err(e) => {
                        let _ = event_tx.send(AppEvent::Log {
                            level: "error".into(),
                            source: "core".into(),
                            message: format!("system 栈也启动失败: {e}"),
                        });
                    }
                }
            });
        }
        Ok(())
    }

    pub async fn stop(&self) -> AppResult<()> {
        *self.inner.probe_config.lock().await = None;
        *self.inner.started_at.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let mut guard = self.inner.child.lock().await;
        if let Some(mut child) = guard.take() {
            let _ = child.start_kill();
            let _ = child.wait().await;
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
        let rendered = render::render(profile, app_cfg);
        let s = serde_json::to_string_pretty(&rendered)?;

        // 配置与正在运行的实质一致（如订阅更新只改了「剩余流量」等节点名 / 节点顺序）：不重启核心。
        // 重启会让 TUN 短暂断开（期间新建连接可能漏到直连），rule-set 也要重新初始化。
        // 跳过时不写 config.json，让它始终等于运行中的配置，供下次比较。
        if self.is_running().await
            && std::fs::read_to_string(&self.inner.config_path)
                .ok()
                .and_then(|old| serde_json::from_str::<serde_json::Value>(&old).ok())
                .is_some_and(|old| same_effective_config(&old, &rendered))
        {
            let _ = self.inner.event_tx.send(AppEvent::Log {
                level: "info".into(),
                source: "core".into(),
                message: "配置未变化，跳过核心重启".into(),
            });
            return Ok(());
        }

        let backup = cfgmgr::backup_config(&self.inner.config_path).ok().flatten();
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
        let was_running = self.is_running().await;
        self.stop().await?;

        match self.spawn_captured(&self.inner.binary).await {
            Ok(child) => {
                *self.inner.child.lock().await = Some(child);
                self.remember_probe_config(profile, app_cfg).await;
                if was_running {
                    notify_core_restarted();
                }
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

    /// 核心已运行时长与进程 id；未运行返回 None。
    pub async fn uptime_and_pid(&self) -> Option<(std::time::Duration, Option<u32>)> {
        if !self.is_running().await {
            return None;
        }
        let started = (*self.inner.started_at.lock().unwrap_or_else(|e| e.into_inner()))?;
        let pid = self.inner.child.lock().await.as_ref().and_then(|c| c.id());
        Some((started.elapsed(), pid))
    }

    /// 拉起核心并捕获其 stdout/stderr 转发为日志事件（网页日志面板可见）。
    async fn spawn_captured(&self, binary: &Path) -> AppResult<tokio::process::Child> {
        *self.inner.last_fatal.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.inner.started_at.lock().unwrap_or_else(|e| e.into_inner()) = Some(std::time::Instant::now());
        let mut child = process::spawn(binary, &self.inner.config_path).await?;
        if let Some(out) = child.stdout.take() {
            spawn_log_reader(out, self.inner.event_tx.clone(), self.inner.last_fatal.clone());
        }
        if let Some(err) = child.stderr.take() {
            spawn_log_reader(err, self.inner.event_tx.clone(), self.inner.last_fatal.clone());
        }
        Ok(child)
    }

    /// 取走最近一条 FATAL 输出（取后清空，避免重复提示）。
    pub fn take_last_fatal(&self) -> Option<String> {
        self.inner.last_fatal.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    #[allow(dead_code)]
    pub fn config_path(&self) -> &PathBuf {
        &self.inner.config_path
    }
}

/// 将核心子进程的 stdout/stderr 逐行转发为 AppEvent::Log 事件（网页日志面板可见），
/// 并默认追加到 data/logs/yap-xfish.log。
/// 持续排空管道，避免缓冲区满导致核心阻塞。
///
/// 注意：sing-box 本身也通过 config.log.output 写 data/logs/sing-box.log；
/// 这里的落盘是「管道捕获副本」，两者互补（核心 output 更完整，管道副本带 source 标签）。
fn spawn_log_reader(
    stream: impl AsyncRead + Unpin + Send + 'static,
    tx: tokio::sync::broadcast::Sender<AppEvent>,
    last_fatal: Arc<std::sync::Mutex<Option<String>>>,
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
                        let level = log_level_of(&l);
                        let upper = l.to_uppercase();
                        if upper.contains("FATAL") || upper.contains("PANIC") {
                            *last_fatal.lock().unwrap_or_else(|e| e.into_inner()) =
                                Some(strip_ansi(&l));
                        }
                        crate::system::log_file::append("core", &level, &l);
                        let _ = tx.send(AppEvent::Log {
                            level,
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

/// 去掉 sing-box 输出里的 ANSI 颜色码（\x1b[...m）。
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// FATAL 是否为配置错误（而非网络问题）。网络类：规则集下载失败、超时、连接失败。
pub fn is_config_error(fatal: &str) -> bool {
    let l = fatal.to_lowercase();
    let network = [
        "timeout", "timed out", "connection", "connectex", "refused", "wsarecv", "dial",
        "handshake", "no such host", "eof",
    ];
    !network.iter().any(|k| l.contains(k))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::{Node, ProxyMode};

    fn cfg_with(outbounds: serde_json::Value, final_tag: &str) -> serde_json::Value {
        serde_json::json!({
            "outbounds": outbounds,
            "route": { "final": final_tag, "rules": [{ "outbound": "direct", "rule_set": "geoip-cn" }] }
        })
    }

    #[test]
    fn effective_config_ignores_node_tag_and_order() {
        let a = cfg_with(
            serde_json::json!([
                { "type": "vless", "tag": "s-aaa", "server": "1.1.1.1", "server_port": 443 },
                { "type": "vless", "tag": "s-info1", "server": "9.9.9.9", "server_port": 1 },
                { "type": "direct", "tag": "direct" }
            ]),
            "s-aaa",
        );
        // 信息节点改名 → 新 tag；顺序也变了
        let b = cfg_with(
            serde_json::json!([
                { "type": "direct", "tag": "direct" },
                { "type": "vless", "tag": "s-info2", "server": "9.9.9.9", "server_port": 1 },
                { "type": "vless", "tag": "s-aaa", "server": "1.1.1.1", "server_port": 443 }
            ]),
            "s-aaa",
        );
        assert!(same_effective_config(&a, &b));
    }

    #[test]
    fn effective_config_detects_real_changes() {
        let base = serde_json::json!([
            { "type": "vless", "tag": "s-aaa", "server": "1.1.1.1", "server_port": 443 },
            { "type": "vless", "tag": "s-bbb", "server": "2.2.2.2", "server_port": 443 },
            { "type": "direct", "tag": "direct" }
        ]);
        let a = cfg_with(base.clone(), "s-aaa");
        // 改端口
        let mut changed = base.clone();
        changed[0]["server_port"] = 8443.into();
        assert!(!same_effective_config(&a, &cfg_with(changed, "s-aaa")));
        // 换选中节点
        assert!(!same_effective_config(&a, &cfg_with(base.clone(), "s-bbb")));
        // 新增节点
        let mut added = base.clone();
        added.as_array_mut().unwrap().push(
            serde_json::json!({ "type": "vless", "tag": "s-ccc", "server": "3.3.3.3", "server_port": 443 }),
        );
        assert!(!same_effective_config(&a, &cfg_with(added, "s-aaa")));
    }

    #[test]
    fn probe_config_ignores_latency_and_node_selection() {
        let mut profile = AppProfile {
            nodes: vec![Node {
                id: "node".into(), kind: "shadowsocks".into(), server: "127.0.0.1".into(),
                port: 1234, password: Some("first".into()), ..Default::default()
            }],
            ..Default::default()
        };
        let original = outbounds_fingerprint(&profile);
        profile.nodes[0].latency = Some(50);
        profile.nodes[0].latency_status = Some("ok".into());
        profile.mode = ProxyMode::Rule;
        profile.selected_node = Some("node".into());
        assert_eq!(outbounds_fingerprint(&profile), original);
        profile.nodes[0].password = Some("changed".into());
        assert_ne!(outbounds_fingerprint(&profile), original);
    }

    #[test]
    fn classifies_fatal_lines() {
        let net = strip_ansi("\x1b[31mFATAL\x1b[0m[0015] start service: initialize rule-set[1]: initial rule-set: geosite-cn: Get \"https://x\": net/http: TLS handshake timeout");
        assert!(net.starts_with("FATAL[0015]"));
        assert!(!is_config_error(&net));
        assert!(is_config_error("FATAL[0000] decode config at ./config.json: outbounds[2].method: unknown method"));
    }

    #[test]
    fn probe_config_detects_changed_detour_credentials() {
        let entry = Node {
            id: "entry".into(), kind: "shadowsocks".into(), server: "127.0.0.1".into(),
            port: 1234, password: Some("first".into()), ..Default::default()
        };
        let mut exit = entry.clone();
        exit.id = "exit".into();
        exit.detour = Some("entry".into());
        let mut profile = AppProfile { nodes: vec![exit, entry], ..Default::default() };
        let original = outbounds_fingerprint(&profile);
        profile.nodes[1].password = Some("updated".into());
        assert_ne!(outbounds_fingerprint(&profile), original);
    }
}
