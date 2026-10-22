use crate::db::shows;
use crate::error::{AppError, AppResult};
use crate::models::language_label;
use crate::services::omnivoice::{OmnivoiceClient, Voice};
use crate::state::AppState;
use crate::web::csrf::{self, CsrfToken};
use askama::Template;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, HeaderValue};
use axum::response::{Html, Response};
use axum::Extension;
use axum::Form;
use serde::Deserialize;

#[derive(Template)]
#[template(path = "partials/voice_picker.html")]
pub struct VoicePickerTemplate {
    pub csrf: String,
    pub show_id: i64,
    pub language: String,
    pub language_label: &'static str,
    pub voices: Vec<Voice>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PreviewQuery {
    pub voice_id: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct VoiceForm {
    pub _csrf: String,
    pub voice_id: i64,
    pub voice_name: String,
}

pub fn validate_language(language: &str) -> AppResult<()> {
    if matches!(language, "vi" | "en" | "ko") {
        Ok(())
    } else {
        Err(AppError::BadRequest("Ngôn ngữ không được hỗ trợ.".into()))
    }
}

pub fn filter_ready_voices<'a>(voices: &'a [Voice], language: &str) -> Vec<&'a Voice> {
    voices
        .iter()
        .filter(|voice| voice.status == "ready" && voice.language.as_deref() == Some(language))
        .collect()
}

