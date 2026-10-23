use crate::db::{episodes, shows};
use crate::error::{AppError, AppResult};
use crate::models::language_label;
use crate::services::omnivoice::{DesignRequest, OmnivoiceClient, Voice};
use crate::state::AppState;
use crate::web::csrf::{self, CsrfToken};
use askama::Template;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{Html, Redirect, Response};
use axum::Extension;
use axum::Form;
use serde::Deserialize;

pub struct VoiceRow {
    pub voice: Voice,
    pub assignments: Vec<(i64, String, String)>,
    pub used_by_episodes: i64,
}

impl VoiceRow {
    pub fn deletable(&self) -> bool {
        self.assignments.is_empty() && self.used_by_episodes == 0
    }
}

#[derive(Template)]
#[template(path = "voices.html")]
pub struct VoiceLibraryTemplate {
    csrf: String,
    voices: Vec<VoiceRow>,
}

impl VoiceLibraryTemplate {
    pub fn language_label(&self, language: &str) -> &'static str {
        language_label(language)
    }

    pub fn status_label(&self, status: &str) -> &'static str {
        match status {
            "ready" => "Sẵn sàng",
            "measuring" => "Đang đo",
            "failed" => "Lỗi",
            _ => "Đang xử lý",
        }
    }
}

#[derive(Template)]
#[template(path = "voice_detail.html")]
pub struct VoiceDetailTemplate {
    csrf: String,
    voice: Voice,
    assignments: Vec<(i64, String, String)>,
    used_by_episodes: i64,
    active: bool,
}

impl VoiceDetailTemplate {
    pub fn language_label(&self, language: &str) -> &'static str {
        language_label(language)
    }
}

#[derive(Template)]
#[template(path = "partials/voice_detail_card.html")]
pub struct VoiceDetailCardTemplate {
    voice: Voice,
    active: bool,
}

#[derive(Debug, Deserialize)]
pub struct DeleteVoiceForm {
    pub _csrf: String,
}

