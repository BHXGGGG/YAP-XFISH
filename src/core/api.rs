use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use reqwest::{Client, Method, RequestBuilder};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        http::{header::AUTHORIZATION, HeaderValue},
        Message,
    },
};

use crate::app::AppEvent;

#[derive(Clone)]
pub struct MihomoApi {
    client: Client,
    http_base: String,
    ws_base: String,
    secret: String,
}

impl MihomoApi {
    pub fn new(port: u16, secret: String) -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .timeout(Duration::from_secs(5))
                .no_proxy()
                .build()?,
            http_base: format!("http://127.0.0.1:{port}"),
            ws_base: format!("ws://127.0.0.1:{port}"),
            secret,
        })
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let request = self.client.request(method, format!("{}{path}", self.http_base));
        if self.secret.is_empty() {
            request
        } else {
            request.bearer_auth(&self.secret)
        }
    }

    async fn response(&self, request: RequestBuilder, action: &str) -> Result<reqwest::Response> {
        let response = request.send().await.with_context(|| action.to_string())?;
        if !response.status().is_success() {
            let status = response.status();
            let detail = response.json::<Value>().await.ok()
                .and_then(|value| value.get("message")?.as_str().map(str::to_owned))
                .unwrap_or_default();
            bail!("{action}: HTTP {status}{}", if detail.is_empty() { String::new() } else { format!(": {detail}") });
        }
        Ok(response)
    }

    pub async fn get_json(&self, path: &str) -> Result<Value> {
        let response = self
            .response(self.request(Method::GET, path), &format!("Mihomo API GET {path}"))
            .await?;
        response.json().await.context("读取 Mihomo API JSON 响应失败")
    }

    pub async fn put_json(&self, path: &str, value: &Value) -> Result<()> {
        let request = self.request(Method::PUT, path).json(value);
        self.response(request, &format!("Mihomo API PUT {path}"))
            .await?;
        Ok(())
    }

    pub async fn put(&self, path: &str) -> Result<()> {
        self.response(self.request(Method::PUT, path), &format!("Mihomo API PUT {path}"))
            .await?;
        Ok(())
    }

    pub async fn version(&self) -> Result<String> {
        self.get_json("/version")
            .await?
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("Mihomo /version 响应缺少 version"))
    }

    pub async fn proxy_names(&self) -> Result<Vec<String>> {
        let value = self.get_json("/proxies").await?;
        Ok(value
            .get("proxies")
            .and_then(Value::as_object)
            .context("Mihomo /proxies 响应缺少 proxies 对象")?
            .keys()
            .cloned()
            .collect())
    }

    pub async fn delay(&self, name: &str, url: &str, timeout_ms: u64) -> Option<u32> {
        self.delay_at(&format!("/proxies/{}/delay", encode_path_segment(name)), url, timeout_ms).await
    }

    pub async fn provider_proxy_names(&self, provider: &str) -> Result<Vec<String>> {
        let value = self.get_json(&format!("/providers/proxies/{}", encode_path_segment(provider))).await?;
        Ok(value["proxies"].as_array().context("Mihomo Provider 响应缺少 proxies 列表")?
            .iter().filter_map(|proxy| proxy["name"].as_str().map(str::to_owned)).collect())
    }

    pub async fn provider_delay(&self, provider: &str, name: &str, url: &str, timeout_ms: u64) -> Option<u32> {
        self.delay_at(&format!("/providers/proxies/{}/{}/healthcheck", encode_path_segment(provider), encode_path_segment(name)), url, timeout_ms).await
    }

    async fn delay_at(&self, endpoint: &str, url: &str, timeout_ms: u64) -> Option<u32> {
        let path = format!(
            "{endpoint}?timeout={}&url={}",
            timeout_ms.max(500),
            encode_path_segment(url)
        );
        let response = self
            .response(
                self.request(Method::GET, &path)
                    .timeout(Duration::from_millis(timeout_ms.max(500).saturating_add(500))),
                "Mihomo 节点测速",
            )
            .await
            .ok()?;
        response
            .json::<Value>()
            .await
            .ok()?
            .get("delay")?
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0)
    }

    pub async fn select_proxy(&self, group: &str, name: &str) -> Result<()> {
        self.put_json(
            &format!("/proxies/{}", encode_path_segment(group)),
            &serde_json::json!({ "name": name }),
        )
        .await
    }

    pub async fn update_provider(&self, name: &str) -> Result<()> {
        self.put(&format!(
            "/providers/proxies/{}",
            encode_path_segment(name)
        ))
        .await
    }

    pub async fn reload_config(&self, path: &Path) -> Result<()> {
        self.put_json(
            "/configs?force=true",
            &serde_json::json!({
                "path": path.to_string_lossy(),
                "payload": ""
            }),
        )
        .await
    }

    pub async fn connections(&self) -> Result<Value> {
        self.get_json("/connections").await
    }

    pub async fn close_connection(&self, id: Option<&str>) -> Result<()> {
        let path = match id {
            Some(id) => format!("/connections/{}", encode_path_segment(id)),
            None => "/connections".to_string(),
        };
        self.response(self.request(Method::DELETE, &path), "关闭 Mihomo 连接")
            .await?;
        Ok(())
    }

    pub async fn stream_logs(&self, tx: broadcast::Sender<AppEvent>) -> Result<()> {
        let mut request = format!("{}/logs?level=info", self.ws_base)
            .into_client_request()
            .context("创建 Mihomo 日志 WebSocket 请求失败")?;
        if !self.secret.is_empty() {
            request.headers_mut().insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {}", self.secret))?,
            );
        }
        let (mut socket, _) = connect_async(request)
            .await
            .context("连接 Mihomo 日志 WebSocket 失败")?;
        while let Some(message) = socket.next().await {
            let message = message.context("读取 Mihomo 日志 WebSocket 失败")?;
            let Message::Text(text) = message else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let message = value
                .get("payload")
                .and_then(Value::as_str)
                .or_else(|| value.get("message").and_then(Value::as_str))
                .unwrap_or_default();
            if message.is_empty() {
                continue;
            }
            let level = match value
                .get("type")
                .or_else(|| value.get("level"))
                .and_then(Value::as_str)
                .unwrap_or("info")
            {
                "warning" => "warn",
                "error" => "error",
                "debug" => "debug",
                _ => "info",
            };
            crate::system::log_file::append("core", level, message);
            let _ = tx.send(AppEvent::Log {
                level: level.to_string(),
                source: "core".into(),
                message: message.to_string(),
            });
        }
        Ok(())
    }
}

fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}
