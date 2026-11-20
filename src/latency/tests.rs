use super::*;
use crate::app::AppConfig;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::{mpsc, Semaphore};

struct Fixture {
    state: Arc<AppState>,
    dir: PathBuf,
}

impl Fixture {
    fn new(nodes: Vec<Node>) -> Self {
        let dir =
            std::env::temp_dir().join(format!("yap-xfish-latency-test-{}", crate::app::gen_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = AppConfig {
            data_dir: dir.clone(),
            core_binary: dir.join("missing-mihomo.exe"),
            ..Default::default()
        };
        let profile = AppProfile {
            nodes,
            ..Default::default()
        };
        Self {
            state: Arc::new(AppState::new(config, profile, dir.clone())),
            dir,
        }
    }

    async fn start_core(&self) {
        let binary = PathBuf::from(
            std::env::var_os("MIHOMO_TEST_BINARY").expect("MIHOMO_TEST_BINARY"),
        );
        assert!(binary.is_file());
        let api_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy_listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        {
            let mut cfg = self.state.config.write().await;
            cfg.core_binary = binary;
            cfg.clash_api_port = api_listener.local_addr().unwrap().port();
            cfg.proxy_port = proxy_listener.local_addr().unwrap().port();
        }
        drop(api_listener);
        drop(proxy_listener);
        let cfg = self.state.config.read().await.clone();
        let profile = self.state.visible_profile().await;
        self.state.core.start(&profile, &cfg).await.unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn node(id: &str) -> Node {
    Node {
        id: id.into(),
        name: id.into(),
        kind: "vless".into(),
        server: "127.0.0.1".into(),
        port: 1,
        ..Default::default()
    }
}

#[derive(Debug)]
struct RequestSeen {
    tag: String,
    timeout: String,
    url: String,
    authorization: String,
}

#[derive(Clone)]
struct MockState {
    calls: mpsc::UnboundedSender<RequestSeen>,
    gate: Arc<Semaphore>,
    active: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

struct MockApi {
    port: u16,
    state: MockState,
    calls: mpsc::UnboundedReceiver<RequestSeen>,
    server: tokio::task::JoinHandle<()>,
}

impl MockApi {
    async fn start() -> Self {
        let (calls, received) = mpsc::unbounded_channel();
        let state = MockState {
            calls,
            gate: Arc::new(Semaphore::new(0)),
            active: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
        };
        let router = Router::new()
            .route("/proxies/:tag/delay", get(mock_delay))
            .route("/generate_204", get(|| async { StatusCode::NO_CONTENT }))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self {
            port,
            state,
            calls: received,
            server,
        }
    }
}

impl Drop for MockApi {
    fn drop(&mut self) {
        self.state.gate.add_permits(1000);
        self.server.abort();
    }
}

async fn mock_delay(
    State(state): State<MockState>,
    Path(tag): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let active = state.active.fetch_add(1, Ordering::SeqCst) + 1;
    state.peak.fetch_max(active, Ordering::SeqCst);
    state
        .calls
        .send(RequestSeen {
            tag: tag.clone(),
            timeout: query.get("timeout").cloned().unwrap_or_default(),
            url: query.get("url").cloned().unwrap_or_default(),
            authorization: headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .into(),
        })
        .unwrap();
    if tag.starts_with("slow") {
        state.gate.acquire().await.unwrap().forget();
    }
    state.active.fetch_sub(1, Ordering::SeqCst);
    if tag.starts_with("fail") {
        StatusCode::GATEWAY_TIMEOUT.into_response()
    } else {
        Json(serde_json::json!({ "delay": 7 })).into_response()
    }
}

async fn next_latency(
    rx: &mut tokio::sync::broadcast::Receiver<AppEvent>,
) -> (String, Option<u32>) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let AppEvent::Latency { id, latency, .. } = rx.recv().await.unwrap() {
                return (id, latency);
            }
        }
    })
    .await
    .expect("已完成的节点不应等待其他节点超时后才推送")
}

#[tokio::test]
async fn fast_result_is_pushed_while_slow_nodes_are_still_queued() {
    let api = MockApi::start().await;
    let nodes = vec![
        node("slow-0"),
        node("fast"),
        node("slow-1"),
        node("slow-2"),
        node("slow-3"),
    ];
    let fixture = Fixture::new(nodes.clone());
    let context = ProbeContext::for_api(api.port, "", &nodes);
    let mut batch = prepare(&fixture.state, None).await.unwrap();
    let mut events = fixture.state.event_tx.subscribe();
    let state = fixture.state.clone();
    let task = tokio::spawn(async move {
        batch
            .run_probes(
                &state,
                &context,
                "https://example.test/check",
                5000,
                2,
                Instant::now(),
            )
            .await
    });

    assert_eq!(next_latency(&mut events).await, ("fast".into(), Some(7)));
    assert!(!task.is_finished(), "慢节点仍被测试服务器阻塞");
    assert!(api.state.peak.load(Ordering::SeqCst) <= 2);
    assert_eq!(fixture.state.profile.read().await.nodes[1].latency, Some(7));
    // 已完成节点可以单独重新测速，仍在等待的节点继续去重。
    assert_eq!(
        prepare(&fixture.state, Some(vec!["fast".into()]))
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        prepare(&fixture.state, Some(vec!["slow-0".into()]))
            .await
            .unwrap()
            .len(),
        0
    );

    api.state.gate.add_permits(nodes.len());
    let stats = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stats.completed, nodes.len());
    assert_eq!(stats.successful, nodes.len());
    assert!(fixture.state.latency_in_flight.lock().unwrap().is_empty());
}

