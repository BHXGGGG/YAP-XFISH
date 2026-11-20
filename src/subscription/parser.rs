use crate::config::model::{Node, WsOptions};
use crate::error::{AppError, AppResult};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde_json::Value;

/// 将订阅内容解析为内部节点列表。支持四种形态（按优先级自上而下尝试）：
///
/// 1. sing-box 配置 JSON（含 `outbounds` 数组）
/// 2. Clash 订阅 YAML（含 `proxies` 列表）
/// 3. v2rayN 风格：base64 编码后的链接集合（每行一个 `scheme://...`）
/// 4. 直接是 `scheme://...` 链接集合（每行一个）
///
/// 无法识别的节点或参数会拒绝整次导入，保留已有订阅。
pub fn parse(content: &str) -> AppResult<Vec<Node>> {
    parse_inner(content, 0)
}

fn parse_inner(content: &str, depth: usize) -> AppResult<Vec<Node>> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Err(AppError(anyhow::anyhow!("订阅内容为空")));
    }

    // 1) JSON（sing-box 配置 或 clash 风格节点数组）
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        let value = serde_json::from_str::<Value>(trimmed)?;
        return parse_json_value(&value);
    }

    // 2) Clash YAML
    if trimmed.starts_with("proxies:") || trimmed.contains("\nproxies:") {
        let yaml = serde_yaml::from_str::<Value>(trimmed).map_err(anyhow::Error::from)?;
        return parse_json_value(&yaml);
    }

    // 3) base64 包裹的链接 / 配置（仅第一层尝试，防止无限递归）
    if depth < 1 {
        if let Some(decoded) = try_base64_decode(trimmed) {
            return parse_inner(&decoded, depth + 1);
        }
    }

    // 4) 直接按 scheme:// 链接逐行解析
    let mut nodes = Vec::new();
    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        nodes.push(parse_link(line)?);
    }
    if nodes.is_empty() {
        return Err(AppError(anyhow::anyhow!(
            "无法从订阅内容识别任何节点（需为 sing-box JSON / Clash YAML / v2rayN 链接）"
        )));
    }
    Ok(nodes)
}

fn parse_json_value(v: &Value) -> AppResult<Vec<Node>> {
    if let Some(obj) = v.as_object() {
        if let Some(outbounds) = obj.get("outbounds").and_then(|o| o.as_array()) {
            return outbounds.iter().filter(|item| !matches!(item["type"].as_str(), Some("direct" | "block" | "dns" | "selector" | "urltest")))
                .enumerate().map(|(index, item)| require_node(parse_singbox_outbound(item), item, index)).collect();
        }
        if let Some(proxies) = obj.get("proxies").and_then(|p| p.as_array()) {
            return proxies.iter().enumerate().map(|(index, item)| require_node(parse_clash_proxy(item), item, index)).collect();
        }
        return Err(anyhow::anyhow!("订阅 JSON/YAML 缺少 outbounds 或 proxies 列表").into());
    }
    if let Some(arr) = v.as_array() {
        let mut nodes = Vec::new();
        for (index, item) in arr.iter().enumerate() {
            if let Some(_o) = item.as_object() {
                nodes.push(require_node(parse_clash_proxy(item), item, index)?);
            } else if let Some(s) = item.as_str() {
                nodes.push(parse_link(s)?);
            } else {
                return Err(anyhow::anyhow!("第 {} 个订阅节点不是对象或链接", index + 1).into());
            }
        }
        return Ok(nodes);
    }
    Err(anyhow::anyhow!("订阅内容不是节点列表").into())
}

fn require_node(node: Option<Node>, raw: &Value, index: usize) -> AppResult<Node> {
    node.ok_or_else(|| anyhow::anyhow!("第 {} 个节点（{}）缺少有效的协议、服务器或端口", index + 1,
        raw.get("name").or_else(|| raw.get("tag")).and_then(Value::as_str).unwrap_or("未命名")).into())
}

// ---------------- sing-box outbound ----------------

