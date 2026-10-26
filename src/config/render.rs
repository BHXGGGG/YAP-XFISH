use crate::app::AppConfig;
use crate::config::model::{AppProfile, Node, ProxyMode, RouteRule};
use serde_json::{json, Value};
use std::collections::HashSet;

/// 将内部配置模型渲染为 sing-box 的 config.json（serde_json::Value）。
pub fn render(profile: &AppProfile, cfg: &AppConfig) -> Value {
    let mut outbounds = Vec::new();
    for node in &profile.nodes {
        outbounds.push(node_to_outbound_in_profile(node, profile));
    }
    outbounds.push(json!({ "type": "direct", "tag": "direct" }));
    outbounds.push(json!({ "type": "block", "tag": "block" }));

    let final_outbound = match profile.mode {
        ProxyMode::Direct => "direct".to_string(),
        ProxyMode::Global | ProxyMode::Rule => profile
            .selected_node
            .clone()
            .unwrap_or_else(|| "direct".to_string()),
    };

    // 用户规则必须渲染为 sing-box 1.13 合法字段（domain/ip_cidr/...），
    // 旧写法 {type,payload} 会 FATAL: unknown rule type，导致核心挂掉全站不可用。
    let user_rules: Vec<Value> = profile
        .rules
        .iter()
        .filter_map(route_rule_to_json)
        .collect();
    let mut needed_rule_sets: HashSet<String> = HashSet::new();
    for r in &profile.rules {
        if let Some(tag) = rule_set_tag_for(r) {
            needed_rule_sets.insert(tag);
        }
    }

    // sing-box 1.13：inbound 上的 sniff 字段已移除，改用 route 规则动作 `sniff`。
    //
    // TUN DNS 关键（实测日志）：
    // Windows 会把 DNS 打到 TUN 网关 172.19.0.2:53。若未 hijack 到 DNS 模块，
    // Global 会把 UDP/53 当普通包送进 VLESS（log: outbound/vless → 172.19.0.2:53），
    // 解析全挂，网页表现为「完全上不了网」；系统代理不受影响（HTTP/SOCKS 自带域名）。
    //
    // 顺序必须是：
    // 1) 先 sniff，才能识别 protocol=dns
    // 2) 再 protocol:dns → hijack-dns
    // 3) 再 port 53/853 兜底（部分查询 sniff 识别不到）
    // 不要给 sniff 加过短 timeout，TLS 嗅探失败会导致分流错判。
    let mut all_rules: Vec<Value> = vec![
        json!({ "action": "sniff" }),
        json!({ "protocol": "dns", "action": "hijack-dns" }),
        json!({ "port": [53, 853], "action": "hijack-dns" }),
    ];
    all_rules.extend(user_rules);

    let mut route = json!({
        "rules": all_rules,
        "final": final_outbound.clone(),
        "auto_detect_interface": true,
        // sing-box 1.12+ 要求显式指定域名解析器（否则 1.13 FATAL）：指向本地 DNS server tag
        "default_domain_resolver": { "server": "local" },
    });

    if profile.mode == ProxyMode::Rule {
        // 规则模式默认放行中国大陆流量。
        // sing-box 1.12 起移除内置 geosite/geoip 数据库，改用远程 rule_set（.srs）。
        if let Some(arr) = route.get_mut("rules").and_then(|v| v.as_array_mut()) {
            arr.push(json!({ "rule_set": "geoip-cn", "outbound": "direct" }));
            arr.push(json!({ "rule_set": "geosite-cn", "outbound": "direct" }));
        }
        needed_rule_sets.insert("geoip-cn".into());
        needed_rule_sets.insert("geosite-cn".into());
    }

    if !needed_rule_sets.is_empty() {
        let mut sets = Vec::new();
        let mut tags: Vec<String> = needed_rule_sets.into_iter().collect();
        tags.sort();
        for tag in tags {
            if let Some(url) = rule_set_url(&tag) {
                sets.push(json!({
                    "type": "remote",
                    "tag": tag,
                    "format": "binary",
                    "url": url,
                    // 国内直连 raw.githubusercontent.com 常超时，首次下载失败会让核心 FATAL 退出；走当前出站更稳
                    "download_detour": final_outbound.clone()
                }));
            }
        }
        if !sets.is_empty() {
            route["rule_set"] = Value::Array(sets);
        }
    }

    let clash_api = json!({
        "external_controller": format!("127.0.0.1:{}", cfg.clash_api_port),
        "secret": cfg.api_secret,
    });

    // 核心日志默认落盘到 data/logs/sing-box.log，便于排查 TUN/路由问题。
    // output 用绝对路径，避免 cwd 变化导致写丢。
    let data_dir = if cfg.data_dir.as_os_str().is_empty() {
        crate::app::default_data_dir()
    } else {
        cfg.data_dir.clone()
    };
    let core_log_path = {
        let logs = data_dir.join("logs");
        let _ = std::fs::create_dir_all(&logs);
        logs.join("sing-box.log")
            .to_string_lossy()
            .replace('\\', "/")
    };

    let mut config = json!({
        "log": {
            "level": "info",
            "timestamp": true,
            "output": core_log_path
        },
        // sing-box 1.12+ 新版 DNS server 格式：用 type + server 取代旧的 address 字段
        // （旧格式在 1.12 起弃用、1.14 移除，1.13 直接 FATAL 拒绝启动）
        "dns": {
            // local 走直连 UDP（国内可达）；remote 为 DoH 备用。
            // TUN 下 DNS 被 hijack 后由本模块处理，不再进 VLESS。
            // 注意：sing-box 1.13 禁止 DNS server detour 到「空」direct
            // （FATAL: detour to an empty direct outbound makes no sense），
            // 所以这里不要写 detour:"direct"。
            "servers": [
                {
                    "type": "udp",
                    "tag": "local",
                    "server": "223.5.5.5"
                },
                {
                    "type": "https",
                    "tag": "remote",
                    "server": "1.1.1.1"
                }
            ],
            // TUN 场景优先 local，避免 DoH 超时拖垮全部建连。
            "final": "local",
            "strategy": "prefer_ipv4"
        },
        "inbounds": [
            {
                "type": "mixed",
                "tag": "mixed-in",
                "listen": "127.0.0.1",
                "listen_port": cfg.proxy_port
            }
        ],
        "outbounds": outbounds,
        "route": route,
        "experimental": {
            "clash_api": clash_api,
            // 持久化远程 rule_set：重启核心直接读缓存，不再每次联网下载（下载失败 = 核心 FATAL）
            "cache_file": {
                "enabled": true,
                "path": data_dir.join("cache.db").to_string_lossy().replace('\\', "/")
            }
        }
    });

    if cfg.enable_tun {
        // 默认连通优先：gvisor + MTU 1500 + auto_route，不加 strict_route。
        // Windows 上 strict_route / 过大 MTU / mixed 都曾导致「完全上不了网」。
        // 吞吐相关参数仍可在设置里改 tun_stack / tun_mtu 后 regen。
        // gvisor 启动失败时 core/manager 仍会回退写 stack=system 再启一次。
        let stack = match cfg.tun_stack.trim().to_ascii_lowercase().as_str() {
            "mixed" => "mixed",
            "system" => "system",
            _ => "gvisor",
        };
        let mtu = if cfg.tun_mtu >= 1280 && cfg.tun_mtu <= 65535 {
            cfg.tun_mtu
        } else {
            1500
        };
        config["inbounds"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "type": "tun",
                "tag": "tun-in",
                "address": ["172.19.0.1/30"],
                // 显式声明 TUN 侧 DNS，配合 route hijack-dns；否则系统常把
                // 172.19.0.2:53 当普通 UDP 送进 Global outbound。
                "mtu": mtu,
                "auto_route": true,
                "stack": stack
            }));
    }

    config
}

