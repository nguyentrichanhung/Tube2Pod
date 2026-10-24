use crate::db::{episodes, shows};
use crate::error::{AppError, AppResult};
use crate::models::language_label;
use crate::services::omnivoice::{DesignRequest, OmnivoiceClient, Voice, VoiceDesign};
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
    sample_text: String,
    min_chars: usize,
    max_chars: usize,
    levels: Vec<String>,
    default_level: String,
    default_name: String,
    show_id: Option<i64>,
}

impl DesignFormTemplate {
    pub fn level_label(&self, level: &str) -> String {
        match level {
            "stable" => "Ổn định",
            "balanced" => "Cân bằng",
            "creative" => "Sáng tạo",
            "wild" => "Tự do",
            other => other,
        }
        .to_string()
    }
}

#[derive(Debug, Deserialize)]
pub struct SaveDesignForm {
    pub _csrf: String,
    pub name: String,
    pub candidate: i64,
    pub show_id: Option<i64>,
    pub language: Option<String>,
}

#[derive(Template)]
#[template(path = "voice_design_page.html")]
pub struct DesignPageTemplate {
    csrf: String,
    design: VoiceDesign,
    language: Option<String>,
    language_label: &'static str,
    show_id: Option<i64>,
    show_title: Option<String>,
    poll_url: String,
    retry_url: String,
}

#[derive(Template)]
#[template(path = "partials/voice_design_card.html")]
pub struct DesignCardTemplate {
    csrf: String,
    design: VoiceDesign,
    show_id: Option<i64>,
    language: Option<String>,
    poll_url: String,
    retry_url: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct DesignPageQuery {
    pub show: Option<i64>,
    pub language: Option<String>,
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
    let selected = catalog
        .languages
        .get(&selected_language)
        .cloned()
        .unwrap_or_default();
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
            sample_text: selected.sample_text,
            min_chars: selected.min_chars,
            max_chars: selected.max_chars,
            levels: catalog.levels,
            default_level: catalog.default_level,
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
    if !catalog.level_params.contains_key(level) {
        return Err(AppError::BadRequest("Mức sáng tạo không hợp lệ.".into()));
    }
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

fn design_card_url(id: i64, show_id: Option<i64>, language: &str) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("language", language);
    if let Some(show_id) = show_id {
        serializer.append_pair("show", &show_id.to_string());
    }
    format!("/voices/designs/{id}/card?{}", serializer.finish())
}

fn design_retry_url(show_id: Option<i64>, language: Option<&str>) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    if let Some(show_id) = show_id {
        serializer.append_pair("show", &show_id.to_string());
    }
    if let Some(language) = language {
        serializer.append_pair("language", language);
    }
    let query = serializer.finish();
    if query.is_empty() {
        "/voices/new".into()
    } else {
        format!("/voices/new?{query}")
    }
}

fn log_design_failure(design: &VoiceDesign) {
    if design.status == "failed" {
        tracing::warn!(
            design_id = design.id,
            error = ?design.error,
            "voice design failed"
        );
    }
}

pub async fn design(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Path(id): Path<i64>,
    Query(query): Query<DesignPageQuery>,
) -> AppResult<Html<String>> {
    let client = OmnivoiceClient::new(&state.cfg.omnivoice);
    let design = client
        .design_status(id)
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    let language = query
        .language
        .clone()
        .or_else(|| design.language.clone())
        .unwrap_or_else(|| "vi".into());
    log_design_failure(&design);
    let poll_url = design_card_url(design.id, query.show, &language);
    let retry_url = design_retry_url(query.show, Some(&language));
    let show_title = match query.show {
        Some(show_id) => shows::get(&state.pool, show_id)
            .await?
            .map(|show| show.title),
        None => None,
    };
    Ok(Html(
        DesignPageTemplate {
            csrf: token.0,
            design,
            language: Some(language.clone()),
            language_label: language_label(&language),
            show_id: query.show,
            show_title,
            poll_url,
            retry_url,
        }
        .render()?,
    ))
}

pub async fn design_card(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Path(id): Path<i64>,
    Query(query): Query<DesignPageQuery>,
) -> AppResult<Html<String>> {
    let client = OmnivoiceClient::new(&state.cfg.omnivoice);
    let design = client
        .design_status(id)
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    let language = query.language.clone().or_else(|| design.language.clone());
    log_design_failure(&design);
    let poll_url = design_card_url(design.id, query.show, language.as_deref().unwrap_or("vi"));
    let retry_url = design_retry_url(query.show, language.as_deref());
    Ok(Html(
        DesignCardTemplate {
            csrf: token.0,
            design,
            show_id: query.show,
            language,
            poll_url,
            retry_url,
        }
        .render()?,
    ))
}