fn parse_singbox_outbound(v: &Value) -> Option<Node> {
    let type_ = v.get("type")?.as_str()?;
    let name = v
        .get("tag")
        .and_then(|t| t.as_str())
        .unwrap_or("node")
        .to_string();
    let server = v.get("server")?.as_str()?.to_string();
    let port = u16::try_from(v.get("server_port")?.as_u64()?).ok().filter(|port| *port > 0)?;

    let mut node = Node {
        id: String::new(),
        name,
        kind: type_.to_string(),
        server,
        port,
        ..Default::default()
    };

    match type_ {
        "shadowsocks" => {
            node.cipher = v.get("method").and_then(|x| x.as_str()).map(|s| s.to_string());
            node.password = v.get("password").and_then(|x| x.as_str()).map(|s| s.to_string());
            maybe_singbox_ws(&mut node, v);
        }
        "vmess" => {
            node.uuid = v.get("uuid").and_then(|x| x.as_str()).map(|s| s.to_string());
            node.security = v.get("security").and_then(|x| x.as_str()).map(|s| s.to_string());
            maybe_singbox_ws(&mut node, v);
        }
        "vless" => {
            node.uuid = v.get("uuid").and_then(|x| x.as_str()).map(|s| s.to_string());
            node.flow = v.get("flow").and_then(|x| x.as_str()).map(|s| s.to_string());
            if let Some(tls) = v.get("tls") {
                node.tls = tls.get("enabled").and_then(|e| e.as_bool());
                node.sni = tls.get("server_name").and_then(|s| s.as_str()).map(|s| s.to_string());
            }
            maybe_singbox_ws(&mut node, v);
        }
        "trojan" => {
            node.password = v.get("password").and_then(|x| x.as_str()).map(|s| s.to_string());
            if let Some(tls) = v.get("tls") {
                node.sni = tls.get("server_name").and_then(|s| s.as_str()).map(|s| s.to_string());
            }
            maybe_singbox_ws(&mut node, v);
        }
        "http" => {
            node.username = v.get("username").and_then(|x| x.as_str()).map(|s| s.to_string());
            node.password = v.get("password").and_then(|x| x.as_str()).map(|s| s.to_string());
            if let Some(tls) = v.get("tls") {
                node.tls = tls.get("enabled").and_then(|x| x.as_bool());
                node.sni = tls.get("server_name").and_then(|x| x.as_str()).map(|s| s.to_string());
                node.skip_cert_verify = tls.get("insecure").and_then(|x| x.as_bool());
            }
        }
        "hysteria2" | "hysteria" => {
            node.password = v.get("password").and_then(|x| x.as_str()).map(|s| s.to_string());
            node.sni = v.get("server").and_then(|x| x.as_str()).map(|s| s.to_string());
        }
        _ => {}
    }
    // 保留完整原始 JSON，渲染时通过 extra 透传，避免丢字段
    node.extra = Some(v.clone());
    Some(node)
}

fn maybe_singbox_ws(node: &mut Node, v: &Value) {
    if let Some(transport) = v.get("transport") {
        let t = transport.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if t == "ws" {
            node.network = Some("ws".into());
            let path = transport.get("path").and_then(|p| p.as_str()).map(|s| s.to_string());
            let host = transport
                .get("headers")
                .and_then(|h| h.get("Host"))
                .and_then(|h| h.as_str())
                .map(|s| s.to_string());
            node.ws = Some(WsOptions { path, host, headers: None });
        }
    }
}

// ---------------- Clash proxy ----------------

