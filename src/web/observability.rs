use crate::error::AppResult;
use crate::services::observability::{self, Metrics, Window};
use crate::services::storage::{dir_size, format_bytes};
use crate::state::AppState;
use crate::web::csrf::CsrfToken;
use askama::Template;
use axum::extract::{Query, State};
use axum::response::Html;
use axum::Extension;
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct WindowQuery {
    pub window: Option<String>,
}

#[derive(Template)]
#[template(path = "observability.html")]
pub struct ObservabilityTemplate {
    csrf: String,
    window: &'static str,
    metrics: Metrics,
    total: String,
    audio: String,
    transcripts: String,
    shows: String,
    omnivoice_ok: bool,
    omnivoice_message: Option<String>,
    omnivoice_checked: String,
}

pub async fn index(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Query(query): Query<WindowQuery>,
) -> AppResult<Html<String>> {
    let window = Window::parse(query.window.as_deref());
    let now = crate::db::now_ts();
    let metrics = observability::load_metrics(&state.pool, window, now).await?;
    let health = observability::omnivoice_health(&state).await;
    let checked_secs = health.checked_at.elapsed().as_secs();
    let omnivoice_checked = if checked_secs < 60 {
        format!("{checked_secs} giây trước")
    } else {
        format!("{} phút trước", checked_secs / 60)
    };
    let total = dir_size(&state.cfg.data_dir).await;
    let audio = dir_size(&state.cfg.data_dir.join("audio")).await;
    let transcripts = dir_size(&state.cfg.data_dir.join("transcripts")).await;
    let shows = dir_size(&state.cfg.data_dir.join("shows")).await;
    Ok(Html(
        ObservabilityTemplate {
            csrf: token.0,
            window: window.label(),
            metrics,
            total: format_bytes(total),
            audio: format_bytes(audio),
            transcripts: format_bytes(transcripts),
            shows: format_bytes(shows),
            omnivoice_ok: health.ok,
            omnivoice_message: health.message,
            omnivoice_checked,
        }
        .render()?,
    ))
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::db::{self, episodes, shows};
    use crate::state::AppState;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use std::sync::Arc;
    use tokio::sync::Notify;
    use tower::ServiceExt;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    async fn context() -> AppState {
        let cfg = Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "OMNIVOICE_BASE_URL" => Some("http://127.0.0.1:1".into()),
            "DATA_DIR" => Some("/tmp/t2p-observability-web".into()),
            _ => None,
        })
        .unwrap();
        AppState {
            pool: db::test_pool().await,
            cfg: Arc::new(cfg),
            notify: Arc::new(Notify::new()),
            health_cache: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    async fn get(state: &AppState, uri: &str) -> axum::response::Response {
        crate::web::admin_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .header(header::COOKIE, format!("t2p_csrf={TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn body(response: axum::response::Response) -> String {
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn dashboard_renders_empty_state_and_window_links() {
        let state = context().await;
        let response = get(&state, "/observability").await;
        assert_eq!(response.status(), StatusCode::OK);
        let html = body(response).await;
        assert!(html.contains("Vận hành"));
        assert!(html.contains("Chưa có dữ liệu vận hành"));
        assert!(html.contains("window=7d"));
    }

    #[tokio::test]
    async fn dashboard_renders_failures_and_latency() {
        let state = context().await;
        let show = shows::create(
            &state.pool,
            shows::NewShow {
                title: "Show".into(),
                ..shows::NewShow::default()
            },
        )
        .await
        .unwrap();
        let ep = episodes::create(
            &state.pool,
            episodes::NewEpisode {
                show_id: show.id,
                video_id: "obs".into(),
                source_url: "https://www.youtube.com/watch?v=obs".into(),
                source_title: "Tập lỗi".into(),
                source_channel: "Kênh".into(),
                source_duration_s: None,
                source_thumbnail_url: None,
                source_upload_date: None,
                language: "vi".into(),
                target_minutes: 10,
                review_required: false,
            },
        )
        .await
        .unwrap();
        sqlx::query(
            "UPDATE stage_events SET exited_at = entered_at + 10, outcome = 'failed',
                error_code = 'tts_unavailable'
             WHERE episode_uuid = ?1 AND exited_at IS NULL",
        )
        .bind(&ep.uuid)
        .execute(&state.pool)
        .await
        .unwrap();

        let html = body(get(&state, "/observability?window=7d").await).await;
        assert!(html.contains("Tập lỗi"));
        assert!(html.contains("Dịch vụ đọc chưa sẵn sàng"));
        assert!(html.contains("Không kết nối được"));
        assert!(html.contains("window=24h"));
    }
}
