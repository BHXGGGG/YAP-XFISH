use super::*;
use crate::app::AppConfig;
use axum::{http::StatusCode, routing::get, Router};

#[tokio::test]
async fn reordered_shared_endpoints_preserve_ids_selection_rules_and_detours() {
    use crate::config::model::RouteRule;
    use std::sync::Arc;
    let raw = |names: &[&str]| serde_json::json!({"proxies":names.iter().map(|name| {
        serde_json::json!({"type":"http", "name":name, "server":"127.0.0.1", "port":8080})
    }).collect::<Vec<_>>()}).to_string();
    let body = Arc::new(tokio::sync::RwLock::new(raw(&["info", "A", "B"])));
    let response = body.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/", get(move || {
            let response = response.clone();
            async move { response.read().await.clone() }
        }))).await.unwrap();
    });
    let dir = std::env::temp_dir().join(format!("yap-subscription-order-test-{}", crate::app::gen_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = AppConfig { data_dir:dir.clone(), core_binary:dir.join("missing-mihomo.exe"), ..Default::default() };
    let mut old = parser::parse(&raw(&["info", "A", "B"])).unwrap();
    for (index, node) in old.iter_mut().enumerate() {
        node.id = stable_node_id("test", node);
        if index > 0 { node.id.push_str(&format!("-{index}")); }
        node.subscription_id = Some("test".into());
    }
    let front = old[1].id.clone();
    let exit = old[2].id.clone();
    old[1].detour = Some(String::new());
    old[2].detour = Some(front.clone());
    let profile = AppProfile { nodes:old, selected_node:Some(exit.clone()), rules:vec![RouteRule {
        id:"route".into(), name:"route".into(), r#type:"domain".into(), payload:"example.com".into(), outbound:exit.clone(),
    }], ..Default::default() };
    let state = AppState::new(cfg, profile, dir.clone());
    state.subscriptions.write().await.push(serde_json::from_value(serde_json::json!({"id":"test", "name":"test", "url":url})).unwrap());
    let result = async {
        *body.write().await = raw(&["B", "updated-info", "A", "B"]);
        update_subscription(&state, "test").await?;
        let saved = cfgmgr::load_profile(&dir)?;
        assert_eq!(saved.nodes[0].id, exit);
        assert_eq!(saved.nodes[2].id, front);
        assert_eq!(saved.nodes[0].detour.as_deref(), Some(front.as_str()));
        assert_eq!(saved.nodes[2].detour.as_deref(), Some(""));
        assert_eq!(saved.selected_node.as_deref(), Some(exit.as_str()));
        assert_eq!(saved.rules[0].outbound, exit);
        assert_eq!(saved.nodes.iter().map(|n| &n.id).collect::<std::collections::HashSet<_>>().len(), 4);
        let duplicate = saved.nodes[3].id.clone();
        *body.write().await = raw(&["A", "B", "B"]);
        update_subscription(&state, "test").await?;
        let saved = cfgmgr::load_profile(&dir)?;
        assert_eq!(saved.nodes[0].id, front);
        assert_eq!(saved.nodes[1].id, exit);
        assert_eq!(saved.nodes[2].id, duplicate);
        Ok::<_, AppError>(())
    }.await;
    server.abort();
    let _ = server.await;
    std::fs::remove_dir_all(&dir).unwrap();
    result.unwrap();
}

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
        .route("/partial", get(|| async { "ss://aes-256-gcm:p@host:8388#valid\nunsupported://p@host:443#invalid" }))
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
        ("/partial", "解析失败"),
        ("/valid", "保存配置失败"),
        ("/valid", "应用失败"),
    ] {
        state.subscriptions.write().await[0].url = format!("{base_url}{path}");
        let blocked = match expected_error {
            "保存配置失败" => Some(dir.join("profile.json")),
            "应用失败" => Some(dir.join("mihomo.yaml")),
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

#[tokio::test]
async fn subscription_commit_waits_for_edits_and_preserves_them_on_failure() {
    let dir = std::env::temp_dir().join(format!("yap-subscription-lock-test-{}", crate::app::gen_id()));
    std::fs::create_dir_all(dir.join("mihomo.yaml")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/", get(|| async {
            "ss://aes-256-gcm:p@127.0.0.1:8388#node"
        }))).await.unwrap();
    });
    let cfg = AppConfig { data_dir: dir.clone(), core_binary: dir.join("missing-mihomo.exe"), ..Default::default() };
    let state = std::sync::Arc::new(AppState::new(cfg, AppProfile::default(), dir.clone()));
    state.subscriptions.write().await.push(serde_json::from_value(serde_json::json!({"id":"test","name":"test","url":url})).unwrap());
    let mut events = state.event_tx.subscribe();
    let mutation = state.mutation.lock().await;
    let updating = state.clone();
    let update = tokio::spawn(async move { update_subscription(&updating, "test").await });
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while let Ok(event) = events.recv().await {
            if matches!(event, AppEvent::Subscription { progress: 60, .. }) { break; }
        }
    }).await.unwrap();
    assert!(state.profile.read().await.nodes.is_empty(), "downloads must wait for the commit lock");
    {
        let mut profile = state.profile.write().await;
        profile.rules.push(crate::config::model::RouteRule {
            id:"new-rule".into(), name:"new-rule".into(), r#type:"domain".into(), payload:"example.com".into(), outbound:"direct".into(),
        });
        cfgmgr::save_profile(&dir, &profile).unwrap();
    }
    drop(mutation);
    assert!(update.await.unwrap().is_err());
    assert!(state.profile.read().await.nodes.is_empty());
    assert_eq!(state.profile.read().await.rules[0].id, "new-rule");
    assert_eq!(cfgmgr::load_profile(&dir).unwrap().rules[0].id, "new-rule");
    server.abort();
    let _ = server.await;
    std::fs::remove_dir_all(dir).unwrap();
}