pub async fn picker(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Path((show_id, language)): Path<(i64, String)>,
) -> AppResult<Html<String>> {
    validate_language(&language)?;
    shows::get(&state.pool, show_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let client = OmnivoiceClient::new(&state.cfg.omnivoice);
    let voices = client
        .list_voices()
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    let voices = filter_ready_voices(&voices, &language)
        .into_iter()
        .cloned()
        .collect();
    Ok(Html(
        VoicePickerTemplate {
            csrf: token.0,
            show_id,
            language_label: language_label(&language),
            language,
            voices,
        }
        .render()?,
    ))
}

pub async fn preview(
    State(state): State<AppState>,
    Path((show_id, language)): Path<(i64, String)>,
    Query(query): Query<PreviewQuery>,
) -> AppResult<Response> {
    validate_language(&language)?;
    shows::get(&state.pool, show_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let voice_id = match query.voice_id {
        Some(id) => id,
        None => shows::get_voice(&state.pool, show_id, &language)
            .await?
            .map(|(id, _)| id)
            .ok_or(AppError::NotFound)?,
    };
    let client = OmnivoiceClient::new(&state.cfg.omnivoice);
    let voices = client
        .list_voices()
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    let voice = voices
        .iter()
        .find(|voice| {
            voice.id == voice_id
                && voice.status == "ready"
                && voice.language.as_deref() == Some(language.as_str())
        })
        .ok_or_else(|| AppError::BadRequest("Giọng đọc không còn khả dụng.".into()))?;
    let preview = client.preview_voice(voice.id).await?;
    let content_type = HeaderValue::from_str(&preview.content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("audio/mpeg"));
    let content_length = HeaderValue::from_str(&preview.bytes.len().to_string())
        .unwrap_or_else(|_| HeaderValue::from_static("0"));
    let mut response = Response::new(Body::from(preview.bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, content_type);
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=86400"));
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, content_length);
    Ok(response)
}

pub async fn select(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Path((show_id, language)): Path<(i64, String)>,
    Form(form): Form<VoiceForm>,
) -> AppResult<Response> {
    csrf::verify(&token, &headers, &form._csrf)?;
    validate_language(&language)?;
    shows::get(&state.pool, show_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let client = OmnivoiceClient::new(&state.cfg.omnivoice);
    let voices = client
        .list_voices()
        .await
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    let selected = voices
        .iter()
        .find(|voice| {
            voice.id == form.voice_id
                && voice.status == "ready"
                && voice.language.as_deref() == Some(language.as_str())
        })
        .ok_or_else(|| AppError::BadRequest("Giọng đọc không còn khả dụng.".into()))?;
    let name = selected.name.clone().unwrap_or(form.voice_name);
    shows::upsert_voice(&state.pool, show_id, &language, selected.id, &name).await?;
    Ok(crate::web::shows::redirect_response(
        &headers,
        &format!("/shows/{show_id}?tab=voices"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::services::omnivoice::Voice;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Method, Request, StatusCode};
    use std::sync::Arc;
    use tokio::sync::Notify;
    use tower::ServiceExt;
    use wiremock::matchers::{header as wm_header, method, path};
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
                "ok": true,
                "data": voices,
                "error": null
            })))
            .mount(server)
            .await;
    }

    fn ready_voices() -> serde_json::Value {
        serde_json::json!([{"id": 42, "name": "Phượng", "status": "ready", "language": "vi"}])
    }

    async fn create_show(context: &Ctx) -> i64 {
        shows::create(
            &context.state.pool,
            shows::NewShow {
                title: "Show".into(),
                ..shows::NewShow::default()
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

    async fn request(
        context: &Ctx,
        method: Method,
        path: &str,
        body: Option<String>,
        cookie: &str,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(header::COOKIE, format!("t2p_csrf={cookie}"));
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

    fn voice(id: i64, language: Option<&str>, status: &str) -> Voice {
        Voice {
            id,
            name: Some(format!("Voice {id}")),
            status: status.into(),
            language: language.map(str::to_string),
            ..Voice::default()
        }
    }

    #[test]
    fn filters_to_ready_voices_in_the_requested_language() {
        let voices = vec![
            voice(1, Some("vi"), "ready"),
            voice(2, Some("vi"), "failed"),
            voice(3, Some("en"), "ready"),
            voice(4, None, "ready"),
        ];
        let filtered = filter_ready_voices(&voices, "vi");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, 1);
    }

    #[test]
    fn reject_unsupported_language_before_network() {
        assert!(validate_language("ja").is_err());
        assert!(validate_language("vi").is_ok());
    }

    #[tokio::test]
    async fn select_persists_a_ready_voice() {
        let server = MockServer::start().await;
        mount_voices(&server, ready_voices()).await;
        let context = ctx(&server).await;
        let show_id = create_show(&context).await;

        let response = request(
            &context,
            Method::POST,
            &format!("/shows/{show_id}/voices/vi"),
            Some(form(&[
                ("_csrf", TOKEN),
                ("voice_id", "42"),
                ("voice_name", ""),
            ])),
            TOKEN,
        )
        .await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            shows::get_voice(&context.state.pool, show_id, "vi")
                .await
                .unwrap(),
            Some((42, "Phượng".to_string()))
        );
    }

    #[tokio::test]
    async fn select_rejects_a_bad_csrf_token_without_writing() {
        let server = MockServer::start().await;
        mount_voices(&server, ready_voices()).await;
        let context = ctx(&server).await;
        let show_id = create_show(&context).await;

        let response = request(
            &context,
            Method::POST,
            &format!("/shows/{show_id}/voices/vi"),
            Some(form(&[
                ("_csrf", "deadbeef"),
                ("voice_id", "42"),
                ("voice_name", ""),
            ])),
            TOKEN,
        )
        .await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            shows::get_voice(&context.state.pool, show_id, "vi")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn select_rejects_a_voice_that_is_not_ready() {
        let server = MockServer::start().await;
        mount_voices(
            &server,
            serde_json::json!([{"id": 42, "name": "Phượng", "status": "failed", "language": "vi"}]),
        )
        .await;
        let context = ctx(&server).await;
        let show_id = create_show(&context).await;

        let response = request(
            &context,
            Method::POST,
            &format!("/shows/{show_id}/voices/vi"),
            Some(form(&[
                ("_csrf", TOKEN),
                ("voice_id", "42"),
                ("voice_name", ""),
            ])),
            TOKEN,
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            shows::get_voice(&context.state.pool, show_id, "vi")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn picker_lists_ready_voices_and_rejects_unsupported_languages() {
        let server = MockServer::start().await;
        mount_voices(&server, ready_voices()).await;
        let context = ctx(&server).await;
        let show_id = create_show(&context).await;

        let response = request(
            &context,
            Method::GET,
            &format!("/shows/{show_id}/voices/vi"),
            None,
            TOKEN,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_text(response).await;
        assert!(html.contains("Phượng"));
        assert!(html.contains(&format!("action=\"/shows/{show_id}/voices/vi\"")));

        let unsupported = request(
            &context,
            Method::GET,
            &format!("/shows/{show_id}/voices/ja"),
            None,
            TOKEN,
        )
        .await;
        assert_eq!(unsupported.status(), StatusCode::BAD_REQUEST);
    }

    async fn mount_preview(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/api/voices/42/preview"))
            .and(wm_header("tailscale-user-login", "owner"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "audio/mpeg")
                    .set_body_bytes(b"ID3fake".to_vec()),
            )
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn preview_proxy_serves_ready_voice_audio() {
        let server = MockServer::start().await;
        mount_voices(&server, ready_voices()).await;
        mount_preview(&server).await;
        let context = ctx(&server).await;
        let show_id = create_show(&context).await;

        let response = request(
            &context,
            Method::GET,
            &format!("/shows/{show_id}/voices/vi/preview?voice_id=42"),
            None,
            TOKEN,
        )
        .await;

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
        assert!(response.headers().get(header::CACHE_CONTROL).is_some());
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        assert_eq!(&bytes[..], b"ID3fake");
    }

    #[tokio::test]
    async fn preview_proxy_rejects_wrong_language_and_non_ready_voices() {
        let server = MockServer::start().await;
        mount_voices(
            &server,
            serde_json::json!([
                {"id": 42, "name": "Phượng", "status": "ready", "language": "en"},
                {"id": 43, "name": "Minh", "status": "failed", "language": "vi"}
            ]),
        )
        .await;
        let context = ctx(&server).await;
        let show_id = create_show(&context).await;

        for voice_id in [42, 43] {
            let response = request(
                &context,
                Method::GET,
                &format!("/shows/{show_id}/voices/vi/preview?voice_id={voice_id}"),
                None,
                TOKEN,
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn preview_proxy_rejects_a_missing_show() {
        let server = MockServer::start().await;
        let context = ctx(&server).await;
        let response = request(
            &context,
            Method::GET,
            "/shows/999/voices/vi/preview?voice_id=42",
            None,
            TOKEN,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
