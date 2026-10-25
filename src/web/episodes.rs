use crate::db::{episodes, shows};
use crate::error::{AppError, AppResult};
use crate::models::{status, Episode, Show};
use crate::services::omnivoice::OmnivoiceClient;
use crate::state::AppState;
use crate::web::csrf::{self, CsrfToken};
use crate::web::shows::purge_episode_files;
use askama::Template;
use axum::body::{Body, Bytes};
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, Request, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Extension;
use axum::Form;
use serde::Deserialize;
use std::path::PathBuf;

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
    progress_active: bool,
}

#[derive(Template)]
#[template(path = "partials/episode_progress.html")]
pub struct ProgressTemplate {
    pub episode: Episode,
    pub progress_active: bool,
}

#[derive(Debug, Default, Deserialize)]
pub struct DetailQuery {
    pub duplicate: Option<String>,
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
    let progress_active = episode.is_active();
    Ok(Html(
        EpisodeTemplate {
            csrf: token.0,
            episode,
            show,
            feed_url,
            audio_url,
            transcript,
            duplicate: query.duplicate.as_deref() == Some("1"),
            progress_active,
        }
        .render()?,
    ))
}

pub async fn progress(
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> AppResult<Html<String>> {
    let episode = episodes::get(&state.pool, &uuid)
        .await?
        .ok_or(AppError::NotFound)?;
    let progress_active = episode.is_active();
    Ok(Html(
        ProgressTemplate {
            episode,
            progress_active,
        }
        .render()?,
    ))
}

pub async fn audio(
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    request: Request<Body>,
) -> AppResult<Response> {
    let episode = episodes::get(&state.pool, &uuid)
        .await?
        .ok_or(AppError::NotFound)?;
    let path = episode.audio_path.ok_or(AppError::NotFound)?;
    let mut response =
        crate::web::public::serve_contained(&state, PathBuf::from(path), request).await?;
    if matches!(
        response.status(),
        StatusCode::OK | StatusCode::PARTIAL_CONTENT | StatusCode::NOT_MODIFIED
    ) {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    Ok(response)
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
    if episode
        .audio_script
        .as_deref()
        .unwrap_or_default()
        .trim()
        .is_empty()
    {
        return Err(AppError::BadRequest(
            "Cần có script audio trước khi đọc lại.".into(),
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
    episodes::cancel(&state.pool, &uuid).await?;
    if let Some(take_id) = episode.active_take_id {
        let client = OmnivoiceClient::new(&state.cfg.omnivoice);
        let _ = client.delete_speech(take_id).await;
    }
    if let Some(design_id) = episode.voice_design_id {
        let client = OmnivoiceClient::new(&state.cfg.omnivoice);
        let _ = client.delete_design(design_id).await;
        episodes::set_voice_design(&state.pool, &uuid, None).await?;
    }
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
    if !episodes::publish_one(&state.pool, &uuid).await? {
        return Err(AppError::Conflict(
            "Chỉ phát hành tập đang sẵn sàng.".into(),
        ));
    }
    let parts = crate::worker::synth::parts_dir(&state.cfg, &uuid);
    let _ = tokio::fs::remove_dir_all(parts).await;
    Ok(redirect_response(&headers, &format!("/episodes/{uuid}")))
}

pub async fn approve(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Path(uuid): Path<String>,
    Form(form): Form<CsrfForm>,
) -> AppResult<Response> {
    csrf::verify(&token, &headers, &form._csrf)?;
    episodes::get(&state.pool, &uuid)
        .await?
        .ok_or(AppError::NotFound)?;
    if !episodes::approve_review(&state.pool, &uuid).await? {
        return Err(AppError::Conflict(
            "Chỉ duyệt được tập đang chờ duyệt.".into(),
        ));
    }
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
        ctx_with_omnivoice("http://127.0.0.1:1").await
    }

    async fn ctx_with_omnivoice(base_url: &str) -> Ctx {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let base_url = base_url.to_string();
        let cfg = Config::from_lookup(move |key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "OMNIVOICE_BASE_URL" => Some(base_url.clone()),
            "DATA_DIR" => Some(path.display().to_string()),
            _ => None,
        })
        .unwrap();
        Ctx {
            state: AppState {
                pool: crate::db::test_pool().await,
                cfg: Arc::new(cfg),
                notify: Arc::new(Notify::new()),
                health_cache: Arc::new(tokio::sync::Mutex::new(None)),
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
                review_required: false,
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

    #[tokio::test]
    async fn approve_resumes_only_a_review_episode() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Tập").await;
        sqlx::query("UPDATE episodes SET status = 'review', stage = 'review' WHERE uuid = ?1")
            .bind(&episode.uuid)
            .execute(&context.state.pool)
            .await
            .unwrap();

        let response = send(
            &context,
            Method::POST,
            &format!("/episodes/{}/approve", episode.uuid),
            Some(form(&[("_csrf", TOKEN)]).into_bytes()),
            &[],
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let got = episodes::get(&context.state.pool, &episode.uuid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.status, status::QUEUED);
        assert_eq!(got.stage, stage::VOICING);

        let again = send(
            &context,
            Method::POST,
            &format!("/episodes/{}/approve", episode.uuid),
            Some(form(&[("_csrf", TOKEN)]).into_bytes()),
            &[],
        )
        .await;
        assert_eq!(again.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn review_episode_renders_read_only_summary_and_continue() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Tập").await;
        sqlx::query("UPDATE episodes SET status = 'review', stage = 'review', summary_text = 'Tóm tắt ngắn.', audio_script = 'Script đầy đủ để admin duyệt.' WHERE uuid = ?1")
            .bind(&episode.uuid)
            .execute(&context.state.pool)
            .await
            .unwrap();

        let html = body(get(&context, &format!("/episodes/{}", episode.uuid)).await).await;
        assert!(html.contains("Chờ duyệt tóm tắt"));
        assert!(html.contains("Tiếp tục tạo audio"));
        assert!(html.contains("Tóm tắt ngắn."));
        assert!(html.contains("Script audio đầy đủ"));
        assert!(html.contains("Script đầy đủ để admin duyệt."));
        assert!(html.contains(&format!("action=\"/episodes/{}/approve\"", episode.uuid)));
        assert!(!html.contains("name=\"summary_text\""));
    }

    #[tokio::test]
    async fn review_audio_script_is_escaped() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Tập").await;
        sqlx::query(
            "UPDATE episodes SET status = 'review', stage = 'review', audio_script = '<script>alert(1)</script>' WHERE uuid = ?1",
        )
        .bind(&episode.uuid)
        .execute(&context.state.pool)
        .await
        .unwrap();

        let html = body(get(&context, &format!("/episodes/{}", episode.uuid)).await).await;
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
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

    #[tokio::test]
    async fn detail_shows_the_pipeline_stage_for_active_episodes() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Tập").await;
        sqlx::query(
            "UPDATE episodes SET status = 'running', stage = 'summarizing' WHERE uuid = ?1",
        )
        .bind(&episode.uuid)
        .execute(&context.state.pool)
        .await
        .unwrap();

        let html = body(get(&context, &format!("/episodes/{}", episode.uuid)).await).await;
        assert!(html.contains("Đang xử lý · Đang tóm tắt"));
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
        assert!(html.contains("<h2>Tóm tắt</h2>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(!html.contains("không tạo thêm bản sao"));
        assert!(html.contains(&format!("action=\"/episodes/{}/delete\"", episode.uuid)));
        assert!(!html.contains("hx-delete"));
        assert!(html.contains(&format!("action=\"/episodes/{}/reread\"", episode.uuid)));

        let duplicate = get(&context, &format!("/episodes/{}?duplicate=1", episode.uuid)).await;
        assert_eq!(duplicate.status(), StatusCode::OK);
        assert!(body(duplicate).await.contains("không tạo thêm bản sao"));

        let missing = get(&context, "/episodes/does-not-exist").await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn episode_detail_hides_advanced_actions_in_a_disclosure() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Tập").await;
        set_status(&context, &episode.uuid, status::READY, stage::READY).await;
        let html = body(get(&context, &format!("/episodes/{}", episode.uuid)).await).await;
        assert!(html.contains("<details"));
        assert!(html.contains("Nâng cao"));
        let summary = "<summary>Nâng cao</summary>";
        let start = html.find(summary).expect("advanced disclosure summary");
        let end = html[start..]
            .find("</details>")
            .expect("advanced disclosure end");
        let advanced = &html[start..start + end];
        assert!(advanced.contains(&format!("action=\"/episodes/{}/reread\"", episode.uuid)));
        assert!(advanced.contains(&format!("hx-post=\"/episodes/{}/reread\"", episode.uuid)));
        assert!(advanced.contains("hx-confirm=\"Bắt đầu tạo audio mới từ script hiện tại?\""));
        assert!(advanced.contains(&format!("action=\"/episodes/{}/publish\"", episode.uuid)));
        assert!(!advanced.contains(&format!("action=\"/episodes/{}/delete\"", episode.uuid)));
        assert!(!advanced.contains(&format!("hx-post=\"/episodes/{}/delete\"", episode.uuid)));
        assert!(html.contains(&format!("action=\"/episodes/{}/delete\"", episode.uuid)));
        assert!(html.contains(&format!("hx-post=\"/episodes/{}/delete\"", episode.uuid)));
        assert!(html.contains("hx-confirm=\"Xoá vĩnh viễn tập này, transcript và mọi bản audio?\""));
        assert!(html.contains("<h2>Transcript</h2>"));
        assert!(!advanced.contains("<h2>Transcript</h2>"));
    }

    #[tokio::test]
    async fn detail_warns_when_audio_was_generated_from_an_older_script() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Tập").await;
        sqlx::query(
            "UPDATE episodes SET status = 'ready', stage = 'ready', summary_hash = 'new', audio_summary_hash = 'old', audio_path = '/tmp/audio.mp3' WHERE uuid = ?1",
        )
        .bind(&episode.uuid)
        .execute(&context.state.pool)
        .await
        .unwrap();

        let html = body(get(&context, &format!("/episodes/{}", episode.uuid)).await).await;
        assert!(html.contains("Cần đọc lại"));
    }

    #[tokio::test]
    async fn review_episode_without_audio_does_not_warn_about_stale_audio() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Tập").await;
        sqlx::query(
            "UPDATE episodes SET status = 'review', stage = 'review', summary_hash = 'new', audio_summary_hash = NULL WHERE uuid = ?1",
        )
        .bind(&episode.uuid)
        .execute(&context.state.pool)
        .await
        .unwrap();

        let html = body(get(&context, &format!("/episodes/{}", episode.uuid)).await).await;
        assert!(!html.contains("Cần đọc lại"));
        assert!(!html.contains("Audio khớp"));
    }

    #[tokio::test]
    async fn approve_rejects_a_bad_csrf_token() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Tập").await;
        set_status(&context, &episode.uuid, status::REVIEW, stage::REVIEW).await;

        let response = send(
            &context,
            Method::POST,
            &format!("/episodes/{}/approve", episode.uuid),
            Some(form(&[("_csrf", "bad-token")]).into_bytes()),
            &[],
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn admin_audio_serves_ready_audio_even_when_the_feed_is_disabled() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Tập").await;
        set_status(&context, &episode.uuid, status::READY, stage::READY).await;

        let audio_dir = context.state.cfg.data_dir.join("audio");
        tokio::fs::create_dir_all(&audio_dir).await.unwrap();
        let audio_path = audio_dir.join(format!("{}-1.mp3", episode.uuid));
        tokio::fs::write(&audio_path, b"ID3fake-mp3").await.unwrap();
        episodes::set_audio(
            &context.state.pool,
            &episode.uuid,
            &audio_path.display().to_string(),
            11,
            600,
            "hash",
            1,
        )
        .await
        .unwrap();

        let html = body(get(&context, &format!("/episodes/{}", episode.uuid)).await).await;
        assert!(html.contains(&format!("src=\"/episodes/{}/audio\"", episode.uuid)));
        assert!(html.contains(&format!(
            "https://pod.example.ts.net:8443/feed/{}.xml",
            show.feed_token
        )));
        assert!(!show.feed_enabled);

        let response = get(&context, &format!("/episodes/{}/audio", episode.uuid)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .unwrap()
                .to_str()
                .unwrap(),
            "audio/mpeg"
        );
        assert_eq!(
            response
                .headers()
                .get(header::CACHE_CONTROL)
                .unwrap()
                .to_str()
                .unwrap(),
            "no-store"
        );
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert_eq!(&bytes[..], b"ID3fake-mp3");

        let ranged = send(
            &context,
            Method::GET,
            &format!("/episodes/{}/audio", episode.uuid),
            None,
            &[("range", "bytes=0-3")],
        )
        .await;
        assert_eq!(ranged.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            ranged
                .headers()
                .get(header::CONTENT_RANGE)
                .unwrap()
                .to_str()
                .unwrap(),
            "bytes 0-3/11"
        );
        let bytes = to_bytes(ranged.into_body(), 1024 * 1024).await.unwrap();
        assert_eq!(&bytes[..], b"ID3f");

        let head = send(
            &context,
            Method::HEAD,
            &format!("/episodes/{}/audio", episode.uuid),
            None,
            &[],
        )
        .await;
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(
            head.headers()
                .get(header::CONTENT_LENGTH)
                .unwrap()
                .to_str()
                .unwrap(),
            "11"
        );
    }

    #[tokio::test]
    async fn admin_audio_unknown_episode_or_missing_audio_is_404() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let no_audio = create_episode(&context, show.id, "Chưa có audio").await;

        for uuid in [&no_audio.uuid, "does-not-exist"] {
            let response = get(&context, &format!("/episodes/{uuid}/audio")).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
    }

    #[tokio::test]
    async fn reread_keeps_guid_published_at_and_old_audio() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Tập").await;
        episodes::set_summary(
            &context.state.pool,
            &episode.uuid,
            "Nội dung",
            "Nội dung",
            "old-hash",
        )
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
    async fn reread_requires_an_audio_script_and_a_settled_status() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let blank = create_episode(&context, show.id, "Trống").await;
        set_status(&context, &blank.uuid, status::READY, stage::READY).await;
        sqlx::query("UPDATE episodes SET summary_text = 'Tóm tắt' WHERE uuid = ?1")
            .bind(&blank.uuid)
            .execute(&context.state.pool)
            .await
            .unwrap();

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
        episodes::set_summary(
            &context.state.pool,
            &busy.uuid,
            "Nội dung",
            "Nội dung",
            "hash",
        )
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
    async fn cancel_deletes_an_active_voice_design_and_clears_it() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("DELETE"))
            .and(wiremock::matchers::path("/api/voice-designs/7"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"ok": true, "data": null, "error": null})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let context = ctx_with_omnivoice(&server.uri()).await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Đang tạo giọng").await;
        set_status(&context, &episode.uuid, status::RUNNING, stage::VOICING).await;
        episodes::set_voice_design(&context.state.pool, &episode.uuid, Some(7))
            .await
            .unwrap();

        let response = send(
            &context,
            Method::POST,
            &format!("/episodes/{}/cancel", episode.uuid),
            Some(csrf_form().into_bytes()),
            &[],
        )
        .await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let got = episodes::get(&context.state.pool, &episode.uuid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.status, status::CANCELLED);
        assert_eq!(got.voice_design_id, None);
        server.verify().await;
    }

    #[tokio::test]
    async fn publish_publishes_ready_episodes_without_auto_publish() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        sqlx::query("UPDATE shows SET auto_publish = 0 WHERE id = ?1")
            .bind(show.id)
            .execute(&context.state.pool)
            .await
            .unwrap();
        let ready = create_episode(&context, show.id, "Sẵn sàng").await;
        set_status(&context, &ready.uuid, status::READY, stage::READY).await;
        let parts = crate::worker::synth::parts_dir(&context.state.cfg, &ready.uuid);
        tokio::fs::create_dir_all(&parts).await.unwrap();
        tokio::fs::write(parts.join("part-000.mp3"), b"part")
            .await
            .unwrap();
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
        assert_eq!(published.status, status::PUBLISHED);
        assert_eq!(published.stage, stage::PUBLISHED);
        assert!(published.published_at.is_some());
        assert!(!parts.exists(), "temporary parts must be removed");

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
    async fn progress_partial_uses_labels_and_stops_for_terminal_episode() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Vi").await;
        crate::db::episodes::set_stage(
            &context.state.pool,
            &episode.uuid,
            crate::models::stage::SYNTHESIZING,
        )
        .await
        .unwrap();
        crate::db::episodes::set_chunk_progress(&context.state.pool, &episode.uuid, "[]", 2, 4)
            .await
            .unwrap();
        let active = send(
            &context,
            Method::GET,
            &format!("/episodes/{}/progress", episode.uuid),
            None,
            &[],
        )
        .await;
        assert_eq!(active.status(), StatusCode::OK);
        let html = body(active).await;
        assert!(html.contains("Đang tạo audio"));
        assert!(html.contains("67%"));
        assert!(html.contains("hx-trigger=\"every 2s\""));
        assert!(html.contains("aria-live=\"polite\""));

        crate::db::episodes::finish(&context.state.pool, &episode.uuid, "ready", "ready", false)
            .await
            .unwrap();
        let done = send(
            &context,
            Method::GET,
            &format!("/episodes/{}/progress", episode.uuid),
            None,
            &[],
        )
        .await;
        let html = body(done).await;
        assert!(!html.contains("every 2s"));
    }

    #[tokio::test]
    async fn removed_status_endpoint_is_404() {
        let context = ctx().await;
        let show = create_show(&context, "Show").await;
        let episode = create_episode(&context, show.id, "Tập").await;
        let response = get(&context, &format!("/api/episodes/{}/status", episode.uuid)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