#[derive(Debug, Deserialize)]
pub struct DesignForm {
    pub _csrf: String,
    pub language: String,
    pub gender: Option<String>,
    pub age: Option<String>,
    pub pitch: Option<String>,
    pub style: Option<String>,
    pub level: Option<String>,
    pub text: String,
    pub show_id: Option<i64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct DesignQuery {
    pub show: Option<i64>,
    pub language: Option<String>,
}

pub struct LanguageChoice {
    pub code: &'static str,
    pub label: &'static str,
    pub sample_text: String,
    pub min_chars: usize,
    pub max_chars: usize,
}

#[derive(Template)]
#[template(path = "voice_design.html")]
pub struct DesignFormTemplate {
    csrf: String,
    languages: Vec<LanguageChoice>,
    selected_language: String,
    default_name: String,
    show_id: Option<i64>,
}

pub async fn index(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
) -> AppResult<Html<String>> {
    let client = OmnivoiceClient::new(&state.cfg.omnivoice);
    let voices = client
        .list_voices()
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    let assignments = shows::voice_assignments(&state.pool).await?;
    let usage = episodes::voice_usage_counts(&state.pool).await?;
    let mut rows = Vec::with_capacity(voices.len());
    for voice in voices {
        let assigned = assignments
            .iter()
            .filter(|(voice_id, _, _)| *voice_id == voice.id)
            .map(|(_, show_id, language)| (*show_id, String::new(), language.clone()))
            .collect::<Vec<_>>();
        let assigned = enrich_assignments(&state, assigned).await?;
        let used_by_episodes = usage
            .iter()
            .find(|(voice_id, _)| *voice_id == voice.id)
            .map(|(_, count)| *count)
            .unwrap_or(0);
        rows.push(VoiceRow {
            voice,
            assignments: assigned,
            used_by_episodes,
        });
    }
    Ok(Html(
        VoiceLibraryTemplate {
            csrf: token.0,
            voices: rows,
        }
        .render()?,
    ))
}

async fn enrich_assignments(
    state: &AppState,
    rows: Vec<(i64, String, String)>,
) -> AppResult<Vec<(i64, String, String)>> {
    let mut enriched = Vec::with_capacity(rows.len());
    for (show_id, _, language) in rows {
        let title = shows::get(&state.pool, show_id)
            .await?
            .map(|show| show.title)
            .unwrap_or_else(|| "Show đã xoá".into());
        enriched.push((show_id, title, language));
    }
    Ok(enriched)
}

pub async fn detail(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Path(id): Path<i64>,
) -> AppResult<Html<String>> {
    let client = OmnivoiceClient::new(&state.cfg.omnivoice);
    let voices = client
        .list_voices()
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    let voice = voices
        .into_iter()
        .find(|voice| voice.id == id)
        .ok_or(AppError::NotFound)?;
    let assignments = shows::voice_assignment_shows(&state.pool, id).await?;
    let used_by_episodes = episodes::voice_usage_count(&state.pool, id).await?;
    let active = matches!(voice.status.as_str(), "queued" | "running" | "measuring");
    Ok(Html(
        VoiceDetailTemplate {
            csrf: token.0,
            voice,
            assignments,
            used_by_episodes,
            active,
        }
        .render()?,
    ))
}

pub async fn detail_card(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Html<String>> {
    let client = OmnivoiceClient::new(&state.cfg.omnivoice);
    let voices = client
        .list_voices()
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    let voice = voices
        .into_iter()
        .find(|voice| voice.id == id)
        .ok_or(AppError::NotFound)?;
    let active = matches!(voice.status.as_str(), "queued" | "running" | "measuring");
    Ok(Html(VoiceDetailCardTemplate { voice, active }.render()?))
}

pub async fn preview(State(state): State<AppState>, Path(id): Path<i64>) -> AppResult<Response> {
    let client = OmnivoiceClient::new(&state.cfg.omnivoice);
    let voices = client
        .list_voices()
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    let voice = voices
        .iter()
        .find(|voice| voice.id == id && voice.status == "ready")
        .ok_or_else(|| AppError::BadRequest("Giọng đọc không còn khả dụng.".into()))?;
    let preview = client.preview_voice(voice.id).await?;
    audio_response(preview)
}

pub async fn delete(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<DeleteVoiceForm>,
) -> AppResult<Redirect> {
    csrf::verify(&token, &headers, &form._csrf)?;
    let assignments = shows::voice_assignment_shows(&state.pool, id).await?;
    let used_by_episodes = episodes::voice_usage_count(&state.pool, id).await?;
    if !assignments.is_empty() || used_by_episodes > 0 {
        let mut reasons = Vec::new();
        if !assignments.is_empty() {
            let names = assignments
                .iter()
                .map(|(_, title, language)| format!("{title} ({language})"))
                .collect::<Vec<_>>()
                .join(", ");
            reasons.push(format!("show {names}"));
        }
        if used_by_episodes > 0 {
            reasons.push(format!("{used_by_episodes} tập"));
        }
        return Err(AppError::BadRequest(format!(
            "Giọng đang được dùng bởi {}. Gỡ trước khi xoá.",
            reasons.join(", ")
        )));
    }
    let client = OmnivoiceClient::new(&state.cfg.omnivoice);
    client.delete_voice(id).await?;
    Ok(Redirect::to("/voices?deleted=1"))
}

pub async fn new_design(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Query(query): Query<DesignQuery>,
) -> AppResult<Html<String>> {
    let client = OmnivoiceClient::new(&state.cfg.omnivoice);
    let catalog = client
        .design_catalog()
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    let selected_language = query
        .language
        .as_deref()
        .filter(|value| matches!(*value, "vi" | "en" | "ko"))
        .unwrap_or("vi")
        .to_string();
    let show_title = match query.show {
        Some(show_id) => shows::get(&state.pool, show_id)
            .await?
            .map(|show| show.title),
        None => None,
    };
    let languages = ["vi", "en", "ko"]
        .into_iter()
        .map(|code| {
            let options = catalog.languages.get(code).cloned().unwrap_or_default();
            LanguageChoice {
                code,
                label: language_label(code),
                sample_text: options.sample_text,
                min_chars: options.min_chars,
                max_chars: options.max_chars,
            }
        })
        .collect();
    let default_name = match &show_title {
        Some(title) => format!(
            "Tube2Pod · {title} · {}",
            language_label(&selected_language)
        ),
        None => format!("Tube2Pod · {}", language_label(&selected_language)),
    };
    Ok(Html(
        DesignFormTemplate {
            csrf: token.0,
            languages,
            selected_language,
            default_name,
            show_id: query.show,
        }
        .render()?,
    ))
}

pub async fn create_design(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Form(form): Form<DesignForm>,
) -> AppResult<Redirect> {
    csrf::verify(&token, &headers, &form._csrf)?;
    if !matches!(form.language.as_str(), "vi" | "en" | "ko") {
        return Err(AppError::BadRequest("Ngôn ngữ không được hỗ trợ.".into()));
    }
    let client = OmnivoiceClient::new(&state.cfg.omnivoice);
    let catalog = client
        .design_catalog()
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    let language = catalog
        .languages
        .get(&form.language)
        .cloned()
        .ok_or_else(|| AppError::BadRequest("Ngôn ngữ không được hỗ trợ.".into()))?;
    let text = form.text.trim().to_string();
    let length = text.chars().count();
    if length < language.min_chars || length > language.max_chars {
        return Err(AppError::BadRequest(format!(
            "Nội dung mẫu phải từ {} đến {} ký tự.",
            language.min_chars, language.max_chars
        )));
    }
    for (label, allowed, value) in [
        ("giới tính", &catalog.genders, &form.gender),
        ("độ tuổi", &catalog.ages, &form.age),
        ("cao độ", &catalog.pitches, &form.pitch),
        ("phong cách", &catalog.styles, &form.style),
    ] {
        if let Some(value) = value {
            if !value.trim().is_empty() && !allowed.contains(value) {
                return Err(AppError::BadRequest(format!("{label} không hợp lệ.")));
            }
        }
    }
    let level = form
        .level
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(catalog.default_level.as_str());
    let params = catalog.level_params.get(level).cloned();
    let request = DesignRequest {
        language: form.language.clone(),
        text,
        gender: form.gender.filter(|value| !value.trim().is_empty()),
        age: form.age.filter(|value| !value.trim().is_empty()),
        pitch: form.pitch.filter(|value| !value.trim().is_empty()),
        style: form.style.filter(|value| !value.trim().is_empty()),
        params,
        seed: None,
    };
    let design = client
        .create_design(&request)
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    let mut location = format!("/voices/designs/{}", design.id);
    if let Some(show_id) = form.show_id {
        location.push_str(&format!("?show={show_id}&language={}", form.language));
    }
    Ok(Redirect::to(&location))
}

pub fn audio_response(preview: crate::services::omnivoice::VoicePreview) -> AppResult<Response> {
    let content_type = if preview
        .content_type
        .to_ascii_lowercase()
        .starts_with("audio/")
    {
        HeaderValue::from_str(&preview.content_type)
            .unwrap_or_else(|_| HeaderValue::from_static("audio/mpeg"))
    } else {
        HeaderValue::from_static("audio/mpeg")
    };
    let content_length = HeaderValue::from_str(&preview.bytes.len().to_string())
        .unwrap_or_else(|_| HeaderValue::from_static("0"));
    let mut response = Response::new(Body::from(preview.bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, content_type);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=86400"),
    );
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, content_length);
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::shows;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Method, Request, StatusCode};
    use std::sync::Arc;
    use tokio::sync::Notify;
    use tower::ServiceExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    struct Ctx {
        state: AppState,
        _dir: tempfile::TempDir,
    }

