use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub async fn healthz(State(state): State<AppState>) -> Response {
    match sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(&state.pool)
        .await
    {
        Ok(_) => (StatusCode::OK, "ok").into_response(),
        Err(error) => {
            tracing::error!(%error, "health check failed");
            (StatusCode::SERVICE_UNAVAILABLE, "database unavailable").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::Notify;

    async fn state() -> crate::state::AppState {
        let cfg = crate::config::Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "DATA_DIR" => Some("/tmp/t2p-health".into()),
            _ => None,
        })
        .unwrap();
        crate::state::AppState {
            pool: crate::db::test_pool().await,
            cfg: Arc::new(cfg),
            notify: Arc::new(Notify::new()),
        }
    }

    #[tokio::test]
    async fn reports_ok_when_the_database_answers() {
        let state = state().await;
        let response = healthz(State(state)).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn reports_unavailable_when_the_database_is_closed() {
        let state = state().await;
        state.pool.close().await;
        let response = healthz(State(state)).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
