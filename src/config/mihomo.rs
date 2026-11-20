use anyhow::{anyhow, bail, Result};
use serde_json::{json, Map, Value};
use std::collections::HashSet;

use crate::app::AppConfig;
use crate::config::model::{AppProfile, Node, ProxyMode, RouteRule};

pub const NODE_PROVIDER: &str = "YAP-NODES";
pub const NODE_GROUP: &str = "YAP-PROXY";
pub const NODE_PROVIDER_PATH: &str = "providers/nodes.yaml";
pub const TUN_DEVICE: &str = "YAP-XFISH";

/// Render the application profile to Mihomo's main YAML data model.
pub fn render(profile: &AppProfile, cfg: &AppConfig) -> Result<Value> {
    if cfg.proxy_port == 0 || cfg.clash_api_port == 0 || cfg.proxy_port == cfg.clash_api_port {
        bail!("代理端口和控制 API 端口必须非零且不能相同");
    }
    let dns_mode = cfg.tun_dns_mode.trim().to_ascii_lowercase();
    if !matches!(dns_mode.as_str(), "native" | "hijack") {
        bail!("不支持的 TUN DNS 接管模式: {dns_mode}");
    }

    let mut rules = Vec::new();
    for rule in &profile.rules {
        rules.extend(render_rule(rule, profile)?);
    }
    if profile.mode == ProxyMode::Rule {
        rules.push(json!("GEOIP,CN,DIRECT"));
        rules.push(json!("GEOSITE,CN,DIRECT"));
    }
    rules.push(json!(match profile.mode {
        ProxyMode::Direct => "MATCH,DIRECT",
        ProxyMode::Global | ProxyMode::Rule => "MATCH,YAP-PROXY",
    }));

    let mut tun = json!({
        "enable": cfg.enable_tun,
        "device": TUN_DEVICE,
        "stack": normalize_stack(&cfg.tun_stack)?,
        "auto-route": true,
        "auto-detect-interface": true,
        "strict-route": dns_mode == "hijack",
        "dns-mode": dns_mode,
        "inet4-address": ["172.19.0.1/30"],
        "mtu": normalize_mtu(cfg.tun_mtu),
    });
    if dns_mode == "hijack" {
        tun["dns-hijack"] = json!(["any:53", "tcp://any:53"]);
    }

    let mut providers = Map::new();
    let node_group = if profile.nodes.is_empty() {
        json!({ "name": NODE_GROUP, "type": "select", "proxies": ["DIRECT"] })
    } else {
        providers.insert(
            NODE_PROVIDER.into(),
            json!({ "type": "file", "path": NODE_PROVIDER_PATH }),
        );
        json!({ "name": NODE_GROUP, "type": "select", "use": [NODE_PROVIDER], "proxies": ["DIRECT"] })
    };
    let mut groups = vec![node_group];
    let mut referenced = HashSet::new();
    for rule in &profile.rules {
        if profile.nodes.iter().any(|node| node.id == rule.outbound.trim()) {
            referenced.insert(rule.outbound.trim().to_string());
        }
    }
    for node in &profile.nodes {
        if let Some(id) = resolve_detour(node, profile)? {
            referenced.insert(id.to_string());
        }
    }
    for node in &profile.nodes {
        if referenced.contains(&node.id) {
            let filter: String = node.id.chars().flat_map(|ch| {
                if "\\.^$|?*+()[]{}".contains(ch) { vec!['\\', ch] } else { vec![ch] }
            }).collect();
            groups.push(json!({
                "name": reference_group(&node.id), "type": "select",
                "use": [NODE_PROVIDER], "filter": format!("^{filter}$"), "hidden": true
            }));
        }
    }

    Ok(json!({
        "mixed-port": cfg.proxy_port,
        "allow-lan": false,
        "bind-address": "127.0.0.1",
        "mode": "rule",
        "log-level": "info",
        "external-controller": format!("127.0.0.1:{}", cfg.clash_api_port),
        "secret": cfg.api_secret,
        "find-process-mode": "strict",
        "profile": { "store-selected": false },
        "dns": {
            "enable": true,
            "ipv6": true,
            "enhanced-mode": "redir-host",
            "default-nameserver": ["223.5.5.5"],
            "nameserver": ["223.5.5.5"],
            "proxy-server-nameserver": ["223.5.5.5"],
        },
        "sniffer": { "enable": true, "parse-pure-ip": true },
        "tun": tun,
        "proxy-providers": providers,
        "proxy-groups": groups,
        "rules": rules,
    }))
}

/// Provider files keep subscription refreshes out of the core process lifecycle.
pub fn render_provider(profile: &AppProfile) -> Result<Value> {
    let mut ids = HashSet::with_capacity(profile.nodes.len());
    let mut proxies = Vec::with_capacity(profile.nodes.len());
    for node in &profile.nodes {
        if !ids.insert(node.id.as_str()) {
            bail!("节点 ID 重复，无法生成 Mihomo Provider: {}", node.id);
        }
        let mut proxy = node_to_proxy(node, profile)?;
        if let Some(detour) = proxy.get("dialer-proxy").and_then(Value::as_str) {
            proxy["dialer-proxy"] = json!(reference_group(detour));
        }
        proxies.push(proxy);
    }
    Ok(json!({ "proxies": proxies }))
}