#[tokio::test]
async fn configured_concurrency_above_sixteen_is_used() {
    let mut api = MockApi::start().await;
    let nodes: Vec<Node> = (0..40).map(|i| node(&format!("slow-{i}"))).collect();
    let fixture = Fixture::new(nodes.clone());
    let context = ProbeContext::for_api(api.port, "", &nodes);
    let mut batch = prepare(&fixture.state, None).await.unwrap();
    let state = fixture.state.clone();
    let task = tokio::spawn(async move {
        batch
            .run_probes(
                &state,
                &context,
                "https://example.test/check",
                5000,
                50,
                Instant::now(),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(3), async {
        for _ in 0..40 {
            api.calls.recv().await.unwrap();
        }
    })
    .await
    .expect("默认并发 50 应允许 40 个请求同时进入，不能限制为 16");
    assert_eq!(api.state.peak.load(Ordering::SeqCst), 40);
    api.state.gate.add_permits(40);
    assert_eq!(task.await.unwrap().completed, 40);
}

#[tokio::test]
async fn http_proxy_is_not_queued_behind_protocol_timeouts() {
    let api = MockApi::start().await;
    let mut http = node("http");
    http.kind = "http".into();
    http.port = api.port;
    let nodes = vec![node("slow-protocol"), http];
    let fixture = Fixture::new(nodes.clone());
    let context = ProbeContext::for_api(api.port, "", &nodes);
    let mut batch = prepare(&fixture.state, None).await.unwrap();
    let mut events = fixture.state.event_tx.subscribe();
    let state = fixture.state.clone();
    let task = tokio::spawn(async move {
        batch
            .run_probes(
                &state,
                &context,
                "http://probe.invalid/generate_204",
                5000,
                2,
                Instant::now(),
            )
            .await
    });
    let (id, latency) = next_latency(&mut events).await;
    assert_eq!(id, "http");
    assert!(latency.is_some());
    assert!(!task.is_finished());
    api.state.gate.add_permits(1);
    assert_eq!(task.await.unwrap().successful, 2);
}

#[tokio::test]
async fn selected_node_monitor_skips_when_core_is_stopped() {
    let fixture = Fixture::new(vec![node("selected")]);
    fixture.state.profile.write().await.selected_node = Some("selected".into());
    fixture.state.status.write().await.current_node = Some("selected".into());
    let mut events = fixture.state.event_tx.subscribe();

    assert!(super::probe_selected_node(&fixture.state).await.is_none());
    assert!(!fixture.state.core.is_running().await);
    assert!(events.try_recv().is_err());
}

#[test]
fn repeated_selected_node_failures_are_pushed_and_stopping_clears_the_alert() {
    let fixture = Fixture::new(vec![node("selected")]);
    let mut events = fixture.state.event_tx.subscribe();
    let mut last_node = None;

    for result in [Some(false), Some(false), None, None, Some(false)] {
        publish_selected_node_result(
            &fixture.state,
            &mut last_node,
            result.map(|reachable| ("selected".into(), reachable)),
        );
    }

    for expected in [Some(false), Some(false), None, Some(false)] {
        let AppEvent::SelectedNodeReachability { id, reachable } = events.try_recv().unwrap() else {
            panic!("应推送当前节点的连通状态");
        };
        assert_eq!(id, "selected");
        assert_eq!(reachable, expected);
    }
    assert!(events.try_recv().is_err());
}

#[test]
fn authenticated_and_tls_http_proxies_use_mihomo() {
    let mut http = node("http");
    http.kind = "http".into();
    let uses_core = |node: &Node| {
        let context = ProbeContext::for_api(1, "", std::slice::from_ref(node));
        matches!(
            context.method_for(node),
            super::probe::ProbeMethod::Clash(_, _)
        )
    };

    assert!(!uses_core(&http));
    http.username = Some("user".into());
    http.password = Some("pass".into());
    assert!(uses_core(&http));

    http.username = None;
    http.password = None;
    http.tls = Some(true);
    assert!(uses_core(&http));
}

#[test]
fn imported_http_and_socks_detours_use_mihomo_until_explicitly_cleared() {
    for kind in ["http", "socks5"] {
        for key in ["dialer-proxy", "detour"] {
            let mut target = node("chained");
            target.kind = kind.into();
            target.extra = Some(serde_json::json!({key:"front"}));
            let context = ProbeContext::for_api(1, "", std::slice::from_ref(&target));
            assert!(matches!(context.method_for(&target), super::probe::ProbeMethod::Clash(_, _)));
            target.detour = Some(String::new());
            assert!(matches!(context.method_for(&target), super::probe::ProbeMethod::Proxy));
        }
    }
}

#[tokio::test]
async fn scope_validation_and_duplicate_requests_preserve_visible_nodes() {
    let mut enabled = node("enabled");
    enabled.subscription_id = Some("sub-on".into());
    let mut disabled = node("disabled");
    disabled.subscription_id = Some("sub-off".into());
    let fixture = Fixture::new(vec![node("manual"), enabled, disabled]);
    *fixture.state.subscriptions.write().await = serde_json::from_value(serde_json::json!([
        { "id": "sub-on", "name": "on", "url": "https://example.test/on", "enabled": true },
        { "id": "sub-off", "name": "off", "url": "https://example.test/off", "enabled": false }
    ]))
    .unwrap();

    let all = prepare(&fixture.state, None).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(prepare(&fixture.state, None).await.unwrap().len(), 0);
    drop(all);
    let selected = prepare(
        &fixture.state,
        Some(vec!["enabled".into(), "enabled".into()]),
    )
    .await
    .unwrap();
    assert_eq!(selected.len(), 1);
    drop(selected);
    assert_eq!(
        prepare(&fixture.state, Some(vec![])).await.unwrap().len(),
        0
    );
    assert!(prepare(&fixture.state, Some(vec!["disabled".into()]))
        .await
        .is_err());
    assert!(prepare(&fixture.state, Some(vec!["unknown".into()]))
        .await
        .is_err());
    assert!(fixture.state.latency_in_flight.lock().unwrap().is_empty());
}

#[tokio::test]
async fn clash_request_preserves_auth_tag_url_and_timeout() {
    let mut api = MockApi::start().await;
    let node = node("fast /香港?+%");
    let context = ProbeContext::for_api(api.port, "local-test-secret", std::slice::from_ref(&node));
    let url = "https://example.test/check?q=a&b=c";
    assert_eq!(
        context.method_for(&node).measure(&node, url, 700).await,
        Some(7)
    );
    let request = api.calls.recv().await.unwrap();
    assert_eq!(request.tag, node.id);
    assert_eq!(request.timeout, "700");
    assert_eq!(request.url, url);
    assert_eq!(request.authorization, "Bearer local-test-secret");
}

#[tokio::test]
async fn failed_probe_emits_result_and_releases_node() {
    let api = MockApi::start().await;
    let nodes = vec![node("fail")];
    let fixture = Fixture::new(nodes.clone());
    let context = ProbeContext::for_api(api.port, "", &nodes);
    let mut batch = prepare(&fixture.state, None).await.unwrap();
    let mut events = fixture.state.event_tx.subscribe();
    let stats = batch
        .run_probes(
            &fixture.state,
            &context,
            "https://example.test/check",
            5000,
            1,
            Instant::now(),
        )
        .await;
    assert_eq!(next_latency(&mut events).await, ("fail".into(), None));
    assert_eq!(stats.completed, 1);
    assert_eq!(stats.successful, 0);
    assert_eq!(
        fixture.state.profile.read().await.nodes[0]
            .latency_status
            .as_deref(),
        Some("timeout")
    );
    assert!(fixture.state.latency_in_flight.lock().unwrap().is_empty());
}

#[tokio::test]
    #[ignore = "使用 MIHOMO_TEST_BINARY 指定本地 Mihomo，仅启动隔离核心，不启动应用或系统代理"]
async fn selected_node_monitor_does_not_block_manual_latency() {
    let mut api = MockApi::start().await;
    let mut target = node("selected");
    target.kind = "http".into();
    target.port = api.port;
    let fixture = Fixture::new(vec![target]);
    fixture.state.profile.write().await.selected_node = Some("selected".into());
    {
        let mut status = fixture.state.status.write().await;
        status.running = true;
        status.current_node = Some("selected".into());
    }
    fixture.state.config.write().await.latency_test_url =
        "http://probe.invalid/proxies/slow-monitor/delay".into();
    fixture.start_core().await;
    let state = fixture.state.clone();
    let probe = tokio::spawn(async move { probe_selected_node(&state).await });
    let request = tokio::time::timeout(Duration::from_secs(2), api.calls.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(request.tag, "slow-monitor");
    let batch = prepare(&fixture.state, None).await.unwrap();
    assert_eq!(batch.len(), 1, "后台探测不能让手动测速跳过当前节点");
    api.state.gate.add_permits(1);
    let result = tokio::time::timeout(Duration::from_secs(2), probe)
        .await
        .unwrap()
        .unwrap();
    fixture.state.core.stop().await.unwrap();
    assert_eq!(result, Some(("selected".into(), true)));
}

#[tokio::test]
#[ignore = "使用 MIHOMO_TEST_BINARY 指定本地 Mihomo，仅启动隔离核心，不启动应用或系统代理"]
async fn temporary_probe_includes_imported_front_and_uses_the_chain() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (calls, mut requests) = mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let calls = calls.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                while let Ok(byte) = stream.read_u8().await {
                    request.push(byte);
                    if !request.ends_with(b"\r\n\r\n") { continue; }
                    let text = String::from_utf8_lossy(&request).to_string();
                    let _ = calls.send(text.clone());
                    if text.starts_with("CONNECT ") {
                        if stream.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await.is_err() { return; }
                        request.clear();
                    } else {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n").await;
                        return;
                    }
                }
            });
        }
    });
    for (list, name, port_key, detour) in [
        ("proxies", "name", "port", "dialer-proxy"),
        ("outbounds", "tag", "server_port", "detour"),
    ] {
        let raw = serde_json::json!({list:[
            {"type":"http", name:"front", "server":"127.0.0.1", port_key:port},
            {"type":"http", name:"exit", "server":"127.0.0.2", port_key:23456, detour:"front"}
        ]});
        let mut nodes = crate::subscription::parser::parse(&raw.to_string()).unwrap();
        for node in &mut nodes { node.id = format!("{}-id", node.name); }
        let fixture = Fixture::new(nodes);
        fixture.state.config.write().await.core_binary = std::env::var_os("MIHOMO_TEST_BINARY").unwrap().into();
        let profile = fixture.state.profile.read().await.clone();
        let mut context = ProbeContext::prepare(&fixture.state, &profile, &profile.nodes[1..]).await;
        let started = context.temporary_path().is_some();
        let target = &profile.nodes[1];
        let measured = context.method_for(target).measure(target, "http://127.0.0.1:18080/probe", 2000).await;
        context.shutdown().await;
        assert!(started, "测速单个出口也必须加载导入的前置节点");
        assert!(measured.is_some(), "出口必须经前置访问测试地址");
        let mut connected = false;
        while let Ok(request) = requests.try_recv() {
            connected |= request.starts_with("CONNECT 127.0.0.2:23456 ");
        }
        assert!(connected, "前置应收到出口节点的 CONNECT 请求");
    }
    server.abort();
    let _ = server.await;
}