/// 将 Clash 等订阅源使用的协议名翻译为 sing-box 1.13 接受的 outbound `type`。
/// 例如 Clash 的 `socks5` 在 sing-box 中应为 `socks`。已持久化的旧 profile.json 可能仍
/// 保留 Clash 原名，渲染时统一归一化，避免核心因 `unknown outbound type` 而 FATAL 拒绝启动。
fn normalize_kind(kind: &str) -> &str {
    match kind {
        "ss" => "shadowsocks",
        "socks5" | "socks" => "socks",
        "http" => "http",
        "vmess" => "vmess",
        "vless" => "vless",
        "trojan" => "trojan",
        "hysteria" | "hysteria2" => "hysteria2",
        "anytls" => "anytls",
        "tuic" => "tuic",
        "wireguard" => "wireguard",
        "ssh" => "ssh",
        // snell / ssr 等 Clash 专有协议在 sing-box 无对应实现，保持原名（核心将明确报错）。
        other => other,
    }
}

/// 将内部 RouteRule 转为 sing-box 1.13 route.rules 条目。
/// 返回 None 表示规则无效/空 payload，渲染时跳过。
fn route_rule_to_json(r: &RouteRule) -> Option<Value> {
    let outbound = r.outbound.trim();
    if outbound.is_empty() {
        return None;
    }
    let t = r.r#type.trim().to_ascii_lowercase();
    let raw = r.payload.trim();
    if raw.is_empty() {
        return None;
    }

    // 兼容 payload 形如 "geosite:cn" / "geoip:cn" / "domain_suffix:example.com"
    let (t, raw) = if let Some((head, rest)) = raw.split_once(':') {
        let head = head.trim().to_ascii_lowercase();
        match head.as_str() {
            "geosite" | "geoip" | "domain" | "domain_suffix" | "domain_keyword"
            | "domain_regex" | "ip_cidr" | "process_name" | "port" | "protocol" => {
                (head, rest.trim().to_string())
            }
            _ => (t, raw.to_string()),
        }
    } else {
        (t, raw.to_string())
    };
    if raw.is_empty() {
        return None;
    }

    match t.as_str() {
        "domain" | "domain_suffix" | "domain_keyword" | "domain_regex" => {
            let host = normalize_host(&raw)?;
            Some(json!({ t: [host], "outbound": outbound }))
        }
        "ip_cidr" | "ip" => {
            // 单 IP 自动补 /32 或 /128
            let cidr = normalize_ip_cidr(&raw)?;
            Some(json!({ "ip_cidr": [cidr], "outbound": outbound }))
        }
        "port" => {
            let ports = parse_ports(&raw)?;
            Some(json!({ "port": ports, "outbound": outbound }))
        }
        "process_name" => Some(json!({ "process_name": [raw], "outbound": outbound })),
        "protocol" => Some(json!({ "protocol": [raw], "outbound": outbound })),
        "geosite" => {
            // 内置 geosite 已移除，映射到远程 rule_set 标签
            let tag = if raw == "cn" || raw == "geosite-cn" {
                "geosite-cn".to_string()
            } else {
                format!("geosite-{raw}")
            };
            Some(json!({ "rule_set": tag, "outbound": outbound }))
        }
        "geoip" => {
            let tag = if raw.eq_ignore_ascii_case("cn") || raw == "geoip-cn" {
                "geoip-cn".to_string()
            } else {
                format!("geoip-{}", raw.to_ascii_lowercase())
            };
            Some(json!({ "rule_set": tag, "outbound": outbound }))
        }
        "rule_set" => Some(json!({ "rule_set": raw, "outbound": outbound })),
        _ => None,
    }
}

