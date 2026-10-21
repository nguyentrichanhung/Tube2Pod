use crate::config::Config;
use crate::db::{episodes, shows};
use crate::error::AppError;
use crate::models::{stage, supported_language, Episode, VoicePreset};
use crate::services::omnivoice::{OmnivoiceClient, VoiceDesign};
use crate::worker::PipelineError;
use sqlx::SqlitePool;
use std::time::Duration;

const NAME_MAX: usize = 80;
const DESIGN_MAX_ATTEMPTS: usize = 2;
const POLL_INTERVAL: Duration = Duration::from_secs(3);
const DESIGN_TIMEOUT: Duration = Duration::from_secs(600);
const VOICE_TIMEOUT: Duration = Duration::from_secs(300);
const VOICE_ABSENT_GRACE_POLLS: usize = 3;

enum DesignPoll {
    Ready,
    Failed,
}

enum VoicePoll {
    Ready,
    Failed,
}

pub async fn resolve_voice(
    pool: &SqlitePool,
    cfg: &Config,
    episode: &Episode,
    client: &OmnivoiceClient,
) -> Result<i64, PipelineError> {
    if crate::worker::is_cancelled(pool, &episode.uuid).await {
        return Ok(episode.voice_id.unwrap_or(0));
    }
    let show = shows::get(pool, episode.show_id)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?
        .ok_or_else(|| PipelineError::new("internal", "show not found", false))?;
    let language = match supported_language(&episode.language) {
        Some(language) => language,
        None => episode.language.as_str(),
    };

    if let Some((voice_id, name)) = shows::get_voice(pool, show.id, language)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?
    {
        let voices = client.list_voices().await.map_err(tts_error)?;
        if voices
            .iter()
            .any(|voice| voice.id == voice_id && voice.status == "ready")
        {
            activate_episode(pool, &episode.uuid, voice_id).await?;
            return Ok(voice_id);
        }
        shows::delete_voice(pool, show.id, language)
            .await
            .map_err(|e| PipelineError::from_app(e, "internal"))?;
        tracing::warn!(
            show_id = show.id,
            language = %language,
            name,
            "stale voice removed"
        );
    }

    let name = format!("Tube2Pod · {} · {}", show.title, language);
    let name = name.chars().take(NAME_MAX).collect::<String>();

    let existing_voice_id = episode.voice_id.unwrap_or(0);

    if let Some(design_id) = episode.voice_design_id {
        let polled =
            crate::worker::cancellable(pool, &episode.uuid, poll_design(client, design_id)).await;
        match polled {
            None => {
                discard_design_if_current(pool, client, &episode.uuid, design_id).await?;
                return Ok(existing_voice_id);
            }
            Some(Ok(DesignPoll::Ready)) => {
                return save_design_and_activate(
                    pool,
                    client,
                    show.id,
                    language,
                    &episode.uuid,
                    design_id,
                    &name,
                )
                .await
                .map(|voice_id| voice_id.unwrap_or(existing_voice_id));
            }
            Some(Ok(DesignPoll::Failed)) | Some(Err(_)) => {
                let _ = client.delete_design(design_id).await;
                clear_design(pool, &episode.uuid).await?;
            }
        }
    }

    if crate::worker::is_cancelled(pool, &episode.uuid).await {
        return Ok(existing_voice_id);
    }

    let options = client.voice_options(language).await.map_err(tts_error)?;
    let sample = sample_text(language);
    let sample = fit_sample(&sample, options.min_chars, options.max_chars);
    let preset: VoicePreset = serde_json::from_str(&show.voice_preset_json).unwrap_or_default();
    let Some(design) =
        create_design(pool, client, &episode.uuid, language, &sample, &preset).await?
    else {
        return Ok(existing_voice_id);
    };

    let voice_id = save_design_and_activate(
        pool,
        client,
        show.id,
        language,
        &episode.uuid,
        design.id,
        &name,
    )
    .await?
    .unwrap_or(existing_voice_id);
    let _ = cfg;
    Ok(voice_id)
}

