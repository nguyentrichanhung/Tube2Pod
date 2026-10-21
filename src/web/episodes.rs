use crate::db::{episodes, shows};
use crate::error::{AppError, AppResult};
use crate::models::{sha256_hex, stage, status, Episode, Show};
use crate::services::omnivoice::OmnivoiceClient;
use crate::state::AppState;
use crate::web::csrf::{self, CsrfToken};
use crate::web::shows::purge_episode_files;
use askama::Template;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Json, Redirect, Response};
use axum::Extension;
use axum::Form;
use serde::Deserialize;
use serde_json::json;

#[derive(Template)]
#[template(path = "episode.html")]
pub struct EpisodeTemplate {
    csrf: String,
    episode: Episode,
    show: Show,
    feed_url: String,
    audio_url: Option<String>,
    transcript: String,
    duplicate: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct DetailQuery {
    pub duplicate: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SummaryForm {
    pub _csrf: String,
    pub summary_text: String,
}

#[derive(Debug, Deserialize)]
pub struct CsrfForm {
    pub _csrf: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct CsrfQuery {
    pub _csrf: Option<String>,
}

pub async fn detail(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Path(uuid): Path<String>,
    Query(query): Query<DetailQuery>,
) -> AppResult<Html<String>> {
    let episode = episodes::get(&state.pool, &uuid)
        .await?
        .ok_or(AppError::NotFound)?;
    let show = shows::get(&state.pool, episode.show_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let feed_url = format!("{}/feed/{}.xml", state.cfg.public_base_url, show.feed_token);
    let audio_url = episode.audio_path.as_ref().map(|_| {
        format!(
            "{}/audio/{}/{}-{}.mp3",
            state.cfg.public_base_url, show.feed_token, episode.uuid, episode.audio_version
        )
    });
    let transcript = read_transcript(&episode).await;
    Ok(Html(
        EpisodeTemplate {
            csrf: token.0,
            episode,
            show,
            feed_url,
            audio_url,
            transcript,
            duplicate: query.duplicate.as_deref() == Some("1"),
        }
        .render()?,
    ))
}

pub async fn update_summary(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Path(uuid): Path<String>,
    Form(form): Form<SummaryForm>,
) -> AppResult<Response> {
    csrf::verify(&token, &headers, &form._csrf)?;
    let text = validate_summary(&form.summary_text)?;
    let hash = sha256_hex(&text);
    let episode = episodes::get(&state.pool, &uuid)
        .await?
        .ok_or(AppError::NotFound)?;
    if matches!(episode.status.as_str(), status::QUEUED | status::RUNNING) {
        return Err(AppError::Conflict("Không sửa được tập đang xử lý.".into()));
    }
    episodes::set_summary_edit(&state.pool, &uuid, &text, &hash).await?;
    Ok(redirect_response(&headers, &format!("/episodes/{uuid}")))
}

pub async fn reread(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Path(uuid): Path<String>,
    Form(form): Form<CsrfForm>,
) -> AppResult<Response> {
    csrf::verify(&token, &headers, &form._csrf)?;
    let episode = episodes::get(&state.pool, &uuid)
        .await?
        .ok_or(AppError::NotFound)?;
    if episode.summary().trim().is_empty() {
        return Err(AppError::BadRequest(
            "Cần có bản tóm tắt trước khi đọc lại.".into(),
        ));
    }
    if !matches!(
        episode.status.as_str(),
        status::PUBLISHED | status::READY | status::FAILED
    ) {
        return Err(AppError::Conflict(
            "Tập đang xử lý; chưa thể đọc lại.".into(),
        ));
    }
    episodes::reset_for_reread(&state.pool, &uuid).await?;
    state.notify.notify_one();
    Ok(redirect_response(&headers, &format!("/episodes/{uuid}")))
}

pub async fn retry(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Path(uuid): Path<String>,
    Form(form): Form<CsrfForm>,
) -> AppResult<Response> {
    csrf::verify(&token, &headers, &form._csrf)?;
    let episode = episodes::get(&state.pool, &uuid)
        .await?
        .ok_or(AppError::NotFound)?;
    if !matches!(episode.status.as_str(), status::FAILED | status::CANCELLED) {
        return Err(AppError::Conflict(
            "Chỉ thử lại được tập đã lỗi hoặc đã huỷ.".into(),
        ));
    }
    episodes::retry(&state.pool, &uuid).await?;
    state.notify.notify_one();
    Ok(redirect_response(&headers, &format!("/episodes/{uuid}")))
}

pub async fn cancel(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Path(uuid): Path<String>,
    Form(form): Form<CsrfForm>,
) -> AppResult<Response> {
    csrf::verify(&token, &headers, &form._csrf)?;
    let episode = episodes::get(&state.pool, &uuid)
        .await?
        .ok_or(AppError::NotFound)?;
    if !matches!(episode.status.as_str(), status::QUEUED | status::RUNNING) {
        return Err(AppError::Conflict("Chỉ huỷ được tập đang xử lý.".into()));
    }
    if let Some(take_id) = episode.active_take_id {
        let client = OmnivoiceClient::new(&state.cfg.omnivoice);
        let _ = client.delete_speech(take_id).await;
    }
    episodes::cancel(&state.pool, &uuid).await?;
    state.notify.notify_one();
    Ok(redirect_response(&headers, &format!("/episodes/{uuid}")))
}

pub async fn publish(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Path(uuid): Path<String>,
    Form(form): Form<CsrfForm>,
) -> AppResult<Response> {
    csrf::verify(&token, &headers, &form._csrf)?;
    let episode = episodes::get(&state.pool, &uuid)
        .await?
        .ok_or(AppError::NotFound)?;
    if episode.status != status::READY {
        return Err(AppError::Conflict(
            "Chỉ phát hành tập đang sẵn sàng.".into(),
        ));
    }
    episodes::queue_at_stage(&state.pool, &uuid, stage::PUBLISHING).await?;
    state.notify.notify_one();
    Ok(redirect_response(&headers, &format!("/episodes/{uuid}")))
}

pub async fn delete(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Path(uuid): Path<String>,
    Query(query): Query<CsrfQuery>,
    body: Bytes,
) -> AppResult<Response> {
    let form_token = form_value(&body, "_csrf");
    let provided = form_token
        .as_deref()
        .filter(|value| !value.is_empty())
        .or(query._csrf.as_deref())
        .unwrap_or("");
    csrf::verify(&token, &headers, provided)?;
    let episode = episodes::get(&state.pool, &uuid)
        .await?
        .ok_or(AppError::NotFound)?;
    if let Some(take_id) = episode.active_take_id {
        let client = OmnivoiceClient::new(&state.cfg.omnivoice);
        let _ = client.delete_speech(take_id).await;
    }
    let _ = episodes::cancel(&state.pool, &uuid).await;
    state.notify.notify_one();
    purge_episode_files(&state.cfg.data_dir, &episode).await;
    episodes::delete(&state.pool, &uuid).await?;
    Ok(redirect_response(&headers, "/library"))
}

pub async fn status(
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> AppResult<impl IntoResponse> {
    let episode = episodes::get(&state.pool, &uuid)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(json!({
        "ok": true,
        "stage": episode.stage,
        "status": episode.status,
        "progress": episode.progress_json,
        "error_code": episode.error_code,
        "error_message": episode.error_message,
        "audio_needs_reread": episode.audio_needs_reread(),
    })))
}

fn validate_summary(text: &str) -> AppResult<String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(AppError::BadRequest(
            "Bản tóm tắt không được để trống.".into(),
        ));
    }
    Ok(text.to_string())
}

async fn read_transcript(episode: &Episode) -> String {
    let Some(path) = &episode.transcript_path else {
        return String::new();
    };
    match tokio::fs::read_to_string(path).await {
        Ok(text) => text.chars().take(200_000).collect(),
        Err(_) => String::new(),
    }
}

fn form_value(body: &[u8], key: &str) -> Option<String> {
    url::form_urlencoded::parse(body)
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
}

fn is_htmx(headers: &HeaderMap) -> bool {
    headers
        .get("hx-request")
        .and_then(|value| value.to_str().ok())
        == Some("true")
}

fn redirect_response(headers: &HeaderMap, location: &str) -> Response {
    if is_htmx(headers) {
        let mut response = StatusCode::OK.into_response();
        let value =
            HeaderValue::from_str(location).unwrap_or_else(|_| HeaderValue::from_static("/"));
        response.headers_mut().insert("hx-redirect", value);
        response
    } else {
        Redirect::to(location).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::{episodes, shows};
    use crate::models::{stage, status, Episode, Show};
    use crate::state::AppState;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Method, Request, StatusCode};
    use std::sync::Arc;
    use tokio::sync::Notify;
    use tower::ServiceExt;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    struct Ctx {
        state: AppState,
        _dir: tempfile::TempDir,
    }

    async fn ctx() -> Ctx {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let cfg = Config::from_lookup(move |key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "OMNIVOICE_BASE_URL" => Some("http://127.0.0.1:1".into()),
            "DATA_DIR" => Some(path.display().to_string()),
            _ => None,
        })
        .unwrap();
        Ctx {
            state: AppState {
                pool: crate::db::test_pool().await,
                cfg: Arc::new(cfg),
                notify: Arc::new(Notify::new()),
            },
            _dir: dir,
        }
    }

    async fn create_show(context: &Ctx, title: &str) -> Show {
        shows::create(
            &context.state.pool,
            shows::NewShow {
                title: title.into(),
                description: "Mô tả".into(),
                owner_name: "Owner".into(),
                owner_email: "owner@example.com".into(),
                ..shows::NewShow::default()
            },
        )
        .await
        .unwrap()
    }

    async fn create_episode(context: &Ctx, show_id: i64, title: &str) -> Episode {
        episodes::create(
            &context.state.pool,
            episodes::NewEpisode {
                show_id,
                video_id: format!("video-{}", uuid::Uuid::new_v4()),
                source_url: "https://www.youtube.com/watch?v=video".into(),
                source_title: title.into(),
                source_channel: "Channel".into(),
                source_duration_s: Some(600),
                source_thumbnail_url: None,
                source_upload_date: None,
                language: "vi".into(),
                target_minutes: 10,
            },
        )
        .await
        .unwrap()
    }

    async fn set_status(context: &Ctx, uuid: &str, new_status: &str, new_stage: &str) {
        sqlx::query("UPDATE episodes SET status = ?1, stage = ?2 WHERE uuid = ?3")
            .bind(new_status)
            .bind(new_stage)
            .bind(uuid)
            .execute(&context.state.pool)
            .await
            .unwrap();
    }

    fn form(pairs: &[(&str, &str)]) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in pairs {
            serializer.append_pair(key, value);
        }
        serializer.finish()
    }