pub fn expand_with_detours(all: &[Node], selected: &[Node]) -> Vec<Node> {
    let mut have: HashSet<String> = selected.iter().map(|node| node.id.clone()).collect();
    let mut result = selected.to_vec();
    for node in selected {
        let mut current = node;
        let mut seen = HashSet::new();
        for _ in 0..32 {
            let Some(next) = detour_target(current, all) else {
                break;
            };
            if !seen.insert(next.id.clone()) {
                break;
            }
            if have.insert(next.id.clone()) {
                result.push(next.clone());
            }
            current = next;
        }
    }
    result
}

pub fn detour_is_safe(nodes: &[Node], from_id: &str, detour: &str) -> bool {
    let detour = detour.trim();
    if detour.is_empty() {
        return false;
    }
    let Some(mut current) = nodes.iter().find(|node| node.id == detour) else {
        return false;
    };
    let mut seen = HashSet::from([from_id.to_string()]);
    for _ in 0..32 {
        if !seen.insert(current.id.clone()) {
            return false;
        }
        if current.detour().is_none() {
            return true;
        }
        let Some(next) = detour_target(current, nodes) else {
            return false;
        };
        current = next;
    }
    false
}

pub fn node_to_proxy(node: &Node, profile: &AppProfile) -> Result<Value> {
    let mut proxy = match node.extra.as_ref() {
        Some(Value::Object(raw)) if raw.contains_key("server_port") => {
            singbox_node_to_proxy(node, raw)?
        }
        Some(Value::Object(raw)) if raw.contains_key("type") => {
            clash_node_to_proxy(node, raw)?
        }
        Some(_) => bail!("节点 {} 的扩展字段不是对象，无法转换为 Mihomo 代理", node.name),
        None => structured_node_to_proxy(node)?,
    };

    // sing-box 与链接模型默认支持 UDP，Mihomo 的这些协议需要显式开启。
    if node.extra.as_ref().is_none_or(|raw| raw.get("server_port").is_some())
        && matches!(proxy["type"].as_str(), Some("ss" | "socks5" | "vmess" | "vless" | "trojan" | "anytls"))
    {
        proxy["udp"] = json!(true);
    }
    proxy["name"] = json!(node.id);
    proxy["server"] = json!(node.server);
    proxy["port"] = json!(node.port);
    proxy.as_object_mut().unwrap().remove("dialer-proxy");
    if let Some(detour) = resolve_detour(node, profile)? {
        proxy["dialer-proxy"] = json!(detour);
    }
    Ok(proxy)
}

fn reference_group(id: &str) -> String {
    format!("YAP-NODE-{id}")
}

pub fn detour_target<'a>(node: &Node, nodes: &'a [Node]) -> Option<&'a Node> {
    let detour = node.detour()?;
    if node.detour.is_some() {
        return nodes.iter().find(|candidate| candidate.id == detour);
    }
    nodes.iter().find(|candidate| {
        candidate.subscription_id == node.subscription_id
            && (candidate.id == detour
                || candidate.name == detour
                || candidate
                    .extra
                    .as_ref()
                    .and_then(|extra| extra.get("tag"))
                    .and_then(Value::as_str)
                    == Some(detour))
    })
}

fn resolve_detour<'a>(node: &Node, profile: &'a AppProfile) -> Result<Option<&'a str>> {
    if let Some(detour) = node.detour() {
        let detour = detour_target(node, &profile.nodes)
            .map(|candidate| candidate.id.as_str())
            .ok_or_else(|| anyhow!("节点 {} 引用的前置代理不存在: {detour}", node.name))?;
        if !detour_is_safe(&profile.nodes, &node.id, detour) {
            bail!("节点 {} 的链式前置代理无效或形成环路", node.name);
        }
        return Ok(Some(detour));
    }
    Ok(None)
}

fn clash_node_to_proxy(node: &Node, raw: &Map<String, Value>) -> Result<Value> {
    let mut proxy = Value::Object(raw.clone());
    let source_type = raw
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or(&node.kind);
    let kind = normalize_kind(source_type)?;
    if kind == "anytls" && raw.contains_key("reality-opts") {
        bail!("Mihomo 不支持 AnyTLS + Reality: {}", node.name);
    }
    proxy["type"] = json!(kind);
    Ok(proxy)
}

fn singbox_node_to_proxy(node: &Node, raw: &Map<String, Value>) -> Result<Value> {
    let source_type = raw
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or(&node.kind);
    let kind = normalize_kind(source_type)?;
    let mut proxy = json!({
        "name": node.id,
        "type": kind,
        "server": node.server,
        "port": node.port,
    });

    for (key, value) in raw {
        match key.as_str() {
            "type" | "tag" | "server" | "server_port" => {}
            "method" | "security" => proxy["cipher"] = value.clone(),
            "username" | "password" | "uuid" | "flow" | "alpn" => {
                proxy[key] = value.clone()
            }
            "alter_id" => proxy["alterId"] = value.clone(),
            "tls" => apply_singbox_tls(&mut proxy, value, &node.name)?,
            "transport" => apply_singbox_transport(&mut proxy, value, &node.name)?,
            "detour" => {}
            "plugin" => proxy["plugin"] = json!(plugin_name(value.as_str().unwrap_or_default(), &node.name)?),
            "plugin_opts" => {
                let plugin = raw.get("plugin").and_then(Value::as_str).unwrap_or_default();
                proxy["plugin-opts"] = plugin_options(plugin, value, &node.name)?;
            }
            "up_mbps" => proxy["up"] = value.clone(),
            "down_mbps" => proxy["down"] = value.clone(),
            "congestion_control" => proxy["congestion-controller"] = value.clone(),
            "udp_relay_mode" => proxy["udp-relay-mode"] = value.clone(),
            "disable_sni" | "packet_encoding" | "domain_resolver" => {
                bail!("节点 {} 的 sing-box 字段 {key} 没有 Mihomo 等价项", node.name)
            }
            other => bail!("节点 {} 的 sing-box 字段 {other} 尚无 Mihomo 映射", node.name),
        }
    }
    if kind == "anytls" && raw.get("tls").and_then(|tls| tls.get("reality")).is_some() {
        bail!("Mihomo 不支持 AnyTLS + Reality: {}", node.name);
    }
    Ok(proxy)
}

