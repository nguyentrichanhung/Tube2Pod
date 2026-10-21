use crate::config::Config;
use crate::db::{episodes, shows};
use crate::models::{stage, Episode};
use crate::services::omnivoice::OmnivoiceClient;
use crate::worker::PipelineError;
use sqlx::SqlitePool;
use std::time::Duration;

const NAME_MAX: usize = 80;

pub async fn resolve_voice(
    pool: &SqlitePool,
    cfg: &Config,
    episode: &Episode,
    client: &OmnivoiceClient,
) -> Result<i64, PipelineError> {
    let show = shows::get(pool, episode.show_id)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?
        .ok_or_else(|| PipelineError::new("internal", "show not found", false))?;
    if let Some((voice_id, name)) = shows::get_voice(pool, show.id, &episode.language)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?
    {
        let voices = client
            .list_voices()
            .await
            .map_err(|e| PipelineError::new("tts_unavailable", e.to_string(), true))?;
        if voices
            .iter()
            .any(|voice| voice.id == voice_id && voice.status == "ready")
        {
            episodes::set_voice(pool, &episode.uuid, voice_id)
                .await
                .map_err(|e| PipelineError::from_app(e, "internal"))?;
            episodes::set_stage(pool, &episode.uuid, stage::SYNTHESIZING)
                .await
                .map_err(|e| PipelineError::from_app(e, "internal"))?;
            episodes::set_progress(pool, &episode.uuid, r#"{"percent":55}"#)
                .await
                .map_err(|e| PipelineError::from_app(e, "internal"))?;
            return Ok(voice_id);
        }
        shows::delete_voice(pool, show.id, &episode.language)
            .await
            .map_err(|e| PipelineError::from_app(e, "internal"))?;
        tracing::warn!(
            show_id = show.id,
            language = %episode.language,
            name,
            "stale voice removed"
        );
    }

    let options = client
        .voice_options()
        .await
        .map_err(|e| PipelineError::new("voice_design_failed", e.to_string(), true))?;
    let sample = sample_text(&episode.language);
    let sample = fit_sample(&sample, options.min_chars, options.max_chars);
    let preset: crate::models::VoicePreset =
        serde_json::from_str(&show.voice_preset_json).unwrap_or_default();
    let design = client
        .create_design(&episode.language, &sample, &preset)
        .await
        .map_err(|e| PipelineError::new("voice_design_failed", e.to_string(), true))?;
    episodes::set_voice_design(pool, &episode.uuid, Some(design.id))
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;

    let design = poll_design(client, design.id).await?;
    if design.status != "ready" {
        let _ = client.delete_design(design.id).await;
        return Err(PipelineError::new(
            "voice_design_failed",
            "Không tạo được giọng đọc.",
            true,
        ));
    }
    let name = format!("Tube2Pod · {} · {}", show.title, episode.language);
    let name = name.chars().take(NAME_MAX).collect::<String>();
    let voice = client
        .save_design(design.id, &name)
        .await
        .map_err(|e| PipelineError::new("voice_design_failed", e.to_string(), true))?;
    shows::upsert_voice(pool, show.id, &episode.language, voice.id, &name)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    episodes::set_voice(pool, &episode.uuid, voice.id)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    episodes::set_voice_design(pool, &episode.uuid, None)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;

    let ready = poll_voice(client, voice.id).await?;
    if !ready {
        return Err(PipelineError::new(
            "voice_design_failed",
            "Giọng đọc chưa sẵn sàng.",
            true,
        ));
    }
    episodes::set_stage(pool, &episode.uuid, stage::SYNTHESIZING)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    episodes::set_progress(pool, &episode.uuid, r#"{"percent":55}"#)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    let _ = cfg;
    Ok(voice.id)
}

async fn poll_design(
    client: &OmnivoiceClient,
    id: i64,
) -> Result<crate::services::omnivoice::VoiceDesign, PipelineError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(600);
    loop {
        let design = client
            .design_status(id)
            .await
            .map_err(|e| PipelineError::new("voice_design_failed", e.to_string(), true))?;
        if design.status == "ready" {
            return Ok(design);
        }
        if design.status == "failed" {
            return Err(PipelineError::new(
                "voice_design_failed",
                "Không tạo được giọng đọc.",
                true,
            ));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(PipelineError::new(
                "voice_design_failed",
                "Hết thời gian tạo giọng đọc.",
                true,
            ));
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

async fn poll_voice(client: &OmnivoiceClient, id: i64) -> Result<bool, PipelineError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    loop {
        let voices = client
            .list_voices()
            .await
            .map_err(|e| PipelineError::new("voice_design_failed", e.to_string(), true))?;
        if let Some(voice) = voices.iter().find(|voice| voice.id == id) {
            if voice.status == "ready" {
                return Ok(true);
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

fn sample_text(language: &str) -> String {
    match language {
        "en" => "Welcome to this podcast. Here is a short sample of the narrator's voice, warm and clear.".into(),
        "ko" => "안녕하세요. 오늘의 팟캐스트에서 따뜻하고 또렷한 목소리로 핵심 내용을 전해 드립니다.".into(),
        _ => "Xin chào, đây là giọng đọc thử nghiệm cho podcast công nghệ hằng ngày, rõ ràng và ấm áp.".into(),
    }
}

fn fit_sample(sample: &str, min_chars: usize, max_chars: usize) -> String {
    let mut text = sample.to_string();
    while text.chars().count() < min_chars {
        text.push(' ');
        text.push_str(sample);
    }
    text.chars().take(max_chars.max(1)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthMode, OmnivoiceConfig};
    use crate::db::{
        self,
        shows::{self, NewShow},
    };
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn designs_saves_and_reuses_a_voice() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": [{"id": 42, "name": "Tube2Pod", "status": "ready", "language": "vi"}],
                "error": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/voice-designs/options"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {"min_chars": 20, "max_chars": 500, "sample_text": "sample"},
                "error": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/voice-designs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": {"id": 5, "status": "queued"}, "error": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/voice-designs/5"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": {"id": 5, "status": "ready"}, "error": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/voice-designs/5/save"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": {"id": 42, "name": "Tube2Pod", "status": "ready", "language": "vi"}, "error": null
            })))
            .mount(&server)
            .await;

        let cfg = OmnivoiceConfig {
            base_url: server.uri(),
            auth_mode: AuthMode::Tailscale {
                user: "owner".into(),
            },
            chunk_chars: 4500,
            request_timeout_secs: 10,
            synthesis_timeout_secs: 60,
        };
        let client = OmnivoiceClient::new(&cfg);
        let pool = db::test_pool().await;
        let show = shows::create(
            &pool,
            NewShow {
                title: "Show".into(),
                ..NewShow::default()
            },
        )
        .await
        .unwrap();
        let ep = db::episodes::create(
            &pool,
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
        let root = tempfile::tempdir().unwrap();
        let app_cfg = Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "DATA_DIR" => Some(root.path().display().to_string()),
            _ => None,
        })
        .unwrap();

        let voice = resolve_voice(&pool, &app_cfg, &ep, &client).await.unwrap();
        assert_eq!(voice, 42);
        assert_eq!(
            shows::get_voice(&pool, show.id, "vi")
                .await
                .unwrap()
                .unwrap()
                .0,
            42
        );
    }
}
