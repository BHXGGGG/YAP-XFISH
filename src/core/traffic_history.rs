//! 流量历史：跨进程持久化今日 + 30 天每日。
//!
//! 数据文件：`data/traffic_history.json`
//! 内存：~1–2 KB。
//!
//! 行为：
//! - 启动读盘；运行时由 `record_tick` 每 30s 把今日累计落盘；
//! - 跨日：检测今天与上次日期不同则归档昨日（按日期降序裁剪到 30 天）后重置；
//! - core stop / 进程退出时 `flush` 同步落盘。

use crate::app::AppState;
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::sync::RwLock;

const TRAFFIC_HISTORY_FILE: &str = "traffic_history.json";
pub const DAILY_HISTORY_DAYS: usize = 30;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DayTraffic {
    pub date: String,
    pub up: u64,
    pub down: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrafficHistory {
    #[serde(default)]
    pub today: String,
    #[serde(default)]
    pub today_up: u64,
    #[serde(default)]
    pub today_down: u64,
    #[serde(default)]
    pub days: Vec<DayTraffic>,
}

impl TrafficHistory {
    pub fn today_local() -> String {
        chrono::Local::now().format("%Y-%m-%d").to_string()
    }
}

static HISTORY: OnceCell<RwLock<TrafficHistory>> = OnceCell::new();
static DATA_DIR: OnceCell<PathBuf> = OnceCell::new();
/// 上次 record_tick 时的会话累计（traffic_up/down），用于算增量，避免把累计值当增量重复叠加。
static LAST_SESSION: OnceCell<RwLock<(u64, u64)>> = OnceCell::new();

/// 启动时调用：读 JSON（或建空）、安装全局单例。未调用则所有读写 no-op，侧栏会一直 0。
pub fn init_history(data_dir: &Path) {
    let _ = DATA_DIR.set(data_dir.to_path_buf());
    let mut h = load_inner(data_dir);
    // 跨日：启动时若磁盘里的 today 不是今天，先归档再重置。
    let today = TrafficHistory::today_local();
    if !h.today.is_empty() && h.today != today {
        push_or_update_day(&mut h.days, &h.today, h.today_up, h.today_down);
        trim_days(&mut h.days);
        h.today = today;
        h.today_up = 0;
        h.today_down = 0;
        let _ = save_inner(&h, data_dir);
    } else if h.today.is_empty() {
        h.today = today;
    }
    let _ = HISTORY.set(RwLock::new(h));
    let _ = LAST_SESSION.set(RwLock::new((0, 0)));
}

fn load_inner(data_dir: &Path) -> TrafficHistory {
    let p = data_dir.join(TRAFFIC_HISTORY_FILE);
    if p.exists() {
        if let Ok(s) = std::fs::read_to_string(&p) {
            if let Ok(h) = serde_json::from_str::<TrafficHistory>(&s) {
                return h;
            }
        }
    }
    TrafficHistory {
        today: TrafficHistory::today_local(),
        ..Default::default()
    }
}

fn save_inner(h: &TrafficHistory, data_dir: &Path) -> std::io::Result<()> {
    if data_dir.as_os_str().is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(data_dir)?;
    let p = data_dir.join(TRAFFIC_HISTORY_FILE);
    let s = serde_json::to_string_pretty(h).unwrap_or_default();
    std::fs::write(&p, s)
}

fn push_or_update_day(days: &mut Vec<DayTraffic>, date: &str, up: u64, down: u64) {
    if let Some(d) = days.iter_mut().find(|d| d.date == date) {
        if up > d.up { d.up = up; }
        if down > d.down { d.down = down; }
    } else {
        days.push(DayTraffic { date: date.to_string(), up, down });
    }
}

fn trim_days(days: &mut Vec<DayTraffic>) {
    days.sort_by(|a, b| b.date.cmp(&a.date));
    days.truncate(DAILY_HISTORY_DAYS);
}

/// 把会话累计的**增量**并入今日并落盘；同时处理跨日归档。
///
/// `state.status.traffic_up/down` 是进程内累计，不能直接 `+=`，否则每 30s 调用一次
/// 会把累计值反复叠加（侧栏「今日」会指数级膨胀）。这里用 LAST_SESSION 记上次采样点。
pub async fn record_tick(state: &AppState) {
    let Some(lock) = HISTORY.get() else { return; };
    let Some(last_lock) = LAST_SESSION.get() else { return; };
    let today = TrafficHistory::today_local();
    let (cur_up, cur_down) = {
        let st = state.status.read().await;
        (st.traffic_up, st.traffic_down)
    };
    let data_dir = state.data_dir.clone();

    let (du, dd) = {
        let mut last = last_lock.write().await;
        // 会话累计若被外部清零（理论上不应），按「从 0 重新计」处理。
        let du = if cur_up >= last.0 {
            cur_up - last.0
        } else {
            cur_up
        };
        let dd = if cur_down >= last.1 {
            cur_down - last.1
        } else {
            cur_down
        };
        *last = (cur_up, cur_down);
        (du, dd)
    };

    let mut h = lock.write().await;
    if h.today != today {
        if !h.today.is_empty() {
            let old_date = h.today.clone();
            let old_up = h.today_up;
            let old_down = h.today_down;
            push_or_update_day(&mut h.days, &old_date, old_up, old_down);
        }
        h.today = today;
        h.today_up = 0;
        h.today_down = 0;
    }
    h.today_up = h.today_up.saturating_add(du);
    h.today_down = h.today_down.saturating_add(dd);
    trim_days(&mut h.days);
    let _ = save_inner(&h, &data_dir);
}

/// 进程退出 / core stop 时强制落盘。
pub async fn flush() {
    let (Some(lock), Some(d)) = (HISTORY.get(), DATA_DIR.get()) else { return; };
    let h = lock.read().await;
    let _ = save_inner(&h, d);
}

/// 取最近 N 天（含今天），按日期升序；缺日补 0。
pub async fn last_n_days(n: usize) -> Vec<DayTraffic> {
    let Some(lock) = HISTORY.get() else { return vec![]; };
    let h = lock.read().await;
    let mut out: Vec<DayTraffic> = h.days.iter().cloned().collect();
    if !h.today.is_empty() {
        if let Some(d) = out.iter_mut().find(|d| d.date == h.today) {
            d.up = h.today_up;
            d.down = h.today_down;
        } else {
            out.push(DayTraffic { date: h.today.clone(), up: h.today_up, down: h.today_down });
        }
    }
    out.sort_by(|a, b| b.date.cmp(&a.date));
    out.truncate(n);
    out.reverse();
    out
}

/// 同步取当前 today（用于 StatusResp）。
pub async fn snapshot_today() -> (String, u64, u64) {
    let Some(lock) = HISTORY.get() else { return (TrafficHistory::today_local(), 0, 0); };
    let h = lock.read().await;
    (h.today.clone(), h.today_up, h.today_down)
}
