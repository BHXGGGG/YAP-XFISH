use crate::app::AppState;
use crate::subscription::manager;
use crate::subscription::model::{Subscription, UpdateInterval};
use chrono::{DateTime, Local, TimeZone};
use std::str::FromStr;

use std::sync::Arc;
use std::time::Duration;

/// 启动后台定时调度任务。
///
/// 每 30 秒扫描一次所有「已启用且非手动」的订阅，按各自周期计算下次到期时间；
/// 到期且未在更新中的订阅会被触发更新（复用 manager::update_subscription，
/// 其内部自带并发去重）。调度失败不影响其他订阅，也不阻塞主服务。
pub fn start(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        // 首次立即检查一次（跳过 instant 0）
        tick.tick().await;
        loop {
            tick.tick().await;
            let now = Local::now();

            let due: Vec<String> = {
                let subs = state.subscriptions.read().await;
                subs.iter()
                    .filter(|s| s.enabled && s.interval != UpdateInterval::Manual)
                    .filter(|s| now >= next_due(s, now))
                    .map(|s| s.id.clone())
                    .collect()
            };

            for id in due {
                let _ = manager::update_subscription(&state, &id).await;
            }
        }
    });
}

/// 计算某订阅的「下次到期时间」。已更新过的以 last_updated 为基准，否则立即到期。
fn next_due(sub: &Subscription, now: DateTime<Local>) -> DateTime<Local> {
    let base = sub
        .last_updated
        .as_ref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Local))
        .unwrap_or(now);

    let dur = match &sub.interval {
        UpdateInterval::Manual => return far_future(now),
        UpdateInterval::Every30Min => Duration::from_secs(30 * 60),
        UpdateInterval::Hourly => Duration::from_secs(60 * 60),
        UpdateInterval::Every6Hours => Duration::from_secs(6 * 60 * 60),
        UpdateInterval::Every12Hours => Duration::from_secs(12 * 60 * 60),
        UpdateInterval::Daily => Duration::from_secs(24 * 60 * 60),
        UpdateInterval::Cron(expr) => return next_cron(expr).unwrap_or_else(|| far_future(now)),
    };
    base + chrono::Duration::from_std(dur).unwrap_or_else(|_| chrono::Duration::seconds(0))
}

/// 解析自定义 Cron 表达式，返回当前时刻之后的首次触发时刻。
fn next_cron(expr: &str) -> Option<DateTime<Local>> {
    let schedule = cron::Schedule::from_str(expr).ok()?;
    let next_utc = schedule.after(&chrono::Utc::now()).next()?;
    let secs = next_utc.timestamp();
    let nsec = next_utc.timestamp_subsec_nanos();
    Local.timestamp_opt(secs, nsec).single()
}

fn far_future(now: DateTime<Local>) -> DateTime<Local> {
    now + chrono::Duration::days(3650)
}
