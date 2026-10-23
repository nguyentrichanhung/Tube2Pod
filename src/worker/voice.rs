use crate::config::Config;
use crate::db::{episodes, shows};
use crate::models::{stage, supported_language, Episode};
use crate::services::omnivoice::OmnivoiceClient;
use crate::worker::PipelineError;
use sqlx::SqlitePool;

pub async fn resolve_voice(
    pool: &SqlitePool,
    _cfg: &Config,
    episode: &Episode,
    client: &OmnivoiceClient,
) -> Result<i64, PipelineError> {
    let existing_voice_id = episode.voice_id.unwrap_or(0);
    if crate::worker::is_cancelled(pool, &episode.uuid).await {
        return Ok(existing_voice_id);
    }
    let language = supported_language(&episode.language).ok_or_else(|| {
        PipelineError::new(
            "voice_not_selected",
            "Chưa chọn giọng đọc cho ngôn ngữ này. Liên hệ admin.",
            false,
        )
    })?;
    let voices = client
        .list_voices()
        .await
        .map_err(|e| PipelineError::new("tts_unavailable", e.to_string(), true))?;
    let show_voice = shows::get_voice(pool, episode.show_id, language)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    let override_ready = episode.voice_id.filter(|voice_id| {
        voices
            .iter()
            .any(|voice| voice.id == *voice_id && voice.is_ready_for(language))
    });
    let show_ready = show_voice
        .as_ref()
        .map(|(voice_id, _)| *voice_id)
        .filter(|voice_id| {
            voices
                .iter()
                .any(|voice| voice.id == *voice_id && voice.is_ready_for(language))
        });
    let voice_id = override_ready.or(show_ready).ok_or_else(|| {
        if episode.voice_id.is_some() || show_voice.is_some() {
            PipelineError::new(
                "voice_not_selected",
                "Giọng đọc đã chọn không còn khả dụng. Liên hệ admin.",
                false,
            )
        } else {
            PipelineError::new(
                "voice_not_selected",
                "Chưa chọn giọng đọc cho ngôn ngữ này. Liên hệ admin.",
                false,
            )
        }
    })?;
    if crate::worker::is_cancelled(pool, &episode.uuid).await {
        return Ok(existing_voice_id);
    }
    episodes::set_voice(pool, &episode.uuid, voice_id)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    episodes::set_stage(pool, &episode.uuid, stage::SYNTHESIZING)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    episodes::set_progress(pool, &episode.uuid, r#"{"percent":55}"#)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    Ok(voice_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthMode, OmnivoiceConfig};
    use crate::db::{
        self,
        shows::{self, NewShow},
    };
    use crate::models::Show;
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn omni_cfg(server: &MockServer) -> OmnivoiceConfig {
        OmnivoiceConfig {
            base_url: server.uri(),
            auth_mode: AuthMode::Tailscale {
                user: "owner".into(),
            },
            chunk_chars: 4500,
            request_timeout_secs: 10,
            synthesis_timeout_secs: 60,
        }
    }

    fn app_cfg(dir: &std::path::Path) -> Config {
        Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "DATA_DIR" => Some(dir.display().to_string()),
            _ => None,
        })
        .unwrap()
    }

    async fn seed_episode(pool: &SqlitePool) -> (Show, Episode) {
        let show = shows::create(
            pool,
            NewShow {
                title: "Show".into(),
                ..NewShow::default()
            },
        )
        .await
        .unwrap();
        let ep = db::episodes::create(
            pool,
            db::episodes::NewEpisode {
                show_id: show.id,
                video_id: "v1".into(),
                source_url: "https://www.youtube.com/watch?v=v1".into(),
                source_title: "Video".into(),
                source_channel: "".into(),
                source_duration_s: Some(600),
                source_thumbnail_url: None,
                source_upload_date: None,
                language: "vi".into(),
                target_minutes: 10,
            },
        )
        .await
        .unwrap();
        (show, ep)
    }

    async fn selected_voice_fixture(
        voice_id: i64,
        language: &str,
    ) -> (SqlitePool, Config, Episode) {
        let pool = db::test_pool().await;
        let (show, episode) = seed_episode(&pool).await;
        shows::upsert_voice(&pool, show.id, language, voice_id, "Selected voice")
            .await
            .unwrap();
        (
            pool,
            app_cfg(std::path::Path::new("/tmp/t2p-voice-test")),
            episode,
        )
    }

    #[tokio::test]
    async fn uses_the_selected_ready_voice() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": [{"id": 42, "name": "Phượng", "status": "ready", "language": "vi"}],
                "error": null
            })))
            .mount(&server)
            .await;

        let (pool, cfg, episode) = selected_voice_fixture(42, "vi").await;
        let client = OmnivoiceClient::new(&omni_cfg(&server));
        let id = resolve_voice(&pool, &cfg, &episode, &client).await.unwrap();
        assert_eq!(id, 42);
        let got = crate::db::episodes::get(&pool, &episode.uuid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.stage, crate::models::stage::SYNTHESIZING);
    }

    #[tokio::test]
    async fn uses_the_episode_voice_override_when_ready() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": [{"id": 99, "name": "Override", "status": "ready", "language": "vi"}],
                "error": null
            })))
            .mount(&server)
            .await;

        let pool = db::test_pool().await;
        let (show, episode) = seed_episode(&pool).await;
        db::episodes::set_voice(&pool, &episode.uuid, 99)
            .await
            .unwrap();
        shows::upsert_voice(&pool, show.id, "vi", 42, "Show voice")
            .await
            .unwrap();
        let episode = db::episodes::get(&pool, &episode.uuid)
            .await
            .unwrap()
            .unwrap();
        let client = OmnivoiceClient::new(&omni_cfg(&server));
        let id = resolve_voice(
            &pool,
            &app_cfg(std::path::Path::new("/tmp/t2p-voice-test")),
            &episode,
            &client,
        )
        .await
        .unwrap();
        assert_eq!(id, 99);
    }

    #[tokio::test]
    async fn falls_back_to_the_show_voice_when_the_override_is_not_ready() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": [{"id": 42, "name": "Show voice", "status": "ready", "language": "vi"}],
                "error": null
            })))
            .mount(&server)
            .await;

        let pool = db::test_pool().await;
        let (show, episode) = seed_episode(&pool).await;
        db::episodes::set_voice(&pool, &episode.uuid, 99)
            .await
            .unwrap();
        shows::upsert_voice(&pool, show.id, "vi", 42, "Show voice")
            .await
            .unwrap();
        let episode = db::episodes::get(&pool, &episode.uuid)
            .await
            .unwrap()
            .unwrap();
        let client = OmnivoiceClient::new(&omni_cfg(&server));
        let id = resolve_voice(
            &pool,
            &app_cfg(std::path::Path::new("/tmp/t2p-voice-test")),
            &episode,
            &client,
        )
        .await
        .unwrap();
        assert_eq!(id, 42);
    }

    #[tokio::test]
    async fn missing_or_failed_selection_is_not_retryable() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": [],
                "error": null
            })))
            .mount(&server)
            .await;

        let (pool, cfg, episode) = selected_voice_fixture(42, "vi").await;
        let client = OmnivoiceClient::new(&omni_cfg(&server));
        let error = resolve_voice(&pool, &cfg, &episode, &client)
            .await
            .unwrap_err();
        assert_eq!(error.code, "voice_not_selected");
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn cancelled_episode_skips_voice_lookup() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&server)
            .await;

        let (pool, cfg, episode) = selected_voice_fixture(42, "vi").await;
        db::episodes::set_voice(&pool, &episode.uuid, 42)
            .await
            .unwrap();
        db::episodes::cancel(&pool, &episode.uuid).await.unwrap();
        let episode = db::episodes::get(&pool, &episode.uuid)
            .await
            .unwrap()
            .unwrap();
        let client = OmnivoiceClient::new(&omni_cfg(&server));

        let started = std::time::Instant::now();
        let voice = resolve_voice(&pool, &cfg, &episode, &client).await.unwrap();
        assert_eq!(voice, 42);
        assert!(started.elapsed() < Duration::from_secs(2));
        server.verify().await;
    }
}
