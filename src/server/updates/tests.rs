use super::*;
use axum::{http::HeaderMap, response::IntoResponse, routing::get, Router};
use serde_json::json;
use std::io::Write;

fn release(tag: &str) -> GitHubRelease {
    GitHubRelease {
        tag_name: tag.to_owned(),
        draft: false,
        prerelease: false,
        assets: Vec::new(),
    }
}

fn asset(name: &str, size: u64) -> GitHubAsset {
    GitHubAsset {
        name: name.to_owned(),
        size,
        browser_download_url: format!("https://example.invalid/{name}"),
        digest: None,
    }
}

#[test]
fn compares_version_precedence_without_offering_downgrades() {
    let cases = [
        ("0.1.9", "v0.1.10", UpdateStatus::UpdateAvailable),
        ("0.9.99", "v0.10.0", UpdateStatus::UpdateAvailable),
        ("1.99.99", "v2.0.0", UpdateStatus::UpdateAvailable),
        ("0.1.41", "v0.1.30", UpdateStatus::Ahead),
        ("0.1.41", "v0.1.41", UpdateStatus::UpToDate),
        ("1.0.0", "V1.0.0", UpdateStatus::UpToDate),
        ("0.2.0-rc.2", "0.2.0", UpdateStatus::UpdateAvailable),
        ("0.2.0", "v0.2.0-rc.3", UpdateStatus::Ahead),
        ("0.1.41+local.1", "v0.1.41+build.2", UpdateStatus::UpToDate),
    ];
    for (current, tag, expected) in cases {
        let result = release_report(current, release(tag)).unwrap();
        assert_eq!(result.status, expected, "{current} vs {tag}");
        assert_eq!(result.current_version, current);
        assert_eq!(result.release_url, format!("{RELEASES_URL}/tag/{tag}"));
    }
}

#[test]
fn ignores_unpublished_releases_and_rejects_unknown_tags() {
    for (draft, prerelease) in [(true, false), (false, true)] {
        let mut unpublished = release("v0.2.0");
        unpublished.draft = draft;
        unpublished.prerelease = prerelease;
        let result = release_report("0.1.41", unpublished).unwrap();
        assert_eq!(result.status, UpdateStatus::NoRelease);
        assert_eq!(result.latest_version, None);
    }
    assert!(release_report("0.1.41", release("nightly"))
        .unwrap_err()
        .to_string()
        .contains("无法识别 GitHub 版本号"));
}

struct MockGitHub {
    endpoint: String,
    port: u16,
    server: tokio::task::JoinHandle<()>,
}

impl MockGitHub {
    async fn start(router: Router) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            endpoint: format!("http://127.0.0.1:{port}/releases/latest"),
            port,
            server,
        }
    }
}

impl Drop for MockGitHub {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[tokio::test]
async fn sends_github_headers_and_returns_download_page() {
    let (sent, mut received) = tokio::sync::mpsc::unbounded_channel();
    let mock = MockGitHub::start(Router::new().fallback(get(move |headers: HeaderMap| {
        let sent = sent.clone();
        async move {
            sent.send(headers).unwrap();
            Json(json!({ "tag_name": "v0.2.0", "draft": false, "prerelease": false }))
        }
    })))
    .await;
    let result = fetch_latest(&build_client(None).unwrap(), &mock.endpoint, "0.1.41")
        .await
        .unwrap();
    assert_eq!(result.status, UpdateStatus::UpdateAvailable);
    assert_eq!(result.latest_version.as_deref(), Some("0.2.0"));
    assert_eq!(result.release_url, format!("{RELEASES_URL}/tag/v0.2.0"));
    let headers = received.recv().await.unwrap();
    assert_eq!(headers["accept"], "application/vnd.github+json");
    assert_eq!(headers["x-github-api-version"], "2022-11-28");
    assert_eq!(
        headers["user-agent"],
        concat!("YAP-XFISH/", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        serde_json::to_value(result).unwrap()["status"],
        "update_available"
    );
}

#[tokio::test]
async fn distinguishes_no_release_from_rate_limits_and_server_failures() {
    for (status, limited, error_text) in [
        (StatusCode::NOT_FOUND, false, None),
        (StatusCode::FORBIDDEN, true, Some("请求次数已达限制")),
        (
            StatusCode::TOO_MANY_REQUESTS,
            false,
            Some("请求次数已达限制"),
        ),
        (StatusCode::FORBIDDEN, false, Some("HTTP 403")),
        (StatusCode::INTERNAL_SERVER_ERROR, false, Some("HTTP 500")),
    ] {
        let mock = MockGitHub::start(Router::new().fallback(get(move || async move {
            let mut response = (status, "{}").into_response();
            if limited {
                response
                    .headers_mut()
                    .insert("x-ratelimit-remaining", "0".parse().unwrap());
            }
            response
        })))
        .await;
        let result = fetch_latest(&build_client(None).unwrap(), &mock.endpoint, "0.1.41").await;
        if let Some(message) = error_text {
            assert!(result.unwrap_err().to_string().contains(message));
        } else {
            let result = result.unwrap();
            assert_eq!(result.status, UpdateStatus::NoRelease);
            assert_eq!(result.latest_version, None);
            assert_eq!(result.release_url, RELEASES_URL);
        }
    }
}

#[tokio::test]
async fn invalid_json_is_an_error_instead_of_no_update() {
    let mock = MockGitHub::start(Router::new().fallback(get(|| async { "not JSON" }))).await;
    let error = fetch_latest(&build_client(None).unwrap(), &mock.endpoint, "0.1.41")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("版本信息无效"));
}

#[tokio::test]
async fn configured_proxy_handles_the_update_request() {
    let mock = MockGitHub::start(Router::new().fallback(get(|| async {
        Json(json!({ "tag_name": "v0.2.0", "draft": false, "prerelease": false }))
    })))
    .await;
    let result = fetch_latest(
        &build_client(Some(mock.port)).unwrap(),
        "http://update-check.invalid/releases/latest",
        "0.1.41",
    )
    .await
    .unwrap();
    assert_eq!(result.status, UpdateStatus::UpdateAvailable);
}

#[tokio::test]
async fn stalled_request_times_out_with_a_readable_error() {
    let mock = MockGitHub::start(Router::new().fallback(get(|| async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Json(json!({ "tag_name": "v0.2.0", "draft": false, "prerelease": false }))
    })))
    .await;
    let client = Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap();
    let error = fetch_latest(&client, &mock.endpoint, "0.1.41")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("超时"));
}