async fn save_design_and_activate(
    pool: &SqlitePool,
    client: &OmnivoiceClient,
    show_id: i64,
    language: &str,
    uuid: &str,
    design_id: i64,
    name: &str,
) -> Result<Option<i64>, PipelineError> {
    if crate::worker::is_cancelled(pool, uuid).await {
        return Ok(None);
    }
    let voice = match client.save_design(design_id, name).await {
        Ok(voice) => voice,
        Err(error) => {
            let _ = client.delete_design(design_id).await;
            clear_design(pool, uuid).await?;
            return Err(design_error(error.to_string()));
        }
    };
    shows::upsert_voice(pool, show_id, language, voice.id, name)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    episodes::set_voice(pool, uuid, voice.id)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;

    let polled = crate::worker::cancellable(pool, uuid, poll_voice(client, voice.id)).await;
    match polled {
        None => return Ok(None),
        Some(Ok(VoicePoll::Ready)) => {}
        Some(Ok(VoicePoll::Failed)) => {
            shows::delete_voice(pool, show_id, language)
                .await
                .map_err(|e| PipelineError::from_app(e, "internal"))?;
            return Err(design_error("Giọng đọc chưa sẵn sàng."));
        }
        Some(Err(error)) => return Err(error),
    }
    activate_episode(pool, uuid, voice.id).await?;
    Ok(Some(voice.id))
}

async fn activate_episode(
    pool: &SqlitePool,
    uuid: &str,
    voice_id: i64,
) -> Result<(), PipelineError> {
    episodes::set_voice(pool, uuid, voice_id)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    episodes::set_stage(pool, uuid, stage::SYNTHESIZING)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    episodes::set_progress(pool, uuid, r#"{"percent":55}"#)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    Ok(())
}

async fn create_design(
    pool: &SqlitePool,
    client: &OmnivoiceClient,
    uuid: &str,
    language: &str,
    sample: &str,
    preset: &VoicePreset,
) -> Result<Option<VoiceDesign>, PipelineError> {
    let mut last_error = None;
    for _ in 0..DESIGN_MAX_ATTEMPTS {
        if crate::worker::is_cancelled(pool, uuid).await {
            return Ok(None);
        }
        let design = match client.create_design(language, sample, preset).await {
            Ok(design) => design,
            Err(error) => {
                last_error = Some(design_error(error.to_string()));
                continue;
            }
        };
        episodes::set_voice_design(pool, uuid, Some(design.id))
            .await
            .map_err(|e| PipelineError::from_app(e, "internal"))?;
        let polled = crate::worker::cancellable(pool, uuid, poll_design(client, design.id)).await;
        match polled {
            None => {
                discard_design_if_current(pool, client, uuid, design.id).await?;
                return Ok(None);
            }
            Some(Ok(DesignPoll::Ready)) => return Ok(Some(design)),
            Some(Ok(DesignPoll::Failed)) => {
                cleanup_design(pool, client, uuid, design.id).await?;
                last_error = Some(design_error("Không tạo được giọng đọc."));
            }
            Some(Err(error)) => {
                cleanup_design(pool, client, uuid, design.id).await?;
                last_error = Some(error);
            }
        }
    }
    clear_design(pool, uuid).await?;
    Err(last_error.unwrap_or_else(|| design_error("Không tạo được giọng đọc.")))
}

async fn discard_design_if_current(
    pool: &SqlitePool,
    client: &OmnivoiceClient,
    uuid: &str,
    design_id: i64,
) -> Result<(), PipelineError> {
    let Some(current) = episodes::get(pool, uuid)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?
    else {
        return Ok(());
    };
    if current.voice_design_id == Some(design_id) {
        let _ = client.delete_design(design_id).await;
        clear_design(pool, uuid).await?;
    }
    Ok(())
}

async fn cleanup_design(
    pool: &SqlitePool,
    client: &OmnivoiceClient,
    uuid: &str,
    design_id: i64,
) -> Result<(), PipelineError> {
    let _ = client.delete_design(design_id).await;
    clear_design(pool, uuid).await
}

async fn clear_design(pool: &SqlitePool, uuid: &str) -> Result<(), PipelineError> {
    episodes::set_voice_design(pool, uuid, None)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))
}

