use serde::{Deserialize, Serialize};

/// 订阅更新周期。Manual 表示仅手动触发；
/// 预设周期映射到固定间隔定时器；Cron 为自定义 5 段表达式。
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateInterval {
    Manual,
    #[serde(alias = "every_30_min")]
    Every30Min,
    Hourly,
    #[serde(alias = "every_6_hours")]
    Every6Hours,
    #[serde(alias = "every_12_hours")]
    Every12Hours,
    Daily,
    Cron(String),
}

impl Default for UpdateInterval {
    fn default() -> Self {
        // 默认每 6 小时自动更新订阅
        UpdateInterval::Every6Hours
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpdateStatus {
    Idle,
    Updating,
    Success,
    Failed,
}

impl Default for UpdateStatus {
    fn default() -> Self {
        UpdateStatus::Idle
    }
}

/// 单条订阅。节点列表在阶段 5 由 fetcher/parser 填充并合并进 AppProfile。
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub interval: UpdateInterval,
    #[serde(default = "default_enabled_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<String>,
    #[serde(default)]
    pub last_status: UpdateStatus,
    #[serde(default)]
    pub last_message: String,
    #[serde(default)]
    pub node_count: usize,
}

fn default_enabled_true() -> bool {
    true
}
