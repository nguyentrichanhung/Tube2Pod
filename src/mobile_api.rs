use crate::{
    db::{devices, episodes, shows},
    error::{AppError, AppResult},
    models::{Episode, Show},
    services::shows as show_service,
    state::AppState,
};
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path, Query, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone)]
pub struct MobileState {
    pub app: AppState,
    pub server_id: Arc<String>,
}
#[derive(Clone, Copy)]
pub enum RouteKind {
    Lan,
    Relay,
}

pub fn router(state: MobileState, kind: RouteKind) -> Router {
    let secured = Router::new()
        .route("/v1/connection", get(connection))
        .route("/v1/shows", get(list_shows).post(create_show))
        .route("/v1/shows/{id}", get(get_show).patch(update_show))
        .route(
            "/v1/shows/{id}/cover",
            get(cover).post(upload_cover).layer(DefaultBodyLimit::max(
                crate::services::cover::MAX_UPLOAD_BYTES + 1024 * 1024,
            )),
        )
        .route("/v1/shows/{id}/feed", put(feed))
        .route("/v1/shows/{id}/episodes", get(list_episodes))
        .route("/v1/episodes/{uuid}", get(get_episode))
        .route("/v1/episodes/{uuid}/publish", post(publish))
        .route("/v1/episodes/{uuid}/audio", get(audio))
        .layer(middleware::from_fn_with_state(state.clone(), authenticate));
    let routes = match kind {
        RouteKind::Lan => secured.route("/v1/pair", post(pair)),
        RouteKind::Relay => secured,
    };
    routes.with_state(state)
}
async fn authenticate(State(state): State<MobileState>, request: Request, next: Next) -> Response {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    if let Some(token) = token {
        if devices::authorized(&state.app.pool, token)
            .await
            .unwrap_or(false)
        {
            return next.run(request).await;
        }
    }
    StatusCode::UNAUTHORIZED.into_response()
}
#[derive(Deserialize)]
struct Pair {
    ticket: String,
    label: String,
}
async fn pair(State(state): State<MobileState>, Json(req): Json<Pair>) -> AppResult<Json<Value>> {
    let grant = devices::redeem(
        &state.app.pool,
        &req.ticket,
        &req.label,
        crate::db::now_ts(),
    )
    .await?;
    Ok(Json(
        json!({"device_id":grant.device_id, "credential":grant.credential, "server_id": state.server_id.as_str()}),
    ))
}
async fn connection(State(state): State<MobileState>) -> Json<Value> {
    Json(
        json!({"server_id":state.server_id.as_str(), "admin_uri":state.app.relay.enabled().then(|| state.app.relay.registration()).flatten().map(|r| r.admin_uri), "public_origin":state.app.effective_public_base_url().ok().flatten().map(|u| u.origin().ascii_serialization())}),
    )
}
async fn show_json(state: &AppState, show: Show) -> AppResult<Value> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM episodes WHERE show_id = ? AND status = 'ready'")
            .bind(show.id)
            .fetch_one(&state.pool)
            .await?;
    let public = state
        .effective_public_base_url()?
        .filter(|_| show.feed_enabled && show.feed_complete())
        .map(|base| {
            format!(
                "{}/feed/{}.xml",
                base.origin().ascii_serialization(),
                show.feed_token
            )
        });
    Ok(
        json!({"id":show.id,"title":show.title,"description":show.description,"author":show.author,"category":show.category,"language":show.language,"feed_enabled":show.feed_enabled,"public_feed_url":public,"cover_path":show.cover_path.as_ref().map(|_| format!("/v1/shows/{}/cover",show.id)),"ready_count":count}),
    )
}
async fn list_shows(State(s): State<MobileState>) -> AppResult<Json<Vec<Value>>> {
    let mut result = Vec::new();
    for show in shows::list(&s.app.pool).await? {
        result.push(show_json(&s.app, show).await?);
    }
    Ok(Json(result))
}
async fn get_show(State(s): State<MobileState>, Path(id): Path<i64>) -> AppResult<Json<Value>> {
    let show = shows::get(&s.app.pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(show_json(&s.app, show).await?))
}
#[derive(Default, Deserialize)]
struct ShowInput {
    title: Option<String>,
    author: Option<String>,
    description: Option<String>,
    category: Option<String>,
    language: Option<String>,
}
async fn create_show(
    State(s): State<MobileState>,
    Json(v): Json<ShowInput>,
) -> AppResult<Json<Value>> {
    let show = show_service::create(
        &s.app,
        shows::NewShow {
            title: v.title.unwrap_or_default(),
            author: v.author.unwrap_or_default(),
            description: v.description.unwrap_or_default(),
            category: v.category.unwrap_or_else(|| "Technology".into()),
            language: v.language.unwrap_or_else(|| "vi".into()),
            ..Default::default()
        },
    )
    .await?;
    Ok(Json(show_json(&s.app, show).await?))
}
async fn update_show(
    State(s): State<MobileState>,
    Path(id): Path<i64>,
    Json(v): Json<ShowInput>,
) -> AppResult<Json<Value>> {
    let show = show_service::update(
        &s.app,
        id,
        shows::ShowPatch {
            title: v.title,
            author: v.author,
            description: v.description,
            category: v.category,
            language: v.language,
            ..Default::default()
        },
    )
    .await?;
    Ok(Json(show_json(&s.app, show).await?))
}
async fn upload_cover(
    State(s): State<MobileState>,
    Path(id): Path<i64>,
    mut multipart: Multipart,
) -> AppResult<Json<Value>> {
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        if field.name() == Some("cover") {
            let bytes = field
                .bytes()
                .await
                .map_err(|e| AppError::BadRequest(e.to_string()))?;
            let show = show_service::upload_cover(&s.app, id, &bytes).await?;
            return Ok(Json(show_json(&s.app, show).await?));
        }
    }
    Err(AppError::BadRequest("Thiếu file ảnh bìa.".into()))
}
#[derive(Deserialize)]
struct Feed {
    enabled: bool,
}
async fn feed(
    State(s): State<MobileState>,
    Path(id): Path<i64>,
    Json(v): Json<Feed>,
) -> AppResult<Json<Value>> {
    let show = shows::set_feed_enabled(&s.app.pool, id, v.enabled).await?;
    Ok(Json(show_json(&s.app, show).await?))
}
fn episode_json(e: Episode) -> Value {
    json!({"id":e.uuid,"show_id":e.show_id,"title":e.source_title,"description":e.source_description,"language":e.language,"duration_seconds":e.audio_duration_s,"status":e.status,"published_at":e.published_at,"media_path":if e.audio_path.is_some() && e.status=="published" {Some(format!("/v1/episodes/{}/audio",e.uuid))} else {None}})
}
#[derive(Deserialize)]
struct Filter {
    status: String,
}
async fn list_episodes(
    State(s): State<MobileState>,
    Path(id): Path<i64>,
    Query(q): Query<Filter>,
) -> AppResult<Json<Vec<Value>>> {
    if !matches!(q.status.as_str(), "ready" | "published") {
        return Err(AppError::BadRequest("invalid status".into()));
    }
    shows::get(&s.app.pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let items: Vec<Episode> = sqlx::query_as("SELECT * FROM episodes WHERE show_id = ? AND status = ? ORDER BY created_at DESC, uuid DESC").bind(id).bind(q.status).fetch_all(&s.app.pool).await?;
    Ok(Json(items.into_iter().map(episode_json).collect()))
}
async fn get_episode(
    State(s): State<MobileState>,
    Path(id): Path<String>,
) -> AppResult<Json<Value>> {
    let e = episodes::get(&s.app.pool, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(episode_json(e)))
}
async fn publish(State(s): State<MobileState>, Path(id): Path<String>) -> AppResult<Json<Value>> {
    episodes::get(&s.app.pool, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    if !episodes::publish_one(&s.app.pool, &id).await? {
        return Err(AppError::Conflict(
            "Chỉ phát hành tập đang sẵn sàng.".into(),
        ));
    }
    get_episode(State(s), Path(id)).await
}
async fn audio(
    State(s): State<MobileState>,
    Path(id): Path<String>,
    req: Request<Body>,
) -> AppResult<Response> {
    let e = episodes::get(&s.app.pool, &id)
        .await?
        .ok_or(AppError::NotFound)?;
    if e.status != "published" {
        return Err(AppError::NotFound);
    }
    crate::web::public::serve_contained(&s.app, e.audio_path.ok_or(AppError::NotFound)?.into(), req)
        .await
}
async fn cover(
    State(s): State<MobileState>,
    Path(id): Path<i64>,
    req: Request<Body>,
) -> AppResult<Response> {
    let show = shows::get(&s.app.pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    crate::web::public::serve_contained(
        &s.app,
        show.cover_path.ok_or(AppError::NotFound)?.into(),
        req,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{header, Request},
    };
    use tower::ServiceExt;

    async fn fixture() -> (tempfile::TempDir, MobileState, String) {
        let dir = tempfile::tempdir().unwrap();
        let cfg = crate::config::Config::from_lookup(|key| {
            if key == "DATA_DIR" {
                Some(dir.path().display().to_string())
            } else {
                None
            }
        })
        .unwrap();
        let app = AppState {
            pool: crate::db::test_pool().await,
            cfg: Arc::new(cfg),
            notify: Arc::new(tokio::sync::Notify::new()),
            relay: Default::default(),
        };
        let (ticket, _) = devices::issue(&app.pool, 0).await.unwrap();
        let grant = devices::redeem(&app.pool, &ticket, "phone", 1)
            .await
            .unwrap();
        (
            dir,
            MobileState {
                app,
                server_id: Arc::new("server".into()),
            },
            grant.credential,
        )
    }
    fn request(method: &str, uri: &str, token: &str, body: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))
            .unwrap()
    }
    #[tokio::test]
    async fn share_url_requires_enabled_complete_feed() {
        let (dir, mut state, _) = fixture().await;
        state.app.cfg = Arc::new(
            crate::config::Config::from_lookup(|key| match key {
                "DATA_DIR" => Some(dir.path().display().to_string()),
                "PUBLIC_BASE_URL" => Some("https://feeds.example".into()),
                _ => None,
            })
            .unwrap(),
        );
        let mut show = shows::create(
            &state.app.pool,
            shows::NewShow {
                title: "Show".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(show_json(&state.app, show.clone()).await.unwrap()["public_feed_url"].is_null());
        show.feed_enabled = true;
        assert!(show_json(&state.app, show.clone()).await.unwrap()["public_feed_url"].is_null());
        show.description = "Description".into();
        show.category = "Technology".into();
        show.owner_name = "Owner".into();
        show.owner_email = "owner@example.com".into();
        show.cover_path = Some("cover.jpg".into());
        assert!(
            show_json(&state.app, show.clone()).await.unwrap()["public_feed_url"]
                .as_str()
                .unwrap()
                .starts_with("https://feeds.example/feed/")
        );
        show.feed_enabled = false;
        assert!(show_json(&state.app, show).await.unwrap()["public_feed_url"].is_null());
    }
    #[tokio::test]
    async fn pair_route_is_lan_only() {
        let (_dir, state, token) = fixture().await;
        let (ticket, _) = devices::issue(&state.app.pool, crate::db::now_ts())
            .await
            .unwrap();
        let body = json!({"ticket":ticket,"label":"phone"}).to_string();
        let relay = router(state.clone(), RouteKind::Relay)
            .oneshot(request("POST", "/v1/pair", &token, &body))
            .await
            .unwrap();
        assert_eq!(relay.status(), StatusCode::NOT_FOUND);
        let public = crate::web::public::public_router(state.app.clone())
            .oneshot(request("POST", "/v1/pair", &token, &body))
            .await
            .unwrap();
        assert_eq!(public.status(), StatusCode::NOT_FOUND);
        let lan = router(state.clone(), RouteKind::Lan)
            .oneshot(request("POST", "/v1/pair", "", &body))
            .await
            .unwrap();
        assert_eq!(lan.status(), StatusCode::OK);
        let duplicate = router(state, RouteKind::Lan)
            .oneshot(request("POST", "/v1/pair", "", &body))
            .await
            .unwrap();
        assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    }
    #[tokio::test]
    async fn revoked_device_cannot_access_api() {
        let (_dir, state, token) = fixture().await;
        let device_id: String = sqlx::query_scalar("SELECT id FROM devices WHERE credential = ?")
            .bind(&token)
            .fetch_one(&state.app.pool)
            .await
            .unwrap();
        let response = router(state.clone(), RouteKind::Lan)
            .oneshot(request("GET", "/v1/connection", &token, ""))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        devices::revoke(&state.app.pool, &device_id).await.unwrap();
        let response = router(state, RouteKind::Lan)
            .oneshot(request("GET", "/v1/connection", &token, ""))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    #[tokio::test]
    async fn audio_requires_published_grant_and_preserves_range() {
        let (dir, state, token) = fixture().await;
        let show = shows::create(
            &state.app.pool,
            shows::NewShow {
                title: "Show".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let ep = episodes::create(
            &state.app.pool,
            episodes::NewEpisode {
                show_id: show.id,
                video_id: "range000001".into(),
                source_url: "https://youtube.com/watch?v=range000001".into(),
                source_title: "Episode".into(),
                source_channel: String::new(),
                source_duration_s: None,
                source_thumbnail_url: None,
                source_upload_date: None,
                language: "vi".into(),
                source_audio_track: None,
                source_description: None,
            },
        )
        .await
        .unwrap();
        let path = dir.path().join("audio.mp3");
        tokio::fs::write(&path, b"0123456789").await.unwrap();
        sqlx::query("UPDATE episodes SET audio_path = ?, status = 'ready' WHERE uuid = ?")
            .bind(path.to_str().unwrap())
            .bind(&ep.uuid)
            .execute(&state.app.pool)
            .await
            .unwrap();
        let uri = format!("/v1/episodes/{}/audio", ep.uuid);
        let response = router(state.clone(), RouteKind::Lan)
            .oneshot(request("GET", &uri, &token, ""))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        episodes::publish_one(&state.app.pool, &ep.uuid)
            .await
            .unwrap();
        let response = router(state.clone(), RouteKind::Lan)
            .oneshot(request("GET", &uri, "bad", ""))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let req = Request::builder()
            .uri(&uri)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::RANGE, "bytes=2-5")
            .body(Body::empty())
            .unwrap();
        let response = router(state.clone(), RouteKind::Lan)
            .oneshot(req)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 100)
                .await
                .unwrap()
                .as_ref(),
            b"2345"
        );
        let response = router(state, RouteKind::Lan)
            .oneshot(request("HEAD", &uri, &token, ""))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    #[tokio::test]
    async fn mobile_and_web_share_show_validation() {
        let (_dir, state, token) = fixture().await;
        for body in [
            r#"{"title":"  ","language":"vi"}"#,
            r#"{"title":"Show","language":"xx"}"#,
        ] {
            let response = router(state.clone(), RouteKind::Lan)
                .oneshot(request("POST", "/v1/shows", &token, body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
        assert!(shows::list(&state.app.pool).await.unwrap().is_empty());
    }
    #[tokio::test]
    async fn show_created_by_mobile_appears_on_web() {
        let (_dir, state, token) = fixture().await;
        let response = router(state.clone(), RouteKind::Lan)
            .oneshot(request(
                "POST",
                "/v1/shows",
                &token,
                r#"{"title":"Mobile show","language":"en"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = crate::web::admin_router(state.app)
            .oneshot(
                Request::builder()
                    .uri("/shows")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("Mobile show"));
    }
    #[tokio::test]
    async fn publish_readback_keeps_uuid_and_timestamp() {
        let (_dir, state, token) = fixture().await;
        let show = shows::create(
            &state.app.pool,
            shows::NewShow {
                title: "Show".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let ep = episodes::create(
            &state.app.pool,
            episodes::NewEpisode {
                show_id: show.id,
                video_id: "abcdef01234".into(),
                source_url: "https://youtube.com/watch?v=abcdef01234".into(),
                source_title: "Episode".into(),
                source_channel: String::new(),
                source_duration_s: None,
                source_thumbnail_url: None,
                source_upload_date: None,
                language: "vi".into(),
                source_audio_track: None,
                source_description: None,
            },
        )
        .await
        .unwrap();
        episodes::fixture_finish(
            &state.app.pool,
            &ep.uuid,
            crate::models::stage::READY,
            crate::models::status::READY,
            false,
        )
        .await
        .unwrap();
        let uri = format!("/v1/episodes/{}/publish", ep.uuid);
        let response = router(state.clone(), RouteKind::Lan)
            .oneshot(request("POST", &uri, &token, "{}"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let first: Value = serde_json::from_slice(&bytes).unwrap();
        let response = router(state.clone(), RouteKind::Lan)
            .oneshot(request(
                "GET",
                &format!("/v1/episodes/{}", ep.uuid),
                &token,
                "",
            ))
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 10000)
            .await
            .unwrap();
        let second: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(first["id"], ep.uuid);
        assert_eq!(first["status"], "published");
        assert!(first["published_at"].is_number());
        assert_eq!(first, second);
        assert!(
            !shows::get(&state.app.pool, show.id)
                .await
                .unwrap()
                .unwrap()
                .feed_enabled
        );
    }
}