    async fn ctx(server: &MockServer) -> Ctx {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let base = server.uri();
        let cfg = Config::from_lookup(move |key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "OMNIVOICE_BASE_URL" => Some(base.clone()),
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

    async fn mount_voices(server: &MockServer, voices: serde_json::Value) {
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": voices, "error": null
            })))
            .mount(server)
            .await;
    }

    async fn request(
        context: &Ctx,
        method: Method,
        path: &str,
        body: Option<String>,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::COOKIE, format!("t2p_csrf={TOKEN}"));
        let body = match body {
            Some(body) => {
                builder = builder.header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
                Body::from(body)
            }
            None => Body::empty(),
        };
        crate::web::admin_router(context.state.clone())
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap()
    }

    async fn body_text(response: axum::response::Response) -> String {
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn form(pairs: &[(&str, &str)]) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in pairs {
            serializer.append_pair(key, value);
        }
        serializer.finish()
    }

    #[tokio::test]
    async fn library_lists_voices_with_assignments_and_status() {
        let server = MockServer::start().await;
        mount_voices(
            &server,
            serde_json::json!([
                {"id": 42, "name": "Phượng", "status": "ready", "language": "vi", "preview_url": "/api/voices/42/preview",
                 "design": {"gender": "female", "age": "middle_aged"}}
            ]),
        )
        .await;
        let context = ctx(&server).await;
        let show = shows::create(
            &context.state.pool,
            shows::NewShow {
                title: "Show".into(),
                ..shows::NewShow::default()
            },
        )
        .await
        .unwrap();
        shows::upsert_voice(&context.state.pool, show.id, "vi", 42, "Phượng")
            .await
            .unwrap();

        let response = request(&context, Method::GET, "/voices", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_text(response).await;
        assert!(html.contains("Phượng"));
        assert!(html.contains("Nữ"));
        assert!(html.contains("Show"));
        assert!(html.contains("voice-preview-button"));
    }

    #[tokio::test]
    async fn delete_is_blocked_when_a_show_uses_the_voice() {
        let server = MockServer::start().await;
        mount_voices(
            &server,
            serde_json::json!([{"id": 42, "name": "Phượng", "status": "ready", "language": "vi"}]),
        )
        .await;
        let context = ctx(&server).await;
        let show = shows::create(
            &context.state.pool,
            shows::NewShow {
                title: "Show".into(),
                ..shows::NewShow::default()
            },
        )
        .await
        .unwrap();
        shows::upsert_voice(&context.state.pool, show.id, "vi", 42, "Phượng")
            .await
            .unwrap();

        let response = request(
            &context,
            Method::POST,
            "/voices/42/delete",
            Some(form(&[("_csrf", TOKEN)])),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_removes_an_unused_voice() {
        let server = MockServer::start().await;
        mount_voices(
            &server,
            serde_json::json!([{"id": 42, "name": "Phượng", "status": "ready", "language": "vi"}]),
        )
        .await;
        Mock::given(method("DELETE"))
            .and(path("/api/voices/42"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"ok": true, "data": null, "error": null})),
            )
            .mount(&server)
            .await;
        let context = ctx(&server).await;

        let response = request(
            &context,
            Method::POST,
            "/voices/42/delete",
            Some(form(&[("_csrf", TOKEN)])),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn voice_preview_proxies_ready_audio() {
        let server = MockServer::start().await;
        mount_voices(
            &server,
            serde_json::json!([{"id": 42, "name": "Phượng", "status": "ready", "language": "vi"}]),
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/api/voices/42/preview"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "audio/mpeg")
                    .set_body_bytes(b"ID3fake".to_vec()),
            )
            .mount(&server)
            .await;
        let context = ctx(&server).await;

        let response = request(&context, Method::GET, "/voices/42/preview", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "audio/mpeg"
        );
    }

    #[tokio::test]
    async fn detail_card_polls_while_measuring() {
        let server = MockServer::start().await;
        mount_voices(
            &server,
            serde_json::json!([{"id": 42, "name": "Phượng", "status": "measuring", "language": "vi"}]),
        )
        .await;
        let context = ctx(&server).await;

        let response = request(&context, Method::GET, "/voices/42/card", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_text(response).await;
        assert!(html.contains("id=\"voice-detail-card\""));
        assert!(html.contains("hx-get=\"/voices/42/card\""));
        assert!(html.contains("hx-trigger=\"every 3s\""));
        assert!(html.contains("data-voice-preview"));
    }

    #[tokio::test]
    async fn detail_card_stops_polling_when_ready() {
        let server = MockServer::start().await;
        mount_voices(
            &server,
            serde_json::json!([{"id": 42, "name": "Phượng", "status": "ready", "language": "vi"}]),
        )
        .await;
        let context = ctx(&server).await;

        let response = request(&context, Method::GET, "/voices/42/card", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_text(response).await;
        assert!(html.contains("id=\"voice-detail-card\""));
        assert!(!html.contains("hx-trigger"));
    }

    #[tokio::test]
    async fn design_form_renders_language_samples() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voice-designs/options"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {
                    "genders": ["male", "female"],
                    "ages": ["young_adult", "middle_aged"],
                    "pitches": ["low", "moderate"],
                    "styles": ["whisper"],
                    "levels": {"balanced": {"position_temperature": 5.0}},
                    "default_level": "balanced",
                    "languages": {
                        "vi": {"min_chars": 10, "max_chars": 200, "sample_text": "Xin chào"},
                        "en": {"min_chars": 10, "max_chars": 200, "sample_text": "Hello"},
                        "ko": {"min_chars": 5, "max_chars": 100, "sample_text": "안녕"}
                    }
                },
                "error": null
            })))
            .mount(&server)
            .await;
        let context = ctx(&server).await;

        let response = request(&context, Method::GET, "/voices/new", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_text(response).await;
        assert!(html.contains("Xin chào"));
        assert!(html.contains("balanced"));
        assert!(html.contains("Tạo giọng"));
    }

    #[tokio::test]
    async fn create_design_validates_and_redirects() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voice-designs/options"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {
                    "genders": ["female"], "ages": ["middle_aged"], "pitches": ["low"], "styles": ["whisper"],
                    "levels": {"balanced": {"position_temperature": 5.0}},
                    "default_level": "balanced",
                    "languages": {"vi": {"min_chars": 5, "max_chars": 50, "sample_text": "Xin chào"}}
                },
                "error": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/voice-designs"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "ok": true,
                "data": {"id": 3, "status": "queued", "error": null, "language": "vi", "candidates": []},
                "error": null
            })))
            .mount(&server)
            .await;
        let context = ctx(&server).await;

        let response = request(
            &context,
            Method::POST,
            "/voices/designs",
            Some(form(&[
                ("_csrf", TOKEN),
                ("language", "vi"),
                ("gender", "female"),
                ("age", "middle_aged"),
                ("pitch", "low"),
                ("style", "whisper"),
                ("level", "balanced"),
                ("text", "Xin chào các bạn"),
            ])),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "/voices/designs/3"
        );
    }

    #[tokio::test]
    async fn create_design_rejects_bad_text_and_csrf() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voice-designs/options"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {
                    "genders": ["female"], "ages": ["middle_aged"], "pitches": ["low"], "styles": [],
                    "levels": {"balanced": {}}, "default_level": "balanced",
                    "languages": {"vi": {"min_chars": 5, "max_chars": 10, "sample_text": "Xin chào"}}
                },
                "error": null
            })))
            .mount(&server)
            .await;
        let context = ctx(&server).await;

        let too_long = request(
            &context,
            Method::POST,
            "/voices/designs",
            Some(form(&[
                ("_csrf", TOKEN),
                ("language", "vi"),
                ("text", "nội dung quá dài cho giới hạn"),
            ])),
        )
        .await;
        assert_eq!(too_long.status(), StatusCode::BAD_REQUEST);

        let bad_csrf = request(
            &context,
            Method::POST,
            "/voices/designs",
            Some(form(&[
                ("_csrf", "deadbeef"),
                ("language", "vi"),
                ("text", "Xin chào"),
            ])),
        )
        .await;
        assert_eq!(bad_csrf.status(), StatusCode::FORBIDDEN);
    }
}
