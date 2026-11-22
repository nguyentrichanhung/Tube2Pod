use crate::config::Config;
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::Notify;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
    pub notify: Arc<Notify>,
    pub relay: Arc<crate::relay::client::Runtime>,
}

impl AppState {
    pub fn effective_public_base_url(&self) -> crate::error::AppResult<Option<url::Url>> {
        Ok(self.cfg.public_base_url.clone().or_else(|| {
            self.relay
                .enabled()
                .then(|| self.relay.registration())
                .flatten()
                .and_then(|r| url::Url::parse(&r.public_origin).ok())
        }))
    }
}
