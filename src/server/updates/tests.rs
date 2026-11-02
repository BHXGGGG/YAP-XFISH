use super::*;
use axum::{
    http::HeaderMap,
    response::IntoResponse,
    routing::get,
    Router,
};
use std::io::Write;

/// 模拟 GitHub 的 `releases/latest`：302 到 tag 页面。
fn redirects_to(tag: &str) -> impl IntoResponse {
    (
        StatusCode::FOUND,
        [(
            "location",
            format!("https://github.com/BHXGGGG/YAP-XFISH/releases/tag/{tag}"),
        )],
    )
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
        let result = release_report(current, tag).unwrap();
        assert_eq!(result.status, expected, "{current} vs {tag}");
        assert_eq!(result.current_version, current);
        assert_eq!(result.release_url, format!("{RELEASES_URL}/tag/{tag}"));
    }
}

#[test]
fn rejects_unknown_tags_and_empty_redirects() {
    assert!(release_report("0.1.41", "nightly")
        .unwrap_err()
        .to_string()
        .contains("无法识别 GitHub 版本号"));
    assert_eq!(
        release_report("0.1.41", "").unwrap().status,
        UpdateStatus::NoRelease
    );
}

#[test]
fn builds_asset_names_from_the_tag() {
    let result = release_report("0.1.41", "v0.2.0").unwrap();
    let update = result.update_asset.unwrap();
    assert_eq!(update.name, "yap-xfish-v0.2.0-update.zip");
    // 不查询 API，所以体积未知
    assert_eq!(update.size, 0);
    assert_eq!(
        result.full_asset.unwrap().name,
        "yap-xfish-v0.2.0-windows-amd64.zip"
    );
}

#[test]
fn parses_the_tag_out_of_a_release_redirect() {
    assert_eq!(
        parse_tag_from_location("https://github.com/o/r/releases/tag/v0.1.51").as_deref(),
        Some("v0.1.51")
    );
    assert_eq!(
        parse_tag_from_location("/o/r/releases/tag/v1.0.0-beta.1?x=1").as_deref(),
        Some("v1.0.0-beta.1")
    );
    assert_eq!(parse_tag_from_location("https://github.com/o/r/releases"), None);
    assert_eq!(
        parse_tag_from_location("https://github.com/o/r/releases/tag/"),
        None
    );
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
async fn follows_the_release_redirect_without_the_api() {
    let (sent, mut received) = tokio::sync::mpsc::unbounded_channel();
    let mock = MockGitHub::start(Router::new().fallback(get(move |headers: HeaderMap| {
        let sent = sent.clone();
        async move {
            sent.send(headers).unwrap();
            redirects_to("v0.2.0")
        }
    })))
    .await;
    let result = fetch_latest(&build_probe_client(None).unwrap(), &mock.endpoint, "0.1.41")
        .await
        .unwrap();
    assert_eq!(result.status, UpdateStatus::UpdateAvailable);
    assert_eq!(result.latest_version.as_deref(), Some("0.2.0"));
    let headers = received.recv().await.unwrap();
    // 请求的是网页而不是 API，因此不带 API 专用头
    assert_eq!(
        headers["user-agent"],
        concat!("YAP-XFISH/", env!("CARGO_PKG_VERSION"))
    );
    assert!(headers.get("x-github-api-version").is_none());
}

#[tokio::test]
async fn missing_release_page_is_reported_as_no_release() {
    let mock = MockGitHub::start(Router::new().fallback(get(|| async {
        (StatusCode::NOT_FOUND, "").into_response()
    })))
    .await;
    let result = fetch_latest(&build_probe_client(None).unwrap(), &mock.endpoint, "0.1.41")
        .await
        .unwrap();
    assert_eq!(result.status, UpdateStatus::NoRelease);
    assert_eq!(result.latest_version, None);
    assert_eq!(result.release_url, RELEASES_URL);
}

#[tokio::test]
async fn a_rendered_page_means_there_is_no_published_release() {
    // 仓库还没有任何正式版时，GitHub 不跳转，直接返回 200 的列表页
    let mock = MockGitHub::start(Router::new().fallback(get(|| async { "<html></html>" }))).await;
    let result = fetch_latest(&build_probe_client(None).unwrap(), &mock.endpoint, "0.1.41")
        .await
        .unwrap();
    assert_eq!(result.status, UpdateStatus::NoRelease);
}

#[tokio::test]
async fn server_failures_are_reported_with_the_status_code() {
    for status in [
        StatusCode::FORBIDDEN,
        StatusCode::TOO_MANY_REQUESTS,
        StatusCode::INTERNAL_SERVER_ERROR,
    ] {
        let mock = MockGitHub::start(Router::new().fallback(get(move || async move {
            (status, "").into_response()
        })))
        .await;
        let error = fetch_latest(&build_probe_client(None).unwrap(), &mock.endpoint, "0.1.41")
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains(&status.as_u16().to_string()),
            "{status}: {error}"
        );
    }
}

