pub mod csrf;
pub mod episodes;
pub mod library;
pub mod public;
pub mod settings;
pub mod shows;
pub mod studio;

use crate::services::cover;
use crate::state::AppState;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path};
use axum::http::{header, StatusCode};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "assets/"]
struct Assets;

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
    let pages = Router::new()
        .route("/", get(studio::index))
        .route("/api/preview", post(studio::preview))
        .route("/episodes", post(studio::create))
        .route(
            "/episodes/{uuid}",
            get(episodes::detail)
                .patch(episodes::update_summary)
                .delete(episodes::delete),
        )
        .route("/episodes/{uuid}/delete", post(episodes::delete))
        .route("/episodes/{uuid}/retry", post(episodes::retry))
        .route("/episodes/{uuid}/cancel", post(episodes::cancel))
        .route("/episodes/{uuid}/reread", post(episodes::reread))
        .route("/episodes/{uuid}/publish", post(episodes::publish))
        .route("/api/episodes/{uuid}/status", get(episodes::status))
        .route("/api/queue", get(studio::queue))
        .route("/library", get(library::index))
        .route("/api/library/rows", get(library::rows))
        .route("/shows", get(shows::index).post(shows::create))
        .route(
            "/shows/{id}",
            get(shows::detail)
                .patch(shows::update)
                .delete(shows::delete),
        )
        .route("/shows/{id}/edit", post(shows::update))
        .route("/shows/{id}/delete", post(shows::delete))
        .route(
            "/shows/{id}/cover",
            post(shows::upload_cover)
                .layer(DefaultBodyLimit::max(cover::MAX_UPLOAD_BYTES + 1024 * 1024)),
        )
        .route("/shows/{id}/feed/toggle", post(shows::toggle_feed))
        .route("/shows/{id}/feed/rotate", post(shows::rotate_feed))
        .route("/shows/{id}/publish-ready", post(shows::publish_ready))
        .route("/settings", get(settings::index).post(settings::save))
        .layer(middleware::from_fn(csrf::csrf_middleware));
    let assets = Router::new().route("/assets/{*path}", get(asset));
    pages.merge(assets).with_state(state)
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

    const VALID: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    async fn test_state() -> AppState {
        let cfg = Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "DATA_DIR" => Some("/tmp/t2p-shell".into()),
            _ => None,
        })
        .unwrap();
        AppState {
            pool: crate::db::test_pool().await,
            cfg: Arc::new(cfg),
            notify: Arc::new(Notify::new()),
        }
    }

    async fn get(path: &str, cookie: Option<&str>) -> axum::response::Response {
        let mut request = Request::builder().uri(path);
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        admin_router(test_state().await)
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn cookie_token(cookie: &str) -> String {
        cookie
            .split("t2p_csrf=")
            .nth(1)
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string()
    }

    #[tokio::test]
    async fn shell_sets_hardened_csrf_cookie_and_embeds_token() {
        let response = get("/", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        for part in [
            "t2p_csrf=",
            "Path=/",
            "HttpOnly",
            "Secure",
            "SameSite=Strict",
        ] {
            assert!(cookie.contains(part), "missing {part} in {cookie}");
        }
        let token = cookie_token(&cookie);
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));

        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains(&format!("content=\"{token}\"")));
        assert!(html.contains(&format!("\"X-CSRF-Token\":\"{token}\"")));
        assert!(html.contains("Thư viện"));
    }

    #[tokio::test]
    async fn malformed_cookie_is_regenerated() {
        let response = get("/", Some("t2p_csrf=")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let token = cookie_token(&cookie);
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert!(!token.is_empty());
    }

    #[tokio::test]
    async fn valid_cookie_is_not_reissued() {
        let cookie = format!("t2p_csrf={VALID}");
        let response = get("/", Some(&cookie)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::SET_COOKIE).is_none());
    }

    #[tokio::test]
    async fn assets_do_not_receive_csrf_cookie() {
        let response = get("/assets/app.css", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::SET_COOKIE).is_none());
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "text/css"
        );
    }
}
