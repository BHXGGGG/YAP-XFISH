use std::{
    cmp::Ordering,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use anyhow::{anyhow, bail, Context};
use axum::{extract::State, Json};
use once_cell::sync::Lazy;
use reqwest::{Client, StatusCode, Url};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::{app::AppState, error::{AppError, AppResult}};

const LATEST_RELEASE_API: &str = "https://api.github.com/repos/BHXGGGG/YAP-XFISH/releases/latest";
const RELEASES_URL: &str = "https://github.com/BHXGGGG/YAP-XFISH/releases";
/// 更新包里允许覆盖到程序目录的文件；其余条目一律忽略。
const UPDATE_ALLOWED: &[&str] = &["yap-xfish.exe", "sing-box.exe", "libcronet.dll", "README.txt"];

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStatus {
    UpdateAvailable,
    UpToDate,
    Ahead,
    NoRelease,
}

#[derive(Debug, Serialize)]
pub struct UpdateCheck {
    current_version: String,
    latest_version: Option<String>,
    status: UpdateStatus,
    release_url: String,
    /// 仅含程序文件的更新包（体积小，自动更新默认用它）
    update_asset: Option<AssetInfo>,
    /// 含 sing-box 的完整包
    full_asset: Option<AssetInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssetInfo {
    name: String,
    size: u64,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubAsset {
    name: String,
    size: u64,
    browser_download_url: String,
    /// GitHub 提供的 "sha256:<hex>"，旧版本或非 GitHub 托管时可能为空。
    digest: Option<String>,
}

impl GitHubRelease {
    fn asset_suffix(&self, suffix: &str) -> Option<&GitHubAsset> {
        self.assets.iter().find(|a| a.name.ends_with(suffix))
    }
}

/// 仅在用户点击时查询正式发布版；网络请求期间不持有配置锁。
pub async fn check_update(State(state): State<Arc<AppState>>) -> AppResult<Json<UpdateCheck>> {
    let proxy_port = if state.core.is_running().await {
        Some(state.config.read().await.proxy_port)
    } else {
        None
    };
    let client = build_client(proxy_port)?;
    let result = fetch_latest(&client, LATEST_RELEASE_API, env!("CARGO_PKG_VERSION")).await?;
    Ok(Json(result))
}

fn build_client(proxy_port: Option<u16>) -> anyhow::Result<Client> {
    build_client_with_timeout(proxy_port, Duration::from_secs(10))
}

fn build_client_with_timeout(proxy_port: Option<u16>, timeout: Duration) -> anyhow::Result<Client> {
    let mut builder = Client::builder()
        .user_agent(concat!("YAP-XFISH/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(5))
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::limited(10))
        .no_proxy();
    if let Some(port) = proxy_port {
        // GitHub 在部分网络不可直连，核心运行时通过当前节点访问。
        builder = builder.proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}"))?);
    }
    builder.build().context("创建更新检查连接失败")
}

async fn fetch_latest(
    client: &Client,
    endpoint: &str,
    current_version: &str,
) -> anyhow::Result<UpdateCheck> {
    match fetch_release(client, endpoint).await? {
        Some(release) => release_report(current_version, release),
        None => Ok(no_release(current_version)),
    }
}

async fn fetch_release(client: &Client, endpoint: &str) -> anyhow::Result<Option<GitHubRelease>> {
    let response = client
        .get(endpoint)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(request_error)?;

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if status == StatusCode::TOO_MANY_REQUESTS
        || (status == StatusCode::FORBIDDEN
            && response
                .headers()
                .get("x-ratelimit-remaining")
                .is_some_and(|value| value == "0"))
    {
        bail!("GitHub 请求次数已达限制，请稍后再试，或直接查看发布页");
    }
    if !status.is_success() {
        bail!(
            "GitHub 暂时无法提供版本信息（HTTP {}），请稍后重试",
            status.as_u16()
        );
    }

    let release: GitHubRelease = response.json().await.map_err(request_error)?;
    Ok(Some(release))
}

fn request_error(error: reqwest::Error) -> anyhow::Error {
    if error.is_timeout() {
        anyhow!("连接 GitHub 超时，请检查网络或代理后重试")
    } else if error.is_connect() {
        anyhow!("无法连接 GitHub，请检查网络或代理后重试")
    } else if error.is_decode() {
        anyhow!("GitHub 返回的版本信息无效，请稍后重试")
    } else {
        anyhow!("获取 GitHub 版本信息失败：{error}")
    }
}

fn no_release(current_version: &str) -> UpdateCheck {
    UpdateCheck {
        current_version: current_version.to_owned(),
        latest_version: None,
        status: UpdateStatus::NoRelease,
        release_url: RELEASES_URL.to_owned(),
        update_asset: None,
        full_asset: None,
    }
}

fn release_report(current_version: &str, release: GitHubRelease) -> anyhow::Result<UpdateCheck> {
    if release.draft || release.prerelease {
        return Ok(no_release(current_version));
    }

    let tag = release.tag_name.trim();
    let version = tag
        .strip_prefix('v')
        .or_else(|| tag.strip_prefix('V'))
        .unwrap_or(tag);
    let latest = Version::parse(version)
        .map_err(|_| anyhow!("无法识别 GitHub 版本号「{tag}」，请直接查看发布页"))?;
    let current = Version::parse(current_version).context("无法识别当前程序版本号")?;
    // 按语义版本优先级比较，忽略 +build 元数据，避免字符串排序或降级提示。
    let status = match latest.cmp_precedence(&current) {
        Ordering::Greater => UpdateStatus::UpdateAvailable,
        Ordering::Equal => UpdateStatus::UpToDate,
        Ordering::Less => UpdateStatus::Ahead,
    };

    let mut release_url = Url::parse(RELEASES_URL)?;
    release_url
        .path_segments_mut()
        .unwrap()
        .push("tag")
        .push(tag);
    let asset_info = |a: Option<&GitHubAsset>| {
        a.map(|a| AssetInfo {
            name: a.name.clone(),
            size: a.size,
        })
    };
    Ok(UpdateCheck {
        current_version: current_version.to_owned(),
        latest_version: Some(latest.to_string()),
        status,
        release_url: release_url.to_string(),
        update_asset: asset_info(release.asset_suffix("-update.zip")),
        full_asset: asset_info(release.asset_suffix("-windows-amd64.zip")),
    })
}

// ---------------- 自动更新：下载 / 校验 / 暂存 / 安装 ----------------

#[derive(Debug, Clone, Serialize)]
pub struct UpdateProgress {
    /// idle / downloading / extracting / ready / installing / error
    pub state: &'static str,
    pub downloaded: u64,
    pub total: u64,
    pub version: Option<String>,
    /// 本次更新是否同时包含 sing-box
    pub includes_singbox: bool,
    pub error: Option<String>,
}

impl Default for UpdateProgress {
    fn default() -> Self {
        Self {
            state: "idle",
            downloaded: 0,
            total: 0,
            version: None,
            includes_singbox: false,
            error: None,
        }
    }
}

static PROGRESS: Lazy<StdMutex<UpdateProgress>> = Lazy::new(|| StdMutex::new(UpdateProgress::default()));

fn progress_snapshot() -> UpdateProgress {
    PROGRESS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn set_progress(update: impl FnOnce(&mut UpdateProgress)) {
    let mut guard = PROGRESS.lock().unwrap_or_else(|e| e.into_inner());
    update(&mut guard);
}

pub async fn update_progress() -> AppResult<Json<UpdateProgress>> {
    Ok(Json(progress_snapshot()))
}

/// 开始下载并校验更新包；完成后前端再调用 `/api/update/install` 重启安装。
pub async fn apply_update(State(state): State<Arc<AppState>>) -> AppResult<Json<serde_json::Value>> {
    {
        let p = progress_snapshot();
        if matches!(
            p.state,
            "downloading" | "extracting" | "ready" | "installing"
        ) {
            return Err(AppError(anyhow!("更新已在进行中")));
        }
    }
    *PROGRESS.lock().unwrap_or_else(|e| e.into_inner()) = UpdateProgress {
        state: "downloading",
        ..Default::default()
    };
    let st = state.clone();
    tokio::spawn(async move {
        if let Err(e) = run_update(&st).await {
            let message = format!("{e:#}");
            set_progress(|p| {
                p.state = "error";
                p.error = Some(message.clone());
            });
            st.log_with("app", "error", format!("自动更新失败: {message}"));
        }
    });
    Ok(Json(serde_json::json!({ "started": true })))
}

async fn run_update(state: &Arc<AppState>) -> anyhow::Result<()> {
    let proxy_port = if state.core.is_running().await {
        Some(state.config.read().await.proxy_port)
    } else {
        None
    };
    let client = build_client(proxy_port)?;
    // 下载安装包可能耗时较长，单独用更宽松的总体超时
    let download_client = build_client_with_timeout(proxy_port, Duration::from_secs(15 * 60))?;
    let release = fetch_release(&client, LATEST_RELEASE_API)
        .await?
        .ok_or_else(|| anyhow!("GitHub 暂无正式发布版本"))?;
    if release.draft || release.prerelease {
        bail!("GitHub 暂无正式发布版本");
    }
    let tag = release.tag_name.trim().to_string();
    let version = tag
        .strip_prefix('v')
        .or_else(|| tag.strip_prefix('V'))
        .unwrap_or(&tag)
        .to_string();

    let update_asset = release.asset_suffix("-update.zip").cloned();
    let full_asset = release.asset_suffix("-windows-amd64.zip").cloned();
    if update_asset.is_none() && full_asset.is_none() {
        bail!("发布页没有可用的安装包");
    }

    let dir: PathBuf = state.data_dir.join("update");
    let stage = dir.join("stage");
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage)?;

    let exe_dir = crate::app::exe_dir();
    let local_singbox = sha256_file(&exe_dir.join("sing-box.exe"));
    set_progress(|p| {
        p.state = "downloading";
        p.version = Some(version.clone());
        p.total = update_asset.as_ref().map(|a| a.size).unwrap_or(0);
        p.downloaded = 0;
        p.includes_singbox = false;
        p.error = None;
    });

    // 优先下载体积小的更新包；清单显示 sing-box 变了才回退到完整包。
    let mut source: Option<(PathBuf, bool)> = None;
    if let Some(asset) = update_asset.as_ref() {
        let path = dir.join("update.zip");
        download_asset(&download_client, asset, &path).await?;
        let changed = match (
            read_manifest(&path).and_then(|m| m.singbox_sha256),
            local_singbox.clone(),
        ) {
            (Some(want), Some(local)) => !want.eq_ignore_ascii_case(&local),
            // 本机没有 sing-box 时无法确认一致，按需要完整包处理
            (Some(_), None) => true,
            _ => false,
        };
        if !changed {
            source = Some((path, false));
        }
    }
    if source.is_none() {
        let asset = full_asset
            .as_ref()
            .ok_or_else(|| anyhow!("需要更新 sing-box，但发布页没有完整包"))?;
        let path = dir.join("full.zip");
        download_asset(&download_client, asset, &path).await?;
        source = Some((path, true));
    }
    let (zip_path, includes_singbox) = source.unwrap();

    set_progress(|p| p.state = "extracting");
    let files = extract_zip(&zip_path, &stage, includes_singbox)?;
    let script = dir.join("apply.ps1");
    // Windows PowerShell 5.1 无 BOM 时按 ANSI 读取脚本，中文路径会乱码，必须带 UTF-8 BOM
    fs::write(
        &script,
        format!(
            "\u{feff}{}",
            build_script(&exe_dir, &stage, std::process::id(), &dir, false)
        ),
    )?;

    set_progress(|p| {
        p.state = "ready";
        p.includes_singbox = includes_singbox;
        p.downloaded = p.total;
    });
    state.log_with(
        "app",
        "info",
        format!(
            "更新 v{version} 已下载并校验（{}），等待重启安装",
            files.join(", ")
        ),
    );
    Ok(())
}

/// 下载到 dest，并用 GitHub 提供的 digest 校验；进度写入全局状态。
async fn download_asset(
    client: &Client,
    asset: &GitHubAsset,
    dest: &Path,
) -> anyhow::Result<()> {
    let mut response = client
        .get(&asset.browser_download_url)
        .send()
        .await
        .map_err(request_error)?;
    if !response.status().is_success() {
        bail!("下载更新包失败（HTTP {}）", response.status().as_u16());
    }
    let total = response.content_length().unwrap_or(asset.size);
    set_progress(|p| {
        p.state = "downloading";
        p.downloaded = 0;
        p.total = total;
    });
    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("无法写入 {}", dest.display()))?;
    let mut hasher = Sha256::new();
    let mut downloaded = 0u64;
    while let Some(chunk) = response.chunk().await.map_err(request_error)? {
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        set_progress(|p| p.downloaded = downloaded);
    }
    file.flush().await?;
    let got = hex_digest(hasher.finalize().as_slice());
    if let Some(digest) = asset.digest.as_deref() {
        let want = digest.strip_prefix("sha256:").unwrap_or(digest);
        if !want.eq_ignore_ascii_case(&got) {
            let _ = fs::remove_file(dest);
            bail!("更新包校验失败（SHA256 不一致），已删除下载文件");
        }
    }
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn sha256_file(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hex_digest(hasher.finalize().as_slice()))
}

#[derive(Deserialize)]
struct UpdateManifest {
    #[serde(default)]
    singbox_sha256: Option<String>,
}

fn read_manifest(zip_path: &Path) -> Option<UpdateManifest> {
    let file = File::open(zip_path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut entry = archive.by_name("update.json").ok()?;
    let mut text = String::new();
    entry.read_to_string(&mut text).ok()?;
    serde_json::from_str(&text).ok()
}

/// 只解压白名单文件，忽略路径分隔符，防止 zip 目录穿越。
fn extract_zip(zip_path: &Path, stage: &Path, allow_singbox: bool) -> anyhow::Result<Vec<String>> {
    let file = File::open(zip_path).context("打开更新包失败")?;
    let mut archive = zip::ZipArchive::new(file).context("更新包损坏，无法解压")?;
    let mut written = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name().to_string();
        if name.contains('/') || name.contains('\\') || !UPDATE_ALLOWED.contains(&name.as_str()) {
            continue;
        }
        if name == "sing-box.exe" && !allow_singbox {
            continue;
        }
        if entry.size() > 300 * 1024 * 1024 {
            bail!("更新包中的 {name} 体积异常");
        }
        let dest = stage.join(&name);
        let mut out = File::create(&dest)
            .with_context(|| format!("无法写入 {}", dest.display()))?;
        std::io::copy(&mut entry, &mut out)?;
        written.push(name);
    }
    if !written.iter().any(|n| n == "yap-xfish.exe") {
        bail!("更新包中缺少 yap-xfish.exe");
    }
    Ok(written)
}

/// 生成更新脚本：等旧进程退出 → 覆盖文件 → 重新启动。
fn build_script(
    exe_dir: &Path,
    stage: &Path,
    pid: u32,
    update_dir: &Path,
    skip_start: bool,
) -> String {
    let quoted = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "''"));
    let skip = if skip_start { "$true" } else { "$false" };
    format!(
        r#"# 由 YAP-XFISH 自动生成：等待旧进程退出后替换程序文件并重启。
$ErrorActionPreference = 'Stop'
$exeDir = {exe_dir}
$exe = Join-Path $exeDir 'yap-xfish.exe'
$stage = {stage}
$updateDir = {update_dir}
$targetPid = {pid}
$skipStart = {skip}

for ($i = 0; $i -lt 240; $i++) {{
  if (-not (Get-Process -Id $targetPid -ErrorAction SilentlyContinue)) {{ break }}
  Start-Sleep -Milliseconds 250
}}
Start-Sleep -Milliseconds 600

$files = @('yap-xfish.exe', 'libcronet.dll', 'sing-box.exe', 'README.txt')
foreach ($name in $files) {{
  $src = Join-Path $stage $name
  if (-not (Test-Path -LiteralPath $src)) {{ continue }}
  $dst = Join-Path $exeDir $name
  for ($k = 0; $k -lt 30; $k++) {{
    try {{
      Copy-Item -LiteralPath $src -Destination $dst -Force
      break
    }} catch {{
      Start-Sleep -Milliseconds 300
    }}
  }}
}}

if (-not $skipStart) {{
  Start-Process -FilePath $exe -WorkingDirectory $exeDir
}}
Remove-Item -LiteralPath $stage -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath (Join-Path $updateDir 'update.zip') -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath (Join-Path $updateDir 'full.zip') -Force -ErrorAction SilentlyContinue
"#,
        exe_dir = quoted(exe_dir),
        stage = quoted(stage),
        update_dir = quoted(update_dir),
        pid = pid,
        skip = skip,
    )
}

/// 启动更新脚本并退出当前进程，让脚本完成文件替换后重启程序。
pub async fn install_update(State(state): State<Arc<AppState>>) -> AppResult<Json<serde_json::Value>> {
    if progress_snapshot().state != "ready" {
        return Err(AppError(anyhow!("更新尚未准备好，请先下载更新包")));
    }
    let dir = state.data_dir.join("update");
    let script = dir.join("apply.ps1");
    if !script.exists() {
        return Err(AppError(anyhow!("找不到更新脚本，请重新下载")));
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut cmd = std::process::Command::new("powershell.exe");
        cmd.arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&script)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            // CREATE_NO_WINDOW | DETACHED_PROCESS
            .creation_flags(0x0800_0000 | 0x0000_0008);
        cmd.spawn().context("启动更新脚本失败")?;
    }
    set_progress(|p| p.state = "installing");
    state.log_with("app", "info", "正在安装更新，程序即将重启…");
    let st = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(800)).await;
        if st.config.read().await.system_proxy {
            let _ = crate::system::sysproxy::disable();
        }
        let _ = st.core.stop().await;
        std::process::exit(0);
    });
    Ok(Json(serde_json::json!({ "installing": true })))
}

#[cfg(test)]
mod tests;
