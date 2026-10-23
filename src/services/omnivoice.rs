use crate::config::{AuthMode, OmnivoiceConfig};
use crate::error::{AppError, AppResult};
use reqwest::header::{HeaderValue, SET_COOKIE};
use reqwest::{Client, Method, RequestBuilder};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

const MAX_SPEECH_CHARS: usize = 5000;
const MAX_PREVIEW_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Voice {
    pub id: i64,
    pub name: Option<String>,
    pub status: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub preview_url: Option<String>,
    #[serde(default)]
    pub design: Option<VoiceDesignInfo>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct VoiceDesignInfo {
    #[serde(default)]
    pub gender: Option<String>,
    #[serde(default)]
    pub age: Option<String>,
    #[serde(default)]
    pub pitch: Option<String>,
    #[serde(default)]
    pub style: Option<String>,
}

#[derive(Debug)]
pub struct VoicePreview {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

impl Voice {
    pub fn is_ready_for(&self, language: &str) -> bool {
        self.status == "ready" && self.language.as_deref() == Some(language)
    }

    pub fn gender_label(&self) -> &'static str {
        crate::models::voice_gender_label(
            self.design
                .as_ref()
                .and_then(|design| design.gender.as_deref()),
        )
    }

    pub fn age_label(&self) -> &'static str {
        crate::models::voice_age_label(
            self.design
                .as_ref()
                .and_then(|design| design.age.as_deref()),
        )
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpeechTake {
    pub id: i64,
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawDesignCatalog {
    #[serde(default)]
    genders: Vec<String>,
    #[serde(default)]
    ages: Vec<String>,
    #[serde(default)]
    pitches: Vec<String>,
    #[serde(default)]
    styles: Vec<String>,
    #[serde(default)]
    levels: HashMap<String, serde_json::Value>,
    #[serde(default)]
    default_level: Option<String>,
    #[serde(default)]
    languages: HashMap<String, RawDesignLanguage>,
}

#[derive(Debug, Deserialize)]
struct RawDesignLanguage {
    #[serde(default)]
    min_chars: Option<usize>,
    #[serde(default)]
    max_chars: Option<usize>,
    #[serde(default)]
    sample_text: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DesignLanguageOptions {
    pub min_chars: usize,
    pub max_chars: usize,
    pub sample_text: String,
}

#[derive(Debug, Clone, Default)]
pub struct DesignCatalog {
    pub genders: Vec<String>,
    pub ages: Vec<String>,
    pub pitches: Vec<String>,
    pub styles: Vec<String>,
    pub levels: Vec<String>,
    pub default_level: String,
    pub level_params: HashMap<String, serde_json::Value>,
    pub languages: HashMap<String, DesignLanguageOptions>,
}

#[derive(Debug, Clone, Default)]
pub struct DesignRequest {
    pub language: String,
    pub text: String,
    pub gender: Option<String>,
    pub age: Option<String>,
    pub pitch: Option<String>,
    pub style: Option<String>,
    pub params: Option<serde_json::Value>,
    pub seed: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct VoiceDesign {
    pub id: i64,
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub candidates: Vec<DesignCandidate>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DesignCandidate {
    pub index: i64,
    #[allow(dead_code)]
    #[serde(default)]
    pub audio_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    ok: bool,
    data: Option<T>,
    error: Option<ApiErrorBody>,
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    code: String,
    message: String,
}

pub struct OmnivoiceClient {
    http: Client,
    base_url: String,
    auth_mode: AuthMode,
    cookie: Mutex<Option<String>>,
}

impl OmnivoiceClient {
    pub fn new(cfg: &OmnivoiceConfig) -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(cfg.request_timeout_secs))
            .build()
            .expect("reqwest client");
        Self {
            http,
            base_url: cfg.base_url.trim_end_matches('/').to_string(),
            auth_mode: cfg.auth_mode.clone(),
            cookie: Mutex::new(None),
        }
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let builder = self
            .http
            .request(method, format!("{}{}", self.base_url, path))
            .header("x-requested-with", "omnivoice");
        match &self.auth_mode {
            AuthMode::Tailscale { user } => builder.header("tailscale-user-login", user),
            AuthMode::Password { .. } => {
                let cookie = self.cookie.lock().unwrap().clone();
                match cookie {
                    Some(value) => builder.header(reqwest::header::COOKIE, value),
                    None => builder,
                }
            }
        }
    }

    async fn login(&self) -> AppResult<()> {
        let AuthMode::Password { password } = &self.auth_mode else {
            return Ok(());
        };
        let response = self
            .http
            .post(format!("{}/api/auth/login", self.base_url))
            .header("x-requested-with", "omnivoice")
            .json(&json!({ "password": password }))
            .send()
            .await
            .map_err(|e| AppError::Upstream(e.to_string()))?;
        if !response.status().is_success() {
            return Err(AppError::Upstream("Omnivoice login failed".into()));
        }
        let cookie = response
            .headers()
            .get(SET_COOKIE)
            .and_then(|value: &HeaderValue| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(str::to_string)
            .ok_or_else(|| AppError::Upstream("Omnivoice login returned no cookie".into()))?;
        *self.cookie.lock().unwrap() = Some(cookie);
        Ok(())
    }

    async fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> AppResult<Option<T>> {
        let mut retried = false;
        loop {
            let mut request = self.request(method.clone(), path);
            if let Some(body) = &body {
                request = request.json(body);
            }
            let response = request
                .send()
                .await
                .map_err(|e| AppError::Upstream(e.to_string()))?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED
                && !retried
                && matches!(self.auth_mode, AuthMode::Password { .. })
            {
                self.login().await?;
                retried = true;
                continue;
            }
            let status = response.status();
            let envelope: Envelope<T> = response
                .json()
                .await
                .map_err(|e| AppError::Upstream(format!("invalid Omnivoice response: {e}")))?;
            if !status.is_success() || !envelope.ok {
                let message = envelope
                    .error
                    .map(|e| format!("{}: {}", e.code, e.message))
                    .unwrap_or_else(|| format!("Omnivoice HTTP {status}"));
                return Err(AppError::Upstream(message));
            }
            return Ok(envelope.data);
        }
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> AppResult<T> {
        self.send(method, path, body)
            .await?
            .ok_or_else(|| AppError::Upstream("Omnivoice returned no data".into()))
    }

    pub async fn list_voices(&self) -> AppResult<Vec<Voice>> {
        self.call(Method::GET, "/api/voices", None).await
    }

    pub async fn design_catalog(&self) -> AppResult<DesignCatalog> {
        let raw: RawDesignCatalog = self
            .call(Method::GET, "/api/voice-designs/options", None)
            .await?;
        let mut levels: Vec<String> = raw.levels.keys().cloned().collect();
        levels.sort();
        let languages = raw
            .languages
            .into_iter()
            .map(|(code, language)| {
                (
                    code,
                    DesignLanguageOptions {
                        min_chars: language.min_chars.unwrap_or(20),
                        max_chars: language.max_chars.unwrap_or(500),
                        sample_text: language.sample_text.unwrap_or_default(),
                    },
                )
            })
            .collect();
        Ok(DesignCatalog {
            genders: raw.genders,
            ages: raw.ages,
            pitches: raw.pitches,
            styles: raw.styles,
            default_level: raw.default_level.unwrap_or_else(|| "balanced".into()),
            levels,
            level_params: raw.levels,
            languages,
        })
    }

    pub async fn create_design(&self, request: &DesignRequest) -> AppResult<VoiceDesign> {
        let mut body = json!({
            "language": request.language,
            "text": request.text,
        });
        if let Some(value) = &request.gender {
            body["gender"] = json!(value);
        }
        if let Some(value) = &request.age {
            body["age"] = json!(value);
        }
        if let Some(value) = &request.pitch {
            body["pitch"] = json!(value);
        }
        if let Some(value) = &request.style {
            body["style"] = json!(value);
        }
        if let Some(value) = &request.params {
            body["params"] = value.clone();
        }
        if let Some(value) = request.seed {
            body["seed"] = json!(value);
        }
        self.call(Method::POST, "/api/voice-designs", Some(body))
            .await
    }

    pub async fn design_status(&self, id: i64) -> AppResult<VoiceDesign> {
        self.call(Method::GET, &format!("/api/voice-designs/{id}"), None)
            .await
    }

    pub async fn candidate_audio(&self, design_id: i64, index: i64) -> AppResult<VoicePreview> {
        self.get_audio(&format!(
            "/api/voice-designs/{design_id}/candidates/{index}/audio"
        ))
        .await
    }

    pub async fn save_design(&self, id: i64, name: &str, candidate: i64) -> AppResult<Voice> {
        self.call(
            Method::POST,
            &format!("/api/voice-designs/{id}/save"),
            Some(json!({ "name": name, "candidate": candidate })),
        )
        .await
    }

    pub async fn delete_voice(&self, id: i64) -> AppResult<()> {
        self.send::<Value>(Method::DELETE, &format!("/api/voices/{id}"), None)
            .await
            .map(|_| ())
    }

    pub async fn delete_design(&self, id: i64) -> AppResult<()> {
        self.send::<Value>(Method::DELETE, &format!("/api/voice-designs/{id}"), None)
            .await
            .map(|_| ())
    }

    pub async fn create_speech(&self, text: &str, voice_id: i64) -> AppResult<SpeechTake> {
        if text.chars().count() > MAX_SPEECH_CHARS {
            return Err(AppError::BadRequest(format!(
                "speech text exceeds {MAX_SPEECH_CHARS} characters"
            )));
        }
        self.call(
            Method::POST,
            "/api/speech",
            Some(json!({
                "text": text,
                "voice_id": voice_id,
                "preset": "balanced",
                "variation": "stable",
            })),
        )
        .await
    }

    pub async fn speech(&self, id: i64) -> AppResult<SpeechTake> {
        self.call(Method::GET, &format!("/api/speech/{id}"), None)
            .await
    }

    pub async fn download_mp3(&self, id: i64) -> AppResult<Vec<u8>> {
        let path = format!("/api/speech/{id}/audio?format=mp3");
        let mut retried = false;
        loop {
            let response = self
                .request(Method::GET, &path)
                .send()
                .await
                .map_err(|e| AppError::Upstream(e.to_string()))?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED
                && !retried
                && matches!(self.auth_mode, AuthMode::Password { .. })
            {
                self.login().await?;
                retried = true;
                continue;
            }
            if !response.status().is_success() {
                return Err(AppError::Upstream(format!(
                    "Omnivoice audio HTTP {}",
                    response.status()
                )));
            }
            return Ok(response
                .bytes()
                .await
                .map_err(|e| AppError::Upstream(e.to_string()))?
                .to_vec());
        }
    }

    async fn get_audio(&self, path: &str) -> AppResult<VoicePreview> {
        let mut retried = false;
        loop {
            let response = self
                .request(Method::GET, path)
                .send()
                .await
                .map_err(|e| AppError::Upstream(e.to_string()))?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED
                && !retried
                && matches!(self.auth_mode, AuthMode::Password { .. })
            {
                self.login().await?;
                retried = true;
                continue;
            }
            if !response.status().is_success() {
                return Err(AppError::Upstream(format!(
                    "Omnivoice audio HTTP {}",
                    response.status()
                )));
            }
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("audio/mpeg")
                .to_string();
            let bytes = response
                .bytes()
                .await
                .map_err(|e| AppError::Upstream(e.to_string()))?;
            if bytes.len() > MAX_PREVIEW_BYTES {
                return Err(AppError::Upstream("Omnivoice audio is too large".into()));
            }
            return Ok(VoicePreview {
                content_type,
                bytes: bytes.to_vec(),
            });
        }
    }

    pub async fn preview_voice(&self, id: i64) -> AppResult<VoicePreview> {
        self.get_audio(&format!("/api/voices/{id}/preview")).await
    }

    pub async fn delete_speech(&self, id: i64) -> AppResult<()> {
        self.send::<Value>(Method::DELETE, &format!("/api/speech/{id}"), None)
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthMode, OmnivoiceConfig};
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn cfg(base: &str) -> OmnivoiceConfig {
        OmnivoiceConfig {
            base_url: base.to_string(),
            auth_mode: AuthMode::Tailscale {
                user: "owner@example.com".into(),
            },
            chunk_chars: 4500,
            request_timeout_secs: 10,
            synthesis_timeout_secs: 60,
        }
    }

    #[tokio::test]
    async fn sends_auth_and_csrf_headers_and_unwraps_envelope() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .and(header("tailscale-user-login", "owner@example.com"))
            .and(header("x-requested-with", "omnivoice"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": [{"id": 7, "name": "Giọng vi", "status": "ready", "language": "vi"}],
                "error": null
            })))
            .mount(&server)
            .await;
        let client = OmnivoiceClient::new(&cfg(&server.uri()));
        let voices = client.list_voices().await.unwrap();
        assert_eq!(voices[0].id, 7);
        assert_eq!(voices[0].status, "ready");
    }

    #[tokio::test]
    async fn creates_speech_and_downloads_mp3() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/speech"))
            .and(header("x-requested-with", "omnivoice"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {"id": 91, "status": "queued", "error": null},
                "error": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/speech/91"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {"id": 91, "status": "done", "error": null},
                "error": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/speech/91/audio"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"mp3-bytes".to_vec()))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/api/speech/91"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": null, "error": null
            })))
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&cfg(&server.uri()));
        let take = client.create_speech("Xin chào", 7).await.unwrap();
        assert_eq!(take.id, 91);
        assert_eq!(client.speech(take.id).await.unwrap().status, "done");
        assert_eq!(
            client.download_mp3(take.id).await.unwrap(),
            b"mp3-bytes".to_vec()
        );
        client.delete_speech(take.id).await.unwrap();
    }

    #[tokio::test]
    async fn maps_api_errors_to_upstream() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                "ok": false,
                "data": null,
                "error": {"code": "model_missing", "message": "model is not loaded"}
            })))
            .mount(&server)
            .await;
        let client = OmnivoiceClient::new(&cfg(&server.uri()));
        let error = client.list_voices().await.unwrap_err();
        assert!(error.to_string().contains("model is not loaded"));
    }

    fn password_cfg(base: &str) -> OmnivoiceConfig {
        OmnivoiceConfig {
            auth_mode: AuthMode::Password {
                password: "s3cret".into(),
            },
            ..cfg(base)
        }
    }

    #[tokio::test]
    async fn password_login_posts_password_and_injects_cookie() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/auth/login"))
            .and(header("x-requested-with", "omnivoice"))
            .and(body_partial_json(serde_json::json!({"password": "s3cret"})))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("set-cookie", "session=abc123; Path=/; HttpOnly"),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .and(header("cookie", "session=abc123"))
            .and(header("x-requested-with", "omnivoice"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": [{"id": 7, "name": "Giọng vi", "status": "ready", "language": "vi"}],
                "error": null
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&password_cfg(&server.uri()));
        client.login().await.unwrap();
        let voices = client.list_voices().await.unwrap();
        assert_eq!(voices[0].id, 7);
        server.verify().await;
    }

    #[tokio::test]
    async fn password_mode_retries_once_after_401() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/auth/login"))
            .and(body_partial_json(serde_json::json!({"password": "s3cret"})))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("set-cookie", "session=abc123; Path=/; HttpOnly"),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .and(header("cookie", "session=abc123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": [{"id": 7, "name": "Giọng vi", "status": "ready", "language": "vi"}],
                "error": null
            })))
            .with_priority(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .respond_with(ResponseTemplate::new(401))
            .with_priority(5)
            .expect(1)
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&password_cfg(&server.uri()));
        let voices = client.list_voices().await.unwrap();
        assert_eq!(voices[0].status, "ready");
        server.verify().await;
    }

    #[tokio::test]
    async fn download_mp3_retries_once_after_401() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/auth/login"))
            .and(body_partial_json(serde_json::json!({"password": "s3cret"})))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("set-cookie", "session=abc123; Path=/; HttpOnly"),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/speech/91/audio"))
            .and(header("cookie", "session=abc123"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"mp3-bytes".to_vec()))
            .with_priority(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/speech/91/audio"))
            .respond_with(ResponseTemplate::new(401))
            .with_priority(5)
            .expect(1)
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&password_cfg(&server.uri()));
        let bytes = client.download_mp3(91).await.unwrap();
        assert_eq!(bytes, b"mp3-bytes".to_vec());
        server.verify().await;
    }

    #[tokio::test]
    async fn fetches_voice_preview_audio() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voices/7/preview"))
            .and(header("tailscale-user-login", "owner@example.com"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "audio/mpeg")
                    .set_body_bytes(b"ID3fake".to_vec()),
            )
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&cfg(&server.uri()));
        let preview = client.preview_voice(7).await.unwrap();
        assert_eq!(preview.content_type, "audio/mpeg");
        assert_eq!(preview.bytes, b"ID3fake");
    }

    #[tokio::test]
    async fn rejects_oversized_voice_previews() {
        let server = MockServer::start().await;
        let body = vec![0u8; MAX_PREVIEW_BYTES + 1];
        Mock::given(method("GET"))
            .and(path("/api/voices/7/preview"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(body))
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&cfg(&server.uri()));
        let error = client.preview_voice(7).await.unwrap_err();
        assert!(matches!(error, AppError::Upstream(_)));
    }

    #[tokio::test]
    async fn drives_voice_design_lifecycle() {
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
                    "levels": {
                        "stable": {"position_temperature": 1.0, "class_temperature": 0.0, "guidance_scale": 2.5, "num_step": 32, "speed": 1.0},
                        "balanced": {"position_temperature": 5.0, "class_temperature": 0.0, "guidance_scale": 2.5, "num_step": 32, "speed": 1.0}
                    },
                    "default_level": "balanced",
                    "languages": {
                        "vi": {"min_chars": 20, "max_chars": 300, "sample_text": "Xin chào"}
                    }
                },
                "error": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/voice-designs"))
            .and(body_partial_json(serde_json::json!({
                "language": "vi",
                "gender": "female",
                "params": {"position_temperature": 5.0}
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "ok": true,
                "data": {"id": 3, "status": "queued", "error": null, "language": "vi", "candidates": []},
                "error": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/voice-designs/3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {
                    "id": 3,
                    "status": "ready",
                    "error": null,
                    "language": "vi",
                    "candidates": [
                        {"index": 0, "audio_url": "/api/voice-designs/3/candidates/0/audio"},
                        {"index": 1, "audio_url": "/api/voice-designs/3/candidates/1/audio"},
                        {"index": 2, "audio_url": "/api/voice-designs/3/candidates/2/audio"}
                    ]
                },
                "error": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/voice-designs/3/candidates/1/audio"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "audio/wav")
                    .set_body_bytes(b"RIFFfake".to_vec()),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/voice-designs/3/save"))
            .and(body_partial_json(serde_json::json!({"name": "Giọng mới", "candidate": 1})))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "ok": true,
                "data": {"id": 11, "name": "Giọng mới", "status": "measuring", "language": "vi", "preview_url": null, "design": null},
                "error": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/api/voices/11"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": null, "error": null
            })))
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&cfg(&server.uri()));
        let catalog = client.design_catalog().await.unwrap();
        assert_eq!(catalog.default_level, "balanced");
        assert_eq!(catalog.languages["vi"].sample_text, "Xin chào");
        let request = DesignRequest {
            language: "vi".into(),
            text: "Xin chào".into(),
            gender: Some("female".into()),
            age: Some("middle_aged".into()),
            pitch: None,
            style: None,
            params: catalog.level_params.get("balanced").cloned(),
            seed: None,
        };
        let design = client.create_design(&request).await.unwrap();
        assert_eq!(design.id, 3);
        let ready = client.design_status(3).await.unwrap();
        assert_eq!(ready.candidates.len(), 3);
        let audio = client.candidate_audio(3, 1).await.unwrap();
        assert_eq!(audio.content_type, "audio/wav");
        assert_eq!(audio.bytes, b"RIFFfake");
        let voice = client.save_design(3, "Giọng mới", 1).await.unwrap();
        assert_eq!(voice.id, 11);
        client.delete_voice(11).await.unwrap();
    }

    #[tokio::test]
    async fn rejects_speech_over_the_api_limit_before_sending() {
        let client = OmnivoiceClient::new(&cfg("http://127.0.0.1:1"));
        let text = "a".repeat(MAX_SPEECH_CHARS + 1);
        let error = client.create_speech(&text, 7).await.unwrap_err();
        assert!(matches!(error, AppError::BadRequest(_)));
    }

    #[test]
    fn voice_readiness_requires_ready_status_and_matching_language() {
        let mut voice = Voice {
            status: "ready".into(),
            language: Some("vi".into()),
            ..Default::default()
        };
        assert!(voice.is_ready_for("vi"));
        assert!(!voice.is_ready_for("en"));
        voice.status = "failed".into();
        assert!(!voice.is_ready_for("vi"));
    }

    #[test]
    fn deserializes_voice_design_metadata() {
        let voice: Voice = serde_json::from_str(
            r#"{
                "id": 7,
                "name": "Phượng",
                "status": "ready",
                "language": "vi",
                "preview_url": "/api/voices/7/preview",
                "design": {
                    "gender": "female",
                    "age": "middle_aged",
                    "pitch": "low",
                    "style": "warm",
                    "seed": 1,
                    "params": {}
                }
            }"#,
        )
        .unwrap();
        assert_eq!(voice.preview_url.as_deref(), Some("/api/voices/7/preview"));
        let design = voice.design.unwrap();
        assert_eq!(design.gender.as_deref(), Some("female"));
        assert_eq!(design.age.as_deref(), Some("middle_aged"));
        assert_eq!(
            crate::models::voice_age_label(design.age.as_deref()),
            "Trung niên"
        );
    }
}
