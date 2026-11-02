//! 代理流量统计：只计走代理（非直连）的连接。
//!
//! sing-box 1.13 的 `GET /traffic` 是 **NDJSON 流**（每秒一行速率），不适合
//! 普通 HTTP JSON 一次解析；且 `/connections` 的 `uploadTotal` / `downloadTotal`
//! 包含直连流量。这里改为读取 `/connections` 的每条连接，按连接 id 记住上次的
//! 累计字节，只把出口不是 direct / block 的连接增量计入。
//!
//! 已知误差：两次采样之间关闭的连接，最后一段字节不计入（采样间隔 1s，误差很小）。

use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Default, Deserialize)]
pub struct ConnectionsSnapshot {
    #[serde(default)]
    pub connections: Vec<ConnStat>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ConnStat {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub upload: u64,
    #[serde(default)]
    pub download: u64,
    /// sing-box 的 chains：[出口, ..., 入口侧]，chains[0] 为最终出口。
    #[serde(default)]
    pub chains: Vec<String>,
}

/// 不算「走代理」的出口。
const NON_PROXY_OUTBOUNDS: &[&str] = &["direct", "block", "reject", "dns", "dns-out"];

impl ConnStat {
    fn is_proxied(&self) -> bool {
        match self.chains.first() {
            Some(out) => !NON_PROXY_OUTBOUNDS.contains(&out.to_ascii_lowercase().as_str()),
            None => false,
        }
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
    fn parses_sing_box_connections_json() {
        let snap: ConnectionsSnapshot = serde_json::from_str(
            r#"{"downloadTotal":9,"uploadTotal":9,"connections":[
                {"id":"x","upload":1,"download":2,"chains":["node-1"],"metadata":{}},
                {"id":"y","upload":3,"download":4,"chains":["direct"]}]}"#,
        )
        .unwrap();
        let mut c = ProxyTrafficCounter::default();
        assert_eq!(c.tick(&snap), (1, 2));
    }
}
