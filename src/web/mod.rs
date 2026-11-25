pub mod connection;
pub mod csrf;
pub mod episodes;
pub mod errors;
pub mod library;
pub mod observability;
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
            get(episodes::detail).delete(episodes::delete),
        )
        .route("/episodes/{uuid}/audio", get(episodes::audio))
        .route("/episodes/{uuid}/progress", get(episodes::progress))
        .route("/episodes/{uuid}/delete", post(episodes::delete))
        .route("/episodes/{uuid}/retry", post(episodes::retry))
        .route("/episodes/{uuid}/cancel", post(episodes::cancel))
        .route("/episodes/{uuid}/publish", post(episodes::publish))
        .route("/api/queue", get(studio::queue))
        .route("/library", get(library::index))
        .route("/library/bulk-delete", post(library::bulk_delete))
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
            get(shows::cover_preview)
                .post(shows::upload_cover)
                .layer(DefaultBodyLimit::max(cover::MAX_UPLOAD_BYTES + 1024 * 1024)),
        )
        .route("/shows/{id}/feed/toggle", post(shows::toggle_feed))
        .route("/shows/{id}/feed/rotate", post(shows::rotate_feed))
        .route("/shows/{id}/publish-ready", post(shows::publish_ready))
        .route("/settings", get(settings::index).post(settings::save))
        .route("/observability", get(observability::index))
        .route("/healthz", get(crate::health::healthz))
        .layer(middleware::from_fn(csrf::csrf_middleware))
        .layer(middleware::from_fn(errors::html_errors));
    let assets = Router::new().route("/assets/{*path}", get(asset));
    pages.merge(assets).with_state(state)
}

pub fn owner_router(state: AppState, origin: url::Url) -> Router {
    admin_router(state).layer(middleware::from_fn_with_state(origin, owner_origin))
}

pub fn owner_connection_router(
    state: AppState,
    identity: crate::device::identity::Identity,
    bind: std::net::SocketAddr,
    origin: url::Url,
) -> Router {
    let routes = Router::new()
        .route("/connection", get(connection::index))
        .route("/connection/pair-ticket", post(connection::ticket))
        .route("/connection/remote", post(connection::remote))
        .route(
            "/connection/pair-ticket/status",
            post(connection::ticket_status),
        )
        .route("/connection/devices/{id}/rename", post(connection::rename))
        .route("/connection/devices/{id}/revoke", post(connection::revoke))
        .layer(middleware::from_fn(csrf::csrf_middleware))
        .layer(axum::Extension(identity))
        .layer(axum::Extension(bind))
        .with_state(state.clone());
    admin_router(state)
        .merge(routes)
        .layer(middleware::from_fn_with_state(origin, owner_origin))
}

