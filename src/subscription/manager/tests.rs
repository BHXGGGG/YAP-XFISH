use super::*;
use crate::app::AppConfig;
use axum::{http::StatusCode, routing::get, Router};

#[tokio::test]
async fn failures_preserve_nodes_and_retry_state_until_manual_success() {
    let dir = std::env::temp_dir().join(format!("yap-subscription-test-{}", crate::app::gen_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let config = AppConfig {
        data_dir: dir.clone(),
        core_binary: dir.join("missing-sing-box.exe"),
        ..Default::default()
    };
    let old_node = Node {
        id: "old-node".into(),
        name: "old-node".into(),
        kind: "http".into(),
        server: "127.0.0.1".into(),
        port: 1234,
        subscription_id: Some("test".into()),
        ..Default::default()
    };
    let profile = AppProfile {
        nodes: vec![old_node],
        selected_node: Some("old-node".into()),
        ..Default::default()
    };
    let state = AppState::new(config, profile, dir.clone());
    let sub: Subscription = serde_json::from_value(serde_json::json!({
        "id": "test", "name": "test", "url": "http://127.0.0.1/",
        "node_count": 1, "last_updated": "2026-11-01T12:00:00+08:00",
        "last_status": "success"
    }))
    .unwrap();
    assert!(sub.next_retry_at.is_none());
    state.subscriptions.write().await.push(sub.clone());
    let mut events = state.event_tx.subscribe();

    let router = Router::new()
        .route("/unavailable", get(|| async { StatusCode::SERVICE_UNAVAILABLE }))
        .route("/invalid", get(|| async { "not a subscription" }))
        .route("/valid", get(|| async {
            ([
                ("content-disposition", "attachment; filename=test"),
                ("subscription-userinfo", "total=1024"),
            ], r#"{"outbounds":[{"type":"http","tag":"new-node","server":"127.0.0.1","server_port":2345},{"type":"http","tag":"second-node","server":"127.0.0.1","server_port":3456}]}"#)
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    for (path, expected_error) in [
        ("/unavailable", "下载失败"),
        ("/invalid", "解析失败"),
        ("/valid", "保存配置失败"),
        ("/valid", "应用失败"),
    ] {
        state.subscriptions.write().await[0].url = format!("{base_url}{path}");
        let blocked = match expected_error {
            "保存配置失败" => Some(dir.join("profile.json")),
            "应用失败" => Some(dir.join("config.json")),
            _ => None,
        };
        if let Some(path) = &blocked {
            std::fs::create_dir(path).unwrap();
        }
        let before = chrono::Local::now();
        update_subscription(&state, "test").await.unwrap_err();
        let after = chrono::Local::now();
        if let Some(path) = &blocked {
            std::fs::remove_dir(path).unwrap();
        }

        let saved = load_subscriptions(&dir).unwrap().remove(0);
        assert_eq!(saved.last_status, UpdateStatus::Failed);
        assert!(
            saved.last_message.contains(expected_error),
            "{}",
            saved.last_message
        );
        assert_eq!(saved.node_count, 1);
        assert_eq!(saved.last_updated, sub.last_updated);
        let retry =
            chrono::DateTime::parse_from_rfc3339(saved.next_retry_at.as_ref().unwrap()).unwrap();
        assert!(retry >= before + chrono::Duration::minutes(5));
        assert!(retry <= after + chrono::Duration::minutes(5));
        assert_eq!(state.profile.read().await.nodes[0].id, "old-node");
        assert!(state.updating.lock().await.is_empty());

        let mut completed = false;
        while let Ok(event) = events.try_recv() {
            if let AppEvent::Subscription {
                status,
                node_count,
                last_updated,
                next_retry_at,
                ..
            } = event
            {
                if status == "failed" {
                    assert_eq!(node_count, Some(1));
                    assert_eq!(last_updated, sub.last_updated);
                    assert_eq!(next_retry_at, saved.next_retry_at);
                    completed = true;
                }
            }
        }
        assert!(completed);
    }

    update_subscription(&state, "test").await.unwrap();
    let saved = load_subscriptions(&dir).unwrap().remove(0);
    assert_eq!(saved.last_status, UpdateStatus::Success);
    assert_eq!(saved.node_count, 2);
    assert_ne!(saved.last_updated, sub.last_updated);
    assert!(saved.next_retry_at.is_none());
    assert_eq!(state.profile.read().await.nodes[0].port, 2345);
    let mut completed = false;
    while let Ok(event) = events.try_recv() {
        if let AppEvent::Subscription {
            status,
            node_count,
            last_updated,
            next_retry_at,
            ..
        } = event
        {
            if status == "success" {
                assert_eq!(node_count, Some(2));
                assert_eq!(last_updated, saved.last_updated);
                assert!(next_retry_at.is_none());
                completed = true;
            }
        }
    }
    assert!(completed);
    server.abort();
    std::fs::remove_dir_all(dir).unwrap();
}
