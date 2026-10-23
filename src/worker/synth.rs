use crate::config::Config;
use crate::db::episodes;
use crate::domain::summarize::split_chunks;
use crate::models::{stage, Episode};
use crate::services::omnivoice::{OmnivoiceClient, SpeechTake};
use crate::worker::PipelineError;
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_CHUNK_CHARS: usize = 4500;
const POLL_INTERVAL: Duration = Duration::from_secs(3);

pub fn parts_dir(cfg: &Config, uuid: &str) -> PathBuf {
    cfg.data_dir.join("tmp").join(uuid)
}

fn script_text(episode: &Episode) -> String {
    episode
        .audio_script
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| episode.summary_text.clone())
        .unwrap_or_default()
}

pub async fn synthesize_episode(
    pool: &SqlitePool,
    cfg: &Config,
    episode: &Episode,
    client: &OmnivoiceClient,
) -> Result<(), PipelineError> {
    let text = script_text(episode);
    let limit = cfg.omnivoice.chunk_chars.min(MAX_CHUNK_CHARS);
    let chunks = split_chunks(&text, limit);
    if chunks.is_empty() {
        return Err(PipelineError::new(
            "synthesis_failed",
            "Bản tóm tắt trống.",
            false,
        ));
    }
    let count = chunks.len();
    let dir = parts_dir(cfg, &episode.uuid);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| PipelineError::from_app(e.into(), "disk_full"))?;

    let snapshot_hash = episode.summary_hash.clone().unwrap_or_default();
    let snapshot_path = dir.join("summary_hash.txt");
    let previous_snapshot = tokio::fs::read_to_string(&snapshot_path).await.ok();
    let mut take_ids: Vec<i64> = episode
        .take_ids_json
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default();
    let mut done = (episode.chunk_done.max(0) as usize).min(count);
    take_ids.resize(done, 0);
    let mut active_take = episode.active_take_id;

    let summary_changed = previous_snapshot
        .as_deref()
        .is_some_and(|previous| previous.trim() != snapshot_hash.trim());
    if summary_changed {
        if let Some(active_id) = active_take {
            let _ = client.delete_speech(active_id).await;
        }
        active_take = None;
        episodes::set_active_take(pool, &episode.uuid, None)
            .await
            .map_err(|e| PipelineError::from_app(e, "internal"))?;
        remove_parts(&dir).await?;
        take_ids.clear();
        done = 0;
        save_progress(pool, &episode.uuid, &take_ids, done, count).await?;
    }
    if done == 0 && take_ids.is_empty() && active_take.is_none() {
        remove_parts(&dir).await?;
    }
    tokio::fs::write(&snapshot_path, &snapshot_hash)
        .await
        .map_err(|e| PipelineError::from_app(e.into(), "disk_full"))?;
    episodes::set_chunk_count(pool, &episode.uuid, count as i64)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;

    let voice_id = episode
        .voice_id
        .ok_or_else(|| PipelineError::new("tts_unavailable", "Chưa có giọng đọc.", true))?;
    let timeout = Duration::from_secs(cfg.omnivoice.synthesis_timeout_secs);

    if let Some(active_id) = active_take {
        if cancel_if_needed(pool, client, &episode.uuid, &dir, count).await? {
            return Ok(());
        }
        let part = dir.join(format!("part-{done:03}.mp3"));
        if part.exists() && looks_nonempty(&part).await {
            let _ = client.delete_speech(active_id).await;
            episodes::set_active_take(pool, &episode.uuid, None)
                .await
                .map_err(|e| PipelineError::from_app(e, "internal"))?;
            take_ids.push(active_id);
            done += 1;
            save_progress(pool, &episode.uuid, &take_ids, done, count).await?;
        } else {
            let polled = poll_take(pool, client, &episode.uuid, active_id, timeout).await;
            if cancel_if_needed(pool, client, &episode.uuid, &dir, count).await? {
                return Ok(());
            }
            match polled {
                Ok(PollOutcome::Done(take)) => {
                    if let Err(error) = download_part(client, take.id, &part).await {
                        if cancel_if_needed(pool, client, &episode.uuid, &dir, count).await? {
                            return Ok(());
                        }
                        return Err(error);
                    }
                    let _ = client.delete_speech(take.id).await;
                    take_ids.push(take.id);
                    done += 1;
                    save_progress(pool, &episode.uuid, &take_ids, done, count).await?;
                }
                Ok(PollOutcome::Failed) | Err(_) => {
                    let _ = client.delete_speech(active_id).await;
                    episodes::set_active_take(pool, &episode.uuid, None)
                        .await
                        .map_err(|e| PipelineError::from_app(e, "internal"))?;
                }
                Ok(PollOutcome::Cancelled) => {
                    cancel_if_needed(pool, client, &episode.uuid, &dir, count).await?;
                    return Ok(());
                }
            }
        }
    }

    for (index, chunk) in chunks.iter().enumerate().skip(done) {
        if cancel_if_needed(pool, client, &episode.uuid, &dir, count).await? {
            return Ok(());
        }
        let part = dir.join(format!("part-{index:03}.mp3"));
        if part.exists() && looks_nonempty(&part).await {
            take_ids.push(0);
            save_progress(pool, &episode.uuid, &take_ids, index + 1, count).await?;
            continue;
        }

        let mut attempt = 0;
        loop {
            let take = match client.create_speech(chunk, voice_id).await {
                Ok(take) => take,
                Err(error) => {
                    if cancel_if_needed(pool, client, &episode.uuid, &dir, count).await? {
                        return Ok(());
                    }
                    return Err(PipelineError::new(
                        "tts_unavailable",
                        error.to_string(),
                        true,
                    ));
                }
            };
            episodes::set_active_take(pool, &episode.uuid, Some(take.id))
                .await
                .map_err(|e| PipelineError::from_app(e, "internal"))?;
            let polled = poll_take(pool, client, &episode.uuid, take.id, timeout).await;
            if cancel_if_needed(pool, client, &episode.uuid, &dir, count).await? {
                return Ok(());
            }
            match polled {
                Ok(PollOutcome::Done(take)) => {
                    if let Err(error) = download_part(client, take.id, &part).await {
                        if cancel_if_needed(pool, client, &episode.uuid, &dir, count).await? {
                            return Ok(());
                        }
                        return Err(error);
                    }
                    let _ = client.delete_speech(take.id).await;
                    take_ids.push(take.id);
                    save_progress(pool, &episode.uuid, &take_ids, index + 1, count).await?;
                    break;
                }
                Ok(PollOutcome::Cancelled) => {
                    cancel_if_needed(pool, client, &episode.uuid, &dir, count).await?;
                    return Ok(());
                }
                Ok(PollOutcome::Failed) | Err(_) => {
                    let _ = client.delete_speech(take.id).await;
                    episodes::set_active_take(pool, &episode.uuid, None)
                        .await
                        .map_err(|e| PipelineError::from_app(e, "internal"))?;
                    if attempt == 0 {
                        attempt += 1;
                        continue;
                    }
                    return Err(PipelineError::new(
                        "synthesis_failed",
                        "Không tạo được audio.",
                        true,
                    ));
                }
            }
        }
    }

    if cancel_if_needed(pool, client, &episode.uuid, &dir, count).await? {
        return Ok(());
    }
    episodes::set_stage(pool, &episode.uuid, stage::ASSEMBLING)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    episodes::set_progress(pool, &episode.uuid, r#"{"percent":80}"#)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    Ok(())
}

