use crate::error::AppResult;
use anyhow::anyhow;
use std::path::Path;
use tokio::process::{Child, Command};

pub async fn validate(binary: &Path, home: &Path, config: &Path) -> AppResult<()> {
    let mut cmd = Command::new(binary);
    cmd.arg("-t")
        .arg("-d")
        .arg(home)
        .arg("-f")
        .arg(config);
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    let output = cmd.output().await?;
    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("Mihomo 配置校验失败: {stdout}{stderr}").into());
    }
    Ok(())
}

/// Launch Mihomo with its home directory and generated YAML configuration.
pub async fn spawn(binary: &Path, home: &Path, config: &Path) -> AppResult<Child> {
    let mut cmd = Command::new(binary);
    cmd.arg("-d")
        .arg(home)
        .arg("-f")
        .arg(config)
        .kill_on_drop(false)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    let child = cmd.spawn()?;
    Ok(child)
}
