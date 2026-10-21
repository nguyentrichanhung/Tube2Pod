pub mod csrf;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use askama::Template;
use axum::body::Body;
use axum::extract::Path;
use axum::http::{header, StatusCode};
use axum::middleware;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Router};
use csrf::CsrfToken;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets/"]
struct Assets;

#[derive(Template)]
#[template(path = "base.html")]
struct ShellTemplate {
    csrf: String,
}

async fn shell(Extension(token): Extension<CsrfToken>) -> AppResult<Html<String>> {
    Ok(Html(
        ShellTemplate { csrf: token.0 }
            .render()
            .map_err(|e| AppError::Internal(e.into()))?,
    ))
}

async fn asset(Path(path): Path<String>) -> Response {
    match Assets::get(&path) {
        Some(file) => {
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            (
                [(header::CONTENT_TYPE, mime.as_ref())],
                Body::from(file.data.into_owned()),
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

pub fn admin_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(shell))
        .route("/assets/{*path}", get(asset))
        .layer(middleware::from_fn(csrf::csrf_middleware))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use axum::body::Body;
    use axum::http::{header, Request, StatusCode};
    use std::sync::Arc;
    use tokio::sync::Notify;
    use tower::ServiceExt;

    #[tokio::test]
    async fn shell_renders_nav_and_sets_csrf_cookie() {
        let cfg = Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "DATA_DIR" => Some("/tmp/t2p-shell".into()),
            _ => None,
        })
        .unwrap();
        let state = AppState {
            pool: crate::db::test_pool().await,
            cfg: Arc::new(cfg),
            notify: Arc::new(Notify::new()),
        };
        let response = admin_router(state)
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response.headers().get(header::SET_COOKIE).unwrap();
        assert!(cookie.to_str().unwrap().contains("t2p_csrf="));
    }
}
