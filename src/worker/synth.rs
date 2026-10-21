use crate::config::Config;
use crate::db::episodes;
use crate::domain::summarize::split_chunks;
use crate::models::{stage, Episode};
use crate::services::omnivoice::{OmnivoiceClient, SpeechTake};
use crate::worker::PipelineError;
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn parts_dir(cfg: &Config, uuid: &str) -> PathBuf {
    cfg.data_dir.join("tmp").join(uuid)
}

pub async fn synthesize_episode(
    pool: &SqlitePool,
    cfg: &Config,
    episode: &Episode,
    client: &OmnivoiceClient,
) -> Result<(), PipelineError> {
    let text = episode.summary_text.clone().unwrap_or_default();
    let chunks = split_chunks(&text, cfg.omnivoice.chunk_chars);
    if chunks.is_empty() {
        return Err(PipelineError::new(
            "synthesis_failed",
            "Bản tóm tắt trống.",
            false,
        ));
    }
    let dir = parts_dir(cfg, &episode.uuid);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| PipelineError::from_app(e.into(), "disk_full"))?;
    let snapshot_hash = episode.summary_hash.clone().unwrap_or_default();
    tokio::fs::write(dir.join("summary_hash.txt"), snapshot_hash)
        .await
        .map_err(|e| PipelineError::from_app(e.into(), "disk_full"))?;
    episodes::set_chunk_count(pool, &episode.uuid, chunks.len() as i64)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;

    let mut take_ids: Vec<i64> = episode
        .take_ids_json
        .as_deref()
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default();
    let mut done = episode.chunk_done.max(0) as usize;
    let voice_id = episode
        .voice_id
        .ok_or_else(|| PipelineError::new("tts_unavailable", "Chưa có giọng đọc.", true))?;

    if let Some(active_id) = episode.active_take_id {
        let part = dir.join(format!("part-{done:03}.mp3"));
        if part.exists() && looks_nonempty(&part).await {
            episodes::set_active_take(pool, &episode.uuid, None)
                .await
                .map_err(|e| PipelineError::from_app(e, "internal"))?;
            take_ids.push(active_id);
            done += 1;
            save_progress(pool, &episode.uuid, &take_ids, done, chunks.len()).await?;
        } else {
            let take = poll_take(client, active_id).await?;
            if take.status == "done" {
                download_part(client, active_id, &part).await?;
                let _ = client.delete_speech(active_id).await;
                take_ids.push(active_id);
                done += 1;
                save_progress(pool, &episode.uuid, &take_ids, done, chunks.len()).await?;
            } else {
                let _ = client.delete_speech(active_id).await;
                episodes::set_active_take(pool, &episode.uuid, None)
                    .await
                    .map_err(|e| PipelineError::from_app(e, "internal"))?;
            }
        }
    }

    for index in done..chunks.len() {
        let current = episodes::get(pool, &episode.uuid)
            .await
            .map_err(|e| PipelineError::from_app(e, "internal"))?
            .ok_or_else(|| PipelineError::new("internal", "episode disappeared", false))?;
        if current.status == crate::models::status::CANCELLED {
            if let Some(take_id) = current.active_take_id {
                let _ = client.delete_speech(take_id).await;
            }
            return Ok(());
        }
        let part = dir.join(format!("part-{index:03}.mp3"));
        if part.exists() && looks_nonempty(&part).await {
            take_ids.push(0);
            save_progress(pool, &episode.uuid, &take_ids, index + 1, chunks.len()).await?;
            continue;
        }

        let take = client
            .create_speech(&chunks[index], voice_id)
            .await
            .map_err(|e| PipelineError::new("tts_unavailable", e.to_string(), true))?;
        episodes::set_active_take(pool, &episode.uuid, Some(take.id))
            .await
            .map_err(|e| PipelineError::from_app(e, "internal"))?;
        let take = poll_take(client, take.id).await?;
        if take.status != "done" {
            let _ = client.delete_speech(take.id).await;
            episodes::set_active_take(pool, &episode.uuid, None)
                .await
                .map_err(|e| PipelineError::from_app(e, "internal"))?;
            return Err(PipelineError::new(
                "synthesis_failed",
                "Không tạo được audio.",
                true,
            ));
        }
        download_part(client, take.id, &part).await?;
        let _ = client.delete_speech(take.id).await;
        take_ids.push(take.id);
        save_progress(pool, &episode.uuid, &take_ids, index + 1, chunks.len()).await?;
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

async fn looks_nonempty(path: &Path) -> bool {
    tokio::fs::metadata(path)
        .await
        .map(|meta| meta.len() > 0)
        .unwrap_or(false)
}

async fn poll_take(client: &OmnivoiceClient, id: i64) -> Result<SpeechTake, PipelineError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(7200);
    loop {
        let take = client
            .speech(id)
            .await
            .map_err(|e| PipelineError::new("tts_unavailable", e.to_string(), true))?;
        match take.status.as_str() {
            "done" => return Ok(take),
            "failed" => return Ok(take),
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(PipelineError::new(
                "synthesis_failed",
                "Omnivoice không phản hồi trong 2 giờ.",
                true,
            ));
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
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
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn app_cfg(root: &std::path::Path, chunk_chars: usize) -> Config {
        Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "DATA_DIR" => Some(root.display().to_string()),
            "CHUNK_CHARS" => Some(chunk_chars.to_string()),
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
            },
        )
        .await
        .unwrap();
        db::episodes::set_summary(pool, &ep.uuid, summary, "hash")
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
        assert_eq!(takes.len(), 4);
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