fn plugin_name<'a>(plugin: &'a str, name: &str) -> Result<&'a str> {
    match plugin {
        "obfs-local" | "obfs" => Ok("obfs"),
        "v2ray-plugin" => Ok(plugin),
        _ => bail!("节点 {name} 的 SS 插件不受支持: {plugin}"),
    }
}

fn plugin_options(plugin: &str, value: &Value, name: &str) -> Result<Value> {
    let plugin = plugin_name(plugin, name)?;
    let text = value.as_str().ok_or_else(|| anyhow!("节点 {name} 的 sing-box plugin_opts 必须为字符串"))?;
    let mut options = Map::new();
    for part in text.split(';').filter(|part| !part.is_empty()) {
        let (key, value) = part.split_once('=').unwrap_or((part, "true"));
        let key = match (plugin, key) {
            ("obfs", "obfs") => "mode",
            ("obfs", "obfs-host") => "host",
            ("v2ray-plugin", "mode" | "host" | "path" | "tls" | "mux" | "skip-cert-verify") => key,
            _ => bail!("节点 {name} 的 SS 插件参数没有 Mihomo 映射: {key}"),
        };
        let value = if matches!(key, "tls" | "mux" | "skip-cert-verify") {
            json!(match value {
                "true" | "1" => true,
                "false" | "0" => false,
                _ => bail!("节点 {name} 的插件参数 {key} 必须为布尔值"),
            })
        } else {
            json!(value)
        };
        options.insert(key.into(), value);
    }
    Ok(Value::Object(options))
}

fn sni_field(proxy: &Value) -> &'static str {
    match proxy["type"].as_str().unwrap_or_default() {
        "http" | "socks5" | "trojan" | "hysteria" | "hysteria2" | "tuic" | "anytls" => "sni",
        _ => "servername",
    }
}

fn apply_singbox_tls(proxy: &mut Value, tls: &Value, name: &str) -> Result<()> {
    if let Some(fields) = tls.as_object() {
        for key in fields.keys() {
            if !matches!(key.as_str(), "enabled" | "server_name" | "insecure" | "alpn" | "utls" | "reality") {
                bail!("节点 {name} 的 sing-box TLS 字段 {key} 没有 Mihomo 映射");
            }
        }
    }
    if tls.get("enabled").and_then(Value::as_bool) == Some(false) {
        proxy["tls"] = json!(false);
        return Ok(());
    }
    proxy["tls"] = json!(true);
    if let Some(servername) = tls.get("server_name") {
        let key = sni_field(proxy);
        proxy[key] = servername.clone();
    }
    if let Some(insecure) = tls.get("insecure") {
        proxy["skip-cert-verify"] = insecure.clone();
    }
    if let Some(alpn) = tls.get("alpn") {
        proxy["alpn"] = alpn.clone();
    }
    if let Some(fingerprint) = tls
        .get("utls")
        .and_then(|v| v.get("fingerprint"))
    {
        proxy["client-fingerprint"] = fingerprint.clone();
    }
    if let Some(reality) = tls.get("reality").filter(|v| v.get("enabled").and_then(Value::as_bool) == Some(true)) {
        let mut opts = Map::new();
        if let Some(key) = reality.get("public_key") {
            opts.insert("public-key".into(), key.clone());
        }
        if let Some(id) = reality.get("short_id") {
            opts.insert("short-id".into(), id.clone());
        }
        if opts.is_empty() {
            bail!("节点 {name} 的 Reality 配置缺少 public-key 和 short-id");
        }
        proxy["reality-opts"] = Value::Object(opts);
    }
    Ok(())
}

fn apply_singbox_transport(proxy: &mut Value, transport: &Value, name: &str) -> Result<()> {
    match transport.get("type").and_then(Value::as_str).unwrap_or("tcp") {
        "tcp" => Ok(()),
        "ws" => {
            proxy["network"] = json!("ws");
            let mut options = Map::new();
            if let Some(path) = transport.get("path") {
                options.insert("path".into(), path.clone());
            }
            if let Some(headers) = transport.get("headers") {
                options.insert("headers".into(), headers.clone());
            }
            for (source, target) in [
                ("max_early_data", "max-early-data"),
                ("early_data_header_name", "early-data-header-name"),
            ] {
                if let Some(value) = transport.get(source) {
                    options.insert(target.into(), value.clone());
                }
            }
            proxy["ws-opts"] = Value::Object(options);
            Ok(())
        }
        "grpc" => {
            proxy["network"] = json!("grpc");
            if let Some(service) = transport.get("service_name") {
                proxy["grpc-opts"] = json!({ "grpc-service-name": service });
            }
            if let Some(service) = transport.get("authority") {
                proxy["grpc-opts"]["grpc-authority"] = service.clone();
            }
            Ok(())
        }
        other => bail!("节点 {name} 的 sing-box transport {other} 没有 Mihomo 映射"),
    }
}

