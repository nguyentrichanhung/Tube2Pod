pub mod fetch;
pub mod summarize;
pub mod voice;

use crate::config::Config;
use crate::error::AppError;
use sqlx::SqlitePool;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct PipelineError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
}

impl PipelineError {
    pub fn new(code: &'static str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }

    pub fn from_app(error: AppError, code: &'static str) -> Self {
        let retryable = !matches!(error, AppError::BadRequest(_) | AppError::NotFound);
        Self::new(code, error.to_string(), retryable)
    }
}

#[derive(Clone)]
pub struct Pipeline {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
}

impl Pipeline {
    pub fn new(pool: SqlitePool, cfg: Arc<Config>) -> Self {
        Self { pool, cfg }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_app_marks_client_errors_as_non_retryable() {
        let error = PipelineError::from_app(AppError::BadRequest("bad url".into()), "invalid_url");
        assert_eq!(error.code, "invalid_url");
        assert_eq!(error.message, "bad url");
        assert!(!error.retryable);

        let missing = PipelineError::from_app(AppError::NotFound, "internal");
        assert!(!missing.retryable);
    }

    #[test]
    fn from_app_marks_upstream_and_internal_errors_as_retryable() {
        let upstream =
            PipelineError::from_app(AppError::Upstream("timeout".into()), "source_unavailable");
        assert!(upstream.retryable);

        let internal = PipelineError::from_app(
            AppError::Internal(anyhow::anyhow!("database busy")),
            "internal",
        );
        assert!(internal.retryable);
    }
}
