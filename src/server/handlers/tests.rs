use super::*;
use crate::config::{
    manager, mihomo,
    model::{AppProfile, Node},
};
use serde_json::json;
use std::path::PathBuf;
use crate::subscription::manager as submgr;

fn state(profile: AppProfile, binary: Option<PathBuf>) -> Arc<AppState> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("handlers-test-{}", crate::app::gen_id()));
    std::fs::create_dir_all(&dir).unwrap();
    manager::save_profile(&dir, &profile).unwrap();
    let port = || {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    };
    let config = AppConfig {
        core_binary: binary.unwrap_or_else(|| dir.join("missing-mihomo.exe")),
        data_dir: dir.clone(),
        proxy_port: port(),
        clash_api_port: port(),
        system_proxy: false,
        enable_tun: false,
        ..Default::default()
    };
    Arc::new(AppState::new(config, profile, dir))
}

#[tokio::test]
async fn imported_detours_are_visible_and_clearing_survives_subscription_refresh() {
    for (list, name, port, detour) in [
        ("proxies", "name", "port", "dialer-proxy"),
        ("outbounds", "tag", "server_port", "detour"),
    ] {
        let raw = json!({list:[
            {"type":"http", name:"front", "server":"127.0.0.1", port:8080},
            {"type":"http", name:"exit", "server":"127.0.0.2", port:8080, detour:"front"}
        ]}).to_string();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, axum::Router::new().route("/", axum::routing::get(move || {
                let raw = raw.clone();
                async move { raw }
            }))).await.unwrap();
        });
        let state = state(AppProfile::default(), None);
        state.subscriptions.write().await.push(serde_json::from_value(json!({
            "id":"chain", "name":"chain", "url":url
        })).unwrap());
        submgr::update_subscription(&state, "chain").await.unwrap();
        let Json(visible) = get_profile(State(state.clone())).await.unwrap();
        let front = visible.nodes[0].id.clone();
        let exit = visible.nodes[1].id.clone();
        assert_eq!(visible.nodes[1].detour.as_deref(), Some(front.as_str()));
        assert_eq!(
            mihomo::render_provider(&visible).unwrap()["proxies"][1]["dialer-proxy"],
            format!("YAP-NODE-{front}")
        );
        let _ = set_node_detour(
            State(state.clone()),
            Path(exit.clone()),
            Json(SetDetourReq { detour: None }),
        )
        .await
        .unwrap();
        let saved = manager::load_profile(&state.data_dir).unwrap();
        assert_eq!(saved.nodes[1].detour.as_deref(), Some(""));
        assert!(mihomo::render_provider(&saved).unwrap()["proxies"][1]
            .get("dialer-proxy")
            .is_none());
        let provider: serde_json::Value = serde_yaml::from_str(
            &std::fs::read_to_string(state.data_dir.join(mihomo::NODE_PROVIDER_PATH)).unwrap(),
        )
        .unwrap();
        assert!(provider["proxies"][1].get("dialer-proxy").is_none());

        *state.profile.write().await = saved;
        submgr::update_subscription(&state, "chain").await.unwrap();
        let saved = manager::load_profile(&state.data_dir).unwrap();
        assert_eq!(saved.nodes[1].detour.as_deref(), Some(""));
        assert!(mihomo::render_provider(&saved).unwrap()["proxies"][1].get("dialer-proxy").is_none());

        let _ = set_node_detour(State(state.clone()), Path(exit), Json(SetDetourReq { detour: Some(front.clone()) })).await.unwrap();
        submgr::update_subscription(&state, "chain").await.unwrap();
        assert_eq!(manager::load_profile(&state.data_dir).unwrap().nodes[1].detour.as_deref(), Some(front.as_str()));
        server.abort();
        let _ = server.await;
        std::fs::remove_dir_all(&state.data_dir).unwrap();
    }
}

#[tokio::test]
async fn batch_detours_validate_the_whole_path_before_saving() {
    let state = state(AppProfile {
        nodes: ["a", "b", "c"].into_iter().map(|id| Node {
            id: id.into(), name: id.into(), kind: "http".into(),
            server: "127.0.0.1".into(), port: 8080, ..Default::default()
        }).collect(),
        ..Default::default()
    }, None);
    let changes = |pairs: &[(&str, Option<&str>)]| pairs.iter().map(|(id, detour)| SetNodeDetourReq {
        id: id.to_string(), detour: detour.map(str::to_string),
    }).collect();
    apply_detours(&state, changes(&[("b", Some("a")), ("c", Some("b"))])).await.unwrap();
    let saved = manager::load_profile(&state.data_dir).unwrap();
    assert_eq!(saved.nodes[2].detour(), Some("b"));
    assert_eq!(mihomo::render_provider(&saved).unwrap()["proxies"][2]["dialer-proxy"], "YAP-NODE-b");
    // 最终路径为 b → a → c；逐条更新会先形成 a ↔ b 环。
    apply_detours(&state, changes(&[("a", Some("b")), ("b", None), ("c", Some("a"))])).await.unwrap();
    let before = serde_json::to_value(manager::load_profile(&state.data_dir).unwrap()).unwrap();
    for invalid in [
        changes(&[("a", None), ("c", Some("missing"))]),
        changes(&[("b", Some("c"))]),
        changes(&[("a", None), ("missing", None)]),
    ] {
        assert!(apply_detours(&state, invalid).await.is_err());
        assert_eq!(serde_json::to_value(&*state.profile.read().await).unwrap(), before);
        assert_eq!(serde_json::to_value(manager::load_profile(&state.data_dir).unwrap()).unwrap(), before);
    }
    state.profile.write().await.nodes[2].kind = "hysteria2".into();
    assert!(apply_detours(&state, changes(&[("c", Some("a"))])).await.unwrap_err().to_string().contains("UDP/QUIC"));
    let before_failure = serde_json::to_value(&*state.profile.read().await).unwrap();
    assert!(apply_detours(&state, changes(&[("c", None)])).await.unwrap_err().to_string().contains("password"));
    assert_eq!(serde_json::to_value(&*state.profile.read().await).unwrap(), before_failure);
    assert_eq!(serde_json::to_value(manager::load_profile(&state.data_dir).unwrap()).unwrap(), before_failure);
    state.profile.write().await.nodes[2].password = Some("test-password".into());
    apply_detours(&state, changes(&[("c", None)])).await.unwrap();
    assert!(state.profile.read().await.nodes[2].detour().is_none());
    std::fs::remove_dir_all(&state.data_dir).unwrap();
}

