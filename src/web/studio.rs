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
    url: String,
    error: String,
    preview: String,
}

#[derive(Template)]
#[template(path = "partials/queue.html")]
pub struct QueueTemplate {
    csrf: String,
    episodes: Vec<Episode>,
}

#[derive(Template)]
#[template(path = "partials/preview.html")]
pub(crate) struct PreviewTemplate {
    title: String,
    channel: String,
    duration: String,
    thumbnail: String,
    has_thumbnail: bool,
    video_id: String,
    tracks: Vec<PreviewTrack>,
    language: String,
}

#[derive(Template)]
#[template(path = "partials/preview_error.html")]
pub struct PreviewErrorTemplate {
    message: String,
}

struct PreviewTrack {
    key: String,
    language: String,
    selected: bool,
    default: bool,
    original: bool,
    recommended: bool,
}

pub(crate) fn audio_language(code: &str) -> &str {
    if code == "und" {
        return "Chưa rõ ngôn ngữ";
    }
    match crate::models::supported_language(code) {
        Some("vi") => "Tiếng Việt",
        Some("en") => "Tiếng Anh",
        Some("ko") => "Tiếng Hàn",
        _ => code,
    }
}

fn preview_tracks(
    meta: &ytdlp::Metadata,
    show_language: &str,
    selected: Option<&str>,
    _language: Option<&str>,
) -> Vec<PreviewTrack> {
    let tracks = ytdlp::audio_tracks(meta);
    let recommended = !tracks.iter().any(|track| track.is_default);
    tracks
        .iter()
        .enumerate()
        .map(|(index, track)| PreviewTrack {
            key: track.key.clone(),
            language: if index == 0 {
                audio_language(
                    track
                        .source_language
                        .as_deref()
                        .filter(|code| !code.trim().is_empty() && *code != "und")
                        .unwrap_or(show_language),
                )
                .into()
            } else {
                "Chưa rõ ngôn ngữ".into()
            },
            selected: selected.map_or(index == 0, |key| key == track.key),
            default: track.is_default,
            original: track.is_original,
            recommended: index == 0 && recommended,
        })
        .collect()
}

pub(crate) fn selected_track(
    meta: &ytdlp::Metadata,
    show_language: &str,
    key: &str,
    language: Option<&str>,
) -> AppResult<(ytdlp::AudioTrack, String)> {
    let tracks = ytdlp::audio_tracks(meta);
    let index = tracks
        .iter()
        .position(|track| track.key == key)
        .ok_or_else(|| {
            AppError::BadRequest("Bản audio đã chọn không còn khả dụng. Vui lòng chọn lại.".into())
        })?;
    let track = tracks[index].clone();
    let label = if index == 0 {
        track
            .source_language
            .as_deref()
            .filter(|code| !code.trim().is_empty() && *code != "und")
            .unwrap_or(show_language)
    } else {
        language
            .and_then(crate::models::supported_language)
            .unwrap_or("und")
    };
    let label = label.to_owned();
    Ok((track, label))
}

pub(crate) fn preview_template(
    meta: &ytdlp::Metadata,
    video_id: &str,
    show_language: &str,
    selected: Option<&str>,
    language: Option<&str>,
) -> PreviewTemplate {
    let valid_selection = selected.is_none_or(|key| {
        ytdlp::audio_tracks(meta)
            .iter()
            .any(|track| track.key == key)
    });
    PreviewTemplate {
        title: meta.title.clone(),
        channel: meta.channel.clone().unwrap_or_default(),
        duration: duration_label(meta.duration),
        thumbnail: meta.thumbnail.clone().unwrap_or_default(),
        has_thumbnail: meta.thumbnail.is_some(),
        video_id: video_id.into(),
        tracks: preview_tracks(meta, show_language, selected, language),
        language: if valid_selection {
            language.unwrap_or("und")
        } else {
            "und"
        }
        .into(),
    }
}