fn rule_set_tag_for(r: &RouteRule) -> Option<String> {
    let t = r.r#type.trim().to_ascii_lowercase();
    let raw = r.payload.trim();
    let (t, raw) = if let Some((head, rest)) = raw.split_once(':') {
        let head = head.trim().to_ascii_lowercase();
        if matches!(head.as_str(), "geosite" | "geoip" | "rule_set") {
            (head, rest.trim())
        } else {
            (t, raw)
        }
    } else {
        (t, raw)
    };
    match t.as_str() {
        "geosite" => Some(if raw == "cn" || raw == "geosite-cn" {
            "geosite-cn".into()
        } else {
            format!("geosite-{raw}")
        }),
        "geoip" => Some(if raw.eq_ignore_ascii_case("cn") || raw == "geoip-cn" {
            "geoip-cn".into()
        } else {
            format!("geoip-{}", raw.to_ascii_lowercase())
        }),
        "rule_set" => Some(raw.to_string()),
        _ => None,
    }
}

fn rule_set_url(tag: &str) -> Option<String> {
    match tag {
        "geosite-cn" => Some(
            "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-cn.srs"
                .into(),
        ),
        "geoip-cn" => Some(
            "https://raw.githubusercontent.com/SagerNet/sing-geoip/rule-set/geoip-cn.srs".into(),
        ),
        _ => {
            // 通用：geosite-xxx / geoip-xx
            if let Some(rest) = tag.strip_prefix("geosite-") {
                Some(format!(
                    "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-{rest}.srs"
                ))
            } else if let Some(rest) = tag.strip_prefix("geoip-") {
                Some(format!(
                    "https://raw.githubusercontent.com/SagerNet/sing-geoip/rule-set/geoip-{rest}.srs"
                ))
            } else {
                None
            }
        }
    }
}