fn structured_node_to_proxy(node: &Node) -> Result<Value> {
    let kind = normalize_kind(&node.kind)?;
    let mut proxy = json!({
        "name": node.id,
        "type": kind,
        "server": node.server,
        "port": node.port,
    });
    match kind.as_str() {
        "ss" => {
            proxy["cipher"] = json!(required(node, "cipher", node.cipher.as_deref())?);
            proxy["password"] = json!(required(node, "password", node.password.as_deref())?);
        }
        "socks5" | "http" => {
            insert_option(&mut proxy, "username", node.username.as_deref());
            insert_option(&mut proxy, "password", node.password.as_deref());
            apply_node_tls(&mut proxy, node, false);
        }
        "vmess" => {
            proxy["uuid"] = json!(required(node, "uuid", node.uuid.as_deref())?);
            proxy["cipher"] = json!(node.security.as_deref().unwrap_or("auto"));
            proxy["alterId"] = json!(0);
            apply_node_tls(&mut proxy, node, false);
            apply_node_transport(&mut proxy, node)?;
        }
        "vless" => {
            proxy["uuid"] = json!(required(node, "uuid", node.uuid.as_deref())?);
            insert_option(&mut proxy, "flow", node.flow.as_deref());
            apply_node_tls(&mut proxy, node, true);
            apply_node_transport(&mut proxy, node)?;
        }
        "trojan" => {
            proxy["password"] = json!(required(node, "password", node.password.as_deref())?);
            apply_node_tls(&mut proxy, node, true);
            apply_node_transport(&mut proxy, node)?;
        }
        "hysteria2" | "anytls" => {
            proxy["password"] = json!(required(node, "password", node.password.as_deref())?);
            insert_option(&mut proxy, "sni", node.sni.as_deref());
            proxy["skip-cert-verify"] = json!(node.skip_cert_verify.unwrap_or(true));
            if let Some(alpn) = &node.alpn {
                proxy["alpn"] = json!(alpn);
            }
            if kind == "anytls" {
                if node.reality.unwrap_or(false) || node.reality_public_key.is_some() {
                    bail!("Mihomo 不支持 AnyTLS + Reality: {}", node.name);
                }
                insert_option(&mut proxy, "client-fingerprint", node.fingerprint.as_deref());
            }
        }
        "tuic" => {
            proxy["uuid"] = json!(required(node, "uuid", node.uuid.as_deref())?);
            proxy["password"] = json!(required(node, "password", node.password.as_deref())?);
            insert_option(&mut proxy, "sni", node.sni.as_deref());
            if let Some(alpn) = &node.alpn {
                proxy["alpn"] = json!(alpn);
            }
        }
        "hysteria" | "wireguard" | "ssh" | "snell" | "ssr" => {
            bail!("节点 {} 缺少 Mihomo 所需的原始协议字段，请重新导入订阅", node.name)
        }
        other => bail!("Mihomo 不支持节点协议 {other}（{}）", node.name),
    }
    Ok(proxy)
}

fn apply_node_tls(proxy: &mut Value, node: &Node, default_enabled: bool) {
    let reality = node.reality.unwrap_or(false) || node.reality_public_key.is_some();
    let enabled = node.tls.unwrap_or(default_enabled) || reality;
    if !enabled {
        return;
    }
    proxy["tls"] = json!(true);
    let key = sni_field(proxy);
    insert_option(proxy, key, node.sni.as_deref());
    if let Some(true) = node.skip_cert_verify {
        proxy["skip-cert-verify"] = json!(true);
    }
    if let Some(fingerprint) = node.fingerprint.as_deref().filter(|s| !s.is_empty()) {
        proxy["client-fingerprint"] = json!(fingerprint);
    } else if reality {
        proxy["client-fingerprint"] = json!("chrome");
    }
    if reality {
        let mut options = Map::new();
        if let Some(key) = node.reality_public_key.as_deref() {
            options.insert("public-key".into(), json!(key));
        }
        if let Some(id) = node.reality_short_id.as_deref() {
            options.insert("short-id".into(), json!(id));
        }
        proxy["reality-opts"] = Value::Object(options);
    }
    if let Some(alpn) = &node.alpn {
        proxy["alpn"] = json!(alpn);
    }
}

fn apply_node_transport(proxy: &mut Value, node: &Node) -> Result<()> {
    match node.network.as_deref().unwrap_or("tcp") {
        "tcp" | "" => Ok(()),
        "grpc" => {
            proxy["network"] = json!("grpc");
            proxy["grpc-opts"] = json!({ "grpc-service-name": node.grpc_service_name.as_deref().unwrap_or_default() });
            Ok(())
        }
        "ws" => {
            proxy["network"] = json!("ws");
            let path = node
                .ws
                .as_ref()
                .and_then(|options| options.path.as_deref())
                .or(node.path.as_deref());
            let host = node
                .ws
                .as_ref()
                .and_then(|options| options.host.as_deref())
                .or(node.host.as_deref());
            let mut options = Map::new();
            if let Some(path) = path.filter(|value| !value.is_empty()) {
                options.insert("path".into(), json!(path));
            }
            if let Some(host) = host.filter(|value| !value.is_empty()) {
                options.insert("headers".into(), json!({ "Host": host }));
            }
            proxy["ws-opts"] = Value::Object(options);
            Ok(())
        }
        other => bail!("节点 {} 的传输类型 {other} 没有 Mihomo 映射", node.name),
    }
}

