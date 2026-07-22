use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

/// 统一的后端错误类型。包裹 anyhow::Error，并实现 std::error::Error
/// 以便能在返回 anyhow::Result 的入口（main）中通过 `?` 直接透传。
#[derive(Debug)]
pub struct AppError(pub anyhow::Error);

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError(e)
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError(e.into())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError(e.into())
    }
}

impl From<serde_yaml::Error> for AppError {
    fn from(e: serde_yaml::Error) -> Self {
        AppError(e.into())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(e: reqwest::Error) -> Self {
        AppError(e.into())
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for AppError {}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = Json(serde_json::json!({ "error": self.0.to_string() }));
        (StatusCode::INTERNAL_SERVER_ERROR, body).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
