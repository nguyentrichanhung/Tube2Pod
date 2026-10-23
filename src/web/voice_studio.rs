use crate::db::{episodes, shows};
use crate::error::{AppError, AppResult};
use crate::models::language_label;
use crate::services::omnivoice::{OmnivoiceClient, Voice};
use crate::state::AppState;
use crate::web::csrf::{self, CsrfToken};
use askama::Template;
use axum::body::Body;
use axum::extract::{Path, State};
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

#[derive(Template)]
#[template(path = "partials/voice_detail_card.html")]
pub struct VoiceDetailCardTemplate {
    voice: Voice,
    #[allow(dead_code)]
    active: bool,
}

#[derive(Debug, Deserialize)]
pub struct DeleteVoiceForm {
    pub _csrf: String,
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
    let mut rows = Vec::with_capacity(voices.len());
    for voice in voices {
        let assigned = assignments
            .iter()
            .filter(|(voice_id, _, _)| *voice_id == voice.id)
            .map(|(_, show_id, language)| (*show_id, String::new(), language.clone()))
            .collect::<Vec<_>>();
        let assigned = enrich_assignments(&state, assigned).await?;
        let used_by_episodes = episodes::voice_usage_count(&state.pool, voice.id).await?;
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
}