/// 规范化域名输入：去掉 scheme/path/port/空白。
/// `https://ip.skk.moe/` → `ip.skk.moe`
fn normalize_host(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .or_else(|| s.strip_prefix("HTTPS://"))
        .or_else(|| s.strip_prefix("HTTP://"))
        .unwrap_or(s);
    // 去掉 path / query / fragment
    let s = s.split(['/', '?', '#']).next().unwrap_or(s).trim();
    // 去掉端口
    let s = if s.starts_with('[') {
        // [ipv6]:port
        s
    } else {
        s.rsplit_once(':')
            .and_then(|(h, p)| {
                if p.chars().all(|c| c.is_ascii_digit()) {
                    Some(h)
                } else {
                    None
                }
            })
            .unwrap_or(s)
    };
    let s = s.trim().trim_end_matches('.').to_ascii_lowercase();
    if s.is_empty() || s.contains(' ') {
        return None;
    }
    Some(s)
}

fn normalize_ip_cidr(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains('/') {
        return Some(s.to_string());
    }
    if s.contains(':') {
        // IPv6 bare
        Some(format!("{s}/128"))
    } else {
        Some(format!("{s}/32"))
    }
}

fn parse_ports(raw: &str) -> Option<Vec<u16>> {
    let mut out = Vec::new();
    for part in raw.split([',', ' ', ';']) {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if let Some((a, b)) = p.split_once('-') {
            let a: u16 = a.trim().parse().ok()?;
            let b: u16 = b.trim().parse().ok()?;
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            for x in lo..=hi {
                out.push(x);
            }
        } else {
            out.push(p.parse().ok()?);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 将内部 Node 转换为 sing-box outbound。覆盖常用协议，未知协议走 extra 透传。
/// 公开给 latency（UIF 风格临时 core + Clash delay API）复用同一渲染逻辑。
/// 不带 profile 时无法校验 detour 有效性，仅在 detour 非空时原样写入（测速侧应传入完整链）。
pub fn node_to_outbound(node: &Node) -> Value {
    let mut out = node_to_outbound_base(node);
    if let Some(d) = node.detour.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if d != node.id {
            out["detour"] = json!(d);
        }
    }
    out
}

/// 带 profile 上下文：仅当 detour 目标存在、不构成环、且出口协议支持链式时才写入。
pub fn node_to_outbound_in_profile(node: &Node, profile: &AppProfile) -> Value {
    let mut out = node_to_outbound_base(node);
    if let Some(d) = node.detour.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if detour_is_safe(&profile.nodes, &node.id, d) && detour_protocol_ok(node) {
            out["detour"] = json!(d);
        }
    }
    out
}

/// UDP/QUIC 类出口经 detour 拨号极易超时，渲染时直接忽略其 detour。
pub fn detour_protocol_ok(exit: &Node) -> bool {
    !matches!(
        exit.kind.to_ascii_lowercase().as_str(),
        "hysteria" | "hysteria2" | "hy2" | "tuic" | "wireguard" | "wg"
    )
}

/// 校验 detour：非空、非自环、目标存在、沿链不出现环（最多 32 跳）。
pub fn detour_is_safe(nodes: &[Node], from_id: &str, detour: &str) -> bool {
    let detour = detour.trim();
    if detour.is_empty() || detour == from_id {
        return false;
    }
    if !nodes.iter().any(|n| n.id == detour) {
        return false;
    }
    let mut seen: HashSet<String> = HashSet::new();
    seen.insert(from_id.to_string());
    let mut cur = detour.to_string();
    for _ in 0..32 {
        if !seen.insert(cur.clone()) {
            return false; // 环
        }
        let next = nodes
            .iter()
            .find(|n| n.id == cur)
            .and_then(|n| n.detour.as_ref())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        match next {
            Some(d) => cur = d,
            None => return true,
        }
    }
    false
}

/// 收集 `node` 及其 detour 链上的全部节点（用于测速临时 core 注入 outbound）。
pub fn collect_chain_nodes<'a>(all: &'a [Node], node: &Node) -> Vec<Node> {
    let mut out = vec![node.clone()];
    let mut have: HashSet<String> = HashSet::from([node.id.clone()]);
    let mut cur = node
        .detour
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    for _ in 0..32 {
        let Some(id) = cur else { break };
        if !have.insert(id.clone()) {
            break;
        }
        if let Some(n) = all.iter().find(|n| n.id == id) {
            out.push(n.clone());
            cur = n
                .detour
                .as_ref()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
        } else {
            break;
        }
    }
    out
}

