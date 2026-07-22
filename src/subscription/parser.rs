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
/// 任何节点缺失关键字段（server / port）都会被跳过，不会中断整体解析。
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
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            let nodes = parse_json_value(&v);
            if !nodes.is_empty() {
                return Ok(nodes);
            }
        }
    }

    // 2) Clash YAML
    if trimmed.starts_with("proxies:") || trimmed.contains("\nproxies:") {
        if let Ok(yaml) = serde_yaml::from_str::<Value>(trimmed) {
            if let Some(proxies) = yaml.get("proxies").and_then(|p| p.as_array()) {
                let nodes: Vec<Node> = proxies.iter().filter_map(parse_clash_proxy).collect();
                if !nodes.is_empty() {
                    return Ok(nodes);
                }
            }
        }
    }

    // 3) base64 包裹的链接 / 配置（仅第一层尝试，防止无限递归）
    if depth < 1 {
        if let Some(decoded) = try_base64_decode(trimmed) {
            if let Ok(nodes) = parse_inner(&decoded, depth + 1) {
                if !nodes.is_empty() {
                    return Ok(nodes);
                }
            }
        }
    }

    // 4) 直接按 scheme:// 链接逐行解析
    let mut nodes = Vec::new();
    for line in trimmed.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(node) = parse_link(line) {
            nodes.push(node);
        }
    }
    if nodes.is_empty() {
        return Err(AppError(anyhow::anyhow!(
            "无法从订阅内容识别任何节点（需为 sing-box JSON / Clash YAML / v2rayN 链接）"
        )));
    }
    Ok(nodes)
}

fn parse_json_value(v: &Value) -> Vec<Node> {
    if let Some(obj) = v.as_object() {
        if let Some(outbounds) = obj.get("outbounds").and_then(|o| o.as_array()) {
            return outbounds.iter().filter_map(parse_singbox_outbound).collect();
        }
        if let Some(proxies) = obj.get("proxies").and_then(|p| p.as_array()) {
            return proxies.iter().filter_map(parse_clash_proxy).collect();
        }
        return Vec::new();
    }
    if let Some(arr) = v.as_array() {
        let mut nodes = Vec::new();
        for item in arr {
            if let Some(_o) = item.as_object() {
                if let Some(n) = parse_clash_proxy(item) {
                    nodes.push(n);
                }
            } else if let Some(s) = item.as_str() {
                if let Some(n) = parse_link(s) {
                    nodes.push(n);
                }
            }
        }
        return nodes;
    }
    Vec::new()
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
    let port = v.get("server_port").and_then(|p| p.as_u64())? as u16;

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
    let port = v.get("port")?.as_u64()? as u16;

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
            node.sni = v.get("servername").and_then(|s| s.as_str()).map(|s| s.to_string());
            node.network = v.get("network").and_then(|n| n.as_str()).map(|s| s.to_string());
            maybe_clash_ws(&mut node, v);
        }
        "vless" => {
            node.uuid = v.get("uuid").and_then(|x| x.as_str()).map(|s| s.to_string());
            node.flow = v.get("flow").and_then(|x| x.as_str()).map(|s| s.to_string());
            node.tls = v.get("tls").and_then(|t| t.as_bool());
            node.sni = v.get("servername").and_then(|s| s.as_str()).map(|s| s.to_string());
            node.network = v.get("network").and_then(|n| n.as_str()).map(|s| s.to_string());
            maybe_clash_ws(&mut node, v);
        }
        "trojan" => {
            node.password = v.get("password").and_then(|x| x.as_str()).map(|s| s.to_string());
            node.sni = v.get("sni").and_then(|s| s.as_str()).map(|s| s.to_string());
            node.tls = Some(true);
            node.network = v.get("network").and_then(|n| n.as_str()).map(|s| s.to_string());
            maybe_clash_ws(&mut node, v);
        }
        "hysteria2" => {
            node.password = v.get("password").and_then(|x| x.as_str()).map(|s| s.to_string());
            node.sni = v.get("sni").and_then(|s| s.as_str()).map(|s| s.to_string());
        }
        _ => {}
    }
    // Clash 订阅的字段已完整映射到 Node 结构；不再把整段 Clash JSON 透传进 extra，
    // 否则 `name`/`ws-opts`/`cipher` 等 Clash 专有字段会被渲染进 sing-box outbound，
    // 导致 1.13 因 unknown field 而 FATAL 拒绝启动。
    node.extra = None;
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

