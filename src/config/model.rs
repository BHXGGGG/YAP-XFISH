use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 代理模式：全局 / 规则 / 直连
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ProxyMode {
    #[default]
    Global,
    Rule,
    Direct,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WsOptions {
    pub path: Option<String>,
    pub host: Option<String>,
    pub headers: Option<HashMap<String, String>>,
}

/// 单个节点（出站），是内部配置模型的最小单元。
/// 与具体协议无关的公共字段 + `extra` 透传未知字段，渲染时再转为 sing-box outbound。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Node {
    pub id: String,
    pub name: String,
    #[serde(rename = "type", default = "default_kind")]
    pub kind: String,
    pub server: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cipher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flow: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpn: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ws: Option<WsOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<serde_json::Value>,
    // 运行时字段
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency: Option<u32>,
    /// 测速结果状态：`None` = 从未测速；`Some("ok")` = 测过且有 latency；
    /// `Some("timeout")` / `Some("unreachable")` = 测过但失败。
    /// 用于前端区分"未测速"和"超时/不可达"。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_id: Option<String>,
}

fn default_kind() -> String {
    "shadowsocks".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RouteRule {
    pub id: String,
    pub name: String,
    #[serde(rename = "type", default = "default_rule_type")]
    pub r#type: String, // domain | ip | process_name | port
    pub payload: String, // geosite:cn / domain_suffix:example.com / 10.0.0.0/8
    pub outbound: String, // 目标出站 tag
}

fn default_rule_type() -> String {
    "domain".into()
}

/// 内部配置模型：节点列表 + 选中节点 + 规则 + 模式。
/// 渲染为 sing-box config.json 时再展开为具体内核结构。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppProfile {
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_node: Option<String>,
    #[serde(default)]
    pub rules: Vec<RouteRule>,
    #[serde(default)]
    pub mode: ProxyMode,
}