/// 在 selected 基础上补齐所有被引用的 detour 节点（任意协议）。
pub fn expand_with_detours(all: &[Node], selected: &[Node]) -> Vec<Node> {
    let mut have: HashSet<String> = selected.iter().map(|n| n.id.clone()).collect();
    let mut out: Vec<Node> = selected.to_vec();
    for n in selected {
        let chain = collect_chain_nodes(all, n);
        for c in chain {
            if have.insert(c.id.clone()) {
                out.push(c);
            }
        }
    }
    out
}

fn node_to_outbound_base(node: &Node) -> Value {
    let mut out = match normalize_kind(&node.kind) {
        "shadowsocks" => json!({
            "type": "shadowsocks",
            "server": node.server,
            "server_port": node.port,
            "method": node.cipher.clone().unwrap_or_else(|| "aes-256-gcm".into()),
            "password": node.password.clone().unwrap_or_default(),
        }),
        "vmess" => {
            let mut v = json!({
                "type": "vmess",
                "server": node.server,
                "server_port": node.port,
                "uuid": node.uuid.clone().unwrap_or_default(),
                "security": node.security.clone().unwrap_or_else(|| "auto".into()),
            });
            apply_tls(&mut v, node, false);
            apply_transport(&mut v, node);
            v
        }
        "vless" => {
            let mut v = json!({
                "type": "vless",
                "server": node.server,
                "server_port": node.port,
                "uuid": node.uuid.clone().unwrap_or_default(),
            });
            // flow 仅在非空时写入（xtls-rprx-vision 等）；空字符串会让部分内核异常
            if let Some(flow) = node.flow.as_ref().filter(|s| !s.is_empty()) {
                v["flow"] = json!(flow);
            }
            // VLESS 默认开 TLS（订阅里 tls:true / security=tls|reality 时也会置 true）
            apply_tls(&mut v, node, true);
            apply_transport(&mut v, node);
            v
        }
        "trojan" => {
            let mut v = json!({
                "type": "trojan",
                "server": node.server,
                "server_port": node.port,
                "password": node.password.clone().unwrap_or_default(),
            });
            apply_tls(&mut v, node, true);
            apply_transport(&mut v, node);
            v
        }
        "hysteria2" | "hysteria" => {
            // Hysteria v2 协议强制要求 TLS；sing-box 1.13 若缺省 tls 对象会 FATAL
            // `TLS required`。默认启用，并使用节点 SNI 作为 TLS server_name。
            //
            // 同时默认 `insecure: true`：
            //   - 大量代理服务商的 hy2 节点用「伪装 SNI」（如 `www.bing.com`）做流量伪装，
            //     但 SNI 对应的真实证书和实际节点出口 IP 不匹配；严格校验会立即
            //     `CRYPTO_ERROR 0x12a x509: certificate signed by unknown authority`。
            //   - 业内通用做法（Clash.Meta / mihomo / Clash Party / uif 等）对 hysteria2
            //     outbound 默认 `skip-cert-verify: true`（对应 sing-box 的 `insecure: true`）。
            //   - 用户可在节点编辑里手动关掉 insecure 以启用严格校验（暂未暴露 UI，默认即可）。
            let mut h = json!({
                "type": "hysteria2",
                "server": node.server,
                "server_port": node.port,
                "password": node.password.clone().unwrap_or_default(),
                "tls": {
                    "enabled": true,
                    "insecure": true,
                },
            });
            if let Some(sni) = &node.sni {
                h["tls"]["server_name"] = json!(sni);
            }
            h
        }
        "anytls" => {
            // AnyTLS 协议同样强制要求 TLS，行为与 hysteria2 类似：默认 insecure=true
            // （伪装 SNI 时严格校验必失败），用户可在节点编辑里关掉。
            let mut a = json!({
                "type": "anytls",
                "server": node.server,
                "server_port": node.port,
                "password": node.password.clone().unwrap_or_default(),
                "tls": {
                    "enabled": true,
                    "insecure": node.skip_cert_verify.unwrap_or(true),
                },
            });
            if let Some(sni) = node.sni.as_ref().filter(|s| !s.is_empty()) {
                a["tls"]["server_name"] = json!(sni);
            }
            if let Some(fp) = node.fingerprint.as_ref().filter(|s| !s.is_empty()) {
                a["tls"]["utls"] = json!({ "enabled": true, "fingerprint": fp });
            }
            if let Some(alpn) = node.alpn.as_ref().filter(|v| !v.is_empty()) {
                a["tls"]["alpn"] = json!(alpn);
            }
            a
        }
        other => json!({
            "type": other,
            "server": node.server,
            "server_port": node.port,
        }),
    };

    if let Value::Object(ref mut m) = out {
        m.insert("tag".into(), json!(&node.id));
        // 合并 extra 透传字段。注意跳过 Clash 等非 sing-box 字段（如 `name`、`ws-opts`、
        // `cipher`、`tls`(布尔)、`sni`/`servername`、`network` 等），否则 sing-box 1.13 会因
        // `unknown field` 直接 FATAL 拒绝启动。解析阶段已把 Clash 订阅的 extra 置为 None，
        // 此处再兜底过滤，可兼容旧版解析器已持久化的节点数据。
        if let Some(extra) = &node.extra {
            if let Value::Object(e) = extra {
                for (k, v) in e {
                    let kl = k.to_lowercase();
                    if kl == "name"
                        || kl == "ws-opts"
                        || kl == "plugin"
                        || kl == "plugin-opts"
                        || kl == "origin"
                        || kl == "udp"
                        || kl == "tfo"
                        || kl == "skip-cert-verify"
                        || kl == "client-fingerprint"
                        || kl == "servername"
                        || kl == "network"
                        || kl == "cipher"
                        || kl == "sni"
                    {
                        continue;
                    }
                    // Clash 的 tls 是布尔值，sing-box 用 tls 对象；布尔则跳过（对象保留以兼容高级配置）。
                    if kl == "tls" && v.is_boolean() {
                        continue;
                    }
                    m.insert(k.clone(), v.clone());
                }
            }
        }
    }
    out
}

