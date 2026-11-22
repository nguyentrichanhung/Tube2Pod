use crate::config::Config;
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::Notify;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
    pub notify: Arc<Notify>,
}

impl AppState {
    pub fn effective_public_base_url(&self) -> crate::error::AppResult<Option<url::Url>> {
        Ok(self.cfg.public_base_url.clone())
    }
}
