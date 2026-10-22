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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Voice {
    pub id: i64,
    pub name: Option<String>,
    pub status: String,
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SpeechTake {
    pub id: i64,
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct VoiceOptions {
    pub min_chars: usize,
    pub max_chars: usize,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct RawVoiceOptions {
    #[serde(default)]
    min_chars: Option<usize>,
    #[serde(default)]
    max_chars: Option<usize>,
    #[serde(default)]
    languages: HashMap<String, RawLanguageOptions>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct RawLanguageOptions {
    min_chars: usize,
    max_chars: usize,
}

impl RawVoiceOptions {
    #[allow(dead_code)]
    fn for_language(self, language: &str) -> VoiceOptions {
        if let Some(limits) = self.languages.get(language) {
            return VoiceOptions {
                min_chars: limits.min_chars,
                max_chars: limits.max_chars,
            };
        }
        VoiceOptions {
            min_chars: self.min_chars.unwrap_or(20),
            max_chars: self.max_chars.unwrap_or(500),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct VoiceDesign {
    pub id: i64,
    pub status: String,
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

    #[allow(dead_code)]
    pub async fn voice_options(&self, language: &str) -> AppResult<VoiceOptions> {
        let raw: RawVoiceOptions = self
            .call(Method::GET, "/api/voice-designs/options", None)
            .await?;
        Ok(raw.for_language(language))
    }

    pub async fn list_voices(&self) -> AppResult<Vec<Voice>> {
        self.call(Method::GET, "/api/voices", None).await
    }

    #[allow(dead_code)]
    pub async fn create_design(
        &self,
        language: &str,
        text: &str,
        preset: &crate::models::VoicePreset,
    ) -> AppResult<VoiceDesign> {
        self.call(
            Method::POST,
            "/api/voice-designs",
            Some(json!({
                "language": language,
                "gender": preset.gender,
                "age": preset.age,
                "pitch": preset.pitch,
                "text": text,
            })),
        )
        .await
    }

    #[allow(dead_code)]
    pub async fn design_status(&self, id: i64) -> AppResult<VoiceDesign> {
        self.call(Method::GET, &format!("/api/voice-designs/{id}"), None)
            .await
    }

    #[allow(dead_code)]
    pub async fn save_design(&self, id: i64, name: &str) -> AppResult<Voice> {
        self.call(
            Method::POST,
            &format!("/api/voice-designs/{id}/save"),
            Some(json!({ "name": name, "candidate": 0 })),
        )
        .await
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
    async fn drives_voice_design_lifecycle() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voice-designs/options"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {"min_chars": 20, "max_chars": 300, "sample_text": "Xin chào"},
                "error": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/voice-designs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {"id": 3, "status": "queued", "voice_id": null},
                "error": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/voice-designs/3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {"id": 3, "status": "ready", "voice_id": 11},
                "error": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/voice-designs/3/save"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {"id": 11, "name": "Giọng mới", "status": "ready", "language": "vi"},
                "error": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/api/voice-designs/3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": null, "error": null
            })))
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&cfg(&server.uri()));
        let options = client.voice_options("vi").await.unwrap();
        assert_eq!(options.max_chars, 300);
        let design = client
            .create_design("vi", "Xin chào", &crate::models::VoicePreset::default())
            .await
            .unwrap();
        assert_eq!(design.id, 3);
        let ready = client.design_status(design.id).await.unwrap();
        assert_eq!(ready.status, "ready");
        let voice = client.save_design(design.id, "Giọng mới").await.unwrap();
        assert_eq!(voice.id, 11);
        client.delete_design(design.id).await.unwrap();
    }

    #[tokio::test]
    async fn reads_language_scoped_voice_options() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voice-designs/options"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {
                    "languages": {
                        "vi": {"min_chars": 40, "max_chars": 900, "sample_text": "Xin chào"},
                        "en": {"min_chars": 30, "max_chars": 700, "sample_text": "Hello"}
                    }
                },
                "error": null
            })))
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&cfg(&server.uri()));
        let vi = client.voice_options("vi").await.unwrap();
        assert_eq!((vi.min_chars, vi.max_chars), (40, 900));
        let en = client.voice_options("en").await.unwrap();
        assert_eq!((en.min_chars, en.max_chars), (30, 700));
    }

    #[tokio::test]
    async fn rejects_speech_over_the_api_limit_before_sending() {
        let client = OmnivoiceClient::new(&cfg("http://127.0.0.1:1"));
        let text = "a".repeat(MAX_SPEECH_CHARS + 1);
        let error = client.create_speech(&text, 7).await.unwrap_err();
        assert!(matches!(error, AppError::BadRequest(_)));
    }
}