#[test]
fn reports_both_asset_kinds() {
    let mut published = release("v0.2.0");
    published.assets = vec![
        asset("yap-xfish-v0.2.0-update.zip", 6_000_000),
        asset("yap-xfish-v0.2.0-windows-amd64.zip", 34_000_000),
        asset("notes.txt", 10),
    ];
    let result = release_report("0.1.41", published).unwrap();
    assert_eq!(result.update_asset.unwrap().size, 6_000_000);
    assert_eq!(result.full_asset.unwrap().size, 34_000_000);
}

#[test]
fn builds_a_script_that_waits_and_restarts() {
    let exe_dir = Path::new("C:\\YAP XFISH");
    let script = build_script(exe_dir, Path::new("C:\\data\\update\\stage"), 4242, Path::new("C:\\data\\update"), false);
    assert!(script.contains("$targetPid = 4242"));
    assert!(script.contains("'C:\\YAP XFISH'"));
    assert!(script.contains("$skipStart = $false"));
    assert!(script.contains("Start-Process -FilePath $exe"));
    let skipped = build_script(exe_dir, Path::new("C:\\stage"), 1, Path::new("C:\\u"), true);
    assert!(skipped.contains("$skipStart = $true"));
}

#[test]
fn extracts_only_allowlisted_files() {
    let dir = std::env::temp_dir().join(format!("yap-update-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let zip_path = dir.join("update.zip");
    {
        let file = File::create(&zip_path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in [
            ("yap-xfish.exe", "app"),
            ("libcronet.dll", "dll"),
            ("sing-box.exe", "core"),
            ("../evil.exe", "evil"),
            ("update.json", "{}"),
        ] {
            writer.start_file(name, opts).unwrap();
            writer.write_all(body.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }
    let stage = dir.join("stage");
    fs::create_dir_all(&stage).unwrap();
    let files = extract_zip(&zip_path, &stage, false).unwrap();
    assert_eq!(
        files,
        vec!["yap-xfish.exe".to_string(), "libcronet.dll".to_string()]
    );
    assert!(stage.join("yap-xfish.exe").exists());
    assert!(!stage.join("sing-box.exe").exists());
    assert!(!stage.join("evil.exe").exists());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "runs PowerShell"]
fn generated_script_replaces_files_in_a_sandbox() {
    let dir = std::env::temp_dir().join(format!("yap-update-script-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let install = dir.join("install");
    let stage = dir.join("stage");
    fs::create_dir_all(&install).unwrap();
    fs::create_dir_all(&stage).unwrap();
    fs::write(install.join("yap-xfish.exe"), "old").unwrap();
    fs::write(stage.join("yap-xfish.exe"), "new").unwrap();
    fs::write(stage.join("libcronet.dll"), "dll").unwrap();
    let script = dir.join("apply.ps1");
    // 999999 不存在，脚本会立即越过等待；skip_start 避免在测试里拉起进程
    fs::write(&script, build_script(&install, &stage, 999_999, &dir, true)).unwrap();

    let status = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&script)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(fs::read_to_string(install.join("yap-xfish.exe")).unwrap(), "new");
    assert!(install.join("libcronet.dll").exists());
    assert!(!stage.exists());
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
#[ignore = "downloads the real release asset"]
async fn live_download_verifies_and_extracts() {
    let port: u16 = std::env::var("YAP_PROXY_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(10020);
    let client = build_client_with_timeout(Some(port), Duration::from_secs(15 * 60)).unwrap();
    let release = fetch_release(&client, LATEST_RELEASE_API).await.unwrap().unwrap();
    let asset = release
        .asset_suffix("-update.zip")
        .or_else(|| release.asset_suffix("-windows-amd64.zip"))
        .expect("release has no archive");
    let dir = std::env::temp_dir().join(format!("yap-update-live-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let zip_path = dir.join("asset.zip");
    download_asset(&client, asset, &zip_path).await.unwrap();
    let stage = dir.join("stage");
    fs::create_dir_all(&stage).unwrap();
    let files = extract_zip(&zip_path, &stage, asset.name.ends_with("-windows-amd64.zip")).unwrap();
    println!("{} -> {files:?}", asset.name);
    assert!(files.iter().any(|f| f == "yap-xfish.exe"));
    assert!(stage.join("yap-xfish.exe").metadata().unwrap().len() > 1_000_000);
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
#[ignore = "requires access to GitHub"]
async fn live_github_update_check() {
    let result = fetch_latest(
        &build_client(None).unwrap(),
        LATEST_RELEASE_API,
        env!("CARGO_PKG_VERSION"),
    )
    .await
    .unwrap();
    assert_eq!(result.current_version, env!("CARGO_PKG_VERSION"));
    assert!(result.release_url.starts_with(RELEASES_URL));
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
}
