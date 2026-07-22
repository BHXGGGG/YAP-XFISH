//! 路由规则增强（阶段 6）。
//!
//! 基础 CRUD 与渲染已在 `config::model::RouteRule` / `config::render` 打通；
//! 本模块提供「常用规则预设」，便于前端一键添加典型分流规则。
//! 渲染器直接把 `type`/`payload` 透传为 sing-box 的 route.rules 条目，
//! 因此这里给出的类型（geosite/geoip/ip_cidr/domain_suffix 等）均为合法 sing-box 规则类型。

use crate::app::gen_id;
use crate::config::model::RouteRule;

/// 常用分流规则预设。geoip/geosite 类规则需 sing-box 具备对应地理数据库。
pub fn presets() -> Vec<RouteRule> {
    let mut v = Vec::new();

    v.push(rule("国内 GeoSite 直连", "geosite", "cn", "direct"));
    v.push(rule("国内 GeoIP 直连", "geoip", "cn", "direct"));
    v.push(rule("私网地址直连", "ip_cidr", "192.168.0.0/16", "direct"));
    v.push(rule("本地回环直连", "ip_cidr", "127.0.0.0/8", "direct"));
    v.push(rule("链路本地直连", "ip_cidr", "169.254.0.0/16", "direct"));
    v.push(rule("广告域名拦截", "domain_suffix", "doubleclick.net", "block"));
    v.push(rule("广告域名拦截(2)", "domain_suffix", "googlesyndication.com", "block"));

    v
}

fn rule(name: &str, r#type: &str, payload: &str, outbound: &str) -> RouteRule {
    RouteRule {
        id: gen_id(),
        name: name.to_string(),
        r#type: r#type.to_string(),
        payload: payload.to_string(),
        outbound: outbound.to_string(),
    }
}
