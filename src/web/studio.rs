use crate::db::{episodes, shows};
use crate::error::{AppError, AppResult};
use crate::models::Episode;
use crate::services::omnivoice::{OmnivoiceClient, Voice};
use crate::services::ytdlp;
use crate::state::AppState;
use crate::web::csrf::{self, CsrfToken};
use askama::Template;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Json, Redirect, Response};
use axum::Extension;
use axum::Form;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

#[derive(Template)]
#[template(path = "studio.html")]
pub struct StudioTemplate {
    csrf: String,
    shows: Vec<crate::models::Show>,
    recent: Vec<Episode>,
    selected_show: Option<i64>,
    selected_minutes: i64,
}

#[derive(Template)]
#[template(path = "partials/queue.html")]
pub struct QueueTemplate {
    csrf: String,
    episodes: Vec<Episode>,
}

#[derive(Template)]
#[template(path = "partials/preview.html")]
pub struct PreviewTemplate {
    title: String,
    channel: String,
    duration: String,
    language_label: String,
    has_captions: bool,
    thumbnail: String,
    has_thumbnail: bool,
    voice_name: Option<String>,
    voice_missing: bool,
    voices: Vec<Voice>,
    selected_voice_id: Option<i64>,
}

#[derive(Template)]
#[template(path = "partials/preview_error.html")]
pub struct PreviewErrorTemplate {
    message: String,
}

struct PreviewData {
    title: String,
    channel: Option<String>,
    duration_s: Option<i64>,
    language: Option<&'static str>,
    has_captions: bool,
    thumbnail: Option<String>,
    voice_name: Option<String>,
    voice_missing: bool,
    voices: Vec<Voice>,
    selected_voice_id: Option<i64>,
}

