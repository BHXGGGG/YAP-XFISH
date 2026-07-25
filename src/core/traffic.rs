//! Clash API 会话累计流量。
//!
//! sing-box 1.13 的 `GET /traffic` 是 **NDJSON 流**（每秒一行速率），不适合
//! 普通 HTTP JSON 一次解析。改用 `GET /connections` 的
//! `uploadTotal` / `downloadTotal`（累计字节），与 Clash Meta 兼容。

use serde::Deserialize;

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct TrafficTotals {
    /// 累计上行字节
    #[serde(default, rename = "uploadTotal")]
    pub up: u64,
    /// 累计下行字节
    #[serde(default, rename = "downloadTotal")]
    pub down: u64,
}