/// 应用 TLS 配置（含 REALITY / utls fingerprint / insecure）。
///
/// 关键：大量机场 VLESS 使用 `flow=xtls-rprx-vision` + 伪装 SNI + REALITY。
/// 若只写 `tls.enabled + server_name` 而丢掉 `reality`/`utls`，握手会落到真实
/// HTTPS 站点（如 itunes），服务端回 HTTP，sing-box 报 `unknown version: 72`（'H'）。
///
/// `default_enable`：协议默认是否启用 TLS（vless/trojan 为 true；vmess 看 node.tls）。
fn apply_tls(v: &mut Value, node: &Node, default_enable: bool) {
    let enabled = node.tls.unwrap_or(default_enable)
        || node.reality.unwrap_or(false)
        || node.reality_public_key.is_some();
    if !enabled && node.fingerprint.is_none() && node.skip_cert_verify.is_none() {
        return;
    }
    let mut tls = json!({ "enabled": enabled || default_enable });
    if let Some(sni) = node.sni.as_ref().filter(|s| !s.is_empty()) {
        tls["server_name"] = json!(sni);
    }
    // REALITY
    let use_reality = node.reality.unwrap_or(false) || node.reality_public_key.is_some();
    if use_reality {
        tls["enabled"] = json!(true);
        let mut reality = json!({ "enabled": true });
        if let Some(pbk) = node.reality_public_key.as_ref().filter(|s| !s.is_empty()) {
            reality["public_key"] = json!(pbk);
        }
        if let Some(sid) = node.reality_short_id.as_ref().filter(|s| !s.is_empty()) {
            reality["short_id"] = json!(sid);
        }
        tls["reality"] = reality;
        // REALITY 场景下 utls fingerprint 几乎总是需要；缺省 chrome（与 Clash Party 一致）
        let fp = node
            .fingerprint
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| s.as_str())
            .unwrap_or("chrome");
        tls["utls"] = json!({ "enabled": true, "fingerprint": fp });
    } else if let Some(fp) = node.fingerprint.as_ref().filter(|s| !s.is_empty()) {
        tls["utls"] = json!({ "enabled": true, "fingerprint": fp });
    }
    // insecure / skip-cert-verify（非 REALITY 时有意义；REALITY 本身不校验站点证书）
    if !use_reality {
        if let Some(true) = node.skip_cert_verify {
            tls["insecure"] = json!(true);
        }
    }
    if let Some(alpn) = &node.alpn {
        if !alpn.is_empty() {
            tls["alpn"] = json!(alpn);
        }
    }
    v["tls"] = tls;
}

/// 应用传输层（目前支持 ws），其余网络类型暂按默认处理。
/// 空字符串路径/Host 视作 None：避免 sing-box 写 `path: ""` / `Host: ""` 导致握手失败。
fn apply_transport(v: &mut Value, node: &Node) {
    fn nonempty(s: Option<String>) -> Option<String> {
        s.filter(|s| !s.is_empty())
    }
    let network = node.network.clone().unwrap_or_else(|| "tcp".into());
    if network == "ws" {
        let mut transport = json!({ "type": "ws" });
        if let Some(ws) = &node.ws {
            if let Some(p) = nonempty(ws.path.clone()) {
                transport["path"] = json!(p);
            }
            if let Some(h) = nonempty(ws.host.clone()) {
                transport["headers"] = json!({ "Host": h });
            }
        } else {
            if let Some(p) = nonempty(node.path.clone()) {
                transport["path"] = json!(p);
            }
            if let Some(h) = nonempty(node.host.clone()) {
                transport["headers"] = json!({ "Host": h });
            }
        }
        v["transport"] = transport;
    }
}