impl From<&PreviewData> for PreviewTemplate {
    fn from(data: &PreviewData) -> Self {
        Self {
            title: data.title.clone(),
            channel: data.channel.clone().unwrap_or_default(),
            duration: duration_label(data.duration_s),
            language_label: language_label(data.language.unwrap_or_default()).to_string(),
            has_captions: data.has_captions,
            thumbnail: data.thumbnail.clone().unwrap_or_default(),
            has_thumbnail: data.thumbnail.is_some(),
            voice_name: data.voice_name.clone(),
            voice_missing: data.voice_missing,
            voices: data.voices.clone(),
            selected_voice_id: data.selected_voice_id,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct StudioQuery {
    pub show: Option<i64>,
}

fn empty_as_none<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    match value.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(raw) => raw
            .parse::<i64>()
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateForm {
    pub _csrf: String,
    pub url: String,
    pub show_id: i64,
    pub minutes: i64,
    #[serde(default, deserialize_with = "empty_as_none")]
    pub voice_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct PreviewForm {
    pub _csrf: String,
    pub url: String,
    pub show_id: Option<i64>,
    #[serde(default, deserialize_with = "empty_as_none")]
    pub voice_id: Option<i64>,
}

pub async fn index(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Query(query): Query<StudioQuery>,
) -> AppResult<Html<String>> {
    let shows = shows::list(&state.pool).await?;
    let recent = episodes::list(
        &state.pool,
        &episodes::EpisodeFilter {
            limit: Some(8),
            ..episodes::EpisodeFilter::default()
        },
    )
    .await?;
    let selected_show = query.show.or_else(|| shows.first().map(|show| show.id));
    let selected_minutes = selected_show
        .and_then(|id| shows.iter().find(|show| show.id == id))
        .map(|show| show.default_minutes)
        .unwrap_or(10);
    Ok(Html(
        StudioTemplate {
            csrf: token.0,
            shows,
            recent,
            selected_show,
            selected_minutes,
        }
        .render()?,
    ))
}

pub async fn create(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Form(form): Form<CreateForm>,
) -> AppResult<Redirect> {
    csrf::verify(&token, &headers, &form._csrf)?;
    validate_minutes(form.minutes)?;
    let video = ytdlp::validate_url(&form.url)?;
    let show = shows::get(&state.pool, form.show_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if let Some(existing) = episodes::get_by_video_id(&state.pool, &video.id).await? {
        return Ok(duplicate_redirect(&existing.uuid));
    }
    if let Some(voice_id) = form.voice_id {
        let client = OmnivoiceClient::new(&state.cfg.omnivoice);
        let voices = client
            .list_voices()
            .await
            .map_err(|e| AppError::Upstream(e.to_string()))?;
        voices
            .iter()
            .find(|voice| {
                voice.id == voice_id
                    && voice.status == "ready"
                    && crate::models::supported_language(voice.language.as_deref().unwrap_or(""))
                        .is_some()
            })
            .ok_or_else(|| AppError::BadRequest("Giọng đọc không còn khả dụng.".into()))?;
    }
    let episode = match episodes::create(
        &state.pool,
        episodes::NewEpisode {
            show_id: show.id,
            video_id: video.id.clone(),
            source_url: format!("https://www.youtube.com/watch?v={}", video.id),
            source_title: "Đang tải thông tin…".into(),
            source_channel: String::new(),
            source_duration_s: None,
            source_thumbnail_url: None,
            source_upload_date: None,
            language: show.language.clone(),
            target_minutes: form.minutes,
        },
    )
    .await
    {
        Ok(episode) => episode,
        Err(error) => {
            if let Some(existing) = episodes::get_by_video_id(&state.pool, &video.id).await? {
                return Ok(duplicate_redirect(&existing.uuid));
            }
            return Err(error);
        }
    };
    if let Some(voice_id) = form.voice_id {
        episodes::set_voice(&state.pool, &episode.uuid, voice_id).await?;
    }
    state.notify.notify_one();
    Ok(Redirect::to(&format!("/episodes/{}", episode.uuid)))
}

fn duplicate_redirect(uuid: &str) -> Redirect {
    Redirect::to(&format!("/episodes/{uuid}?duplicate=1"))
}

pub async fn preview(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Form(form): Form<PreviewForm>,
) -> AppResult<Response> {
    let htmx = is_htmx(&headers);
    match load_preview(&state, &token, &headers, &form).await {
        Ok(data) => {
            if htmx {
                Ok(Html(PreviewTemplate::from(&data).render()?).into_response())
            } else {
                Ok(Json(json!({
                    "ok": true,
                    "title": data.title,
                    "channel": data.channel,
                    "duration_s": data.duration_s,
                    "language": data.language,
                    "has_captions": data.has_captions,
                    "thumbnail": data.thumbnail,
                    "voice_name": data.voice_name,
                    "voice_missing": data.voice_missing,
                    "voices": data.voices,
                }))
                .into_response())
            }
        }
        Err(error) => {
            if htmx {
                Ok(Html(
                    PreviewErrorTemplate {
                        message: preview_error_message(&error),
                    }
                    .render()?,
                )
                .into_response())
            } else {
                Err(error)
            }
        }
    }
}

async fn load_preview(
    state: &AppState,
    token: &CsrfToken,
    headers: &HeaderMap,
    form: &PreviewForm,
) -> AppResult<PreviewData> {
    csrf::verify(token, headers, &form._csrf)?;
    let video = ytdlp::validate_url(&form.url)?;
    let meta = tokio::time::timeout(Duration::from_secs(15), ytdlp::probe(&state.cfg, &video))
        .await
        .map_err(|_| AppError::Upstream("preview timed out".into()))??;
    let language = ytdlp::source_language(&meta).ok();
    let has_captions = language
        .and_then(|lang| ytdlp::choose_caption(&meta, lang))
        .is_some();
    let (voice_name, voice_missing, voices) = match (form.show_id, language) {
        (Some(show_id), Some(language)) => {
            let catalog = OmnivoiceClient::new(&state.cfg.omnivoice)
                .list_voices()
                .await
                .unwrap_or_default();
            let selected = shows::get_voice(&state.pool, show_id, language).await?;
            let voice_name = selected.as_ref().map(|(_, name)| name.clone());
            let ready = selected.as_ref().is_some_and(|(voice_id, _)| {
                catalog
                    .iter()
                    .any(|voice| voice.id == *voice_id && voice.is_ready_for(language))
            });
            let voices = catalog
                .into_iter()
                .filter(|voice| voice.is_ready_for(language))
                .collect();
            (voice_name, !ready, voices)
        }
        _ => (None, false, Vec::new()),
    };
    Ok(PreviewData {
        title: meta.title,
        channel: meta.channel,
        duration_s: meta.duration,
        language,
        has_captions,
        thumbnail: meta.thumbnail,
        voice_name,
        voice_missing,
        voices,
        selected_voice_id: form.voice_id,
    })
}

fn is_htmx(headers: &HeaderMap) -> bool {
    headers
        .get("hx-request")
        .and_then(|value| value.to_str().ok())
        == Some("true")
}

fn duration_label(seconds: Option<i64>) -> String {
    match seconds {
        Some(total) => format!("{}:{:02}", total / 60, total % 60),
        None => "Chưa rõ".into(),
    }
}

fn language_label(language: &str) -> &'static str {
    crate::models::language_label(language)
}

fn preview_error_message(error: &AppError) -> String {
    match error {
        AppError::BadRequest(message) | AppError::Conflict(message) => message.clone(),
        _ => "Không xem trước được video. Vui lòng thử lại sau.".into(),
    }
}

pub async fn queue(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
) -> AppResult<impl IntoResponse> {
    let running = episodes::list(
        &state.pool,
        &episodes::EpisodeFilter {
            status: Some(crate::models::status::RUNNING.into()),
            limit: Some(20),
            ..episodes::EpisodeFilter::default()
        },
    )
    .await?;
    let queued = episodes::list(
        &state.pool,
        &episodes::EpisodeFilter {
            status: Some(crate::models::status::QUEUED.into()),
            limit: Some(20),
            ..episodes::EpisodeFilter::default()
        },
    )
    .await?;
    let mut episodes: Vec<Episode> = Vec::new();
    for episode in running.into_iter().chain(queued) {
        if !episodes
            .iter()
            .any(|existing| existing.uuid == episode.uuid)
        {
            episodes.push(episode);
        }
    }
    let recent = episodes::list(
        &state.pool,
        &episodes::EpisodeFilter {
            limit: Some(10),
            ..episodes::EpisodeFilter::default()
        },
    )
    .await?;
    for episode in recent {
        if !episodes
            .iter()
            .any(|existing| existing.uuid == episode.uuid)
        {
            episodes.push(episode);
        }
    }
    Ok(Html(
        QueueTemplate {
            csrf: token.0,
            episodes,
        }
        .render()?,
    ))
}

fn validate_minutes(minutes: i64) -> AppResult<()> {
    if matches!(minutes, 5 | 10 | 15 | 20) {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "Thời lượng phải là 5, 10, 15 hoặc 20 phút.".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::shows::NewShow;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use std::sync::Arc;
    use tokio::sync::Notify;
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const WATCH: &str = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
    const VIDEO_ID: &str = "dQw4w9WgXcQ";

    async fn test_state() -> AppState {
        let cfg = Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "DATA_DIR" => Some("/tmp/t2p-studio".into()),
            _ => None,
        })
        .unwrap();
        AppState {
            pool: crate::db::test_pool().await,
            cfg: Arc::new(cfg),
            notify: Arc::new(Notify::new()),
        }
    }

    async fn state_with_omnivoice(server: &MockServer) -> AppState {
        let cfg = Config::from_lookup({
            let base = server.uri();
            move |key| match key {
                "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
                "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
                "OMNIVOICE_BASE_URL" => Some(base.clone()),
                "DATA_DIR" => Some("/tmp/t2p-studio".into()),
                _ => None,
            }
        })
        .unwrap();
        AppState {
            pool: crate::db::test_pool().await,
            cfg: Arc::new(cfg),
            notify: Arc::new(Notify::new()),
        }
    }

    async fn mount_ready_voice(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": [{"id": 42, "name": "Phượng", "status": "ready", "language": "vi"}],
                "error": null
            })))
            .mount(server)
            .await;
    }

    async fn create_show(state: &AppState) -> i64 {
        shows::create(
            &state.pool,
            NewShow {
                title: "Show".into(),
                ..NewShow::default()
            },
        )
        .await
        .unwrap()
        .id
    }

    fn form(pairs: &[(&str, &str)]) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in pairs {
            serializer.append_pair(key, value);
        }
        serializer.finish()
    }

    fn create_form(show_id: i64) -> String {
        let show_id = show_id.to_string();
        form(&[
            ("_csrf", TOKEN),
            ("url", WATCH),
            ("show_id", &show_id),
            ("minutes", "10"),
        ])
    }

    async fn post(
        state: &AppState,
        path: &str,
        body: String,
        headers: &[(&str, &str)],
    ) -> axum::response::Response {
        let mut request = Request::builder()
            .method("POST")
            .uri(path)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::COOKIE, format!("t2p_csrf={TOKEN}"));
        for (key, value) in headers {
            request = request.header(*key, *value);
        }
        crate::web::admin_router(state.clone())
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }

    async fn get(state: &AppState, path: &str) -> axum::response::Response {
        let request = Request::builder()
            .uri(path)
            .header(header::COOKIE, format!("t2p_csrf={TOKEN}"));
        crate::web::admin_router(state.clone())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn response_body(response: axum::response::Response) -> String {
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn new_episode(show_id: i64, video_id: &str, title: &str) -> episodes::NewEpisode {
        episodes::NewEpisode {
            show_id,
            video_id: video_id.into(),
            source_url: format!("https://www.youtube.com/watch?v={video_id}"),
            source_title: title.into(),
            source_channel: "Channel".into(),
            source_duration_s: Some(600),
            source_thumbnail_url: None,
            source_upload_date: None,
            language: "vi".into(),
            target_minutes: 10,
        }
    }

    fn content_type(response: &axum::response::Response) -> &str {
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap()
    }

    #[test]
    fn only_accepts_the_four_length_presets() {
        assert!(validate_minutes(5).is_ok());
        assert!(validate_minutes(10).is_ok());
        assert!(validate_minutes(15).is_ok());
        assert!(validate_minutes(20).is_ok());
        assert!(validate_minutes(7).is_err());
    }

    #[tokio::test]
    async fn create_rejects_cross_site_requests() {
        let state = test_state().await;
        let show_id = create_show(&state).await;
        let response = post(
            &state,
            "/episodes",
            create_form(show_id),
            &[("sec-fetch-site", "cross-site")],
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(episodes::list(&state.pool, &Default::default())
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn create_accepts_a_header_only_csrf_token() {
        let state = test_state().await;
        let show_id = create_show(&state).await;
        let show_id = show_id.to_string();
        let body = form(&[
            ("_csrf", ""),
            ("url", WATCH),
            ("show_id", &show_id),
            ("minutes", "10"),
        ]);
        let response = post(&state, "/episodes", body, &[("x-csrf-token", TOKEN)]).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            episodes::list(&state.pool, &Default::default())
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn create_accepts_an_empty_voice_override() {
        let state = test_state().await;
        let show_id = create_show(&state).await;
        let body = format!("{}&voice_id=", create_form(show_id));

        let response = post(&state, "/episodes", body, &[]).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let episodes = episodes::list(&state.pool, &Default::default())
            .await
            .unwrap();
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].voice_id, None);
    }

    #[tokio::test]
    async fn create_stores_a_ready_voice_override() {
        let server = MockServer::start().await;
        mount_ready_voice(&server).await;
        let state = state_with_omnivoice(&server).await;
        let show_id = create_show(&state).await;
        let body = form(&[
            ("_csrf", TOKEN),
            ("url", WATCH),
            ("show_id", &show_id.to_string()),
            ("minutes", "10"),
            ("voice_id", "42"),
        ]);

        let response = post(&state, "/episodes", body, &[]).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let episodes = episodes::list(&state.pool, &Default::default())
            .await
            .unwrap();
        assert_eq!(episodes.len(), 1);
        assert_eq!(episodes[0].voice_id, Some(42));
    }

    #[tokio::test]
    async fn create_rejects_an_invalid_voice_without_creating_an_episode() {
        let server = MockServer::start().await;
        mount_ready_voice(&server).await;
        let state = state_with_omnivoice(&server).await;
        let show_id = create_show(&state).await;
        let body = form(&[
            ("_csrf", TOKEN),
            ("url", WATCH),
            ("show_id", &show_id.to_string()),
            ("minutes", "10"),
            ("voice_id", "99"),
        ]);

        let response = post(&state, "/episodes", body, &[]).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(episodes::list(&state.pool, &Default::default())
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn duplicate_video_redirects_with_duplicate_flag() {
        let state = test_state().await;
        let show_id = create_show(&state).await;
        let existing = episodes::create(
            &state.pool,
            new_episode(show_id, VIDEO_ID, "Existing video"),
        )
        .await
        .unwrap();

        let response = post(&state, "/episodes", create_form(show_id), &[]).await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response
                .headers()
                .get(header::LOCATION)
                .unwrap()
                .to_str()
                .unwrap(),
            format!("/episodes/{}?duplicate=1", existing.uuid)
        );
        assert_eq!(
            episodes::list(&state.pool, &Default::default())
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn queue_includes_queued_episodes() {
        let state = test_state().await;
        let show_id = create_show(&state).await;
        let queued = episodes::create(
            &state.pool,
            new_episode(show_id, "queued00001", "Queued video"),
        )
        .await
        .unwrap();
        sqlx::query("UPDATE episodes SET created_at = 0, updated_at = 0 WHERE uuid = ?1")
            .bind(&queued.uuid)
            .execute(&state.pool)
            .await
            .unwrap();
        for index in 0..10 {
            episodes::create(
                &state.pool,
                new_episode(
                    show_id,
                    &format!("vid{index:08}"),
                    &format!("Recent {index}"),
                ),
            )
            .await
            .unwrap();
        }

        let response = get(&state, "/api/queue").await;

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response_body(response).await.contains("Queued video"));
    }

    #[tokio::test]
    async fn index_selects_the_requested_show_and_its_default_minutes() {
        let state = test_state().await;
        for (title, minutes) in [("Alpha", 10), ("Beta", 15)] {
            shows::create(
                &state.pool,
                NewShow {
                    title: title.into(),
                    default_minutes: minutes,
                    ..NewShow::default()
                },
            )
            .await
            .unwrap();
        }
        let beta = shows::list(&state.pool)
            .await
            .unwrap()
            .into_iter()
            .find(|show| show.title == "Beta")
            .unwrap();

        let html = response_body(get(&state, &format!("/?show={}", beta.id)).await).await;

        assert!(html.contains(&format!(
            "value=\"{}\" data-default-minutes=\"15\" selected",
            beta.id
        )));
        assert!(html.contains("value=\"15\" checked"));
    }

    #[tokio::test]
    async fn queue_renders_progress_bars_and_vietnamese_labels() {
        let state = test_state().await;
        let show_id = create_show(&state).await;
        let episode = episodes::create(
            &state.pool,
            new_episode(show_id, VIDEO_ID, "Progress video"),
        )
        .await
        .unwrap();
        crate::db::episodes::set_stage(
            &state.pool,
            &episode.uuid,
            crate::models::stage::SYNTHESIZING,
        )
        .await
        .unwrap();
        crate::db::episodes::set_chunk_progress(&state.pool, &episode.uuid, "[1,2]", 2, 4)
            .await
            .unwrap();
        let response = get(&state, "/api/queue").await;
        let html = response_body(response).await;
        assert!(html.contains("role=\"progressbar\""));
        assert!(html.contains("Đang tạo audio"));
        assert!(html.contains("67%"));
    }

    #[tokio::test]
    async fn queue_retries_failed_episodes_with_a_post_form() {
        let state = test_state().await;
        let show_id = create_show(&state).await;
        let episode = episodes::create(&state.pool, new_episode(show_id, VIDEO_ID, "Failed video"))
            .await
            .unwrap();
        episodes::record_failure(&state.pool, &episode.uuid, "tts", "boom", None)
            .await
            .unwrap();

        let html = response_body(get(&state, "/api/queue").await).await;

        assert!(html.contains(&format!("action=\"/episodes/{}/retry\"", episode.uuid)));
        assert!(html.contains("method=\"post\""));
        assert!(html.contains(&format!("name=\"_csrf\" value=\"{TOKEN}\"")));
        assert!(!html.contains(&format!("href=\"/episodes/{}/retry\"", episode.uuid)));
    }

    #[test]
    fn preview_errors_hide_internal_details() {
        assert_eq!(
            preview_error_message(&AppError::BadRequest("Lỗi công khai.".into())),
            "Lỗi công khai."
        );
        assert_eq!(
            preview_error_message(&AppError::Conflict("Xung đột công khai.".into())),
            "Xung đột công khai."
        );
        for error in [
            AppError::Upstream("secret upstream".into()),
            AppError::Internal(anyhow::anyhow!("secret internal")),
            AppError::NotFound,
            AppError::Forbidden,
        ] {
            assert_eq!(
                preview_error_message(&error),
                "Không xem trước được video. Vui lòng thử lại sau."
            );
        }
    }

    #[test]
    fn preview_renders_the_detected_language_voice_options() {
        use crate::services::omnivoice::{Voice, VoiceDesignInfo};

        let template = PreviewTemplate {
            title: "Video".into(),
            channel: "Channel".into(),
            duration: "10:00".into(),
            language_label: "Tiếng Việt".into(),
            has_captions: true,
            thumbnail: String::new(),
            has_thumbnail: false,
            voice_name: Some("Phượng".into()),
            voice_missing: false,
            voices: vec![Voice {
                id: 42,
                name: Some("Phượng".into()),
                status: "ready".into(),
                language: Some("vi".into()),
                preview_url: None,
                design: Some(VoiceDesignInfo {
                    gender: Some("female".into()),
                    age: Some("middle_aged".into()),
                    ..Default::default()
                }),
            }],
            selected_voice_id: Some(42),
        };
        let html = template.render().unwrap();
        assert!(html.contains("name=\"voice_id\""));
        assert!(html.contains("Theo show"));
        assert!(html.contains("Phượng"));
        assert!(html.contains("Nữ"));
        assert!(html.contains("Trung niên"));
        assert!(html.contains("value=\"42\" selected"));
    }

    #[tokio::test]
    async fn preview_returns_an_html_notice_for_htmx_errors() {
        let state = test_state().await;
        let body = form(&[
            ("_csrf", TOKEN),
            ("url", "https://example.com/watch?v=dQw4w9WgXcQ"),
        ]);

        let response = post(&state, "/api/preview", body, &[("hx-request", "true")]).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert!(content_type(&response).starts_with("text/html"));
        let html = response_body(response).await;
        assert!(html.contains("Chỉ nhận link video YouTube."));
    }

    #[tokio::test]
    async fn preview_keeps_json_errors_for_non_htmx_requests() {
        let state = test_state().await;
        let body = form(&[
            ("_csrf", TOKEN),
            ("url", "https://example.com/watch?v=dQw4w9WgXcQ"),
        ]);

        let response = post(&state, "/api/preview", body, &[]).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(content_type(&response).starts_with("application/json"));
    }
}
