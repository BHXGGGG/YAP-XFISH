use crate::app::AppState;
use crate::subscription::manager;
use crate::subscription::model::{Subscription, UpdateInterval, UpdateStatus};
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
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // 启动 30 秒后开始检查。
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

/// 失败时按重试时间调度，成功时以 last_updated 为周期基准。
fn next_due(sub: &Subscription, now: DateTime<Local>) -> DateTime<Local> {
    if sub.last_status == UpdateStatus::Failed && sub.interval != UpdateInterval::Manual {
        return sub
            .next_retry_at
            .as_ref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Local))
            .unwrap_or(now);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_updates_wait_for_retry_without_changing_the_success_schedule() {
        let now = DateTime::parse_from_rfc3339("2026-11-08T12:00:00+08:00")
            .unwrap()
            .with_timezone(&Local);
        let mut sub: Subscription = serde_json::from_value(serde_json::json!({
            "id": "test", "name": "test", "url": "http://127.0.0.1/",
            "last_updated": (now - chrono::Duration::hours(7)).to_rfc3339(),
            "last_status": "failed",
            "next_retry_at": (now + chrono::Duration::minutes(5)).to_rfc3339()
        }))
        .unwrap();

        let retry = now + chrono::Duration::minutes(5);
        assert_eq!(next_due(&sub, now), retry);
        assert!(now + chrono::Duration::seconds(30) < next_due(&sub, now));
        assert_eq!(next_due(&sub, retry), retry);
        assert!(retry + chrono::Duration::minutes(1) >= next_due(&sub, retry));

        sub.last_updated = None;
        assert_eq!(next_due(&sub, now), retry);
        sub.interval = UpdateInterval::Manual;
        assert!(next_due(&sub, retry) > retry);

        sub.interval = UpdateInterval::Every6Hours;
        sub.next_retry_at = None;
        assert_eq!(next_due(&sub, now), now);
        sub.last_status = UpdateStatus::Success;
        sub.last_updated = Some(now.to_rfc3339());
        assert_eq!(next_due(&sub, now), now + chrono::Duration::hours(6));
    }
}