#[tokio::test]
    #[ignore = "使用 MIHOMO_TEST_BINARY 指定本地 Mihomo，仅启动隔离核心，不启动应用或系统代理"]
    async fn live_core_reuse_and_temporary_fallback() {
    let mut target = node("local-test-ss");
    target.kind = "shadowsocks".into();
    target.cipher = Some("aes-256-gcm".into());
    target.password = Some("test-only-password".into());
    let fixture = Fixture::new(vec![target]);
    fixture.start_core().await;
    let profile = fixture.state.visible_profile().await;

    let mut live = ProbeContext::prepare(&fixture.state, &profile, &profile.nodes).await;
    let live_reused = live.label().starts_with("复用");
    live.shutdown().await;
    let mut changed = profile.clone();
    changed.nodes[0].password = Some("updated-password".into());
    let mut fallback = ProbeContext::prepare(&fixture.state, &changed, &changed.nodes).await;
    let fallback_started = fallback.label().starts_with("临时");
    let temporary_path = fallback.temporary_path();
    fallback.shutdown().await;
    let still_running = fixture.state.core.is_running().await;
    fixture.state.core.stop().await.unwrap();

    assert!(live_reused, "已运行且配置一致的核心应直接复用");
    assert!(fallback_started, "凭据改变但尚未应用时应使用临时核心");
    assert!(still_running, "测速完成不能停止主核心");
    assert!(
        temporary_path.is_some_and(|path| !path.exists()),
        "临时配置应清理"
    );
}
