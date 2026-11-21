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
