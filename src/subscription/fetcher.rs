use crate::error::AppResult;

/// 下载订阅内容。
///
/// - 超时 30s，避免订阅服务器卡死后台。
/// - 支持自定义 User-Agent（部分订阅要求特定 UA，否则返回 403/空）。
/// - reqwest 启用 gzip feature，遇到 `Content-Encoding: gzip` 会自动解压，无需手动处理。
pub async fn fetch_raw(url: &str, user_agent: Option<&str>) -> AppResult<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    // 默认 UA：先后尝试两个常见代理客户端 UA。
    // 部分订阅站（link123 / 52pokemon 等）会拦截浏览器 UA，也拦截 clash-verge 等已知代理客户端，
    // 但放行 Clash Meta (mihomo) / uif。优先 mihomo 以最大化兼容性；用户仍可在订阅设置里自定义。
    let ua = user_agent
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "mihomo/1.18.0".to_string());
    let resp = client
        .get(url)
        .header(reqwest::header::USER_AGENT, ua)
        .header(reqwest::header::ACCEPT, "*/*")
        .header(reqwest::header::ACCEPT_ENCODING, "gzip, deflate")
        .send()
        .await?;

    let resp = resp.error_for_status()?;
    let text = resp.text().await?;
    Ok(text)
}
