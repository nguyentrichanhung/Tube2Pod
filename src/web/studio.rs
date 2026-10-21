use crate::db::{episodes, shows};
use crate::error::{AppError, AppResult};
use crate::models::Episode;
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
}

#[derive(Template)]
#[template(path = "partials/queue.html")]
pub struct QueueTemplate {
    episodes: Vec<Episode>,
}

#[derive(Template)]
#[template(path = "partials/preview.html")]
pub struct PreviewTemplate {
    title: String,
    channel: String,
    duration: String,
    language: String,
    has_captions: bool,
    thumbnail: String,
    has_thumbnail: bool,
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
}

impl From<&PreviewData> for PreviewTemplate {
    fn from(data: &PreviewData) -> Self {
        Self {
            title: data.title.clone(),
            channel: data.channel.clone().unwrap_or_default(),
            duration: duration_label(data.duration_s),
            language: data.language.unwrap_or("không xác định").to_string(),
            has_captions: data.has_captions,
            thumbnail: data.thumbnail.clone().unwrap_or_default(),
            has_thumbnail: data.thumbnail.is_some(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct StudioQuery {
    pub show: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateForm {
    pub _csrf: String,
    pub url: String,
    pub show_id: i64,
    pub minutes: i64,
}

#[derive(Debug, Deserialize)]
pub struct PreviewForm {
    pub _csrf: String,
    pub url: String,
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
    Ok(Html(
        StudioTemplate {
            csrf: token.0,
            shows,
            recent,
            selected_show: query.show,
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
                }))
                .into_response())
            }
        }
        Err(error) => {
            if htmx {
                Ok(Html(
                    PreviewErrorTemplate {
                        message: error.to_string(),
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
    Ok(PreviewData {
        title: meta.title,
        channel: meta.channel,
        duration_s: meta.duration,
        language,
        has_captions,
        thumbnail: meta.thumbnail,
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

pub async fn queue(State(state): State<AppState>) -> AppResult<impl IntoResponse> {
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
    Ok(Html(QueueTemplate { episodes }.render()?))
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
