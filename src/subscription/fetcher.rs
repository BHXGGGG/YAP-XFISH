use crate::error::{AppError, AppResult};

/// 下载订阅内容。
///
/// - 超时 30s，避免订阅服务器卡死后台。
/// - 支持自定义 User-Agent（部分订阅要求特定 UA，否则返回 403/空）。
/// - reqwest 启用 gzip feature，遇到 `Content-Encoding: gzip` 会自动解压。
/// - **默认忽略环境/系统代理**（`no_proxy`）：订阅必须直连源站；
///   否则在设置了坏的 `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` 时，
///   `Client::builder().build()` 会直接失败并只显示含糊的 `builder error`。
pub async fn fetch_raw(url: &str, user_agent: Option<&str>) -> AppResult<(String, SubMeta)> {
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

    let mut meta = SubMeta::from_headers(resp.headers());
    let text = resp
        .text()
        .await
        .map_err(|e| AppError(anyhow::anyhow!("读取订阅正文失败: {}", format_reqwest_error(&e))))?;

    // 多数机场面板（v2board / xboard 等）只对 Clash 系 UA 返回名字和流量头，
    // 但同时会把正文换成 Clash YAML。正文仍用上面的 UA 拉取，缺头时
    // 再用 Clash Verge UA 请求一次，只取响应头、丢弃正文。
    if meta.name.is_none() || meta.userinfo.is_none() {
        if let Ok(resp) = client
            .get(url)
            .header(reqwest::header::USER_AGENT, META_UA)
            .header(reqwest::header::ACCEPT, "*/*")
            .send()
            .await
        {
            if resp.status().is_success() {
                let extra = SubMeta::from_headers(resp.headers());
                meta.name = meta.name.or(extra.name);
                meta.userinfo = meta.userinfo.or(extra.userinfo);
            }
        }
    }
    Ok((text, meta))
}

/// 用来换取订阅名字 / 流量响应头的 UA（与 FlClash、Clash Verge 一致的关键字）。
const META_UA: &str = "clash-verge/v2.2.3";

/// 订阅响应头里的附加信息。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SubMeta {
    /// `Content-Disposition` 里的文件名，即机场名。
    pub name: Option<String>,
    /// `Subscription-Userinfo`：流量与到期时间。
    pub userinfo: Option<SubUserinfo>,
}

#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SubUserinfo {
    pub upload: u64,
    pub download: u64,
    pub total: u64,
    /// Unix 秒；为空 / 0 表示长期有效。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expire: Option<i64>,
}

impl SubMeta {
    fn from_headers(h: &reqwest::header::HeaderMap) -> Self {
        let get = |k: &str| h.get(k).and_then(|v| v.to_str().ok());
        SubMeta {
            name: get("content-disposition").and_then(parse_disposition_filename),
            userinfo: get("subscription-userinfo").and_then(parse_userinfo),
        }
    }
}

/// 解析 `attachment;filename*=UTF-8''%E4%B8%80%E5%88%86` 或 `filename="xxx"`。
/// 优先 RFC 5987 的 `filename*`（可含中文），其次普通 `filename`。
pub fn parse_disposition_filename(v: &str) -> Option<String> {
    let params: Vec<(String, &str)> = v
        .split(';')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim()))
        .collect();
    let star = params.iter().find(|(k, _)| k == "filename*").and_then(|(_, v)| {
        let (_, enc) = v.split_once("''")?;
        percent_decode(enc.trim_matches('"'))
    });
    let plain = || {
        params
            .iter()
            .find(|(k, _)| k == "filename")
            .and_then(|(_, v)| percent_decode(v.trim_matches('"')))
    };
    star.or_else(plain)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// 解析 `upload=1; download=2; total=3; expire=1796647729`（expire 可为空）。
pub fn parse_userinfo(v: &str) -> Option<SubUserinfo> {
    let mut info = SubUserinfo::default();
    let mut any = false;
    for part in v.split(';') {
        let Some((k, val)) = part.split_once('=') else { continue };
        let val = val.trim();
        match k.trim().to_ascii_lowercase().as_str() {
            "upload" => info.upload = val.parse().unwrap_or(0),
            "download" => info.download = val.parse().unwrap_or(0),
            "total" => info.total = val.parse().unwrap_or(0),
            "expire" => info.expire = val.parse().ok().filter(|&t: &i64| t > 0),
            _ => continue,
        }
        any = true;
    }
    any.then_some(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_disposition_names() {
        assert_eq!(
            parse_disposition_filename("attachment;filename*=UTF-8''%E4%B8%80%E5%88%86%E6%9C%BA%E5%9C%BA").as_deref(),
            Some("一分机场")
        );
        assert_eq!(
            parse_disposition_filename("attachment;filename*=UTF-8''SharkCloud").as_deref(),
            Some("SharkCloud")
        );
        assert_eq!(
            parse_disposition_filename("attachment; filename=\"abc.yaml\"").as_deref(),
            Some("abc.yaml")
        );
        assert_eq!(parse_disposition_filename("attachment"), None);
        assert_eq!(parse_disposition_filename("attachment;filename*=UTF-8''%E4%B8"), None);
    }

    /// 联网验证：`SUB_URL=... cargo test fetches_real_meta -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn fetches_real_meta() {
        let url = std::env::var("SUB_URL").expect("SUB_URL");
        let (body, meta) = fetch_raw(&url, None).await.unwrap();
        println!("body={}B name={:?} userinfo={:?}", body.len(), meta.name, meta.userinfo);
        assert!(meta.name.is_some());
    }

    #[test]
    fn parses_userinfo() {
        let u = parse_userinfo("upload=41927691; download=230455529; total=64424509440; expire=1796647729").unwrap();
        assert_eq!((u.upload, u.download, u.total, u.expire), (41927691, 230455529, 64424509440, Some(1796647729)));
        let u = parse_userinfo("upload=1; download=2; total=3; expire=").unwrap();
        assert_eq!(u.expire, None);
        assert_eq!(parse_userinfo("garbage"), None);
    }
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
