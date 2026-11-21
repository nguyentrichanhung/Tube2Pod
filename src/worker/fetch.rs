use crate::config::Config;
use crate::db::episodes;
use crate::models::{stage, Episode};
use crate::services::ytdlp;
use crate::worker::PipelineError;
use sqlx::SqlitePool;

pub async fn fetch_episode(
    pool: &SqlitePool,
    cfg: &Config,
    episode: &Episode,
) -> Result<(), PipelineError> {
    let started = sqlx::query("UPDATE episodes SET stage='fetching', progress_json=?1, updated_at=?2 WHERE uuid=?3 AND status='running' AND stage IN ('queued','fetching')")
        .bind(r#"{"percent":10}"#).bind(crate::db::now_ts()).bind(&episode.uuid).execute(pool).await.map_err(|e| PipelineError::from_app(e.into(), "internal"))?;
    if started.rows_affected() == 0 {
        return Ok(());
    }
    if episode.stage != stage::FETCHING {
        crate::worker::stage_changed(pool, &episode.uuid, stage::FETCHING).await;
    }
    let video = ytdlp::validate_url(&episode.source_url)
        .map_err(|e| PipelineError::new(ytdlp::classify_source_error(&e), e.to_string(), false))?;
    if let Some(existing) = episodes::get_by_video_id(pool, &video.id)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?
    {
        if existing.uuid != episode.uuid {
            return Err(PipelineError::new(
                "invalid_url",
                "Video này đã có trong thư viện.",
                false,
            ));
        }
    }
    let meta =
        match crate::worker::cancellable(pool, &episode.uuid, ytdlp::probe(cfg, &video)).await {
            Some(result) => result.map_err(|e| {
                let code = ytdlp::classify_source_error(&e);
                PipelineError::from_app(e, code)
            })?,
            None => return Ok(()),
        };
    let max = crate::db::settings::read_i64(pool, "max_source_minutes", 360)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    let min = crate::db::settings::read_i64(pool, "min_source_seconds", 60)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    let duration = meta.duration.unwrap_or(0);
    if duration > max.saturating_mul(60) {
        return Err(PipelineError::new(
            "source_too_long",
            "Video dài hơn giới hạn.",
            false,
        ));
    }
    if duration < min {
        return Err(PipelineError::new(
            "source_too_short",
            "Video quá ngắn để làm podcast.",
            false,
        ));
    }
    let key = episode
        .source_audio_track
        .as_deref()
        .ok_or_else(|| PipelineError::new("source_unavailable", "Chưa chọn bản audio.", false))?;
    let tracks = ytdlp::audio_tracks(&meta);
    let track = tracks
        .iter()
        .find(|track| track.key == key)
        .ok_or_else(|| {
            PipelineError::new(
                "source_unavailable",
                "Bản audio đã chọn không còn khả dụng.",
                false,
            )
        })?;
    if tracks
        .iter()
        .filter(|other| other.format_id == track.format_id)
        .count()
        != 1
        || meta
            .formats
            .iter()
            .filter(|format| format.format_id == track.format_id)
            .count()
            != 1
    {
        return Err(PipelineError::new(
            "source_unavailable",
            "Không xác định duy nhất định dạng bản audio đã chọn.",
            false,
        ));
    }
    let dir = cfg.data_dir.join("tmp").join(&episode.uuid);
    let path = match crate::worker::cancellable(
        pool,
        &episode.uuid,
        ytdlp::download_audio(cfg, &video, &track.format_id, &dir),
    )
    .await
    {
        Some(result) => result.map_err(|e| PipelineError::from_app(e, "source_unavailable"))?,
        None => {
            let _ = tokio::fs::remove_dir_all(&dir).await;
            return Ok(());
        }
    };
    tokio::fs::rename(path, dir.join("source.media"))
        .await
        .map_err(|e| PipelineError::from_app(e.into(), "internal"))?;
    let result = sqlx::query("UPDATE episodes SET source_title=?1, source_channel=?2, source_duration_s=?3, source_thumbnail_url=?4, source_upload_date=?5, source_description=?6, stage='assembling', attempts=0, progress_json='{\"percent\":35}', updated_at=?8 WHERE uuid=?7 AND status='running' AND stage='fetching'")
        .bind(&meta.title).bind(meta.channel.as_deref().unwrap_or("")).bind(duration).bind(meta.thumbnail.as_deref()).bind(meta.upload_date.as_deref()).bind(meta.description.as_deref()).bind(&episode.uuid).bind(crate::db::now_ts()).execute(pool).await.map_err(|e| PipelineError::from_app(e.into(), "internal"))?;
    if result.rows_affected() > 0 {
        crate::worker::stage_changed(pool, &episode.uuid, stage::ASSEMBLING).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{
        self,
        shows::{self, NewShow},
    };
    use std::os::unix::fs::PermissionsExt;

    async fn setup(
        root: &std::path::Path,
        script: &str,
        key: &str,
    ) -> (Config, SqlitePool, Episode) {
        let tool = root.join("yt-dlp");
        std::fs::write(&tool, script).unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cfg = Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "DATA_DIR" => Some(root.display().to_string()),
            "YTDLP_PATH" => Some(tool.display().to_string()),
            _ => None,
        })
        .unwrap();
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
        episodes::create(
            &pool,
            episodes::NewEpisode {
                show_id: show.id,
                video_id: "dQw4w9WgXcQ".into(),
                source_url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(),
                source_title: "Video".into(),
                source_channel: "".into(),
                source_duration_s: Some(600),
                source_thumbnail_url: None,
                source_upload_date: None,
                language: "en".into(),
                source_audio_track: Some(key.into()),
                source_description: None,
                target_minutes: 10,
                review_required: false,
            },
        )
        .await
        .unwrap();
        let ep = episodes::claim_next(&pool).await.unwrap().unwrap();
        (cfg, pool, ep)
    }

    #[tokio::test]
    async fn source_limits_are_read_from_settings() {
        let dir = tempfile::tempdir().unwrap();
        let script = "#!/bin/sh\nprintf '%s' '{\"title\":\"Video\",\"duration\":120}'\n";
        let (cfg, pool, ep) = setup(dir.path(), script, "id:alt").await;
        db::settings::set(&pool, "max_source_minutes", "1")
            .await
            .unwrap();
        assert_eq!(
            fetch_episode(&pool, &cfg, &ep).await.unwrap_err().code,
            "source_too_long"
        );
        db::settings::set(&pool, "max_source_minutes", "10")
            .await
            .unwrap();
        db::settings::set(&pool, "min_source_seconds", "180")
            .await
            .unwrap();
        assert_eq!(
            fetch_episode(&pool, &cfg, &ep).await.unwrap_err().code,
            "source_too_short"
        );
    }

    #[tokio::test]
    async fn selected_audio_missing_at_download_fails_without_switching_tracks() {
        let dir = tempfile::tempdir().unwrap();
        let script = "#!/bin/sh\nprintf '%s' '{\"title\":\"Video\",\"duration\":600,\"formats\":[{\"format_id\":\"main\",\"audio_track_id\":\"main\",\"acodec\":\"opus\",\"vcodec\":\"none\"}]}'\n";
        let (cfg, pool, ep) = setup(dir.path(), script, "id:alt").await;
        let err = fetch_episode(&pool, &cfg, &ep).await.unwrap_err();
        assert!(!err.retryable);
        assert!(err.message.contains("audio"));
        assert!(!dir
            .path()
            .join("tmp")
            .join(&ep.uuid)
            .join("source.media")
            .exists());
    }

    #[tokio::test]
    async fn cancellation_and_immediate_retry_kills_old_audio_download() {
        let dir = tempfile::tempdir().unwrap();
        let started = dir.path().join("started");
        let finished = dir.path().join("finished");
        let script = format!("#!/bin/sh\nif [ \"$1\" = --dump-single-json ]; then printf '%s' '{{\"title\":\"Video\",\"duration\":600,\"formats\":[{{\"format_id\":\"alt\",\"audio_track_id\":\"alt\",\"acodec\":\"opus\",\"vcodec\":\"none\"}}]}}'; exit; fi\ntouch '{}'\nsleep 2\ntouch '{}'\n", started.display(), finished.display());
        let (cfg, pool, ep) = setup(dir.path(), &script, "id:alt").await;
        let uuid = ep.uuid.clone();
        let task_pool = pool.clone();
        let task = tokio::spawn(async move { fetch_episode(&task_pool, &cfg, &ep).await });
        for _ in 0..500 {
            if started.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(started.exists());
        episodes::cancel(&pool, &uuid).await.unwrap();
        episodes::retry(&pool, &uuid).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
        assert!(!finished.exists());
        assert_eq!(
            episodes::get(&pool, &uuid).await.unwrap().unwrap().status,
            "queued"
        );
        assert!(!dir.path().join("tmp").join(uuid).exists());
    }
}