#[tokio::test]
async fn configured_proxy_handles_the_update_request() {
    let mock = MockGitHub::start(Router::new().fallback(get(|| async {
        redirects_to("v0.2.0")
    })))
    .await;
    // 代理指向 mock 自己，能连上就说明请求确实走了代理配置
    let result = fetch_latest(
        &build_probe_client(Some(mock.port)).unwrap(),
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
        redirects_to("v0.2.0").into_response()
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
fn builds_asset_download_urls_from_the_tag() {
    assert_eq!(
        release_asset_url("v0.1.51", "update.zip"),
        "https://github.com/BHXGGGG/YAP-XFISH/releases/download/v0.1.51/yap-xfish-v0.1.51-update.zip"
    );
    assert_eq!(
        release_asset_url("v0.1.51", "windows-amd64.zip"),
        "https://github.com/BHXGGGG/YAP-XFISH/releases/download/v0.1.51/yap-xfish-v0.1.51-windows-amd64.zip"
    );
}

#[tokio::test]
async fn download_reports_missing_assets_instead_of_failing() {
    let mock = MockGitHub::start(Router::new().fallback(get(|| async {
        (StatusCode::NOT_FOUND, "").into_response()
    })))
    .await;
    let dir = std::env::temp_dir().join(format!("yap-update-404-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let dest = dir.join("x.zip");
    let found = try_download(
        &build_client(None).unwrap(),
        &format!("http://127.0.0.1:{}/asset", mock.port),
        &dest,
    )
    .await
    .unwrap();
    assert!(!found);
    assert!(!dest.exists());
    let _ = fs::remove_dir_all(&dir);
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
    let probe = build_probe_client(Some(port)).unwrap();
    let client = build_client_with_timeout(Some(port), Duration::from_secs(15 * 60)).unwrap();
    let tag = fetch_latest_tag(&probe, LATEST_RELEASE_PAGE)
        .await
        .unwrap()
        .expect("no published release");
    let dir = std::env::temp_dir().join(format!("yap-update-live-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let zip_path = dir.join("asset.zip");
    if !try_download(&client, &release_asset_url(&tag, "update.zip"), &zip_path)
        .await
        .unwrap()
    {
        assert!(
            try_download(&client, &release_asset_url(&tag, "windows-amd64.zip"), &zip_path)
                .await
                .unwrap()
        );
    }
    let stage = dir.join("stage");
    fs::create_dir_all(&stage).unwrap();
    let files = extract_zip(&zip_path, &stage, false).unwrap();
    println!("{tag} -> {files:?}");
    assert!(files.iter().any(|f| f == "yap-xfish.exe"));
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
#[ignore = "requires access to GitHub"]
async fn live_github_update_check() {
    let result = fetch_latest(
        &build_probe_client(None).unwrap(),
        LATEST_RELEASE_PAGE,
        env!("CARGO_PKG_VERSION"),
    )
    .await
    .unwrap();
    assert_eq!(result.current_version, env!("CARGO_PKG_VERSION"));
    assert!(result.release_url.starts_with(RELEASES_URL));
    println!("{}", serde_json::to_string_pretty(&result).unwrap());
}
