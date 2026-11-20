//! 代理流量统计：只计走代理（非直连）的连接。
//!
//! The controller's traffic stream includes direct connections. Read `/connections`,
//! track each connection's byte deltas, and count only the selected non-direct outbound.
//!
//! 已知误差：两次采样之间关闭的连接，最后一段字节不计入（采样间隔 1s，误差很小）。

use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Default, Deserialize)]
pub struct ConnectionsSnapshot {
    #[serde(default, deserialize_with = "deserialize_connections")]
    pub connections: Vec<ConnStat>,
}

fn deserialize_connections<'de, D>(deserializer: D) -> Result<Vec<ConnStat>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Vec<ConnStat>>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Default, Deserialize)]
pub struct ConnStat {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub upload: u64,
    #[serde(default)]
    pub download: u64,
    /// Mihomo's proxy chain. The local selector groups wrap the actual outbound.
    #[serde(default)]
    pub chains: Vec<String>,
}

/// 不算「走代理」的出口。
const NON_PROXY_OUTBOUNDS: &[&str] = &["direct", "block", "reject", "dns", "dns-out"];

impl ConnStat {
    fn is_proxied(&self) -> bool {
        const APP_GROUPS: &[&str] = &["yap-proxy", "yap-nodes", "global"];
        self.chains
            .iter()
            .find(|name| !APP_GROUPS.contains(&name.to_ascii_lowercase().as_str()))
            .is_some_and(|out| !NON_PROXY_OUTBOUNDS.contains(&out.to_ascii_lowercase().as_str()))
    }
}

/// 按连接累计字节算代理流量增量。
#[derive(Debug, Default)]
pub struct ProxyTrafficCounter {
    /// 连接 id → 上次看到的 (upload, download)
    last: HashMap<String, (u64, u64)>,
}

impl ProxyTrafficCounter {
    /// 核心重启后连接 id 全部换新，清掉旧记录。
    pub fn reset(&mut self) {
        self.last.clear();
    }

    /// 返回本次采样新增的代理 (上行, 下行) 字节。新出现的连接按其已有字节全额计入。
    pub fn tick(&mut self, snap: &ConnectionsSnapshot) -> (u64, u64) {
        let mut next = HashMap::with_capacity(snap.connections.len());
        let (mut du, mut dd) = (0u64, 0u64);
        for c in &snap.connections {
            if c.is_proxied() {
                let (pu, pd) = self.last.get(&c.id).copied().unwrap_or((0, 0));
                du = du.saturating_add(c.upload.saturating_sub(pu));
                dd = dd.saturating_add(c.download.saturating_sub(pd));
            }
            next.insert(c.id.clone(), (c.upload, c.download));
        }
        self.last = next;
        (du, dd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn(id: &str, up: u64, down: u64, out: &str) -> ConnStat {
        ConnStat { id: id.into(), upload: up, download: down, chains: vec![out.into()] }
    }

    #[test]
    fn counts_only_proxied_deltas() {
        let mut c = ProxyTrafficCounter::default();
        let s1 = ConnectionsSnapshot {
            connections: vec![conn("a", 100, 1000, "node-1"), conn("b", 50, 500, "direct")],
        };
        assert_eq!(c.tick(&s1), (100, 1000));
        let s2 = ConnectionsSnapshot {
            connections: vec![
                conn("a", 150, 1800, "node-1"),
                conn("b", 90, 900, "direct"),
                conn("c", 10, 20, "node-2"),
            ],
        };
        assert_eq!(c.tick(&s2), (60, 820));
        // 连接 a 关闭；c 无新增
        let s3 = ConnectionsSnapshot { connections: vec![conn("c", 10, 20, "node-2")] };
        assert_eq!(c.tick(&s3), (0, 0));
    }

    #[test]
    fn skips_selector_groups_before_classifying_direct_connections() {
        let mut counter = ProxyTrafficCounter::default();
        let snap = ConnectionsSnapshot {
            connections: vec![
                ConnStat { id: "direct".into(), upload: 10, download: 20, chains: vec!["YAP-PROXY".into(), "DIRECT".into()] },
                ConnStat { id: "proxy".into(), upload: 30, download: 40, chains: vec!["GLOBAL".into(), "YAP-PROXY".into(), "node-1".into(), "DIRECT".into()] },
            ],
        };
        assert_eq!(counter.tick(&snap), (30, 40));
    }

    #[test]
    fn parses_mihomo_connections_json() {
        let snap: ConnectionsSnapshot = serde_json::from_str(
            r#"{"downloadTotal":9,"uploadTotal":9,"connections":[
                {"id":"x","upload":1,"download":2,"chains":["node-1"],"metadata":{}},
                {"id":"y","upload":3,"download":4,"chains":["direct"]}]}"#,
        )
        .unwrap();
        let mut c = ProxyTrafficCounter::default();
        assert_eq!(c.tick(&snap), (1, 2));

        let idle: ConnectionsSnapshot =
            serde_json::from_str(r#"{"connections":null,"downloadTotal":0,"uploadTotal":0}"#)
                .unwrap();
        assert!(idle.connections.is_empty());
    }
}
