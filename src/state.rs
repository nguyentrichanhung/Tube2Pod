use crate::config::Config;
use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, Notify};

#[derive(Clone, Debug)]
pub struct CachedHealth {
    pub checked_at: Instant,
    pub ok: bool,
    pub message: Option<String>,
}

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
    pub notify: Arc<Notify>,
    pub health_cache: Arc<Mutex<Option<CachedHealth>>>,
}