fn tts_error(error: AppError) -> PipelineError {
    let retryable = matches!(error, AppError::Upstream(_) | AppError::Internal(_));
    PipelineError::new("tts_unavailable", error.to_string(), retryable)
}

fn design_error(message: impl Into<String>) -> PipelineError {
    PipelineError::new("voice_design_failed", message, true)
}

async fn poll_design(client: &OmnivoiceClient, id: i64) -> Result<DesignPoll, PipelineError> {
    let deadline = tokio::time::Instant::now() + DESIGN_TIMEOUT;
    loop {
        let design = client.design_status(id).await.map_err(tts_error)?;
        if design.status == "ready" {
            return Ok(DesignPoll::Ready);
        }
        if design.status == "failed" {
            return Ok(DesignPoll::Failed);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(design_error("Hết thời gian tạo giọng đọc."));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn poll_voice(client: &OmnivoiceClient, id: i64) -> Result<VoicePoll, PipelineError> {
    let deadline = tokio::time::Instant::now() + VOICE_TIMEOUT;
    let mut absent_polls = 0usize;
    loop {
        let voices = client.list_voices().await.map_err(tts_error)?;
        match voices.iter().find(|voice| voice.id == id) {
            Some(voice) if voice.status == "ready" => return Ok(VoicePoll::Ready),
            Some(voice) if voice.status == "failed" => return Ok(VoicePoll::Failed),
            Some(_) => absent_polls = 0,
            None => {
                absent_polls += 1;
                if absent_polls >= VOICE_ABSENT_GRACE_POLLS {
                    return Ok(VoicePoll::Failed);
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(VoicePoll::Failed);
        }
        tokio::time::sleep(POLL_INTERVAL).await;
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
    use crate::models::Show;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn omnivoice_cfg(server: &MockServer) -> OmnivoiceConfig {
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

    async fn mount_options(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path("/api/voice-designs/options"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {"min_chars": 20, "max_chars": 500, "sample_text": "sample"},
                "error": null
            })))
            .mount(server)
            .await;
    }

    fn design_response(id: i64) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true, "data": {"id": id, "status": "queued"}, "error": null
        }))
    }

    async fn mount_design_sequence(server: &MockServer, ids: Vec<i64>) {
        let calls = AtomicUsize::new(0);
        let expected = ids.len() as u64;
        Mock::given(method("POST"))
            .and(path("/api/voice-designs"))
            .respond_with(move |_: &wiremock::Request| {
                let index = calls.fetch_add(1, Ordering::SeqCst);
                design_response(ids[index.min(ids.len() - 1)])
            })
            .expect(expected)
            .mount(server)
            .await;
    }

    async fn mount_design_status(server: &MockServer, id: i64, status: &str) {
        Mock::given(method("GET"))
            .and(path(format!("/api/voice-designs/{id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": {"id": id, "status": status}, "error": null
            })))
            .mount(server)
            .await;
    }

    async fn mount_save(server: &MockServer, design_id: i64, voice_id: i64) {
        Mock::given(method("POST"))
            .and(path(format!("/api/voice-designs/{design_id}/save")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {"id": voice_id, "name": "Tube2Pod", "status": "measuring", "language": "vi"},
                "error": null
            })))
            .mount(server)
            .await;
    }

    fn voices_response(voices: serde_json::Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true, "data": voices, "error": null
        }))
    }

    #[tokio::test]
    async fn designs_saves_and_polls_a_voice() {
        let server = MockServer::start().await;
        mount_options(&server).await;
        mount_design_sequence(&server, vec![5]).await;
        mount_design_status(&server, 5, "ready").await;
        mount_save(&server, 5, 42).await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .respond_with(voices_response(serde_json::json!([
                {"id": 42, "name": "Tube2Pod", "status": "ready", "language": "vi"}
            ])))
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&omnivoice_cfg(&server));
        let pool = db::test_pool().await;
        let (show, ep) = seed_episode(&pool).await;
        let root = tempfile::tempdir().unwrap();

        let voice = resolve_voice(&pool, &app_cfg(root.path()), &ep, &client)
            .await
            .unwrap();
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

    #[tokio::test]
    async fn reuses_ready_cached_voice_without_designing() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .respond_with(voices_response(serde_json::json!([
                {"id": 42, "name": "Tube2Pod", "status": "ready", "language": "vi"}
            ])))
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/voice-designs"))
            .respond_with(design_response(5))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/voice-designs/5/save"))
            .respond_with(design_response(42))
            .expect(0)
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&omnivoice_cfg(&server));
        let pool = db::test_pool().await;
        let (show, ep) = seed_episode(&pool).await;
        sqlx::query("UPDATE episodes SET language = 'vi-VN' WHERE uuid = ?1")
            .bind(&ep.uuid)
            .execute(&pool)
            .await
            .unwrap();
        let ep = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        shows::upsert_voice(&pool, show.id, "vi", 42, "Tube2Pod · vi")
            .await
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        let cfg = app_cfg(root.path());

        assert_eq!(resolve_voice(&pool, &cfg, &ep, &client).await.unwrap(), 42);
        assert_eq!(resolve_voice(&pool, &cfg, &ep, &client).await.unwrap(), 42);
        server.verify().await;
    }

    #[tokio::test]
    async fn redesigns_when_cached_voice_is_stale() {
        let server = MockServer::start().await;
        mount_options(&server).await;
        mount_design_sequence(&server, vec![5]).await;
        mount_design_status(&server, 5, "ready").await;
        mount_save(&server, 5, 42).await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .respond_with(voices_response(serde_json::json!([
                {"id": 42, "name": "Tube2Pod", "status": "ready", "language": "vi"}
            ])))
            .expect(2)
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&omnivoice_cfg(&server));
        let pool = db::test_pool().await;
        let (show, ep) = seed_episode(&pool).await;
        shows::upsert_voice(&pool, show.id, "vi", 99, "Stale")
            .await
            .unwrap();
        let root = tempfile::tempdir().unwrap();

        let voice = resolve_voice(&pool, &app_cfg(root.path()), &ep, &client)
            .await
            .unwrap();
        assert_eq!(voice, 42);
        assert_eq!(
            shows::get_voice(&pool, show.id, "vi")
                .await
                .unwrap()
                .unwrap()
                .0,
            42
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn resumes_a_ready_unsaved_design_after_restart() {
        let server = MockServer::start().await;
        mount_design_status(&server, 7, "ready").await;
        Mock::given(method("POST"))
            .and(path("/api/voice-designs/7/save"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {"id": 42, "name": "Tube2Pod", "status": "measuring", "language": "vi"},
                "error": null
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .respond_with(voices_response(serde_json::json!([
                {"id": 42, "name": "Tube2Pod", "status": "ready", "language": "vi"}
            ])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/voice-designs"))
            .respond_with(design_response(9))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/voice-designs/options"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true,
                "data": {"min_chars": 20, "max_chars": 500, "sample_text": "sample"},
                "error": null
            })))
            .expect(0)
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&omnivoice_cfg(&server));
        let pool = db::test_pool().await;
        let (show, ep) = seed_episode(&pool).await;
        db::episodes::set_voice_design(&pool, &ep.uuid, Some(7))
            .await
            .unwrap();
        let ep = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        let root = tempfile::tempdir().unwrap();

        let voice = resolve_voice(&pool, &app_cfg(root.path()), &ep, &client)
            .await
            .unwrap();
        assert_eq!(voice, 42);
        assert_eq!(
            shows::get_voice(&pool, show.id, "vi")
                .await
                .unwrap()
                .unwrap()
                .0,
            42
        );
        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.voice_id, Some(42));
        assert_eq!(got.voice_design_id, None);
        assert_eq!(got.stage, stage::SYNTHESIZING);
        server.verify().await;
    }

    #[tokio::test]
    async fn discards_a_failed_unsaved_design_before_designing_again() {
        let server = MockServer::start().await;
        mount_design_status(&server, 7, "failed").await;
        Mock::given(method("DELETE"))
            .and(path("/api/voice-designs/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": null, "error": null
            })))
            .expect(1)
            .mount(&server)
            .await;
        mount_options(&server).await;
        mount_design_sequence(&server, vec![8]).await;
        mount_design_status(&server, 8, "ready").await;
        mount_save(&server, 8, 42).await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .respond_with(voices_response(serde_json::json!([
                {"id": 42, "name": "Tube2Pod", "status": "ready", "language": "vi"}
            ])))
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&omnivoice_cfg(&server));
        let pool = db::test_pool().await;
        let (show, ep) = seed_episode(&pool).await;
        db::episodes::set_voice_design(&pool, &ep.uuid, Some(7))
            .await
            .unwrap();
        let ep = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        let root = tempfile::tempdir().unwrap();

        let voice = resolve_voice(&pool, &app_cfg(root.path()), &ep, &client)
            .await
            .unwrap();
        assert_eq!(voice, 42);
        assert_eq!(
            shows::get_voice(&pool, show.id, "vi")
                .await
                .unwrap()
                .unwrap()
                .0,
            42
        );
        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.voice_design_id, None);
        server.verify().await;
    }

    #[tokio::test]
    async fn retries_once_when_design_fails() {
        let server = MockServer::start().await;
        mount_options(&server).await;
        mount_design_sequence(&server, vec![5, 6]).await;
        mount_design_status(&server, 5, "failed").await;
        mount_design_status(&server, 6, "ready").await;
        mount_save(&server, 6, 42).await;
        Mock::given(method("DELETE"))
            .and(path("/api/voice-designs/5"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": null, "error": null
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .respond_with(voices_response(serde_json::json!([
                {"id": 42, "name": "Tube2Pod", "status": "ready", "language": "vi"}
            ])))
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&omnivoice_cfg(&server));
        let pool = db::test_pool().await;
        let (show, ep) = seed_episode(&pool).await;
        let root = tempfile::tempdir().unwrap();

        let voice = resolve_voice(&pool, &app_cfg(root.path()), &ep, &client)
            .await
            .unwrap();
        assert_eq!(voice, 42);
        assert_eq!(
            shows::get_voice(&pool, show.id, "vi")
                .await
                .unwrap()
                .unwrap()
                .0,
            42
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn fails_without_stale_design_id_when_design_fails_twice() {
        let server = MockServer::start().await;
        mount_options(&server).await;
        mount_design_sequence(&server, vec![5, 6]).await;
        mount_design_status(&server, 5, "failed").await;
        mount_design_status(&server, 6, "failed").await;
        Mock::given(method("DELETE"))
            .and(path("/api/voice-designs/5"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": null, "error": null
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/api/voice-designs/6"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": null, "error": null
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/voice-designs/6/save"))
            .respond_with(design_response(42))
            .expect(0)
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&omnivoice_cfg(&server));
        let pool = db::test_pool().await;
        let (show, ep) = seed_episode(&pool).await;
        let root = tempfile::tempdir().unwrap();

        let error = resolve_voice(&pool, &app_cfg(root.path()), &ep, &client)
            .await
            .unwrap_err();
        assert_eq!(error.code, "voice_design_failed");
        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.voice_design_id, None);
        assert!(shows::get_voice(&pool, show.id, "vi")
            .await
            .unwrap()
            .is_none());
        server.verify().await;
    }

    #[tokio::test]
    async fn fails_fast_and_clears_cache_when_voice_fails() {
        let server = MockServer::start().await;
        mount_options(&server).await;
        mount_design_sequence(&server, vec![5]).await;
        mount_design_status(&server, 5, "ready").await;
        mount_save(&server, 5, 42).await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .respond_with(voices_response(serde_json::json!([
                {"id": 42, "name": "Tube2Pod", "status": "failed", "language": "vi"}
            ])))
            .expect(1)
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&omnivoice_cfg(&server));
        let pool = db::test_pool().await;
        let (show, ep) = seed_episode(&pool).await;
        let root = tempfile::tempdir().unwrap();

        let started = std::time::Instant::now();
        let error = resolve_voice(&pool, &app_cfg(root.path()), &ep, &client)
            .await
            .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "failed voice must fail immediately, took {:?}",
            started.elapsed()
        );
        assert_eq!(error.code, "voice_design_failed");
        assert!(shows::get_voice(&pool, show.id, "vi")
            .await
            .unwrap()
            .is_none());
        server.verify().await;
    }

    #[tokio::test]
    async fn fails_within_grace_when_voice_is_absent() {
        let server = MockServer::start().await;
        mount_options(&server).await;
        mount_design_sequence(&server, vec![5]).await;
        mount_design_status(&server, 5, "ready").await;
        mount_save(&server, 5, 42).await;
        Mock::given(method("GET"))
            .and(path("/api/voices"))
            .respond_with(voices_response(serde_json::json!([])))
            .expect(3)
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&omnivoice_cfg(&server));
        let pool = db::test_pool().await;
        let (show, ep) = seed_episode(&pool).await;
        let root = tempfile::tempdir().unwrap();

        let started = std::time::Instant::now();
        let error = resolve_voice(&pool, &app_cfg(root.path()), &ep, &client)
            .await
            .unwrap_err();
        assert!(
            started.elapsed() < Duration::from_secs(12),
            "absent voice must fail within grace, took {:?}",
            started.elapsed()
        );
        assert_eq!(error.code, "voice_design_failed");
        assert!(shows::get_voice(&pool, show.id, "vi")
            .await
            .unwrap()
            .is_none());
        server.verify().await;
    }

    #[tokio::test]
    async fn cancelled_episode_skips_design_and_save_calls() {
        let server = MockServer::start().await;
        for (verb, endpoint) in [
            ("GET", "/api/voices"),
            ("GET", "/api/voice-designs/options"),
            ("GET", "/api/voice-designs/7"),
            ("POST", "/api/voice-designs"),
            ("POST", "/api/voice-designs/7/save"),
            ("DELETE", "/api/voice-designs/7"),
        ] {
            Mock::given(method(verb))
                .and(path(endpoint))
                .respond_with(ResponseTemplate::new(500))
                .expect(0)
                .mount(&server)
                .await;
        }

        let client = OmnivoiceClient::new(&omnivoice_cfg(&server));
        let pool = db::test_pool().await;
        let (_, ep) = seed_episode(&pool).await;
        db::episodes::set_voice(&pool, &ep.uuid, 42).await.unwrap();
        db::episodes::set_voice_design(&pool, &ep.uuid, Some(7))
            .await
            .unwrap();
        db::episodes::cancel(&pool, &ep.uuid).await.unwrap();
        let ep = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(ep.status, crate::models::status::CANCELLED);

        let root = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        let voice = resolve_voice(&pool, &app_cfg(root.path()), &ep, &client)
            .await
            .unwrap();
        assert_eq!(voice, 42);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "cancelled voicing must return promptly, took {:?}",
            started.elapsed()
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn maps_voice_options_upstream_failure_to_tts_unavailable() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/voice-designs/options"))
            .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                "ok": false,
                "data": null,
                "error": {"code": "model_missing", "message": "model is not loaded"}
            })))
            .mount(&server)
            .await;

        let client = OmnivoiceClient::new(&omnivoice_cfg(&server));
        let pool = db::test_pool().await;
        let (_, ep) = seed_episode(&pool).await;
        let root = tempfile::tempdir().unwrap();

        let error = resolve_voice(&pool, &app_cfg(root.path()), &ep, &client)
            .await
            .unwrap_err();
        assert_eq!(error.code, "tts_unavailable");
        assert!(error.retryable);
    }
}