async fn save_progress(
    pool: &SqlitePool,
    uuid: &str,
    take_ids: &[i64],
    done: usize,
    count: usize,
) -> Result<(), PipelineError> {
    let take_ids = serde_json::to_string(take_ids).unwrap_or_else(|_| "[]".into());
    episodes::set_chunk_progress(pool, uuid, &take_ids, done as i64, count as i64)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))
}

async fn remove_parts(dir: &Path) -> Result<(), PipelineError> {
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .map_err(|e| PipelineError::from_app(e.into(), "disk_full"))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| PipelineError::from_app(e.into(), "disk_full"))?
    {
        if entry.file_name().to_string_lossy().starts_with("part-") {
            tokio::fs::remove_file(entry.path())
                .await
                .map_err(|e| PipelineError::from_app(e.into(), "disk_full"))?;
        }
    }
    Ok(())
}

async fn looks_nonempty(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .map(|meta| meta.len() > 0)
        .unwrap_or(false)
}

enum PollOutcome {
    Done(SpeechTake),
    Failed,
    Cancelled,
}

async fn cancel_if_needed(
    pool: &SqlitePool,
    client: &OmnivoiceClient,
    uuid: &str,
    dir: &Path,
    count: usize,
) -> Result<bool, PipelineError> {
    let current = episodes::get(pool, uuid)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?
        .ok_or_else(|| PipelineError::new("internal", "episode disappeared", false))?;
    if current.status != crate::models::status::CANCELLED {
        return Ok(false);
    }
    if let Some(take_id) = current.active_take_id {
        let _ = client.delete_speech(take_id).await;
    }
    episodes::set_active_take(pool, uuid, None)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    remove_parts(dir).await?;
    save_progress(pool, uuid, &[], 0, count).await?;
    Ok(true)
}

