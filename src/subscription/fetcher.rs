use crate::error::{AppError, AppResult};

/// 下载订阅内容。
///
/// - 超时 30s，避免订阅服务器卡死后台。
/// - 支持自定义 User-Agent（部分订阅要求特定 UA，否则返回 403/空）。
/// - reqwest 启用 gzip feature，遇到 `Content-Encoding: gzip` 会自动解压。
/// - **默认忽略环境/系统代理**（`no_proxy`）：订阅必须直连源站；
///   否则在设置了坏的 `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` 时，
///   `Client::builder().build()` 会直接失败并只显示含糊的 `builder error`。
pub async fn fetch_raw(url: &str, user_agent: Option<&str>) -> AppResult<String> {
    let url = url.trim();
    if url.is_empty() {
        return Err(AppError(anyhow::anyhow!("订阅 URL 为空")));
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(AppError(anyhow::anyhow!(
            "订阅 URL 必须以 http:// 或 https:// 开头: {url}"
        )));
    }

    let client = build_direct_client().map_err(|e| {
        AppError(anyhow::anyhow!(
            "创建 HTTP 客户端失败（常见原因：环境变量 HTTP_PROXY/HTTPS_PROXY/ALL_PROXY 无效）: {e}"
        ))
    })?;

    // 默认 UA：部分订阅站拦截浏览器 UA / 部分客户端 UA。
    // 优先 mihomo 以兼容 link123 等；用户可在订阅设置自定义。
    let ua = user_agent
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "mihomo/1.18.0".to_string());

    let resp = client
        .get(url)
        .header(reqwest::header::USER_AGENT, &ua)
        .header(reqwest::header::ACCEPT, "*/*")
        .header(reqwest::header::ACCEPT_ENCODING, "gzip, deflate")
        .send()
        .await
        .map_err(|e| AppError(anyhow::anyhow!("请求订阅失败: {}", format_reqwest_error(&e))))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(200).collect();
        return Err(AppError(anyhow::anyhow!(
            "订阅服务器返回 HTTP {status}{}",
            if snippet.trim().is_empty() {
                String::new()
            } else {
                format!(": {snippet}")
            }
        )));
    }

    let text = resp
        .text()
        .await
        .map_err(|e| AppError(anyhow::anyhow!("读取订阅正文失败: {}", format_reqwest_error(&e))))?;
    Ok(text)
}

/// 订阅下载用直连客户端：不读环境代理，避免坏代理配置导致 builder error。
fn build_direct_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(10))
        // 关键：忽略 HTTP(S)_PROXY / ALL_PROXY / 系统代理
        .no_proxy()
        .build()
        .map_err(|e| format_reqwest_error(&e))
}

/// reqwest 的 Display 经常只有 "builder error"，需要把 source 链拼出来才有用。
fn format_reqwest_error(e: &reqwest::Error) -> String {
    let mut s = e.to_string();
    let mut src = std::error::Error::source(e);
    let mut n = 0;
    while let Some(c) = src {
        s.push_str(" → ");
        s.push_str(&c.to_string());
        src = c.source();
        n += 1;
        if n >= 6 {
            break;
        }
    }
    if e.is_timeout() {
        s.push_str(" (timeout)");
    }
    if e.is_connect() {
        s.push_str(" (connect)");
    }
    if e.is_request() {
        s.push_str(" (request)");
    }
    if e.is_builder() {
        s.push_str(" (builder)");
    }
    s
}
