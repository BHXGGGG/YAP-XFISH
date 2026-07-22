pub mod handlers;
pub mod static_files;
pub mod ws;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use axum::{
    extract::Request,
    middleware::{self, Next},
    response::Response,
    routing::{delete, get, post},
    Router,
};

use crate::app::AppState;

/// 启动 HTTP 服务（静态 WebUI + REST API + WebSocket）。
pub async fn run(state: Arc<AppState>) -> anyhow::Result<()> {
    let web_port = state.config.read().await.web_port;
    let app = build_router(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], web_port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    println!("[proxy] WebUI 已启动: http://127.0.0.1:{}", web_port);
    axum::serve(listener, app).await?;
    Ok(())
}

/// Axum 中间件：每个 HTTP 请求 / 响应都 emit 一条带耗时、状态码的日志（来源 http）。
/// 帮助定位「前端发了一个请求为什么没生效」「请求 404 还是 500」之类问题。
async fn http_log_middleware(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let started = Instant::now();
    let resp = next.run(req).await;
    let elapsed = started.elapsed().as_millis();
    let status = resp.status().as_u16();
    let level = if status >= 500 {
        "error"
    } else if status >= 400 {
        "warn"
    } else {
        "info"
    };
    state.log_with(
        "http",
        level,
        format!("{} {} -> {} ({}ms)", method, uri.path(), status, elapsed),
    );
    resp
}

pub fn build_router(state: Arc<AppState>) -> Router {
    let api = Router::new()
        .route("/api/status", get(handlers::status))
        .route("/api/config", get(handlers::get_config))
        .route("/api/profile", get(handlers::get_profile))
        .route("/api/core/start", post(handlers::core_start))
        .route("/api/core/stop", post(handlers::core_stop))
        .route("/api/core/restart", post(handlers::core_restart))
        .route("/api/profile/select", post(handlers::select_node))
        .route("/api/profile/mode", post(handlers::set_mode))
        .route("/api/nodes/latency", post(handlers::test_all_latency))
        .route("/api/nodes/:id/latency", post(handlers::test_node_latency))
        .route("/api/subscriptions", get(handlers::list_subscriptions).post(handlers::add_subscription))
        .route("/api/subscriptions/update-all", post(handlers::update_all_subscriptions))
        .route("/api/subscriptions/:id", delete(handlers::delete_subscription).put(handlers::update_subscription_settings))
        .route("/api/subscriptions/:id/update", post(handlers::update_subscription_now))
        .route("/api/rules", get(handlers::list_rules).post(handlers::add_rule))
        .route("/api/rules/presets", get(handlers::list_rule_presets))
        .route("/api/rules/:id", delete(handlers::delete_rule))
        .route("/api/config", axum::routing::put(handlers::update_config))
        .route("/api/admin/elevate", post(handlers::admin_elevate))
        .route("/api/debug/memory", get(handlers::mem_debug))
        .route("/api/ws", get(ws::ws_handler))
        // fallback 必须在 with_state 之前：先有 Router<()> 再注入 state。
        .fallback(static_files::static_handler)
        .with_state(state.clone())
        // layer 必须在 with_state 之后：middleware 拿得到注入的 state。
        .layer(middleware::from_fn_with_state(state, http_log_middleware));
    api
}
