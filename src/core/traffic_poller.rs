//! 会话累计流量：轮询主进程 Clash `GET /connections` 的 totals 并经 WS 广播。
//!
//! 说明：`GET /traffic` 在 sing-box 上是持续 NDJSON 流（B/s），普通 reqwest
//! `.json()` 会挂起/断连。因此使用 `/connections` 的 `uploadTotal`/
//! `downloadTotal` 作为会话累计字节，直接写入 RuntimeStatus（非速率累加）。

use crate::app::{AppEvent, AppState};
use crate::core::traffic::TrafficTotals;
use std::sync::Arc;
use std::time::Duration;

const INTERVAL: Duration = Duration::from_secs(1);

/// 启动长期后台任务（只应调用一次）。
pub fn start(state: Arc<AppState>) {
    tokio::spawn(async move {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_millis(800))
            .no_proxy()
            .build()
        {
            Ok(c) => c,
            Err(_) => return,
        };

        let mut ticker = tokio::time::interval(INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let running = state.status.read().await.running;
            if !running {
                continue;
            }
            let (port, secret) = {
                let cfg = state.config.read().await;
                (cfg.clash_api_port, cfg.api_secret.clone())
            };
            let Some(tot) = fetch_totals(&client, port, &secret).await else {
                continue;
            };
            {
                let mut st = state.status.write().await;
                st.traffic_up = tot.up;
                st.traffic_down = tot.down;
            }
            state.emit(AppEvent::Traffic {
                up: tot.up,
                down: tot.down,
            });
        }
    });
}

async fn fetch_totals(
    client: &reqwest::Client,
    port: u16,
    secret: &str,
) -> Option<TrafficTotals> {
    let url = format!("http://127.0.0.1:{port}/connections");
    let mut req = client.get(&url);
    if !secret.is_empty() {
        req = req.header("Authorization", format!("Bearer {secret}"));
    }
    let resp = req.send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<TrafficTotals>().await.ok()
}