async fn episode_cancelled(pool: &SqlitePool, uuid: &str) -> Result<bool, PipelineError> {
    let current = episodes::get(pool, uuid)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?
        .ok_or_else(|| PipelineError::new("internal", "episode disappeared", false))?;
    Ok(current.status == crate::models::status::CANCELLED)
}

async fn poll_take(
    pool: &SqlitePool,
    client: &OmnivoiceClient,
    uuid: &str,
    id: i64,
    timeout: Duration,
) -> Result<PollOutcome, PipelineError> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let take = client
            .speech(id)
            .await
            .map_err(|e| PipelineError::new("tts_unavailable", e.to_string(), true))?;
        match take.status.as_str() {
            "done" => return Ok(PollOutcome::Done(take)),
            "failed" => return Ok(PollOutcome::Failed),
            _ => {}
        }
        if episode_cancelled(pool, uuid).await? {
            return Ok(PollOutcome::Cancelled);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(PipelineError::new(
                "synthesis_failed",
                "Omnivoice không phản hồi trong thời gian cho phép.",
                true,
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn download_part(
    client: &OmnivoiceClient,
    id: i64,
    path: &Path,
) -> Result<(), PipelineError> {
    let bytes = client
        .download_mp3(id)
        .await
        .map_err(|e| PipelineError::new("synthesis_failed", e.to_string(), true))?;
    if bytes.is_empty() {
        return Err(PipelineError::new("synthesis_failed", "Audio rỗng.", true));
    }
    let tmp = path.with_extension("tmp");
    tokio::fs::write(&tmp, bytes)
        .await
        .map_err(|e| PipelineError::from_app(e.into(), "disk_full"))?;
    tokio::fs::rename(&tmp, path)
        .await
        .map_err(|e| PipelineError::from_app(e.into(), "disk_full"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthMode, OmnivoiceConfig};
    use crate::db::{
        self,
        shows::{self, NewShow},
    };
    use crate::models::stage;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn app_cfg(root: &std::path::Path, chunk_chars: usize) -> Config {
        app_cfg_with(root, chunk_chars, None)
    }

    fn app_cfg_with(
        root: &std::path::Path,
        chunk_chars: usize,
        synthesis_timeout_secs: Option<u64>,
    ) -> Config {
        Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "DATA_DIR" => Some(root.display().to_string()),
            "CHUNK_CHARS" => Some(chunk_chars.to_string()),
            "SYNTHESIS_TIMEOUT_SECS" => synthesis_timeout_secs.map(|value| value.to_string()),
            _ => None,
        })
        .unwrap()
    }

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

    async fn seed_episode(pool: &SqlitePool, summary: &str) -> Episode {
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
                review_required: false,
            },
        )
        .await
        .unwrap();
        db::episodes::set_summary(pool, &ep.uuid, summary, summary, "hash")
            .await
            .unwrap();
        db::episodes::set_voice(pool, &ep.uuid, 7).await.unwrap();
        db::episodes::get(pool, &ep.uuid).await.unwrap().unwrap()
    }

    async fn mount_finished_take(
        server: &MockServer,
        id: i64,
        audio: &[u8],
        creates: u64,
        takes: u64,
    ) {
        Mock::given(method("POST"))
            .and(path("/api/speech"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": {"id": id, "status": "queued"}, "error": null
            })))
            .expect(creates)
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/speech/{id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": {"id": id, "status": "done"}, "error": null
            })))
            .expect(takes)
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!("/api/speech/{id}/audio")))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(audio.to_vec()))
            .expect(takes)
            .mount(server)
            .await;
        Mock::given(method("DELETE"))
            .and(path(format!("/api/speech/{id}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": null, "error": null
            })))
            .expect(takes)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn prefers_the_audio_script_for_synthesis() {
        let pool = db::test_pool().await;
        let episode = seed_episode(&pool, "Tóm tắt ngắn.").await;
        db::episodes::set_summary(
            &pool,
            &episode.uuid,
            "Tóm tắt ngắn.",
            "Script đầy đủ để đọc.",
            "hash",
        )
        .await
        .unwrap();
        let episode = db::episodes::get(&pool, &episode.uuid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(script_text(&episode), "Script đầy đủ để đọc.");
    }

    #[tokio::test]
    async fn falls_back_to_the_display_summary_for_legacy_rows() {
        let pool = db::test_pool().await;
        let episode = seed_episode(&pool, "Tóm tắt cũ.").await;
        sqlx::query("UPDATE episodes SET audio_script = NULL WHERE uuid = ?1")
            .bind(&episode.uuid)
            .execute(&pool)
            .await
            .unwrap();
        let episode = db::episodes::get(&pool, &episode.uuid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(script_text(&episode), "Tóm tắt cũ.");
    }

    #[tokio::test]
    async fn splits_a_script_and_synthesizes_each_chunk_in_order() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/speech"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": {"id": 1, "status": "queued"}, "error": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/speech/1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": {"id": 1, "status": "done"}, "error": null
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/speech/1/audio"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"part".to_vec()))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/api/speech/1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": null, "error": null
            })))
            .mount(&server)
            .await;

        let root = tempfile::tempdir().unwrap();
        let cfg = app_cfg(root.path(), 10);
        let client = crate::services::omnivoice::OmnivoiceClient::new(&omni_cfg(&server));
        let pool = db::test_pool().await;
        let ep = seed_episode(&pool, "Câu một. Câu hai. Câu ba. Câu bốn.").await;

        synthesize_episode(&pool, &cfg, &ep, &client).await.unwrap();
        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.stage, stage::ASSEMBLING);
        assert_eq!(got.chunk_done, 4);
        assert_eq!(got.chunk_count, Some(4));
        assert!(root
            .path()
            .join("tmp")
            .join(&ep.uuid)
            .join("part-000.mp3")
            .exists());
        assert!(root
            .path()
            .join("tmp")
            .join(&ep.uuid)
            .join("part-003.mp3")
            .exists());
    }

    #[tokio::test]
    async fn retries_only_missing_chunks_after_a_crash() {
        let server = MockServer::start().await;
        mount_finished_take(&server, 21, b"new", 2, 2).await;

        let root = tempfile::tempdir().unwrap();
        let cfg = app_cfg(root.path(), 10);
        let client = crate::services::omnivoice::OmnivoiceClient::new(&omni_cfg(&server));
        let pool = db::test_pool().await;
        let ep = seed_episode(&pool, "Câu một. Câu hai. Câu ba. Câu bốn.").await;
        let dir = parts_dir(&cfg, &ep.uuid);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("part-000.mp3"), b"old").unwrap();
        std::fs::write(dir.join("part-001.mp3"), b"old").unwrap();
        db::episodes::set_chunk_progress(&pool, &ep.uuid, "[11, 12]", 2, 4)
            .await
            .unwrap();
        let ep = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();

        synthesize_episode(&pool, &cfg, &ep, &client).await.unwrap();
        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.chunk_done, 4);
        assert_eq!(got.chunk_count, Some(4));
        assert_eq!(std::fs::read(dir.join("part-002.mp3")).unwrap(), b"new");
        assert_eq!(std::fs::read(dir.join("part-003.mp3")).unwrap(), b"new");
        let takes: Vec<i64> = serde_json::from_str(got.take_ids_json.as_deref().unwrap()).unwrap();
        assert_eq!(takes, vec![11, 12, 21, 21]);
        server.verify().await;
    }

    #[tokio::test]
    async fn changed_summary_discards_partial_parts_and_active_take() {
        let server = MockServer::start().await;
        mount_finished_take(&server, 41, b"fresh", 4, 4).await;
        Mock::given(method("DELETE"))
            .and(path("/api/speech/99"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": null, "error": null
            })))
            .expect(1)
            .mount(&server)
            .await;

        let root = tempfile::tempdir().unwrap();
        let cfg = app_cfg(root.path(), 10);
        let client = crate::services::omnivoice::OmnivoiceClient::new(&omni_cfg(&server));
        let pool = db::test_pool().await;
        let ep = seed_episode(&pool, "Câu cũ. Câu cũ hai.").await;
        db::episodes::set_summary(
            &pool,
            &ep.uuid,
            "Câu một. Câu hai. Câu ba. Câu bốn.",
            "Câu một. Câu hai. Câu ba. Câu bốn.",
            "new-hash",
        )
        .await
        .unwrap();
        let dir = parts_dir(&cfg, &ep.uuid);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("summary_hash.txt"), "hash").unwrap();
        std::fs::write(dir.join("part-000.mp3"), b"stale").unwrap();
        std::fs::write(dir.join("part-001.mp3"), b"stale").unwrap();
        db::episodes::set_chunk_progress(&pool, &ep.uuid, "[11, 12]", 2, 4)
            .await
            .unwrap();
        db::episodes::set_active_take(&pool, &ep.uuid, Some(99))
            .await
            .unwrap();
        let ep = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();

        synthesize_episode(&pool, &cfg, &ep, &client).await.unwrap();
        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.chunk_done, 4);
        assert_eq!(got.chunk_count, Some(4));
        assert_eq!(got.active_take_id, None);
        assert_eq!(
            serde_json::from_str::<Vec<i64>>(got.take_ids_json.as_deref().unwrap()).unwrap(),
            vec![41, 41, 41, 41]
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("summary_hash.txt")).unwrap(),
            "new-hash"
        );
        for index in 0..4 {
            assert_eq!(
                std::fs::read(dir.join(format!("part-{index:03}.mp3"))).unwrap(),
                b"fresh"
            );
        }
        server.verify().await;
    }

    #[tokio::test]
    async fn recovers_when_the_active_take_is_gone() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/speech/77"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "ok": false, "data": null,
                "error": {"code": "not_found", "message": "take is gone"}
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/api/speech/77"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": null, "error": null
            })))
            .expect(1)
            .mount(&server)
            .await;
        mount_finished_take(&server, 88, b"retry", 1, 1).await;

        let root = tempfile::tempdir().unwrap();
        let cfg = app_cfg(root.path(), 10);
        let client = crate::services::omnivoice::OmnivoiceClient::new(&omni_cfg(&server));
        let pool = db::test_pool().await;
        let ep = seed_episode(&pool, "Câu một.").await;
        db::episodes::set_active_take(&pool, &ep.uuid, Some(77))
            .await
            .unwrap();
        db::episodes::set_chunk_count(&pool, &ep.uuid, 1)
            .await
            .unwrap();
        let ep = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();

        synthesize_episode(&pool, &cfg, &ep, &client).await.unwrap();
        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.active_take_id, None);
        assert_eq!(got.chunk_done, 1);
        assert_eq!(got.stage, stage::ASSEMBLING);
        assert_eq!(
            std::fs::read(parts_dir(&cfg, &ep.uuid).join("part-000.mp3")).unwrap(),
            b"retry"
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn retries_a_failed_take_once() {
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let sequence = calls.clone();
        Mock::given(method("POST"))
            .and(path("/api/speech"))
            .respond_with(move |_: &wiremock::Request| {
                let index = sequence.fetch_add(1, Ordering::SeqCst);
                let id = if index == 0 { 91 } else { 92 };
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "ok": true, "data": {"id": id, "status": "queued"}, "error": null
                }))
            })
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/speech/91"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": {"id": 91, "status": "failed", "error": "boom"}, "error": null
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/api/speech/91"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": null, "error": null
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/speech/92"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": {"id": 92, "status": "done"}, "error": null
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/speech/92/audio"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(b"second".to_vec()))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/api/speech/92"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": null, "error": null
            })))
            .expect(1)
            .mount(&server)
            .await;

        let root = tempfile::tempdir().unwrap();
        let cfg = app_cfg(root.path(), 10);
        let client = crate::services::omnivoice::OmnivoiceClient::new(&omni_cfg(&server));
        let pool = db::test_pool().await;
        let ep = seed_episode(&pool, "Câu một.").await;

        synthesize_episode(&pool, &cfg, &ep, &client).await.unwrap();
        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.active_take_id, None);
        assert_eq!(got.chunk_done, 1);
        assert_eq!(got.stage, stage::ASSEMBLING);
        assert_eq!(
            serde_json::from_str::<Vec<i64>>(got.take_ids_json.as_deref().unwrap()).unwrap(),
            vec![92]
        );
        assert_eq!(
            std::fs::read(parts_dir(&cfg, &ep.uuid).join("part-000.mp3")).unwrap(),
            b"second"
        );
        server.verify().await;
    }

    #[tokio::test]
    async fn fails_after_the_second_failed_take() {
        let server = MockServer::start().await;
        let calls = Arc::new(AtomicUsize::new(0));
        let sequence = calls.clone();
        Mock::given(method("POST"))
            .and(path("/api/speech"))
            .respond_with(move |_: &wiremock::Request| {
                let index = sequence.fetch_add(1, Ordering::SeqCst);
                let id = if index == 0 { 91 } else { 92 };
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "ok": true, "data": {"id": id, "status": "queued"}, "error": null
                }))
            })
            .expect(2)
            .mount(&server)
            .await;
        for id in [91, 92] {
            Mock::given(method("GET"))
                .and(path(format!("/api/speech/{id}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "ok": true, "data": {"id": id, "status": "failed", "error": "boom"}, "error": null
                })))
                .expect(1)
                .mount(&server)
                .await;
            Mock::given(method("DELETE"))
                .and(path(format!("/api/speech/{id}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "ok": true, "data": null, "error": null
                })))
                .expect(1)
                .mount(&server)
                .await;
        }

        let root = tempfile::tempdir().unwrap();
        let cfg = app_cfg(root.path(), 10);
        let client = crate::services::omnivoice::OmnivoiceClient::new(&omni_cfg(&server));
        let pool = db::test_pool().await;
        let ep = seed_episode(&pool, "Câu một.").await;

        let error = synthesize_episode(&pool, &cfg, &ep, &client)
            .await
            .unwrap_err();
        assert_eq!(error.code, "synthesis_failed");
        assert!(error.retryable);
        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.active_take_id, None);
        assert_eq!(got.chunk_done, 0);
        assert_ne!(got.stage, stage::ASSEMBLING);
        server.verify().await;
    }

    #[tokio::test]
    async fn cancellation_during_poll_cleans_up_and_skips_assembly() {
        let server = MockServer::start().await;
        let hit = Arc::new(AtomicBool::new(false));
        let seen = hit.clone();
        Mock::given(method("GET"))
            .and(path("/api/speech/77"))
            .respond_with(move |_: &wiremock::Request| {
                seen.store(true, Ordering::SeqCst);
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "ok": true, "data": {"id": 77, "status": "queued"}, "error": null
                }))
            })
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/api/speech/77"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": null, "error": null
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/speech"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "ok": true, "data": {"id": 88, "status": "queued"}, "error": null
            })))
            .expect(0)
            .mount(&server)
            .await;

        let root = tempfile::tempdir().unwrap();
        let cfg = app_cfg_with(root.path(), 10, Some(1));
        let client = crate::services::omnivoice::OmnivoiceClient::new(&omni_cfg(&server));
        let pool = db::test_pool().await;
        let ep = seed_episode(&pool, "Câu một. Câu hai.").await;
        let dir = parts_dir(&cfg, &ep.uuid);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("part-000.mp3"), b"done").unwrap();
        db::episodes::set_chunk_progress(&pool, &ep.uuid, "[11]", 1, 2)
            .await
            .unwrap();
        db::episodes::set_active_take(&pool, &ep.uuid, Some(77))
            .await
            .unwrap();
        let ep = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();

        let cancel_pool = pool.clone();
        let cancel_uuid = ep.uuid.clone();
        let watcher = hit.clone();
        tokio::spawn(async move {
            while !watcher.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            db::episodes::cancel(&cancel_pool, &cancel_uuid)
                .await
                .unwrap();
        });

        synthesize_episode(&pool, &cfg, &ep, &client).await.unwrap();
        assert!(hit.load(Ordering::SeqCst));
        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.status, crate::models::status::CANCELLED);
        assert_eq!(got.chunk_done, 0);
        assert_eq!(got.take_ids_json.as_deref(), Some("[]"));
        assert_eq!(got.active_take_id, None);
        assert_ne!(got.stage, stage::ASSEMBLING);
        let leftover_parts = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("part-"))
            .count();
        assert_eq!(leftover_parts, 0);
        server.verify().await;
    }

    #[tokio::test]
    async fn clamps_chunk_size_to_4500_chars() {
        let server = MockServer::start().await;
        mount_finished_take(&server, 61, b"clamped", 2, 2).await;
        let root = tempfile::tempdir().unwrap();
        let cfg = app_cfg(root.path(), 5000);
        let client = crate::services::omnivoice::OmnivoiceClient::new(&omni_cfg(&server));
        let pool = db::test_pool().await;
        let summary = "a ".repeat(2300);
        let ep = seed_episode(&pool, &summary).await;

        synthesize_episode(&pool, &cfg, &ep, &client).await.unwrap();
        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.chunk_count, Some(2));
        assert_eq!(got.chunk_done, 2);
        server.verify().await;
    }

    #[tokio::test]
    async fn reread_discards_parts_from_a_previous_summary() {
        let server = MockServer::start().await;
        mount_finished_take(&server, 31, b"fresh", 4, 4).await;
        let root = tempfile::tempdir().unwrap();
        let cfg = app_cfg(root.path(), 10);
        let client = crate::services::omnivoice::OmnivoiceClient::new(&omni_cfg(&server));
        let pool = db::test_pool().await;
        let ep = seed_episode(&pool, "Câu một. Câu hai. Câu ba. Câu bốn.").await;
        let dir = parts_dir(&cfg, &ep.uuid);
        std::fs::create_dir_all(&dir).unwrap();
        for index in 0..4 {
            std::fs::write(dir.join(format!("part-{index:03}.mp3")), b"stale").unwrap();
        }

        synthesize_episode(&pool, &cfg, &ep, &client).await.unwrap();
        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.chunk_done, 4);
        for index in 0..4 {
            assert_eq!(
                std::fs::read(dir.join(format!("part-{index:03}.mp3"))).unwrap(),
                b"fresh"
            );
        }
        server.verify().await;
    }

    #[tokio::test]
    async fn resumes_an_active_take_after_a_crash() {
        let server = MockServer::start().await;
        mount_finished_take(&server, 55, b"recovered", 0, 1).await;

        let root = tempfile::tempdir().unwrap();
        let cfg = app_cfg(root.path(), 10);
        let client = crate::services::omnivoice::OmnivoiceClient::new(&omni_cfg(&server));
        let pool = db::test_pool().await;
        let ep = seed_episode(&pool, "Câu một.").await;
        db::episodes::set_active_take(&pool, &ep.uuid, Some(55))
            .await
            .unwrap();
        db::episodes::set_chunk_count(&pool, &ep.uuid, 1)
            .await
            .unwrap();
        let ep = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();

        synthesize_episode(&pool, &cfg, &ep, &client).await.unwrap();
        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.chunk_done, 1);
        assert_eq!(got.chunk_count, Some(1));
        assert_eq!(got.active_take_id, None);
        assert_eq!(
            std::fs::read(parts_dir(&cfg, &ep.uuid).join("part-000.mp3")).unwrap(),
            b"recovered"
        );
        server.verify().await;
    }
}
