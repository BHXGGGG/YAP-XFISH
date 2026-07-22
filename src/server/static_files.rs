use axum::{
    extract::State,
    http::{header, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use rust_embed::RustEmbed;
use std::sync::Arc;

use crate::app::AppState;

#[derive(RustEmbed)]
#[folder = "webui/dist"]
struct Assets;

/// 静态资源处理器（内置 WebUI）。未知路径回退到 index.html 以支持 SPA。
pub async fn static_handler(uri: Uri, State(_state): State<Arc<AppState>>) -> Response {
    let path = uri.path().trim_start_matches('/').to_string();
    let path = if path.is_empty() {
        "index.html".to_string()
    } else {
        path
    };

    if let Some(file) = Assets::get(&path) {
        return build_response(&path, file.data.to_vec());
    }
    if let Some(file) = Assets::get("index.html") {
        return build_response("index.html", file.data.to_vec());
    }
    (StatusCode::NOT_FOUND, "not found").into_response()
}

fn build_response(path: &str, data: Vec<u8>) -> Response {
    let mut headers = header::HeaderMap::new();
    if let Ok(v) = HeaderValue::from_str(mime_for(path)) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    (headers, data).into_response()
}

fn mime_for(path: &str) -> &'static str {
    if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json; charset=utf-8"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else {
        "application/octet-stream"
    }
}
