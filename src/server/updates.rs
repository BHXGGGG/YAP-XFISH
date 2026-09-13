use std::{cmp::Ordering, sync::Arc, time::Duration};

use anyhow::{anyhow, bail, Context};
use axum::{extract::State, Json};
use reqwest::{Client, StatusCode, Url};
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{app::AppState, error::AppResult};

const LATEST_RELEASE_API: &str = "https://api.github.com/repos/BHXGGGG/YAP-XFISH/releases/latest";
const RELEASES_URL: &str = "https://github.com/BHXGGGG/YAP-XFISH/releases";

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
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
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
    let mut builder = Client::builder()
        .user_agent(concat!("YAP-XFISH/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(3))
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
    let response = client
        .get(endpoint)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(request_error)?;

    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(no_release(current_version));
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
    release_report(current_version, release)
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
    Ok(UpdateCheck {
        current_version: current_version.to_owned(),
        latest_version: Some(latest.to_string()),
        status,
        release_url: release_url.to_string(),
    })
}

#[cfg(test)]
mod tests;