async fn check_subscription_selection(binary: Option<PathBuf>) {
    for delete in [false, true] {
        let nodes = ["a", "b"]
            .into_iter()
            .map(|id| Node {
                id: id.into(),
                name: id.into(),
                kind: "http".into(),
                server: "127.0.0.1".into(),
                port: 8080,
                subscription_id: Some(id.into()),
                ..Default::default()
            })
            .collect();
        let state = state(
            AppProfile {
                nodes,
                selected_node: Some("a".into()),
                rules: vec![RouteRule {
                    id: "rule".into(),
                    name: "rule".into(),
                    r#type: "domain".into(),
                    payload: "example.com".into(),
                    outbound: "a".into(),
                }],
                ..Default::default()
            },
            binary.clone(),
        );
        *state.subscriptions.write().await = serde_json::from_value(json!([
            {"id":"a","name":"A","url":"http://127.0.0.1/a","interval":"manual"},
            {"id":"b","name":"B","url":"http://127.0.0.1/b","interval":"manual"}
        ]))
        .unwrap();
        let cfg = state.config.read().await.clone();
        if binary.is_some() {
            state
                .core
                .start(&state.visible_profile().await, &cfg)
                .await
                .unwrap();
            state.status.write().await.running = true;
        }
        let mut events = state.event_tx.subscribe();
        let result = async {
            let error = if delete {
                delete_subscription(State(state.clone()), Path("a".into()))
                    .await
                    .unwrap_err()
            } else {
                update_subscription_settings(
                    State(state.clone()),
                    Path("a".into()),
                    Json(serde_json::from_value(json!({"enabled":false})).unwrap()),
                )
                .await
                .unwrap_err()
            };
            anyhow::ensure!(error.to_string().contains("不存在的节点"), "{error}");
            anyhow::ensure!(state
                .subscriptions
                .read()
                .await
                .iter()
                .any(|sub| sub.id == "a" && sub.enabled));
            anyhow::ensure!(
                manager::load_profile(&state.data_dir)?
                    .selected_node
                    .as_deref()
                    == Some("a")
            );
            anyhow::ensure!(state.profile.read().await.selected_node.as_deref() == Some("a"));
            anyhow::ensure!(state.status.read().await.current_node.as_deref() == Some("a"));
            while let Ok(event) = events.try_recv() {
                if let AppEvent::Status { current_node, .. } = event {
                    anyhow::ensure!(current_node.as_deref() == Some("a"));
                }
            }
            let api = crate::core::api::MihomoApi::new(cfg.clash_api_port, cfg.api_secret.clone())?;
            if binary.is_some() {
                anyhow::ensure!(api.get_json("/proxies/YAP-PROXY").await?["now"] == "a");
            }
            state.profile.write().await.rules.clear();
            if delete {
                let _ = delete_subscription(State(state.clone()), Path("a".into())).await?;
            } else {
                let _ = update_subscription_settings(
                    State(state.clone()),
                    Path("a".into()),
                    Json(serde_json::from_value(json!({"enabled":false}))?),
                )
                .await?;
            }
            anyhow::ensure!(
                manager::load_profile(&state.data_dir)?
                    .selected_node
                    .as_deref()
                    == Some("b")
            );
            anyhow::ensure!(state.status.read().await.current_node.as_deref() == Some("b"));
            if binary.is_some() {
                anyhow::ensure!(api.get_json("/proxies/YAP-PROXY").await?["now"] == "b");
            }
            Ok::<_, anyhow::Error>(())
        }
        .await;
        let stopped = state.core.stop().await;
        std::fs::remove_dir_all(&state.data_dir).unwrap();
        result.unwrap();
        stopped.unwrap();
    }
}

#[tokio::test]
async fn subscription_selection_changes_only_after_success() {
    check_subscription_selection(None).await;
}

#[tokio::test]
#[ignore = "requires MIHOMO_TEST_BINARY pointing to a Mihomo executable"]
async fn subscription_selection_matches_the_live_core() {
    check_subscription_selection(Some(std::env::var_os("MIHOMO_TEST_BINARY").unwrap().into()))
        .await;
}
