use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,
    #[error("forbidden")]
    Forbidden,
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error("upstream service failed: {0}")]
    Upstream(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for AppError {
    fn from(value: sqlx::Error) -> Self {
        AppError::Internal(anyhow::anyhow!(value))
    }
}

impl From<serde_json::Error> for AppError {
    fn from(value: serde_json::Error) -> Self {
        AppError::Internal(anyhow::anyhow!(value))
    }
}

impl From<std::io::Error> for AppError {
    fn from(value: std::io::Error) -> Self {
        AppError::Internal(anyhow::anyhow!(value))
    }
}

impl From<askama::Error> for AppError {
    fn from(value: askama::Error) -> Self {
        AppError::Internal(anyhow::anyhow!(value))
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::Forbidden => StatusCode::FORBIDDEN,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::Upstream(_) => StatusCode::BAD_GATEWAY,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = %self, "request failed");
        }
        let message = match status {
            StatusCode::INTERNAL_SERVER_ERROR => "Lỗi nội bộ; xem log.".to_string(),
            StatusCode::BAD_GATEWAY => "Dịch vụ đọc chưa sẵn sàng.".to_string(),
            _ => self.to_string(),
        };
        (status, Json(json!({ "ok": false, "error": message }))).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