pub async fn candidate_audio(
    State(state): State<AppState>,
    Path((id, index)): Path<(i64, i64)>,
) -> AppResult<Response> {
    let client = OmnivoiceClient::new(&state.cfg.omnivoice);
    let design = client
        .design_status(id)
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    if design.status != "ready" || !design.candidates.iter().any(|c| c.index == index) {
        return Err(AppError::NotFound);
    }
    let audio = client.candidate_audio(id, index).await?;
    audio_response(audio)
}

pub async fn save_design(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Form(form): Form<SaveDesignForm>,
) -> AppResult<Redirect> {
    csrf::verify(&token, &headers, &form._csrf)?;
    let name = form.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(AppError::BadRequest(
            "Tên giọng phải từ 1 đến 120 ký tự.".into(),
        ));
    }
    let client = OmnivoiceClient::new(&state.cfg.omnivoice);
    let design = client
        .design_status(id)
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    if design.status != "ready" {
        return Err(AppError::Conflict("Giọng chưa sẵn sàng để lưu.".into()));
    }
    if !design.candidates.iter().any(|c| c.index == form.candidate) {
        return Err(AppError::BadRequest("Bản thử không hợp lệ.".into()));
    }
    let assignment = match form.show_id {
        None => None,
        Some(show_id) => {
            let language = form
                .language
                .as_deref()
                .ok_or_else(|| AppError::BadRequest("Thiếu ngôn ngữ để gán.".into()))?;
            if !matches!(language, "vi" | "en" | "ko") {
                return Err(AppError::BadRequest("Ngôn ngữ không được hỗ trợ.".into()));
            }
            shows::get(&state.pool, show_id)
                .await?
                .ok_or(AppError::NotFound)?;
            if let Some(expected) = design.language.as_deref() {
                if expected != language {
                    return Err(AppError::BadRequest(
                        "Ngôn ngữ không khớp với thiết kế.".into(),
                    ));
                }
            }
            Some((show_id, language))
        }
    };
    let voice = client.save_design(id, &name, form.candidate).await?;
    if let Some((show_id, language)) = assignment {
        shows::upsert_voice(&state.pool, show_id, language, voice.id, &name).await?;
    }
    Ok(Redirect::to(&format!("/voices/{}", voice.id)))
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
                health_cache: Arc::new(tokio::sync::Mutex::new(None)),
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

    async fn mount_ready_design(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/api/voice-designs/3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {
                    "id": 3, "status": "ready", "error": null, "language": "vi",
                    "candidates": [
                        {"index": 0, "audio_url": "/api/voice-designs/3/candidates/0/audio"},
                        {"index": 1, "audio_url": "/api/voice-designs/3/candidates/1/audio"},
                        {"index": 2, "audio_url": "/api/voice-designs/3/candidates/2/audio"}
                    ]
                },
                "error": null
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

    #[tokio::test]
    async fn design_form_uses_selected_language_sample() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voice-designs/options"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {
                    "genders": ["female"], "ages": ["middle_aged"], "pitches": ["low"], "styles": ["whisper"],
                    "levels": {"balanced": {"position_temperature": 5.0}},
                    "default_level": "balanced",
                    "languages": {
                        "vi": {"min_chars": 5, "max_chars": 50, "sample_text": "Xin chào"},
                        "en": {"min_chars": 12, "max_chars": 40, "sample_text": "English sample"},
                        "ko": {"min_chars": 3, "max_chars": 20, "sample_text": "안녕"}
                    }
                },
                "error": null
            })))
            .mount(&server)
            .await;
        let context = ctx(&server).await;

        let response = request(&context, Method::GET, "/voices/new?language=en", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_text(response).await;
        assert!(html.contains("required>English sample</textarea>"));
        assert!(html.contains("Từ 12 đến 40 ký tự."));
    }

    #[tokio::test]
    async fn create_design_rejects_unknown_level_without_creating() {
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
                ("level", "wild"),
                ("text", "Xin chào các bạn"),
            ])),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let requests = server.received_requests().await.unwrap();
        assert!(!requests
            .iter()
            .any(|request| request.method.as_str() == "POST"
                && request.url.path() == "/api/voice-designs"));
    }

    #[tokio::test]
    async fn design_page_lists_three_candidates() {
        let server = MockServer::start().await;
        mount_ready_design(&server).await;
        let context = ctx(&server).await;

        let response = request(&context, Method::GET, "/voices/designs/3", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_text(response).await;
        assert!(html.contains("candidates/0/audio"));
        assert!(html.contains("candidates/1/audio"));
        assert!(html.contains("candidates/2/audio"));
        assert!(html.contains("Lưu giọng"));
    }

    #[tokio::test]
    async fn candidate_audio_proxies_ready_take() {
        let server = MockServer::start().await;
        mount_ready_design(&server).await;
        Mock::given(method("GET"))
            .and(path("/api/voice-designs/3/candidates/1/audio"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "audio/wav")
                    .set_body_bytes(b"RIFFfake".to_vec()),
            )
            .mount(&server)
            .await;
        let context = ctx(&server).await;

        let response = request(
            &context,
            Method::GET,
            "/voices/designs/3/candidates/1/audio",
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "audio/wav"
        );
    }

    #[tokio::test]
    async fn save_design_creates_voice_and_auto_assigns_from_show() {
        let server = MockServer::start().await;
        mount_ready_design(&server).await;
        Mock::given(method("POST"))
            .and(path("/api/voice-designs/3/save"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "ok": true,
                "data": {"id": 11, "name": "Giọng mới", "status": "measuring", "language": "vi", "preview_url": null, "design": null},
                "error": null
            })))
            .mount(&server)
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

        let response = request(
            &context,
            Method::POST,
            "/voices/designs/3/save",
            Some(form(&[
                ("_csrf", TOKEN),
                ("name", "Giọng mới"),
                ("candidate", "1"),
                ("show_id", &show.id.to_string()),
                ("language", "vi"),
            ])),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "/voices/11"
        );
        assert_eq!(
            shows::get_voice(&context.state.pool, show.id, "vi")
                .await
                .unwrap(),
            Some((11, "Giọng mới".to_string()))
        );
    }

    #[tokio::test]
    async fn save_design_without_show_saves_unassigned() {
        let server = MockServer::start().await;
        mount_ready_design(&server).await;
        Mock::given(method("POST"))
            .and(path("/api/voice-designs/3/save"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "ok": true,
                "data": {"id": 11, "name": "Giọng mới", "status": "measuring", "language": "vi", "preview_url": null, "design": null},
                "error": null
            })))
            .mount(&server)
            .await;
        let context = ctx(&server).await;

        let response = request(
            &context,
            Method::POST,
            "/voices/designs/3/save",
            Some(form(&[
                ("_csrf", TOKEN),
                ("name", "Giọng mới"),
                ("candidate", "1"),
                ("language", "vi"),
            ])),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "/voices/11"
        );
        assert!(shows::voice_assignments(&context.state.pool)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn save_design_rejects_a_bad_candidate() {
        let server = MockServer::start().await;
        mount_ready_design(&server).await;
        let context = ctx(&server).await;

        let response = request(
            &context,
            Method::POST,
            "/voices/designs/3/save",
            Some(form(&[
                ("_csrf", TOKEN),
                ("name", "Giọng mới"),
                ("candidate", "9"),
            ])),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn save_design_validates_assignment_before_creating_voice() {
        let server = MockServer::start().await;
        mount_ready_design(&server).await;
        Mock::given(method("POST"))
            .and(path("/api/voice-designs/3/save"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "ok": true,
                "data": {"id": 11, "name": "Giọng mới", "status": "measuring", "language": "vi", "preview_url": null, "design": null},
                "error": null
            })))
            .expect(0)
            .mount(&server)
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

        let missing_language = request(
            &context,
            Method::POST,
            "/voices/designs/3/save",
            Some(form(&[
                ("_csrf", TOKEN),
                ("name", "Giọng mới"),
                ("candidate", "1"),
                ("show_id", &show.id.to_string()),
            ])),
        )
        .await;
        assert_eq!(missing_language.status(), StatusCode::BAD_REQUEST);

        let unknown_show = request(
            &context,
            Method::POST,
            "/voices/designs/3/save",
            Some(form(&[
                ("_csrf", TOKEN),
                ("name", "Giọng mới"),
                ("candidate", "1"),
                ("show_id", "999999"),
                ("language", "vi"),
            ])),
        )
        .await;
        assert_eq!(unknown_show.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn save_design_rejects_not_ready_design() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voice-designs/3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {
                    "id": 3, "status": "queued", "error": null, "language": "vi",
                    "candidates": []
                },
                "error": null
            })))
            .mount(&server)
            .await;
        let context = ctx(&server).await;

        let response = request(
            &context,
            Method::POST,
            "/voices/designs/3/save",
            Some(form(&[
                ("_csrf", TOKEN),
                ("name", "Giọng mới"),
                ("candidate", "0"),
            ])),
        )
        .await;
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn failed_design_card_hides_upstream_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voice-designs/3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {
                    "id": 3, "status": "failed", "error": "upstream internal detail",
                    "language": "vi", "candidates": []
                },
                "error": null
            })))
            .mount(&server)
            .await;
        let context = ctx(&server).await;

        let response = request(&context, Method::GET, "/voices/designs/3/card", None).await;
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_text(response).await;
        assert!(!html.contains("upstream internal detail"));
        assert!(html.contains("Không tạo được giọng. Vui lòng thử lại hoặc liên hệ admin."));
    }
}
