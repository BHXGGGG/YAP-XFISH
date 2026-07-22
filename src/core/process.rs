use crate::error::AppResult;
use std::path::Path;
use tokio::process::{Child, Command};

/// 拉起 sing-box 子进程：`sing-box run -c <config>`
///
/// stdout/stderr 用管道捕获（由调用方读取并转发为日志事件），这样网页日志面板也能看到
/// 核心输出（包括启动失败时的 FATAL），而不是只进终端。reader 任务会持续排空管道，
/// 不会因缓冲区满阻塞核心。
///
/// Windows 上设置 `CREATE_NO_WINDOW`，避免启动/重启/切换节点重载核心时弹出黑色控制台窗口。
pub async fn spawn(binary: &Path, config: &Path) -> AppResult<Child> {
    let mut cmd = Command::new(binary);
    cmd.arg("run")
        .arg("-c")
        .arg(config)
        .kill_on_drop(false)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let child = cmd.spawn()?;
    Ok(child)
}