async fn probe(state: &AppState, video: &ytdlp::VideoId) -> AppResult<ytdlp::Metadata> {
    tokio::time::timeout(Duration::from_secs(15), ytdlp::probe(&state.cfg, video))
        .await
        .map_err(|_| AppError::Upstream("preview timed out".into()))?
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
    #[serde(default)]
    pub source_audio_track: String,
    pub preview_video: Option<String>,
    pub language: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PreviewForm {
    pub _csrf: String,
    pub url: String,
    pub show_id: Option<i64>,
    pub preview_video: Option<String>,
    pub source_audio_track: Option<String>,
    pub language: Option<String>,
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
    Ok(Html(
        StudioTemplate {
            csrf: token.0,
            shows,
            recent,
            selected_show,
            url: String::new(),
            error: String::new(),
            preview: String::new(),
        }
        .render()?,
    ))
}

pub async fn create(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Form(form): Form<CreateForm>,
) -> AppResult<Response> {
    csrf::verify(&token, &headers, &form._csrf)?;
    let video = match ytdlp::validate_url(&form.url) {
        Ok(video) => video,
        Err(error) => return create_error(&state, &token, &form, &error, None).await,
    };
    let show = shows::get(&state.pool, form.show_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if let Some(existing) = episodes::get_by_video_id(&state.pool, &video.id).await? {
        return Ok(duplicate_redirect(&existing.uuid).into_response());
    }
    let meta = match probe(&state, &video).await {
        Ok(meta) => meta,
        Err(error) => return create_error(&state, &token, &form, &error, None).await,
    };
    if form
        .preview_video
        .as_deref()
        .is_some_and(|id| id != video.id)
    {
        return create_error(
            &state,
            &token,
            &form,
            &AppError::BadRequest(
                "Video đã thay đổi. Vui lòng chọn bản audio cho video này.".into(),
            ),
            Some(&meta),
        )
        .await;
    }
    let (track, language) = match selected_track(
        &meta,
        &show.language,
        &form.source_audio_track,
        form.language.as_deref(),
    ) {
        Ok(selection) => selection,
        Err(error) => return create_error(&state, &token, &form, &error, Some(&meta)).await,
    };
    let episode = match episodes::create(
        &state.pool,
        episodes::NewEpisode {
            show_id: show.id,
            video_id: video.id.clone(),
            source_url: format!("https://www.youtube.com/watch?v={}", video.id),
            source_title: meta.title,
            source_channel: meta.channel.unwrap_or_default(),
            source_duration_s: meta.duration,
            source_thumbnail_url: meta.thumbnail,
            source_upload_date: meta.upload_date,
            source_description: meta.description,
            source_audio_track: Some(track.key),
            language,
        },
    )
    .await
    {
        Ok(episode) => episode,
        Err(error) => {
            if let Some(existing) = episodes::get_by_video_id(&state.pool, &video.id).await? {
                return Ok(duplicate_redirect(&existing.uuid).into_response());
            }
            return Err(error);
        }
    };
    state.notify.notify_one();
    Ok(Redirect::to(&format!("/episodes/{}", episode.uuid)).into_response())
}

async fn create_error(
    state: &AppState,
    token: &CsrfToken,
    form: &CreateForm,
    error: &AppError,
    meta: Option<&ytdlp::Metadata>,
) -> AppResult<Response> {
    let shows = shows::list(&state.pool).await?;
    let recent = episodes::list(
        &state.pool,
        &episodes::EpisodeFilter {
            limit: Some(8),
            ..Default::default()
        },
    )
    .await?;
    let preview = if let (Some(meta), Some(show), Ok(video)) = (
        meta,
        shows.iter().find(|show| show.id == form.show_id),
        ytdlp::validate_url(&form.url),
    ) {
        preview_template(meta, &video.id, &show.language, Some(""), None).render()?
    } else {
        String::new()
    };
    Ok(Html(
        StudioTemplate {
            csrf: token.0.clone(),
            shows,
            recent,
            selected_show: Some(form.show_id),
            url: form.url.clone(),
            error: preview_error_message(error),
            preview,
        }
        .render()?,
    )
    .into_response())
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
    let result = async {
        csrf::verify(&token, &headers, &form._csrf)?;
        let video = ytdlp::validate_url(&form.url)?;
        let meta = probe(&state, &video).await?;
        let show = match form.show_id { Some(id) => shows::get(&state.pool, id).await?, None => None };
        let same_video = form.preview_video.as_deref() == Some(video.id.as_str());
        let selected = if same_video { Some(form.source_audio_track.as_deref().unwrap_or("")) } else { None };
        let language = if same_video { form.language.as_deref() } else { None };
        let template = preview_template(&meta, &video.id, show.as_ref().map_or("vi", |s| s.language.as_str()), selected, language);
        if template.tracks.is_empty() { return Err(AppError::BadRequest("Video không có bản audio khả dụng.".into())); }
        if is_htmx(&headers) { Ok(Html(template.render()?).into_response()) }
        else { Ok(Json(json!({"ok": true, "title": meta.title, "channel": meta.channel, "duration_s": meta.duration})).into_response()) }
    }.await;
    match result {
        Ok(response) => Ok(response),
        Err(error) if is_htmx(&headers) => Ok(Html(
            PreviewErrorTemplate {
                message: preview_error_message(&error),
            }
            .render()?,
        )
        .into_response()),
        Err(error) => Err(error),
    }
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
        if matches!(
            episode.status.as_str(),
            crate::models::status::READY | crate::models::status::PUBLISHED
        ) {
            continue;
        }
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
            "DATA_DIR" => Some("/tmp/t2p-studio".into()),
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

    async fn create_show(state: &AppState) -> i64 {
        let show = shows::create(
            &state.pool,
            NewShow {
                title: "Show".into(),
                ..NewShow::default()
            },
        )
        .await
        .unwrap();
        show.id
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
            source_audio_track: None,
            source_description: None,
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

    fn audio_metadata(language: Option<&str>) -> ytdlp::Metadata {
        serde_json::from_value(serde_json::json!({
            "title": "Video", "formats": [
                {"format_id":"1", "acodec":"aac", "vcodec":"none", "audio_track_id":"main", "language":language, "language_preference":5},
                {"format_id":"2", "acodec":"aac", "vcodec":"none", "audio_track_id":"alt"}
            ]
        })).unwrap()
    }

    #[test]
    fn preview_uses_youtube_language_before_show_language() {
        let tracks = preview_tracks(&audio_metadata(Some("en")), "ko", None, None);
        assert_eq!(tracks[0].language, "Tiếng Anh");
        assert_eq!(tracks[1].language, "Chưa rõ ngôn ngữ");
    }

    #[test]
    fn preview_falls_back_to_show_language() {
        let tracks = preview_tracks(&audio_metadata(None), "ko", None, None);
        assert_eq!(tracks[0].language, "Tiếng Hàn");
    }

    #[test]
    fn preview_preserves_other_youtube_languages() {
        let meta = audio_metadata(Some("ur"));
        assert_eq!(preview_tracks(&meta, "ko", None, None)[0].language, "ur");
        assert_eq!(
            selected_track(&meta, "ko", "id:main", Some("ko"))
                .unwrap()
                .1,
            "ur"
        );
    }

    #[test]
    fn selected_alternate_language_defaults_to_und() {
        let meta = audio_metadata(Some("en"));
        assert_eq!(
            selected_track(&meta, "ko", "id:alt", Some("ko")).unwrap().1,
            "ko"
        );
        assert_eq!(
            selected_track(&meta, "ko", "id:alt", None).unwrap().1,
            "und"
        );
    }

    #[test]
    fn create_rejects_a_track_that_disappeared_from_preview() {
        assert!(selected_track(&audio_metadata(Some("en")), "ko", "missing", None).is_err());
    }

    async fn state_with_probe() -> (AppState, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let executable = crate::test_tools::tool(dir.path(), "probe", "metadata\noutput={\"title\":\"Probed\",\"channel\":\"Channel\",\"description\":\"Description\",\"duration\":120,\"upload_date\":\"20240101\",\"formats\":[{\"format_id\":\"1\",\"acodec\":\"aac\",\"audio_track_id\":\"main\",\"language\":\"en\",\"language_preference\":5},{\"format_id\":\"2\",\"acodec\":\"aac\",\"audio_track_id\":\"alt\"}]}");
        let mut state = test_state().await;
        Arc::make_mut(&mut state.cfg).ytdlp_path = executable.to_string_lossy().into_owned();
        (state, dir)
    }

    #[tokio::test]
    async fn create_persists_explicit_alternate_and_its_language() {
        for (label, expected) in [(Some("ko"), "ko"), (None, "und")] {
            let (state, _dir) = state_with_probe().await;
            let show = create_show(&state).await;
            let mut body = form(&[
                ("_csrf", TOKEN),
                ("url", WATCH),
                ("show_id", &show.to_string()),
                ("source_audio_track", "id:alt"),
            ]);
            if let Some(label) = label {
                body.push_str(&format!("&language={label}"));
            }
            let response = post(&state, "/episodes", body, &[]).await;
            assert_eq!(response.status(), StatusCode::SEE_OTHER);
            let rows = episodes::list(&state.pool, &Default::default())
                .await
                .unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].source_audio_track.as_deref(), Some("id:alt"));
            assert_eq!(rows[0].language, expected);
            assert_eq!(rows[0].source_title, "Probed");
            assert_eq!(rows[0].source_description.as_deref(), Some("Description"));
            assert_eq!(rows[0].status, "queued");
        }
    }

    #[tokio::test]
    async fn missing_track_returns_a_form_and_does_not_queue() {
        let (state, _dir) = state_with_probe().await;
        let show = create_show(&state).await;
        let response = post(
            &state,
            "/episodes",
            form(&[
                ("_csrf", TOKEN),
                ("url", WATCH),
                ("show_id", &show.to_string()),
                ("source_audio_track", "missing"),
            ]),
            &[],
        )
        .await;
        let html = response_body(response).await;
        assert!(html.contains("không còn khả dụng"));
        assert!(html.contains("name=\"source_audio_track\""));
        assert!(!html.contains("value=\"id:main\" checked"));
        assert!(!html.contains("value=\"id:alt\" checked"));
        assert!(episodes::list(&state.pool, &Default::default())
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn preview_preserves_selection_for_same_video_only() {
        let (state, _dir) = state_with_probe().await;
        let show = create_show(&state).await;
        for (id, selected) in [(VIDEO_ID, true), ("oldvideo123", false)] {
            let response = post(
                &state,
                "/api/preview",
                form(&[
                    ("_csrf", TOKEN),
                    ("url", WATCH),
                    ("show_id", &show.to_string()),
                    ("preview_video", id),
                    ("source_audio_track", "id:alt"),
                    ("language", "ko"),
                ]),
                &[("hx-request", "true")],
            )
            .await;
            let html = response_body(response).await;
            assert_eq!(html.contains("value=\"id:alt\" checked"), selected);
            assert_eq!(html.contains("value=\"ko\" selected"), selected);
        }
    }

    #[tokio::test]
    async fn preview_lost_track_requires_a_new_choice() {
        let (state, _dir) = state_with_probe().await;
        let show = create_show(&state).await;
        let response = post(
            &state,
            "/api/preview",
            form(&[
                ("_csrf", TOKEN),
                ("url", WATCH),
                ("show_id", &show.to_string()),
                ("preview_video", VIDEO_ID),
                ("source_audio_track", "missing"),
                ("language", "ko"),
            ]),
            &[("hx-request", "true")],
        )
        .await;
        let html = response_body(response).await;
        assert!(!html.contains("value=\"id:main\" checked"));
        assert!(!html.contains("value=\"id:alt\" checked"));
        assert!(html.contains("value=\"und\" selected"));
    }

    #[tokio::test]
    async fn create_rejects_stale_video_selection() {
        let (state, _dir) = state_with_probe().await;
        let show = create_show(&state).await;
        let response = post(
            &state,
            "/episodes",
            form(&[
                ("_csrf", TOKEN),
                ("url", WATCH),
                ("show_id", &show.to_string()),
                ("preview_video", "oldvideo123"),
                ("source_audio_track", "id:alt"),
                ("language", "ko"),
            ]),
            &[],
        )
        .await;
        let html = response_body(response).await;
        assert!(html.contains("Video đã thay đổi"));
        assert!(!html.contains("value=\"id:main\" checked"));
        assert!(!html.contains("value=\"id:alt\" checked"));
        assert!(html.contains("value=\"und\" selected"));
        assert!(episodes::list(&state.pool, &Default::default())
            .await
            .unwrap()
            .is_empty());
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
    async fn queue_excludes_completed_episodes_but_keeps_active_and_retryable() {
        let state = test_state().await;
        let show_id = create_show(&state).await;
        for (index, status) in [
            "queued",
            "running",
            "failed",
            "cancelled",
            "ready",
            "published",
        ]
        .iter()
        .enumerate()
        {
            let episode = episodes::create(
                &state.pool,
                new_episode(show_id, &format!("queue{index:06}"), status),
            )
            .await
            .unwrap();
            sqlx::query("UPDATE episodes SET status = ?1 WHERE uuid = ?2")
                .bind(status)
                .bind(&episode.uuid)
                .execute(&state.pool)
                .await
                .unwrap();
        }
        let html = response_body(get(&state, "/api/queue").await).await;
        for title in ["queued", "running", "failed", "cancelled"] {
            assert!(html.contains(&format!(">{title}</strong>")), "{title}");
        }
        for title in ["ready", "published"] {
            assert!(!html.contains(&format!(">{title}</strong>")), "{title}");
        }
        assert_eq!(html.matches(">Thử lại</button>").count(), 2);
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
        crate::db::episodes::set_stage(&state.pool, &episode.uuid, crate::models::stage::FETCHING)
            .await
            .unwrap();
        crate::db::episodes::set_progress(&state.pool, &episode.uuid, r#"{"percent":22}"#)
            .await
            .unwrap();
        let response = get(&state, "/api/queue").await;
        let html = response_body(response).await;
        assert!(html.contains("role=\"progressbar\""));
        assert!(html.contains("Đang tải"));
        assert!(html.contains("22%"));
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
