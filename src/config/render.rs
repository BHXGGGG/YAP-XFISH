use crate::app::AppConfig;
use crate::config::model::{AppProfile, Node, ProxyMode};
use serde_json::{json, Value};

/// 将内部配置模型渲染为 sing-box 的 config.json（serde_json::Value）。
pub fn render(profile: &AppProfile, cfg: &AppConfig) -> Value {
    let mut outbounds = Vec::new();
    for node in &profile.nodes {
        outbounds.push(node_to_outbound(node));
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

    let rules: Vec<Value> = profile
        .rules
        .iter()
        .map(|r| {
            json!({
                "type": r.r#type,
                "payload": r.payload,
                "outbound": r.outbound,
            })
        })
        .collect();

    // sing-box 1.13：inbound 上的 sniff 字段已移除，改用 route 规则动作 `sniff`（放在最前）
    let mut all_rules: Vec<Value> = vec![json!({ "action": "sniff" })];
    all_rules.extend(rules);

    let mut route = json!({
        "rules": all_rules,
        "final": final_outbound,
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
        // 声明远程规则集（首次启动时经直连下载并缓存到 data_dir）
        route["rule_set"] = json!([
            {
                "type": "remote",
                "tag": "geosite-cn",
                "format": "binary",
                "url": "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-cn.srs",
                "download_detour": "direct"
            },
            {
                "type": "remote",
                "tag": "geoip-cn",
                "format": "binary",
                "url": "https://raw.githubusercontent.com/SagerNet/sing-geoip/rule-set/geoip-cn.srs",
                "download_detour": "direct"
            }
        ]);
    }

    let clash_api = json!({
        "external_controller": format!("127.0.0.1:{}", cfg.clash_api_port),
        "secret": cfg.api_secret,
    });

    let mut config = json!({
        "log": { "level": "info", "timestamp": true },
        // sing-box 1.12+ 新版 DNS server 格式：用 type + server 取代旧的 address 字段
        // （旧格式在 1.12 起弃用、1.14 移除，1.13 直接 FATAL 拒绝启动）
        "dns": {
            "servers": [
                { "type": "https", "tag": "remote", "server": "1.1.1.1" },
                { "type": "udp", "tag": "local", "server": "223.5.5.5" }
            ],
            "final": "remote",
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
        "experimental": { "clash_api": clash_api }
    });

    if cfg.enable_tun {
        config["inbounds"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "type": "tun",
                "tag": "tun-in",
                "address": ["172.19.0.1/30"],
                "auto_route": true,
                "stack": "system"
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
        "tuic" => "tuic",
        "wireguard" => "wireguard",
        "ssh" => "ssh",
        // snell / ssr 等 Clash 专有协议在 sing-box 无对应实现，保持原名（核心将明确报错）。
        other => other,
    }
}

/// 将内部 Node 转换为 sing-box outbound。覆盖常用协议，未知协议走 extra 透传。
fn node_to_outbound(node: &Node) -> Value {
    let tag = &node.id;
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
            apply_transport(&mut v, node);
            v
        }
        "vless" => {
            let mut v = json!({
                "type": "vless",
                "server": node.server,
                "server_port": node.port,
                "uuid": node.uuid.clone().unwrap_or_default(),
                "flow": node.flow.clone().unwrap_or_default(),
                "tls": { "enabled": node.tls.unwrap_or(true) },
            });
            if let Some(sni) = &node.sni {
                v["tls"]["server_name"] = json!(sni);
            }
            apply_transport(&mut v, node);
            v
        }
        "trojan" => {
            let mut v = json!({
                "type": "trojan",
                "server": node.server,
                "server_port": node.port,
                "password": node.password.clone().unwrap_or_default(),
                "tls": { "enabled": true },
            });
            if let Some(sni) = &node.sni {
                v["tls"]["server_name"] = json!(sni);
            }
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
        other => json!({
            "type": other,
            "server": node.server,
            "server_port": node.port,
        }),
    };

    if let Value::Object(ref mut m) = out {
        m.insert("tag".into(), json!(tag));
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