async fn owner_origin(
    axum::extract::State(origin): axum::extract::State<url::Url>,
    mut request: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    let expected_host = format!("localhost:{}", origin.port_or_known_default().unwrap_or(80));
    if request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        != Some(expected_host.as_str())
        || request.headers().get(header::ORIGIN).is_some_and(|value| {
            value.to_str().ok() != Some(origin.origin().ascii_serialization().as_str())
        })
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    request.extensions_mut().insert(origin);
    next.run(request).await
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
            "DATA_DIR" => Some("/tmp/t2p-shell".into()),
            _ => None,
        })
        .unwrap();
        AppState {
            pool: crate::db::test_pool().await,
            cfg: Arc::new(cfg),
            notify: Arc::new(Notify::new()),
            relay: Default::default(),
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

    async fn connection_html(state: AppState) -> String {
        let identity = crate::device::identity::Identity {
            server_id: "server".into(),
            server_key: "key".into(),
            cert: Default::default(),
            key: Default::default(),
        };
        let response = owner_connection_router(
            state,
            identity,
            "127.0.0.1:8403".parse().unwrap(),
            url::Url::parse("http://localhost:8402").unwrap(),
        )
        .oneshot(
            Request::builder()
                .uri("/connection")
                .header(header::HOST, "localhost:8402")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        String::from_utf8(
            axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn owner_connection_shows_direct_origin_without_relay() {
        let html = connection_html(test_state().await).await;
        assert!(html.contains("https://pod.example.ts.net:8443"));
        assert!(html.contains("Đã cấu hình"));
        assert!(html.contains("id=\"generate\""));
        assert!(html.contains("id=\"qr\""));
    }

    #[tokio::test]
    async fn owner_connection_without_origin_stays_local() {
        let mut state = test_state().await;
        Arc::make_mut(&mut state.cfg).public_base_url = None;
        let html = connection_html(state).await;
        assert!(html.contains("Chưa cấu hình địa chỉ chia sẻ"));
        assert!(!html.contains("podcast.example.com"));
        assert!(html.contains("id=\"generate\""));
    }

    #[tokio::test]
    async fn owner_connection_does_not_claim_internet_verified() {
        let html = connection_html(test_state().await).await;
        assert!(html.contains("Chưa kiểm tra truy cập từ Internet"));
        assert!(!html.contains("Đã xác minh"));
    }

    #[tokio::test]
    async fn owner_connection_relay_is_independent_of_direct_origin() {
        let mut state = test_state().await;
        Arc::make_mut(&mut state.cfg).public_base_url = None;
        let dir = tempfile::tempdir().unwrap();
        state.relay = Arc::new(
            crate::relay::client::Runtime::load(
                Some(url::Url::parse("https://relay.example.com").unwrap()),
                dir.path().join("relay.json"),
                "127.0.0.1:8401".parse().unwrap(),
                "127.0.0.1:8400".parse().unwrap(),
            )
            .await
            .unwrap(),
        );
        let html = connection_html(state).await;
        assert!(html.contains("Chưa cấu hình địa chỉ chia sẻ"));
        assert!(html.contains("Chuyển tiếp"));
        assert!(html.contains("action=\"/connection/remote\""));
    }

    #[tokio::test]
    async fn owner_connection_escapes_direct_origin() {
        let mut state = test_state().await;
        Arc::make_mut(&mut state.cfg).public_base_url =
            Some(url::Url::parse("https://example.com/?a=1&b=2").unwrap());
        let html = connection_html(state).await;
        assert!(html.contains("https://example.com/?a=1&amp;b=2"));
        assert!(!html.contains("https://example.com/?a=1&b=2"));
    }

    #[tokio::test]
    async fn owner_connection_page_lists_devices_without_credentials() {
        let state = test_state().await;
        let (ticket, _) = crate::db::devices::issue(&state.pool, 10).await.unwrap();
        let grant = crate::db::devices::redeem(&state.pool, &ticket, "Điện thoại", 11)
            .await
            .unwrap();
        let identity = crate::device::identity::Identity {
            server_id: "server".into(),
            server_key: "key".into(),
            cert: Default::default(),
            key: Default::default(),
        };
        let router = owner_connection_router(
            state.clone(),
            identity,
            "127.0.0.1:8403".parse().unwrap(),
            url::Url::parse("http://localhost:8402").unwrap(),
        );
        let response = router
            .oneshot(
                Request::builder()
                    .uri("/connection")
                    .header(header::HOST, "localhost:8402")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Điện thoại"));
        assert!(!html.contains(&grant.credential));
        assert_eq!(
            admin_router(state)
                .oneshot(
                    Request::builder()
                        .uri("/connection")
                        .body(Body::empty())
                        .unwrap()
                )
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn owner_ticket_status_tracks_redemption_and_requires_csrf() {
        let state = test_state().await;
        let now = crate::db::now_ts();
        let (ticket, _) = crate::db::devices::issue(&state.pool, now).await.unwrap();
        let identity = crate::device::identity::Identity {
            server_id: "server".into(),
            server_key: "key".into(),
            cert: Default::default(),
            key: Default::default(),
        };
        let router = owner_connection_router(
            state.clone(),
            identity,
            "127.0.0.1:8403".parse().unwrap(),
            url::Url::parse("http://localhost:8402").unwrap(),
        );
        for (token, expected, expected_status) in [
            ("bad", StatusCode::FORBIDDEN, ""),
            (VALID, StatusCode::OK, "pending"),
            (VALID, StatusCode::OK, "used"),
        ] {
            if expected_status == "used" {
                crate::db::devices::redeem(&state.pool, &ticket, "phone", now)
                    .await
                    .unwrap();
            }
            let response = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/connection/pair-ticket/status")
                        .header(header::HOST, "localhost:8402")
                        .header(header::COOKIE, format!("t2p_csrf={VALID}"))
                        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                        .body(Body::from(format!("_csrf={token}&ticket={ticket}")))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), expected);
            if expected == StatusCode::OK {
                let body = axum::body::to_bytes(response.into_body(), 1024)
                    .await
                    .unwrap();
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&body).unwrap()["status"],
                    expected_status
                );
            }
        }
    }

    #[tokio::test]
    async fn owner_origin_uses_exact_localhost_port() {
        let state = test_state().await;
        let router = owner_router(
            state.clone(),
            url::Url::parse("http://localhost:8402").unwrap(),
        );
        for (host, origin, expected) in [
            (
                "localhost:8402",
                "http://localhost:8402",
                StatusCode::SEE_OTHER,
            ),
            (
                "localhost:8403",
                "http://localhost:8402",
                StatusCode::FORBIDDEN,
            ),
            (
                "localhost:8402",
                "http://localhost:8403",
                StatusCode::FORBIDDEN,
            ),
        ] {
            let response = router.clone().oneshot(Request::builder().method("POST").uri("/settings")
                .header(header::HOST, host).header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header(header::COOKIE, format!("t2p_csrf={VALID}"))
                .body(Body::from(format!("_csrf={VALID}&owner_name=Owner&owner_email=owner@example.com&max_source_minutes=120&min_source_seconds=30"))).unwrap()).await.unwrap();
            assert_eq!(response.status(), expected);
        }
        let legacy = admin_router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/settings")
                    .header(header::HOST, "localhost:8402")
                    .header(header::ORIGIN, "http://localhost:8402")
                    .header(header::COOKIE, format!("t2p_csrf={VALID}"))
                    .header("x-csrf-token", VALID)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from(format!("_csrf={VALID}&owner_name=Owner&owner_email=owner@example.com&max_source_minutes=120&min_source_seconds=30")))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(legacy.status(), StatusCode::FORBIDDEN);
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
    async fn base_shell_exposes_navigation_and_skip_link() {
        let response = get("/", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("skip-link"));
        assert!(html.contains("class=\"app-shell\""));
        assert!(html.contains("class=\"topbar\""));
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
    async fn browser_form_error_renders_html_not_json() {
        let response = admin_router(test_state().await)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/episodes")
                    .header(header::ACCEPT, "text/html")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(Body::from("_csrf=bad&url=x&show_id=1&minutes=10"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Yêu cầu không hợp lệ"));
        assert!(!html.contains("\"ok\":false"));
    }

    #[tokio::test]
    async fn browser_form_redirect_is_not_rewritten_as_html_error() {
        let body = format!(
            "_csrf={VALID}&owner_name=Owner&owner_email=owner@example.com&auto_publish_default=on&max_source_minutes=120&min_source_seconds=30"
        );
        let response = admin_router(test_state().await)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/settings")
                    .header(header::ACCEPT, "text/html")
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, format!("t2p_csrf={VALID}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response
                .headers()
                .get(header::LOCATION)
                .and_then(|value| value.to_str().ok()),
            Some("/settings")
        );
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(!html.contains("Yêu cầu không hợp lệ"));
    }

    #[tokio::test]
    async fn admin_healthz_reports_database_status() {
        let state = test_state().await;
        let response = admin_router(state.clone())
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        state.pool.close().await;
        let response = admin_router(state)
            .oneshot(
                Request::builder()
                    .uri("/healthz")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
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

    #[tokio::test]
    async fn stylesheet_exposes_paper_ink_tokens() {
        let response = get("/assets/app.css", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let css = String::from_utf8(bytes.to_vec()).unwrap();
        assert!(css.contains("--ink:#1d2030"));
        assert!(css.contains("--paper:#fff"));
        assert!(css.contains(".app-shell"));
        assert!(css.contains(".cover-fallback"));
    }
    #[tokio::test]
    async fn retired_voice_and_review_routes_are_absent() {
        for path in ["/voices", "/voices/new", "/shows/1/voices/vi"] {
            assert_eq!(
                get(path, None).await.status(),
                StatusCode::NOT_FOUND,
                "{path}"
            );
        }
        for path in ["/episodes/example/approve", "/episodes/example/reread"] {
            let response = admin_router(test_state().await)
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(path)
                        .header(header::COOKIE, format!("t2p_csrf={VALID}"))
                        .header("x-csrf-token", VALID)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }
    }
}