fn parse_clash_proxy(v: &Value) -> Option<Node> {
    let type_ = v.get("type")?.as_str()?;
    let name = v
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("node")
        .to_string();
    let server = v.get("server")?.as_str()?.to_string();
    let port = u16::try_from(v.get("port")?.as_u64()?).ok().filter(|port| *port > 0)?;

    let mut node = Node {
        id: String::new(),
        name,
        kind: clash_type_to_kind(type_),
        server,
        port,
        ..Default::default()
    };

    match type_ {
        "ss" => {
            node.cipher = v.get("cipher").and_then(|x| x.as_str()).map(|s| s.to_string());
            node.password = v.get("password").and_then(|x| x.as_str()).map(|s| s.to_string());
            maybe_clash_ws(&mut node, v);
        }
        "vmess" => {
            node.uuid = v.get("uuid").and_then(|x| x.as_str()).map(|s| s.to_string());
            node.security = v.get("cipher").and_then(|x| x.as_str()).map(|s| s.to_string());
            node.tls = v.get("tls").and_then(|t| t.as_bool());
            node.sni = v
                .get("servername")
                .or_else(|| v.get("sni"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());
            node.network = v.get("network").and_then(|n| n.as_str()).map(|s| s.to_string());
            apply_clash_tls_extras(&mut node, v);
            maybe_clash_ws(&mut node, v);
        }
        "vless" => {
            node.uuid = v.get("uuid").and_then(|x| x.as_str()).map(|s| s.to_string());
            // 空 flow 丢弃：部分订阅写 `flow: ""`
            node.flow = v
                .get("flow")
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());
            node.tls = v.get("tls").and_then(|t| t.as_bool());
            node.sni = v
                .get("servername")
                .or_else(|| v.get("sni"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());
            node.network = v.get("network").and_then(|n| n.as_str()).map(|s| s.to_string());
            apply_clash_tls_extras(&mut node, v);
            maybe_clash_ws(&mut node, v);
        }
        "trojan" => {
            node.password = v.get("password").and_then(|x| x.as_str()).map(|s| s.to_string());
            node.sni = v
                .get("sni")
                .or_else(|| v.get("servername"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());
            node.tls = Some(true);
            node.network = v.get("network").and_then(|n| n.as_str()).map(|s| s.to_string());
            apply_clash_tls_extras(&mut node, v);
            maybe_clash_ws(&mut node, v);
        }
        "http" => {
            node.username = v.get("username").and_then(|x| x.as_str()).map(|s| s.to_string());
            node.password = v.get("password").and_then(|x| x.as_str()).map(|s| s.to_string());
            node.tls = v.get("tls").and_then(|x| x.as_bool());
            node.sni = v
                .get("sni")
                .or_else(|| v.get("servername"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            apply_clash_tls_extras(&mut node, v);
        }
        "hysteria2" => {
            node.password = v.get("password").and_then(|x| x.as_str()).map(|s| s.to_string());
            node.sni = v.get("sni").and_then(|s| s.as_str()).map(|s| s.to_string());
        }
        _ => {}
    }
    // 保留原始 Clash 字段，供 Mihomo Provider 渲染保留协议扩展项。
    node.extra = Some(v.clone());
    Some(node)
}

fn clash_type_to_kind(t: &str) -> String {
    match t {
        "ss" => "shadowsocks".into(),
        "socks5" | "socks" => "socks".into(),
        "http" => "http".into(),
        "vmess" => "vmess".into(),
        "vless" => "vless".into(),
        "trojan" => "trojan".into(),
        // hy2 是 Clash Meta / mihomo 订阅里 hysteria2 的常见短名
        // （同时支持 hysteria / hysteria2 / hy2 三种写法以最大化兼容）。
        "hysteria" | "hysteria2" | "hy2" => "hysteria2".into(),
        "tuic" => "tuic".into(),
        "wireguard" => "wireguard".into(),
        "ssh" => "ssh".into(),
        other => other.to_string(),
    }
}

/// 解析 Clash Meta 的 TLS 扩展字段：
/// - `client-fingerprint` → utls fingerprint
/// - `reality-opts.public-key` / `short-id` → REALITY
/// - `skip-cert-verify` → insecure
///
/// 这些字段对 vision + 伪装 SNI 的 VLESS 节点是必需的；缺失时 sing-box 会把
/// 服务端返回的 HTTP 页面当成 VLESS 协议头，报 `unknown version: 72`（'H'）。
fn apply_clash_tls_extras(node: &mut Node, v: &Value) {
    if let Some(fp) = v
        .get("client-fingerprint")
        .or_else(|| v.get("client_fingerprint"))
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
    {
        node.fingerprint = Some(fp.to_string());
    }
    if let Some(skip) = v
        .get("skip-cert-verify")
        .or_else(|| v.get("skip_cert_verify"))
        .and_then(|x| x.as_bool())
    {
        node.skip_cert_verify = Some(skip);
    }
    // reality-opts: { public-key, short-id }
    if let Some(ro) = v.get("reality-opts").or_else(|| v.get("reality_opts")) {
        let pbk = ro
            .get("public-key")
            .or_else(|| ro.get("public_key"))
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let sid = ro
            .get("short-id")
            .or_else(|| ro.get("short_id"))
            .and_then(|x| x.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        if pbk.is_some() || sid.is_some() {
            node.reality = Some(true);
            node.tls = Some(true);
            node.reality_public_key = pbk;
            node.reality_short_id = sid;
        }
    }
}

fn maybe_clash_ws(node: &mut Node, v: &Value) {
    // 辅助：从 JSON 字段中取非空字符串（空字符串视作 None）。
    // Clash 订阅里 ws-opts.path / ws-opts.headers.Host 经常写空字符串，透传给 sing-box
    // 会导致 transport 写 `path: ""` / `Host: ""`，HTTP upgrade 被服务端拒识（返回 400）。
    fn nonempty_str(s: Option<&str>) -> Option<String> {
        s.filter(|v| !v.is_empty()).map(|s| s.to_string())
    }

    // ws-opts 形式（vmess/vless/trojan）
    if let Some(ws) = v.get("ws-opts") {
        node.network = Some("ws".into());
        let path = nonempty_str(ws.get("path").and_then(|p| p.as_str()));
        let host = nonempty_str(
            ws.get("headers")
                .and_then(|h| h.get("Host"))
                .and_then(|h| h.as_str()),
        );
        node.ws = Some(WsOptions { path, host, headers: None });
        return;
    }
    // plugin 形式（ss 的 v2ray-plugin）
    if let Some(plugin) = v.get("plugin").and_then(|p| p.as_str()) {
        if plugin.contains("v2ray-plugin") || plugin.contains("ws") {
            if let Some(opts) = v.get("plugin-opts") {
                node.network = Some("ws".into());
                let path = nonempty_str(opts.get("path").and_then(|p| p.as_str()));
                let host = nonempty_str(opts.get("host").and_then(|h| h.as_str()));
                node.ws = Some(WsOptions { path, host, headers: None });
            }
        }
    }
}

// ---------------- scheme:// 链接 ----------------

fn parse_link(line: &str) -> AppResult<Node> {
    let (scheme, rest) = line.split_once("://")
        .ok_or_else(|| anyhow::anyhow!("订阅包含无效的节点链接"))?;
    let (_, name) = split_name(rest);
    let parsed = match scheme {
        "ss" => parse_ss(rest),
        "vmess" => parse_vmess(rest),
        "vless" => parse_vless(rest),
        "trojan" => parse_trojan(rest),
        "hysteria2" | "hy2" => parse_hy2(rest),
        "anytls" => parse_anytls(rest),
        "tuic" => parse_tuic(rest),
        _ => return Err(anyhow::anyhow!("节点 {name} 的链接协议 {scheme} 尚不支持，请使用 Mihomo YAML 订阅").into()),
    };
    let mut node = parsed.ok_or_else(|| anyhow::anyhow!("节点 {name} 的 {scheme} 链接格式无效或包含不支持的选项"))?;
    if scheme != "vmess" {
        let (body, _) = split_name(rest);
        let query = body.split_once('?').map(|(_, query)| query).unwrap_or_default();
        apply_query(&mut node, query)?;
    }
    if let Some(network) = node.network.as_deref() {
        if network == "quic" {
            return Err(anyhow::anyhow!("节点 {name} 使用 Mihomo 不支持的 QUIC 传输").into());
        }
        if !matches!(network, "" | "tcp" | "ws" | "grpc") {
            return Err(anyhow::anyhow!("节点 {name} 的传输类型 {network} 尚不支持").into());
        }
    }
    Ok(node)
}

fn parse_ss(s: &str) -> Option<Node> {
    let (body, name) = split_name(s);
    let body = body.split('?').next().unwrap_or(&body);
    let (userinfo, hostport): (String, String) = if let Some(at) = body.find('@') {
        (body[..at].to_string(), body[at + 1..].to_string())
    } else {
        let dec = String::from_utf8(b64_decode(&body)?).ok()?;
        if let Some(at) = dec.find('@') {
            (dec[..at].to_string(), dec[at + 1..].to_string())
        } else {
            return None;
        }
    };
    // userinfo 含 ':' 视为明文 method:password；否则按 base64(method:password) 解码
    let creds = if userinfo.contains(':') {
        urldecode(&userinfo)
    } else {
        String::from_utf8(b64_decode(&userinfo)?).ok()?
    };
    let colon = creds.find(':')?;
    let method = creds[..colon].to_string();
    let password = creds[colon + 1..].to_string();
    let (server, port) = split_host_port(&hostport)?;
    Some(Node {
        id: String::new(),
        name,
        kind: "shadowsocks".into(),
        server,
        port,
        cipher: Some(method),
        password: Some(password),
        ..Default::default()
    })
}

fn parse_vmess(s: &str) -> Option<Node> {
    let dec = b64_decode(s)?;
    let json = String::from_utf8(dec).ok()?;
    let v: Value = serde_json::from_str(&json).ok()?;
    let name = v.get("ps").and_then(|x| x.as_str()).unwrap_or("node").to_string();
    let server = v.get("add").and_then(|x| x.as_str())?.to_string();
    let port: u16 = v
        .get("port")
        .and_then(|x| x.as_str())
        .and_then(|s| s.parse().ok())
        .or_else(|| v.get("port").and_then(|x| x.as_u64()).map(|u| u as u16))?;
    let uuid = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let security = v.get("scy").and_then(|x| x.as_str()).map(|s| s.to_string());
    let net = v.get("net").and_then(|x| x.as_str()).unwrap_or("tcp").to_string();
    let mut node = Node {
        id: String::new(),
        name,
        kind: "vmess".into(),
        server,
        port,
        uuid: Some(uuid),
        security,
        network: Some(net.clone()),
        ..Default::default()
    };
    node.tls = Some(v.get("tls").and_then(|x| x.as_str()).map(|s| s == "tls" || s == "reality").unwrap_or(false));
    node.sni = v.get("sni").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    node.fingerprint = v.get("fp").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    node.alpn = v.get("alpn").and_then(Value::as_str).map(|s| s.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect());
    if net == "grpc" {
        node.grpc_service_name = v.get("path").and_then(Value::as_str).map(str::to_string);
    }
    let aid = v.get("aid").map(Value::to_string).unwrap_or_default().trim_matches('"').to_string();
    let header = v.get("type").and_then(Value::as_str).unwrap_or("none");
    if !matches!(aid.as_str(), "" | "0") || !matches!(header, "" | "none")
        || v.get("tls").and_then(Value::as_str) == Some("reality") {
        return None;
    }
    if net == "ws" {
        // 空值过滤：v2rayN 风格 vmess 链接里 path/host 常为空字符串。
        let path = v.get("path").and_then(|x| x.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
        let host = v.get("host").and_then(|x| x.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string());
        node.ws = Some(WsOptions { path, host, headers: None });
    }
    Some(node)
}

fn parse_vless(s: &str) -> Option<Node> {
    let (auth_host, name) = split_name(s);
    let (uuid_host, _) = match auth_host.split_once('?') {
        Some((a, q)) => (a, q),
        None => (auth_host.as_str(), ""),
    };
    let (uuid, hostport) = uuid_host.split_once('@')?;
    let (server, port) = split_host_port(hostport)?;
    let node = Node {
        id: String::new(),
        name,
        kind: "vless".into(),
        server,
        port,
        uuid: Some(uuid.to_string()),
        tls: Some(false),
        ..Default::default()
    };
    Some(node)
}

fn parse_trojan(s: &str) -> Option<Node> {
    let (auth_host, name) = split_name(s);
    let (pass_host, _) = match auth_host.split_once('?') {
        Some((a, q)) => (a, q),
        None => (auth_host.as_str(), ""),
    };
    let (password, hostport) = pass_host.split_once('@')?;
    let (server, port) = split_host_port(hostport)?;
    let node = Node {
        id: String::new(),
        name,
        kind: "trojan".into(),
        server,
        port,
        password: Some(urldecode(password)),
        tls: Some(true),
        ..Default::default()
    };
    Some(node)
}

fn parse_hy2(s: &str) -> Option<Node> {
    let mut node = parse_trojan(s)?;
    node.kind = "hysteria2".into();
    Some(node)
}

fn parse_anytls(s: &str) -> Option<Node> {
    let mut node = parse_trojan(s)?;
    node.kind = "anytls".into();
    Some(node)
}

fn parse_tuic(s: &str) -> Option<Node> {
    let mut node = parse_trojan(s)?;
    let credentials = node.password.take()?;
    let (uuid, password) = credentials.split_once(':')?;
    node.kind = "tuic".into();
    node.uuid = Some(uuid.to_string());
    node.password = Some(password.to_string());
    Some(node)
}

fn query_bool(value: &str) -> AppResult<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(anyhow::anyhow!("布尔参数必须为 0/1 或 false/true").into()),
    }
}

fn apply_query(node: &mut Node, query: &str) -> AppResult<()> {
    let mut path = None;
    let mut host = None;
    let mut extra = serde_json::Map::new();
    let transport = matches!(node.kind.as_str(), "vless" | "trojan");
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let value = urldecode(value);
        if value.is_empty() { continue; }
        match key {
            "security" if transport => match value.as_str() {
                "tls" => node.tls = Some(true),
                "reality" if node.kind == "vless" => {
                    node.tls = Some(true);
                    node.reality = Some(true);
                }
                "none" if node.kind == "vless" => node.tls = Some(false),
                _ => return Err(anyhow::anyhow!("节点 {} 的 security 参数不受支持: {value}", node.name).into()),
            },
            "type" if transport => node.network = Some(value),
            "path" if transport => path = Some(value),
            "host" if transport => host = Some(value),
            "serviceName" if transport => node.grpc_service_name = Some(value),
            "flow" if node.kind == "vless" => node.flow = Some(value),
            "sni" | "peer" if node.kind != "shadowsocks" => node.sni = Some(value),
            "fp" | "fingerprint" if transport || node.kind == "anytls" => node.fingerprint = Some(value),
            "pbk" | "publicKey" | "public-key" if node.kind == "vless" => {
                node.reality = Some(true);
                node.tls = Some(true);
                node.reality_public_key = Some(value);
            }
            "sid" | "shortId" | "short-id" if node.kind == "vless" => node.reality_short_id = Some(value),
            "insecure" | "allowInsecure" | "allow_insecure" if node.kind != "shadowsocks" => {
                node.skip_cert_verify = Some(query_bool(&value)?);
            }
            "alpn" if node.kind != "shadowsocks" => {
                node.alpn = Some(value.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect());
            }
            "obfs" | "obfs-password" if node.kind == "hysteria2" => {
                extra.insert(key.into(), Value::String(value));
            }
            "congestion_control" | "udp_relay_mode" if node.kind == "tuic" => {
                let target = if key == "congestion_control" { "congestion-controller" } else { "udp-relay-mode" };
                extra.insert(target.into(), Value::String(value));
            }
            "disable_sni" | "reduce_rtt" if node.kind == "tuic" => {
                extra.insert(key.replace('_', "-"), Value::Bool(query_bool(&value)?));
            }
            "pinSHA256" if node.kind == "hysteria2" => {
                extra.insert("fingerprint".into(), Value::String(value));
            }
            "mport" | "ports" if node.kind == "hysteria2" => {
                extra.insert("ports".into(), Value::String(value));
            }
            "mportHopInt" | "portHopInt" | "hop-interval" if node.kind == "hysteria2" => {
                extra.insert("hop-interval".into(), Value::String(value));
            }
            "quicSecurity" | "key" if node.kind == "vless" => {}
            "plugin" if node.kind == "shadowsocks" => {
                let (plugin, opts) = value.split_once(';').unwrap_or((&value, ""));
                node.extra = Some(serde_json::json!({
                    "type": "shadowsocks", "server_port": node.port,
                    "method": node.cipher, "password": node.password,
                    "plugin": plugin, "plugin_opts": opts,
                }));
            }
            "encryption" if node.kind == "vless" && value == "none" => {}
            "headerType" if transport && value == "none" => {}
            _ => return Err(anyhow::anyhow!("节点 {} 的链接参数 {key} 尚无 Mihomo 映射", node.name).into()),
        }
    }
    if path.is_some() || host.is_some() || node.network.as_deref() == Some("ws") {
        if node.network.as_deref().is_some_and(|n| n != "ws") {
            return Err(anyhow::anyhow!("节点 {} 的 path/host 仅支持 WebSocket 传输", node.name).into());
        }
        node.network = Some("ws".into());
        node.ws = Some(WsOptions { path, host, headers: None });
    }
    if node.grpc_service_name.is_some() && node.network.as_deref() != Some("grpc") {
        return Err(anyhow::anyhow!("节点 {} 的 serviceName 需要 gRPC 传输", node.name).into());
    }
    if matches!(node.kind.as_str(), "hysteria2" | "tuic") {
        extra.insert("type".into(), Value::String(node.kind.clone()));
        extra.insert("password".into(), serde_json::json!(node.password));
        if let Some(uuid) = &node.uuid { extra.insert("uuid".into(), serde_json::json!(uuid)); }
        if let Some(sni) = &node.sni { extra.insert("sni".into(), serde_json::json!(sni)); }
        if let Some(alpn) = &node.alpn { extra.insert("alpn".into(), serde_json::json!(alpn)); }
        extra.insert("skip-cert-verify".into(), serde_json::json!(node.skip_cert_verify.unwrap_or(node.kind == "hysteria2")));
        node.extra = Some(Value::Object(extra));
    }
    Ok(())
}

// ---------------- 工具 ----------------

fn split_name(s: &str) -> (String, String) {
    if let Some(idx) = s.rfind('#') {
        let name = urldecode(&s[idx + 1..]);
        (s[..idx].to_string(), name)
    } else {
        (s.to_string(), "node".to_string())
    }
}

fn split_host_port(hp: &str) -> Option<(String, u16)> {
    let hp = hp.split('?').next().unwrap_or(hp);
    let hp = hp.split('/').next().unwrap_or(hp);
    if let Some(colon) = hp.rfind(':') {
        let server = hp[..colon].trim_start_matches('[').trim_end_matches(']').to_string();
        let port: u16 = hp[colon + 1..].parse().ok()?;
        if server.is_empty() || port == 0 { return None; }
        Some((server, port))
    } else {
        None
    }
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if let Ok(b) = STANDARD.decode(s) {
        return Some(b);
    }
    if let Ok(b) = base64::engine::general_purpose::STANDARD_NO_PAD.decode(s) {
        return Some(b);
    }
    if let Ok(b) = base64::engine::general_purpose::URL_SAFE.decode(s) {
        return Some(b);
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s).ok()
}

fn try_base64_decode(s: &str) -> Option<String> {
    let dec = b64_decode(s)?;
    let text = String::from_utf8(dec).ok()?;
    // 仅当解码后确实是链接或配置时才认为这是 base64 包裹
    if text.lines().any(|l| l.contains("://")) || text.contains("proxies:") || text.contains("outbounds") {
        Some(text)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::STANDARD;

    #[test]
    fn parse_ss_link() {
        let n = parse("ss://aes-256-gcm:pass@example.com:8388#test").unwrap();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].kind, "shadowsocks");
        assert_eq!(n[0].server, "example.com");
        assert_eq!(n[0].port, 8388);
        assert_eq!(n[0].cipher.as_deref(), Some("aes-256-gcm"));
        assert_eq!(n[0].password.as_deref(), Some("pass"));
        assert_eq!(n[0].name, "test");
    }

    #[test]
    fn parse_vmess_link() {
        let json = r#"{"v":"2","ps":"vm","add":"1.2.3.4","port":"443","id":"uuid-1","aid":"0","scy":"auto","net":"tcp","tls":""}"#;
        let b64 = STANDARD.encode(json);
        let n = parse(&format!("vmess://{b64}")).unwrap();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].kind, "vmess");
        assert_eq!(n[0].server, "1.2.3.4");
        assert_eq!(n[0].port, 443);
        assert_eq!(n[0].uuid.as_deref(), Some("uuid-1"));
        assert_eq!(n[0].name, "vm");
    }

    #[test]
    fn parse_vless_link_with_ws() {
        let link = "vless://uu-id@host.example:443?type=ws&path=%2Fwspath&host=ws.host#VL";
        let n = parse(link).unwrap();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].kind, "vless");
        assert_eq!(n[0].server, "host.example");
        assert_eq!(n[0].port, 443);
        assert_eq!(n[0].network.as_deref(), Some("ws"));
        assert_eq!(n[0].ws.as_ref().unwrap().path.as_deref(), Some("/wspath"));
        assert_eq!(n[0].ws.as_ref().unwrap().host.as_deref(), Some("ws.host"));
    }

    #[test]
    fn parse_clash_yaml() {
        let yaml = r#"
proxies:
  - name: "c-ss"
    type: ss
    server: 1.1.1.1
    port: 100
    cipher: aes-256-gcm
    password: pw1
  - name: "c-vm"
    type: vmess
    server: 2.2.2.2
    port: 200
    uuid: uu
    cipher: auto
    network: ws
    ws-opts:
      path: /p
      headers:
        Host: h
"#;
        let n = parse(yaml).unwrap();
        assert_eq!(n.len(), 2);
        assert_eq!(n[0].kind, "shadowsocks");
        assert_eq!(n[1].kind, "vmess");
        assert_eq!(n[1].network.as_deref(), Some("ws"));
        assert_eq!(n[1].ws.as_ref().unwrap().path.as_deref(), Some("/p"));
        assert_eq!(n[1].extra.as_ref().unwrap()["ws-opts"]["path"], "/p");
    }

    #[test]
    fn parse_clash_http_proxy_auth_and_tls() {
        let yaml = r#"
proxies:
  - name: "https-http"
    type: http
    server: proxy.example
    port: 8443
    username: user
    password: pass
    tls: true
    sni: proxy.example
    skip-cert-verify: true
"#;
        let nodes = parse(yaml).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].kind, "http");
        assert_eq!(nodes[0].username.as_deref(), Some("user"));
        assert_eq!(nodes[0].password.as_deref(), Some("pass"));
        assert_eq!(nodes[0].tls, Some(true));
        assert_eq!(nodes[0].sni.as_deref(), Some("proxy.example"));
        assert_eq!(nodes[0].skip_cert_verify, Some(true));
    }

    #[test]
    fn parse_singbox_json() {
        let json = r#"{"outbounds":[{"type":"shadowsocks","tag":"sb","server":"5.6.7.8","server_port":9999,"method":"aes-256-gcm","password":"pw"},{"type":"http","tag":"http","server":"proxy.example","server_port":8443,"username":"user","password":"pass","tls":{"enabled":true,"server_name":"proxy.example","insecure":true}}]}"#;
        let n = parse(json).unwrap();
        assert_eq!(n.len(), 2);
        assert_eq!(n[0].kind, "shadowsocks");
        assert_eq!(n[0].server, "5.6.7.8");
        assert_eq!(n[0].port, 9999);
        assert_eq!(n[1].kind, "http");
        assert_eq!(n[1].username.as_deref(), Some("user"));
        assert_eq!(n[1].password.as_deref(), Some("pass"));
        assert_eq!(n[1].tls, Some(true));
    }

    #[test]
    fn parse_v2rayn_base64() {
        let vmess_json = r#"{"v":"2","ps":"v","add":"9.9.9.9","port":"443","id":"u","aid":"0","scy":"auto","net":"tcp","tls":""}"#;
        let vmess_b64 = STANDARD.encode(vmess_json);
        let links = format!("vmess://{}\nss://aes-256-gcm:p@1.1.1.1:80#n", vmess_b64);
        let b64 = STANDARD.encode(links);
        let n = parse(&b64).unwrap();
        assert_eq!(n.len(), 2);
    }

    #[test]
    fn parse_empty_fails() {
        assert!(parse("not-a-subscription").is_err());
    }

    #[test]
    fn mixed_import_rejects_unsupported_or_invalid_nodes() {
        let valid = "ss://aes-256-gcm:p@127.0.0.1:8388#valid";
        let error = parse(&format!("{valid}\nunsupported://p@127.0.0.1:1234#bad-node")).unwrap_err().to_string();
        assert!(error.contains("bad-node") && error.contains("unsupported"), "{error}");
        assert!(parse(r#"{"proxies":[{"name":"bad","type":"ss","server":"localhost","port":65536}]}"#).is_err());
        let error = parse("vless://uuid@host:443?type=xhttp#xhttp-node").unwrap_err().to_string();
        assert!(error.contains("xhttp-node") && error.contains("xhttp"));
        assert!(parse("vless://uuid@host:443?type=grpc&authority=unsupported#grpc").is_err());
        let error = parse("vless://uuid@host:443?type=quic&quicSecurity=none&key=secret#quic-node")
            .unwrap_err()
            .to_string();
        assert!(error.contains("quic-node") && error.contains("Mihomo 不支持"), "{error}");
    }

    #[test]
    fn links_preserve_grpc_quic_and_ss_plugin_options() {
        let links = [
            "vless://uuid@host:443?type=grpc&serviceName=my-service&security=tls&sni=tls.example&insecure=0#grpc",
            "hy2://password@host:443?obfs=salamander&obfs-password=obfs-pass&insecure=0&sni=tls.example&pinSHA256=deadbeef&mport=443,5000-6000&mportHopInt=30#hy2",
            "tuic://uuid:password@host:443?congestion_control=bbr&udp_relay_mode=native&alpn=h3&allow_insecure=0#tuic",
            "ss://aes-256-gcm:pass@host:8388/?plugin=v2ray-plugin%3Bmode%3Dwebsocket%3Bhost%3Dcdn.example%3Btls#ss",
        ].join("\n");
        let mut nodes = parse(&links).unwrap();
        for (index, node) in nodes.iter_mut().enumerate() { node.id = index.to_string(); }
        let profile = crate::config::model::AppProfile { nodes, ..Default::default() };
        let provider = crate::config::mihomo::render_provider(&profile).unwrap();
        let proxies = &provider["proxies"];
        assert_eq!(proxies[0]["network"], "grpc");
        assert_eq!(proxies[0]["grpc-opts"]["grpc-service-name"], "my-service");
        assert_eq!(proxies[1]["obfs-password"], "obfs-pass");
        assert_eq!(proxies[1]["skip-cert-verify"], false);
        assert_eq!(proxies[1]["fingerprint"], "deadbeef");
        assert_eq!(proxies[1]["ports"], "443,5000-6000");
        assert_eq!(proxies[1]["hop-interval"], "30");
        assert_eq!(proxies[2]["uuid"], "uuid");
        assert_eq!(proxies[2]["congestion-controller"], "bbr");
        assert_eq!(proxies[2]["skip-cert-verify"], false);
        assert_eq!(proxies[3]["plugin-opts"]["host"], "cdn.example");
        assert_eq!(proxies[3]["plugin-opts"]["tls"], true);
    }

    #[test]
    fn parse_anytls_link() {
        // 使用示例地址和密码，验证链接参数及名称解码。
        let link = "anytls://example-password@example.com:5001?sni=example.org&insecure=1&fp=chrome&alpn=h2,http/1.1#anytls%20%E9%A6%99%E6%B8%AF%201";
        let n = parse(link).unwrap();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].kind, "anytls");
        assert_eq!(n[0].server, "example.com");
        assert_eq!(n[0].port, 5001);
        assert_eq!(n[0].password.as_deref(), Some("example-password"));
        assert_eq!(n[0].sni.as_deref(), Some("example.org"));
        assert_eq!(n[0].fingerprint.as_deref(), Some("chrome"));
        assert_eq!(n[0].skip_cert_verify, Some(true));
        assert_eq!(n[0].tls, Some(true));
        assert_eq!(
            n[0].alpn.as_deref(),
            Some(&["h2".to_string(), "http/1.1".to_string()] as &[String])
        );
        assert_eq!(n[0].name, "anytls 香港 1");
    }
}