fn insert_option(proxy: &mut Value, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        proxy[key] = json!(value);
    }
}

fn required<'a>(node: &Node, field: &str, value: Option<&'a str>) -> Result<&'a str> {
    value
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("节点 {} 缺少 Mihomo 必需字段 {field}，请重新导入订阅", node.name))
}

fn normalize_kind(kind: &str) -> Result<String> {
    let kind = match kind.to_ascii_lowercase().as_str() {
        "shadowsocks" | "ss" => "ss",
        "hy2" => "hysteria2",
        "socks" | "socks5" => "socks5",
        "vmess" | "vless" | "trojan" | "http" | "hysteria" | "hysteria2" | "tuic"
        | "wireguard" | "ssh" | "anytls" | "snell" | "ssr" => kind,
        other => bail!("Mihomo 不支持节点协议 {other}"),
    };
    Ok(kind.to_ascii_lowercase())
}

fn render_rule(rule: &RouteRule, profile: &AppProfile) -> Result<Vec<Value>> {
    let (kind, payload) = normalize_rule_payload(&rule.r#type, &rule.payload)?;
    let (kind, payload) = migrate_rule_set_tag(&kind, &payload).unwrap_or((kind, payload));
    let payload = normalize_rule_payload_domain(&kind, &payload)?;
    let payload = match kind.as_str() {
        "ip" | "ip_cidr" => normalize_ip_cidr(&payload)
            .ok_or_else(|| anyhow!("规则 {} 的 IP/CIDR 无效: {payload}", rule.name))?,
        "port" => normalize_ports(&payload)
            .ok_or_else(|| anyhow!("规则 {} 的端口无效: {payload}", rule.name))?,
        _ => payload,
    };
    let target = match rule.outbound.trim().to_ascii_lowercase().as_str() {
        "direct" => "DIRECT".to_string(),
        "block" | "reject" => "REJECT".to_string(),
        "proxy" | "yap-proxy" => NODE_GROUP.to_string(),
        id if profile.nodes.iter().any(|node| node.id == id) => reference_group(id),
        other => bail!("规则 {} 指向不存在的节点或出站: {other}", rule.name),
    };
    let kind = match kind.as_str() {
        "domain" => "DOMAIN",
        "domain_suffix" => "DOMAIN-SUFFIX",
        "domain_keyword" => "DOMAIN-KEYWORD",
        "domain_regex" => "DOMAIN-REGEX",
        "ip_cidr" | "ip" => "IP-CIDR",
        "port" => "DST-PORT",
        "process_name" => "PROCESS-NAME",
        "protocol" if payload.eq_ignore_ascii_case("tcp") => "NETWORK-TCP",
        "protocol" if payload.eq_ignore_ascii_case("udp") => "NETWORK-UDP",
        "protocol" => bail!("Mihomo 仅支持 TCP/UDP 网络规则，不能转换 protocol:{payload}"),
        "geosite" => "GEOSITE",
        "geoip" => "GEOIP",
        "rule_set" => bail!("规则 {} 使用了没有 Mihomo 兼容来源的 rule_set: {payload}", rule.name),
        other => bail!("规则 {} 使用了不支持的类型: {other}", rule.name),
    };
    let text = if kind == "NETWORK-TCP" {
        format!("NETWORK,TCP,{target}")
    } else if kind == "NETWORK-UDP" {
        format!("NETWORK,UDP,{target}")
    } else {
        format!("{kind},{payload},{target}")
    };
    if kind == "DST-PORT" {
        Ok(payload.split(',').map(|port| json!(format!("DST-PORT,{port},{target}"))).collect())
    } else {
        Ok(vec![json!(text)])
    }
}

fn migrate_rule_set_tag(kind: &str, payload: &str) -> Option<(String, String)> {
    if kind != "rule_set" {
        return None;
    }
    for (prefix, migrated_kind) in [("geosite-", "geosite"), ("geoip-", "geoip")] {
        if let Some(category) = payload.strip_prefix(prefix) {
            let category = if category.eq_ignore_ascii_case("cn") {
                "CN"
            } else {
                category
            };
            return Some((migrated_kind.into(), category.into()));
        }
    }
    None
}

fn normalize_rule_payload(kind: &str, payload: &str) -> Result<(String, String)> {
    let kind = kind.trim().to_ascii_lowercase();
    let payload = payload.trim();
    if let Some((prefix, value)) = payload.split_once(':') {
        let prefix = prefix.trim().to_ascii_lowercase();
        if matches!(prefix.as_str(), "geosite" | "geoip" | "domain" | "domain_suffix" | "domain_keyword" | "domain_regex" | "ip_cidr" | "process_name" | "port" | "protocol" | "rule_set") {
            return Ok((prefix, value.trim().to_string()));
        }
    }
    Ok((kind, payload.to_string()))
}

pub fn is_rule_set_rule(rule: &RouteRule) -> bool {
    let (kind, _) = normalize_rule_payload(&rule.r#type, &rule.payload)
        .unwrap_or_else(|_| (rule.r#type.to_ascii_lowercase(), rule.payload.clone()));
    matches!(kind.as_str(), "geosite" | "geoip" | "rule_set")
}

fn normalize_stack(stack: &str) -> Result<&'static str> {
    match stack.trim().to_ascii_lowercase().as_str() {
        "gvisor" => Ok("gvisor"),
        "system" => Ok("system"),
        "mixed" => Ok("mixed"),
        other => bail!("不支持的 Mihomo TUN stack: {other}"),
    }
}

fn normalize_mtu(mtu: u32) -> u32 {
    if (1280..=65535).contains(&mtu) { mtu } else { 1500 }
}

fn normalize_ports(raw: &str) -> Option<String> {
    let values: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect();
    if values.is_empty()
        || values.iter().any(|value| {
            value
                .split_once('-')
                .map(|(start, end)| {
                    start.parse::<u16>().is_err()
                        || end.parse::<u16>().is_err()
                        || start.parse::<u16>().unwrap() > end.parse::<u16>().unwrap()
                })
                .unwrap_or_else(|| value.parse::<u16>().is_err())
        })
    {
        return None;
    }
    Some(values.join(","))
}

pub fn normalize_ip_cidr(raw: &str) -> Option<String> {
    use std::net::{IpAddr, SocketAddr};
    let value = raw.trim();
    if let Some((addr, prefix)) = value.split_once('/') {
        let ip: IpAddr = addr.trim().parse().ok()?;
        let prefix: u8 = prefix.trim().parse().ok()?;
        let max = if ip.is_ipv4() { 32 } else { 128 };
        return (prefix <= max).then(|| format!("{ip}/{prefix}"));
    }
    let ip: IpAddr = value
        .parse()
        .ok()
        .or_else(|| value.parse::<SocketAddr>().ok().map(|address| address.ip()))
        .or_else(|| value.strip_prefix('[')?.strip_suffix(']')?.parse().ok())?;
    Some(format!("{ip}/{}", if ip.is_ipv4() { 32 } else { 128 }))
}

fn normalize_host(raw: &str) -> Result<String> {
    let value = raw.trim();
    let url = if value.contains("://") {
        reqwest::Url::parse(value)
    } else {
        reqwest::Url::parse(&format!("http://{value}"))
    }
    .map_err(|_| anyhow!("无效的域名规则: {raw}"))?;
    let host = url.host_str().unwrap_or_default().trim_matches('.');
    if host.is_empty() {
        bail!("无效的域名规则: {raw}");
    }
    Ok(host.to_ascii_lowercase())
}

fn normalize_rule_payload_domain(kind: &str, payload: &str) -> Result<String> {
    if matches!(kind, "domain" | "domain_suffix" | "domain_keyword") {
        normalize_host(payload)
    } else {
        Ok(payload.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str) -> Node {
        Node {
            id: id.into(),
            name: id.into(),
            kind: "shadowsocks".into(),
            server: "127.0.0.1".into(),
            port: 1080,
            cipher: Some("aes-256-gcm".into()),
            password: Some("pw".into()),
            ..Default::default()
        }
    }

    #[test]
    fn tun_dns_mode_controls_hijack_and_strict_route_together() {
        let profile = AppProfile::default();
        let mut cfg = AppConfig { enable_tun: true, ..Default::default() };
        let native = render(&profile, &cfg).unwrap();
        assert_eq!(native["tun"]["dns-mode"], "native");
        assert_eq!(native["tun"]["strict-route"], false);
        assert!(native["tun"].get("dns-hijack").is_none());

        cfg.tun_dns_mode = "hijack".into();
        let hijack = render(&profile, &cfg).unwrap();
        assert_eq!(hijack["tun"]["dns-mode"], "hijack");
        assert_eq!(hijack["tun"]["strict-route"], true);
        assert_eq!(hijack["tun"]["dns-hijack"][0], "any:53");
    }

    #[test]
    fn provider_preserves_internal_node_ids_and_tls_fields() {
        let mut proxy = node("node-1");
        proxy.kind = "vless".into();
        proxy.uuid = Some("uuid".into());
        proxy.tls = Some(true);
        proxy.sni = Some("example.com".into());
        proxy.network = Some("ws".into());
        proxy.ws = Some(crate::config::model::WsOptions {
            path: Some("/ws".into()),
            host: Some("cdn.example.com".into()),
            ..Default::default()
        });
        let profile = AppProfile { nodes: vec![proxy], ..Default::default() };
        let provider = render_provider(&profile).unwrap();
        assert_eq!(provider["proxies"][0]["name"], "node-1");
        assert_eq!(provider["proxies"][0]["network"], "ws");
        assert_eq!(provider["proxies"][0]["ws-opts"]["path"], "/ws");
    }

    #[test]
    fn converted_nodes_keep_udp_and_native_nodes_keep_their_settings() {
        let profile = AppProfile::default();
        for kind in ["shadowsocks", "socks", "vmess", "vless", "trojan", "anytls"] {
            let mut node = node("udp-node");
            node.kind = kind.into();
            node.uuid = Some("00000000-0000-0000-0000-000000000001".into());
            let structured = node_to_proxy(&node, &profile).unwrap();
            assert_eq!(structured["udp"], true, "{kind}");

            node.extra = Some(json!({"type": kind, "server_port": 1080}));
            assert_eq!(node_to_proxy(&node, &profile).unwrap()["udp"], true, "{kind}");

            for udp in [None, Some(false), Some(true)] {
                let mut native = structured.clone();
                native.as_object_mut().unwrap().remove("udp");
                if let Some(udp) = udp {
                    native["udp"] = json!(udp);
                }
                node.extra = Some(native);
                assert_eq!(node_to_proxy(&node, &profile).unwrap().get("udp").and_then(Value::as_bool), udp, "{kind}");
            }
        }
        let mut nodes = crate::subscription::parser::parse("ss://aes-128-gcm:secret@127.0.0.1:8388#ss").unwrap();
        nodes[0].id = "ss-link".into();
        assert_eq!(render_provider(&AppProfile { nodes, ..Default::default() }).unwrap()["proxies"][0]["udp"], true);
    }

    #[test]
    fn empty_profile_does_not_register_an_empty_provider() {
        let config = render(&AppProfile::default(), &AppConfig::default()).unwrap();
        assert!(config["proxy-providers"].as_object().unwrap().is_empty());
        assert!(config["proxy-groups"][0].get("use").is_none());
        assert_eq!(config["proxy-groups"][0]["proxies"], json!(["DIRECT"]));
    }

    #[test]
    fn rules_map_to_mihomo_proxy_names() {
        let profile = AppProfile {
            nodes: vec![node("node-1")],
            rules: vec![RouteRule {
                id: "rule-1".into(),
                name: "local domain".into(),
                r#type: "domain_suffix".into(),
                payload: "Example.COM".into(),
                outbound: "proxy".into(),
            }],
            mode: ProxyMode::Rule,
            ..Default::default()
        };
        let cfg = render(&profile, &AppConfig::default()).unwrap();
        assert_eq!(cfg["rules"][0], "DOMAIN-SUFFIX,example.com,YAP-PROXY");
        assert_eq!(cfg["rules"][1], "GEOIP,CN,DIRECT");
        assert_eq!(cfg["rules"][2], "GEOSITE,CN,DIRECT");
        assert_eq!(cfg["rules"][3], "MATCH,YAP-PROXY");
    }

    #[test]
    fn domain_rules_extract_hosts_from_urls() {
        for payload in [
            "https://Example.COM:8443/path?query=1#section",
            "https://example.com?query=1",
            "https://example.com#section",
            "HTTPS://EXAMPLE.COM:443/",
            "example.com:8443/path",
            ".Example.COM.",
        ] {
            let rule = RouteRule {
                id: "url".into(), name: "url".into(), r#type: "domain_suffix".into(),
                payload: payload.into(), outbound: "block".into(),
            };
            assert_eq!(render_rule(&rule, &AppProfile::default()).unwrap(), vec![json!("DOMAIN-SUFFIX,example.com,REJECT")], "{payload}");
        }
        assert_eq!(normalize_host("google").unwrap(), "google");
        for invalid in ["", "http://", "bad host"] {
            assert!(normalize_host(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn custom_rules_and_provider_references_survive_all_modes() {
        let first = node("n.1");
        let mut second = node("n2");
        second.detour = Some(first.id.clone());
        let mut profile = AppProfile { nodes: vec![first, second], rules: vec![RouteRule {
            id: "ports".into(), name: "ports".into(), r#type: "port".into(),
            payload: "80,443,8000-8010".into(), outbound: "n.1".into(),
        }], ..Default::default() };
        for mode in [ProxyMode::Global, ProxyMode::Direct, ProxyMode::Rule] {
            profile.mode = mode;
            let cfg = render(&profile, &AppConfig::default()).unwrap();
            assert_eq!(cfg["mode"], "rule");
            assert_eq!(cfg["rules"][0], "DST-PORT,80,YAP-NODE-n.1");
            assert_eq!(cfg["rules"][1], "DST-PORT,443,YAP-NODE-n.1");
            assert_eq!(cfg["rules"][2], "DST-PORT,8000-8010,YAP-NODE-n.1");
            assert_eq!(cfg["proxy-groups"][1]["filter"], "^n\\.1$");
            assert_eq!(cfg["rules"].as_array().unwrap().last().unwrap(),
                if mode == ProxyMode::Direct { "MATCH,DIRECT" } else { "MATCH,YAP-PROXY" });
        }
        assert_eq!(render_provider(&profile).unwrap()["proxies"][1]["dialer-proxy"], "YAP-NODE-n.1");
    }

    #[test]
    fn imported_detours_resolve_within_the_subscription_and_reject_cycles() {
        for (port_key, detour_key) in [("port", "dialer-proxy"), ("server_port", "detour")] {
            let mut front = node("front-id");
            front.name = "front".into();
            front.subscription_id = Some("sub".into());
            let mut duplicate = front.clone();
            duplicate.id = "other-id".into();
            duplicate.subscription_id = Some("other-sub".into());
            let mut exit = node("exit-id");
            exit.name = "exit".into();
            exit.subscription_id = Some("sub".into());
            exit.extra = Some(json!({"type":"http", port_key:8080, detour_key:"front"}));
            let mut profile = AppProfile { nodes: vec![duplicate, front, exit], ..Default::default() };

            assert_eq!(node_to_proxy(&profile.nodes[2], &profile).unwrap()["dialer-proxy"], "front-id");
            let expanded = expand_with_detours(&profile.nodes, &profile.nodes[2..]);
            assert_eq!(expanded.iter().map(|node| node.id.as_str()).collect::<Vec<_>>(), ["exit-id", "front-id"]);
            assert!(detour_is_safe(&profile.nodes, "exit-id", "front-id"));
            assert!(!detour_is_safe(&profile.nodes, "front-id", "exit-id"));

            profile.nodes[1].extra = Some(json!({"type":"http", port_key:8080, detour_key:"exit"}));
            assert!(render_provider(&profile).unwrap_err().to_string().contains("环路"));

            profile.nodes[1].extra = None;
            profile.nodes[2].detour = Some(String::new());
            assert!(node_to_proxy(&profile.nodes[2], &profile).unwrap().get("dialer-proxy").is_none());
            assert_eq!(expand_with_detours(&profile.nodes, &profile.nodes[2..]).len(), 1);

            profile.nodes[2].detour = None;
            profile.nodes.remove(1);
            assert!(render_provider(&profile).unwrap_err().to_string().contains("不存在"));
            profile.nodes[1].detour = Some("other-id".into());
            assert_eq!(node_to_proxy(&profile.nodes[1], &profile).unwrap()["dialer-proxy"], "other-id");
        }
    }

    #[test]
    fn singbox_plugins_become_typed_mihomo_options() {
        assert_eq!(plugin_options("v2ray-plugin", &json!("mode=websocket;host=cdn.example;path=/ws;tls;mux=0"), "ss").unwrap(),
            json!({"mode":"websocket","host":"cdn.example","path":"/ws","tls":true,"mux":false}));
        assert_eq!(plugin_options("obfs-local", &json!("obfs=http;obfs-host=example.com"), "ss").unwrap(),
            json!({"mode":"http","host":"example.com"}));
        assert!(plugin_options("unsupported", &json!("mode=test"), "ss").is_err());
        assert!(plugin_options("v2ray-plugin", &json!("unknown=value"), "ss").is_err());
    }

    #[test]
    fn tls_server_name_uses_each_protocols_field() {
        for kind in ["trojan", "http", "anytls", "vmess", "vless"] {
            let mut node = node(kind);
            node.kind = kind.into();
            node.uuid = Some("uuid".into());
            node.tls = Some(true);
            node.sni = Some("tls.example".into());
            let profile = AppProfile { nodes: vec![node.clone()], ..Default::default() };
            let key = if matches!(kind, "vmess" | "vless") { "servername" } else { "sni" };
            assert_eq!(node_to_proxy(&node, &profile).unwrap()[key], "tls.example", "{kind}");
            node.extra = Some(json!({"type":kind,"server_port":443,"tls":{"enabled":true,"server_name":"tls.example"}}));
            assert_eq!(node_to_proxy(&node, &profile).unwrap()[key], "tls.example", "{kind}");
        }
    }

    #[test]
    fn known_legacy_rule_set_tags_map_to_mihomo_geodata() {
        let profile = AppProfile {
            rules: vec![RouteRule {
                id: "legacy-geosite".into(),
                name: "legacy geosite".into(),
                r#type: "rule_set".into(),
                payload: "geosite-cn".into(),
                outbound: "direct".into(),
            }],
            ..Default::default()
        };
        let rendered = render(&profile, &AppConfig::default()).unwrap();
        assert_eq!(rendered["rules"][0], "GEOSITE,CN,DIRECT");
    }

    #[test]
    fn unsupported_singbox_fields_are_rejected() {
        let mut node = node("unknown-field");
        node.extra = Some(json!({
            "type": "shadowsocks",
            "server": node.server.clone(),
            "server_port": node.port,
            "method": "aes-256-gcm",
            "password": "secret",
            "transport": { "type": "quic" }
        }));
        let profile = AppProfile {
            nodes: vec![node.clone()],
            ..Default::default()
        };
        let error = render_provider(&profile).unwrap_err().to_string();
        assert!(error.contains("transport quic"), "{error}");
    }

    #[test]
    #[ignore = "requires MIHOMO_TEST_BINARY pointing to a Mihomo executable"]
    fn mihomo_accepts_generated_yaml_for_both_dns_modes() {
        let binary = std::env::var_os("MIHOMO_TEST_BINARY").expect("MIHOMO_TEST_BINARY");
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("mihomo-config-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("providers")).unwrap();
        let profile = AppProfile {
            nodes: vec![node("node-1")],
            selected_node: Some("node-1".into()),
            ..Default::default()
        };
        std::fs::write(
            dir.join(NODE_PROVIDER_PATH),
            serde_yaml::to_string(&render_provider(&profile).unwrap()).unwrap(),
        )
        .unwrap();
        let mut cfg = AppConfig {
            enable_tun: true,
            ..Default::default()
        };
        for mode in ["native", "hijack"] {
            cfg.tun_dns_mode = mode.into();
            let path = dir.join(format!("{mode}.yaml"));
            std::fs::write(&path, serde_yaml::to_string(&render(&profile, &cfg).unwrap()).unwrap())
                .unwrap();
            let output = std::process::Command::new(&binary)
                .arg("-t")
                .arg("-d")
                .arg(&dir)
                .arg("-f")
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "Mihomo rejected {mode} configuration: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