    async fn send(
        context: &Ctx,
        method: Method,
        path: &str,
        body: Option<Vec<u8>>,
        headers: &[(&str, &str)],
    ) -> axum::response::Response {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::COOKIE, format!("t2p_csrf={TOKEN}"));
        if body.is_some() {
            request = request.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        }
        for (key, value) in headers {
            request = request.header(*key, *value);
        }
        crate::web::admin_router(context.state.clone())
            .oneshot(request.body(Body::from(body.unwrap_or_default())).unwrap())
            .await
            .unwrap()
    }

    async fn get(context: &Ctx, path: &str) -> axum::response::Response {
        send(context, Method::GET, path, None, &[]).await
    }

    async fn body(response: axum::response::Response) -> String {
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn csrf_form() -> String {
        form(&[("_csrf", TOKEN)])
    }

    async fn patch_summary(context: &Ctx, uuid: &str, text: &str) -> axum::response::Response {
        send(
            context,
            Method::PATCH,
            &format!("/episodes/{uuid}"),
            Some(form(&[("_csrf", TOKEN), ("summary_text", text)]).into_bytes()),
            &[],
        )
        .await
    }

    #[test]
    fn summary_must_not_be_blank_and_hash_is_stable() {
        assert!(validate_summary("   ").is_err());
        let value = validate_summary("  Nội dung mới.  ").unwrap();
        assert_eq!(value, "Nội dung mới.");
        assert_eq!(
            crate::models::sha256_hex(&value),
            crate::models::sha256_hex("Nội dung mới.")
        );
    }

    #[tokio::test]
    async fn detail_renders_duplicate_notice_and_escapes_html() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "<script>alert(1)</script>").await;
        set_status(&context, &episode.uuid, status::READY, stage::READY).await;

        let response = get(&context, &format!("/episodes/{}", episode.uuid)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let html = body(response).await;
        assert!(html.contains("Bản tóm tắt"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(!html.contains("không tạo thêm bản sao"));
        assert!(html.contains(&format!("hx-post=\"/episodes/{}/delete\"", episode.uuid)));
        assert!(!html.contains("hx-delete"));
        assert!(html.contains(&format!("hx-post=\"/episodes/{}/reread\"", episode.uuid)));

        let duplicate = get(&context, &format!("/episodes/{}?duplicate=1", episode.uuid)).await;
        assert_eq!(duplicate.status(), StatusCode::OK);
        assert!(body(duplicate).await.contains("không tạo thêm bản sao"));

        let missing = get(&context, "/episodes/does-not-exist").await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn update_summary_trims_persists_and_flags_audio_for_reread() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Tập").await;
        set_status(&context, &episode.uuid, status::READY, stage::READY).await;
        episodes::set_summary(&context.state.pool, &episode.uuid, "Cũ", "old-hash")
            .await
            .unwrap();
        episodes::set_audio(
            &context.state.pool,
            &episode.uuid,
            "/tmp/old.mp3",
            10,
            5,
            "old-hash",
            1,
        )
        .await
        .unwrap();

        let response = patch_summary(&context, &episode.uuid, "  Nội dung mới.  ").await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
            format!("/episodes/{}", episode.uuid)
        );
        let updated = episodes::get(&context.state.pool, &episode.uuid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.summary(), "Nội dung mới.");
        assert!(updated.summary_edited_at.is_some());
        assert!(updated.audio_needs_reread());
        assert_eq!(updated.audio_path.as_deref(), Some("/tmp/old.mp3"));

        let html = body(get(&context, &format!("/episodes/{}", episode.uuid)).await).await;
        assert!(html.contains("Cần đọc lại"));
    }

    #[tokio::test]
    async fn update_summary_uses_hx_redirect_for_htmx_requests() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Tập").await;
        set_status(&context, &episode.uuid, status::READY, stage::READY).await;

        let response = send(
            &context,
            Method::PATCH,
            &format!("/episodes/{}", episode.uuid),
            Some(form(&[("_csrf", TOKEN), ("summary_text", "Mới")]).into_bytes()),
            &[("hx-request", "true")],
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::LOCATION).is_none());
        assert_eq!(
            response
                .headers()
                .get("hx-redirect")
                .unwrap()
                .to_str()
                .unwrap(),
            format!("/episodes/{}", episode.uuid)
        );
    }

    #[tokio::test]
    async fn update_summary_rejects_blank_busy_and_cross_site_requests() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Tập").await;

        let blank = patch_summary(&context, &episode.uuid, "   ").await;
        assert_eq!(blank.status(), StatusCode::BAD_REQUEST);

        let busy = patch_summary(&context, &episode.uuid, "Nội dung").await;
        assert_eq!(busy.status(), StatusCode::CONFLICT);
        let unchanged = episodes::get(&context.state.pool, &episode.uuid)
            .await
            .unwrap()
            .unwrap();
        assert!(unchanged.summary().is_empty());

        set_status(&context, &episode.uuid, status::READY, stage::READY).await;
        let cross_site = send(
            &context,
            Method::PATCH,
            &format!("/episodes/{}", episode.uuid),
            Some(form(&[("_csrf", TOKEN), ("summary_text", "Bị đổi")]).into_bytes()),
            &[("sec-fetch-site", "cross-site")],
        )
        .await;
        assert_eq!(cross_site.status(), StatusCode::FORBIDDEN);
        let unchanged = episodes::get(&context.state.pool, &episode.uuid)
            .await
            .unwrap()
            .unwrap();
        assert!(unchanged.summary().is_empty());
    }

    #[tokio::test]
    async fn reread_keeps_guid_published_at_and_old_audio() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Tập").await;
        episodes::set_summary(&context.state.pool, &episode.uuid, "Nội dung", "old-hash")
            .await
            .unwrap();
        episodes::set_audio(
            &context.state.pool,
            &episode.uuid,
            "/tmp/old.mp3",
            10,
            5,
            "old-hash",
            1,
        )
        .await
        .unwrap();
        episodes::finish(
            &context.state.pool,
            &episode.uuid,
            stage::PUBLISHED,
            status::PUBLISHED,
            true,
        )
        .await
        .unwrap();
        let before = episodes::get(&context.state.pool, &episode.uuid)
            .await
            .unwrap()
            .unwrap();

        let response = send(
            &context,
            Method::POST,
            &format!("/episodes/{}/reread", episode.uuid),
            Some(csrf_form().into_bytes()),
            &[],
        )
        .await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let after = episodes::get(&context.state.pool, &episode.uuid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.uuid, before.uuid);
        assert_eq!(after.published_at, before.published_at);
        assert_eq!(after.audio_path, before.audio_path);
        assert_eq!(after.audio_summary_hash.as_deref(), Some("old-hash"));
        assert_eq!(after.status, status::QUEUED);
        assert_eq!(after.stage, stage::SYNTHESIZING);
    }

    #[tokio::test]
    async fn reread_requires_a_summary_and_a_settled_status() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let blank = create_episode(&context, show.id, "Trống").await;
        set_status(&context, &blank.uuid, status::READY, stage::READY).await;

        let rejected = send(
            &context,
            Method::POST,
            &format!("/episodes/{}/reread", blank.uuid),
            Some(csrf_form().into_bytes()),
            &[],
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);

        let busy = create_episode(&context, show.id, "Đang chạy").await;
        episodes::set_summary(&context.state.pool, &busy.uuid, "Nội dung", "hash")
            .await
            .unwrap();
        let rejected = send(
            &context,
            Method::POST,
            &format!("/episodes/{}/reread", busy.uuid),
            Some(csrf_form().into_bytes()),
            &[],
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn retry_only_accepts_failed_or_cancelled_episodes() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let failed = create_episode(&context, show.id, "Lỗi").await;
        episodes::record_failure(&context.state.pool, &failed.uuid, "tts", "boom", None)
            .await
            .unwrap();
        let cancelled = create_episode(&context, show.id, "Huỷ").await;
        episodes::cancel(&context.state.pool, &cancelled.uuid)
            .await
            .unwrap();

        for uuid in [&failed.uuid, &cancelled.uuid] {
            let response = send(
                &context,
                Method::POST,
                &format!("/episodes/{uuid}/retry"),
                Some(csrf_form().into_bytes()),
                &[],
            )
            .await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            let retried = episodes::get(&context.state.pool, uuid)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(retried.status, status::QUEUED);
            assert_eq!(retried.attempts, 0);
            assert!(retried.error_code.is_none());
        }

        let ready = create_episode(&context, show.id, "Sẵn sàng").await;
        set_status(&context, &ready.uuid, status::READY, stage::READY).await;
        let rejected = send(
            &context,
            Method::POST,
            &format!("/episodes/{}/retry", ready.uuid),
            Some(csrf_form().into_bytes()),
            &[],
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::CONFLICT);
        assert_eq!(
            episodes::get(&context.state.pool, &ready.uuid)
                .await
                .unwrap()
                .unwrap()
                .status,
            status::READY
        );
    }

    #[tokio::test]
    async fn header_only_csrf_token_can_retry() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Lỗi").await;
        episodes::record_failure(&context.state.pool, &episode.uuid, "tts", "boom", None)
            .await
            .unwrap();

        let response = send(
            &context,
            Method::POST,
            &format!("/episodes/{}/retry", episode.uuid),
            Some(form(&[("_csrf", "")]).into_bytes()),
            &[("x-csrf-token", TOKEN)],
        )
        .await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            episodes::get(&context.state.pool, &episode.uuid)
                .await
                .unwrap()
                .unwrap()
                .status,
            status::QUEUED
        );
    }

    #[tokio::test]
    async fn cancel_only_accepts_active_episodes() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let queued = create_episode(&context, show.id, "Chờ").await;
        let running = create_episode(&context, show.id, "Đang chạy").await;
        episodes::claim_next(&context.state.pool).await.unwrap();
        let ready = create_episode(&context, show.id, "Sẵn sàng").await;
        set_status(&context, &ready.uuid, status::READY, stage::READY).await;

        for uuid in [&queued.uuid, &running.uuid] {
            let response = send(
                &context,
                Method::POST,
                &format!("/episodes/{uuid}/cancel"),
                Some(csrf_form().into_bytes()),
                &[],
            )
            .await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            assert_eq!(
                episodes::get(&context.state.pool, uuid)
                    .await
                    .unwrap()
                    .unwrap()
                    .status,
                status::CANCELLED
            );
        }

        let rejected = send(
            &context,
            Method::POST,
            &format!("/episodes/{}/cancel", ready.uuid),
            Some(csrf_form().into_bytes()),
            &[],
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::CONFLICT);
        assert_eq!(
            episodes::get(&context.state.pool, &ready.uuid)
                .await
                .unwrap()
                .unwrap()
                .status,
            status::READY
        );
    }

    #[tokio::test]
    async fn publish_only_queues_ready_episodes() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let ready = create_episode(&context, show.id, "Sẵn sàng").await;
        set_status(&context, &ready.uuid, status::READY, stage::READY).await;
        let queued = create_episode(&context, show.id, "Chờ").await;

        let response = send(
            &context,
            Method::POST,
            &format!("/episodes/{}/publish", ready.uuid),
            Some(csrf_form().into_bytes()),
            &[],
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let published = episodes::get(&context.state.pool, &ready.uuid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(published.status, status::QUEUED);
        assert_eq!(published.stage, stage::PUBLISHING);

        let rejected = send(
            &context,
            Method::POST,
            &format!("/episodes/{}/publish", queued.uuid),
            Some(csrf_form().into_bytes()),
            &[],
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn delete_removes_feed_item_transcript_and_audio() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Xoá").await;
        let data_dir = context.state.cfg.data_dir.clone();
        let audio_dir = data_dir.join("audio");
        let transcript_dir = data_dir.join("transcripts");
        let tmp_dir = data_dir.join("tmp").join(&episode.uuid);
        tokio::fs::create_dir_all(&audio_dir).await.unwrap();
        tokio::fs::create_dir_all(&transcript_dir).await.unwrap();
        tokio::fs::create_dir_all(&tmp_dir).await.unwrap();
        let audio = audio_dir.join(format!("{}-1.mp3", episode.uuid));
        let transcript = transcript_dir.join(format!("{}.txt", episode.uuid));
        let raw_caption = transcript_dir.join(format!("{}.vtt", episode.uuid));
        tokio::fs::write(&audio, b"audio").await.unwrap();
        tokio::fs::write(&transcript, b"transcript").await.unwrap();
        tokio::fs::write(&raw_caption, b"WEBVTT").await.unwrap();
        tokio::fs::write(tmp_dir.join("chunk.mp3"), b"chunk")
            .await
            .unwrap();
        episodes::set_audio(
            &context.state.pool,
            &episode.uuid,
            &audio.display().to_string(),
            5,
            4,
            "hash",
            1,
        )
        .await
        .unwrap();
        sqlx::query("UPDATE episodes SET transcript_path = ?1 WHERE uuid = ?2")
            .bind(transcript.display().to_string())
            .bind(&episode.uuid)
            .execute(&context.state.pool)
            .await
            .unwrap();

        let response = send(
            &context,
            Method::DELETE,
            &format!("/episodes/{}", episode.uuid),
            Some(csrf_form().into_bytes()),
            &[],
        )
        .await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
            "/library"
        );
        assert!(episodes::get(&context.state.pool, &episode.uuid)
            .await
            .unwrap()
            .is_none());
        assert!(!audio.exists());
        assert!(!transcript.exists());
        assert!(!raw_caption.exists());
        assert!(!tmp_dir.exists());
    }

    #[tokio::test]
    async fn post_delete_alias_removes_episode_and_htmx_redirects() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let normal = create_episode(&context, show.id, "Xoá thường").await;

        let response = send(
            &context,
            Method::POST,
            &format!("/episodes/{}/delete", normal.uuid),
            Some(csrf_form().into_bytes()),
            &[],
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
            "/library"
        );
        assert!(episodes::get(&context.state.pool, &normal.uuid)
            .await
            .unwrap()
            .is_none());

        let htmx = create_episode(&context, show.id, "Xoá htmx").await;
        let response = send(
            &context,
            Method::POST,
            &format!("/episodes/{}/delete", htmx.uuid),
            Some(csrf_form().into_bytes()),
            &[("hx-request", "true")],
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::LOCATION).is_none());
        assert_eq!(
            response
                .headers()
                .get("hx-redirect")
                .unwrap()
                .to_str()
                .unwrap(),
            "/library"
        );
        assert!(episodes::get(&context.state.pool, &htmx.uuid)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn delete_route_accepts_a_csrf_token_from_the_query() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Xoá").await;

        let response = send(
            &context,
            Method::DELETE,
            &format!("/episodes/{}?_csrf={TOKEN}", episode.uuid),
            None,
            &[],
        )
        .await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert!(episodes::get(&context.state.pool, &episode.uuid)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn deleting_an_active_episode_cleans_up_before_purge() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Đang chạy").await;
        episodes::set_active_take(&context.state.pool, &episode.uuid, Some(4242))
            .await
            .unwrap();
        set_status(&context, &episode.uuid, status::RUNNING, stage::VOICING).await;

        let response = send(
            &context,
            Method::POST,
            &format!("/episodes/{}/delete", episode.uuid),
            Some(csrf_form().into_bytes()),
            &[],
        )
        .await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert!(episodes::get(&context.state.pool, &episode.uuid)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn delete_uses_hx_redirect_for_htmx_requests() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Xoá").await;

        let response = send(
            &context,
            Method::DELETE,
            &format!("/episodes/{}", episode.uuid),
            Some(csrf_form().into_bytes()),
            &[("hx-request", "true")],
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::LOCATION).is_none());
        assert_eq!(
            response
                .headers()
                .get("hx-redirect")
                .unwrap()
                .to_str()
                .unwrap(),
            "/library"
        );
        assert!(episodes::get(&context.state.pool, &episode.uuid)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn status_returns_progress_and_missing_episodes_are_404() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Tập").await;
        episodes::set_progress(&context.state.pool, &episode.uuid, r#"{"percent":40}"#)
            .await
            .unwrap();

        let response = get(&context, &format!("/api/episodes/{}/status", episode.uuid)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let json: serde_json::Value = serde_json::from_str(&body(response).await).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["status"], status::QUEUED);
        assert_eq!(json["stage"], stage::QUEUED);
        assert_eq!(json["progress"], r#"{"percent":40}"#);

        let missing = get(&context, "/api/episodes/missing/status").await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }
}