fn parse_link(line: &str) -> Option<Node> {
    if let Some(rest) = line.strip_prefix("ss://") {
        return parse_ss(rest);
    }
    if let Some(rest) = line.strip_prefix("vmess://") {
        return parse_vmess(rest);
    }
    if let Some(rest) = line.strip_prefix("vless://") {
        return parse_vless(rest);
    }
    if let Some(rest) = line.strip_prefix("trojan://") {
        return parse_trojan(rest);
    }
    if let Some(rest) = line.strip_prefix("hysteria2://") {
        return parse_hy2(rest);
    }
    if let Some(rest) = line.strip_prefix("hysteria://") {
        return parse_hy2(rest);
    }
    None
}

fn parse_ss(s: &str) -> Option<Node> {
    let (body, name) = split_name(s);
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
        userinfo
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
    let (uuid_host, query) = match auth_host.split_once('?') {
        Some((a, q)) => (a, q),
        None => (auth_host.as_str(), ""),
    };
    let (uuid, hostport) = uuid_host.split_once('@')?;
    let (server, port) = split_host_port(hostport)?;
    let mut node = Node {
        id: String::new(),
        name,
        kind: "vless".into(),
        server,
        port,
        uuid: Some(uuid.to_string()),
        ..Default::default()
    };
    apply_query(&mut node, query);
    Some(node)
}

fn parse_trojan(s: &str) -> Option<Node> {
    let (auth_host, name) = split_name(s);
    let (pass_host, query) = match auth_host.split_once('?') {
        Some((a, q)) => (a, q),
        None => (auth_host.as_str(), ""),
    };
    let (password, hostport) = pass_host.split_once('@')?;
    let (server, port) = split_host_port(hostport)?;
    let mut node = Node {
        id: String::new(),
        name,
        kind: "trojan".into(),
        server,
        port,
        password: Some(urldecode(password)),
        ..Default::default()
    };
    apply_query(&mut node, query);
    node.tls = Some(node.tls.unwrap_or(true));
    Some(node)
}

fn parse_hy2(s: &str) -> Option<Node> {
    let (auth_host, name) = split_name(s);
    let (pass_host, query) = match auth_host.split_once('?') {
        Some((a, q)) => (a, q),
        None => (auth_host.as_str(), ""),
    };
    let (password, hostport) = pass_host.split_once('@')?;
    let (server, port) = split_host_port(hostport)?;
    let mut node = Node {
        id: String::new(),
        name,
        kind: "hysteria2".into(),
        server,
        port,
        password: Some(urldecode(password)),
        ..Default::default()
    };
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == "sni" {
                node.sni = Some(v.to_string());
            }
        }
    }
    Some(node)
}

fn apply_query(node: &mut Node, query: &str) {
    let mut path: Option<String> = None;
    let mut host: Option<String> = None;
    let mut flow: Option<String> = None;
    let mut sni: Option<String> = None;
    let mut tls = false;
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            let v_dec = urldecode(v);
            // 空值丢弃：v2rayN 风格的链接常留空 `path=&host=`（参数存在但值为空），
            // 传给 sing-box 会导致 transport 写 `path: ""` / `Host: ""`，握手时服务端拒识。
            let v_opt = if v_dec.is_empty() { None } else { Some(v_dec) };
            match k {
                "security" => tls = v == "tls" || v == "reality",
                "type" => {
                    if v == "ws" {
                        node.network = Some("ws".into());
                    }
                }
                "path" => path = v_opt,
                "host" => host = v_opt,
                "flow" => flow = v_opt,
                "sni" => sni = v_opt,
                "peer" if sni.is_none() => sni = v_opt,
                _ => {}
            }
        }
    }
    if node.network.as_deref() == Some("ws") || path.is_some() || host.is_some() {
        node.ws = Some(WsOptions { path, host, headers: None });
        if node.network.is_none() {
            node.network = Some("ws".into());
        }
    }
    node.tls = Some(tls);
    node.flow = flow;
    if sni.is_some() {
        node.sni = sni;
    }
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
        let server = hp[..colon].to_string();
        let port: u16 = hp[colon + 1..].parse().ok()?;
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
    }

    #[test]
    fn parse_singbox_json() {
        let json = r#"{"outbounds":[{"type":"shadowsocks","tag":"sb","server":"5.6.7.8","server_port":9999,"method":"aes-256-gcm","password":"pw"}]}"#;
        let n = parse(json).unwrap();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].kind, "shadowsocks");
        assert_eq!(n[0].server, "5.6.7.8");
        assert_eq!(n[0].port, 9999);
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
}
