use crate::config::Config;
#[cfg(test)]
use crate::db::episodes;
use crate::models::{stage, Episode};
use crate::services::ffmpeg;
use crate::worker::PipelineError;
use sqlx::SqlitePool;

pub async fn assemble_episode(
    pool: &SqlitePool,
    cfg: &Config,
    episode: &Episode,
) -> Result<(), PipelineError> {
    let dir = cfg.data_dir.join("tmp").join(&episode.uuid);
    let source = dir.join("source.media");
    if !source.is_file() {
        let result = sqlx::query("UPDATE episodes SET stage='fetching', updated_at=?1 WHERE uuid=?2 AND status='running' AND stage='assembling'").bind(crate::db::now_ts()).bind(&episode.uuid).execute(pool).await.map_err(|e| PipelineError::from_app(e.into(), "internal"))?;
        if result.rows_affected() > 0 {
            crate::worker::stage_changed(pool, &episode.uuid, stage::FETCHING).await;
        }
        return Ok(());
    }
    let out = dir.join("out.mp3");
    let probe = match crate::worker::cancellable(
        pool,
        &episode.uuid,
        ffmpeg::convert_to_mp3(cfg, &source, &out),
    )
    .await
    {
        Some(result) => result.map_err(|e| PipelineError::from_app(e, "assembly_failed"))?,
        None => {
            let _ = tokio::fs::remove_file(&out).await;
            return Ok(());
        }
    };
    let version = if episode.audio_path.is_some() {
        episode.audio_version + 1
    } else {
        1
    };
    let audio_dir = cfg.data_dir.join("audio");
    tokio::fs::create_dir_all(&audio_dir)
        .await
        .map_err(|e| PipelineError::new("disk_full", e.to_string(), false))?;
    let target = audio_dir.join(format!("{}-{version}.mp3", episode.uuid));
    tokio::fs::rename(&out, &target)
        .await
        .map_err(|e| PipelineError::new("disk_full", e.to_string(), false))?;
    let result = sqlx::query("UPDATE episodes SET audio_path=?1, audio_bytes=?2, audio_duration_s=?3, audio_version=?4, stage=?5, progress_json=?6, attempts=0, updated_at=?7 WHERE uuid=?8 AND status='running' AND stage='assembling' AND audio_version=?9")
        .bind(target.to_string_lossy().as_ref()).bind(probe.bytes as i64).bind(probe.duration_s.round() as i64).bind(version).bind(stage::PUBLISHING).bind(r#"{"percent":95}"#).bind(crate::db::now_ts()).bind(&episode.uuid).bind(episode.audio_version).execute(pool).await;
    match result {
        Ok(result) if result.rows_affected() > 0 => {
            crate::worker::stage_changed(pool, &episode.uuid, stage::PUBLISHING).await;
        }
        Ok(_) => {
            let _ = tokio::fs::remove_file(&target).await;
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(&target).await;
            return Err(PipelineError::from_app(error.into(), "internal"));
        }
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

    async fn setup(root: &std::path::Path, ffmpeg: &str) -> (Config, SqlitePool, Episode) {
        let mut cfg = Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net".into()),
            "DATA_DIR" => Some(root.display().to_string()),
            _ => None,
        })
        .unwrap();
        cfg.ffmpeg_path = crate::test_tools::tool(root, "ffmpeg", ffmpeg)
            .display()
            .to_string();
        cfg.ffprobe_path = crate::test_tools::tool(root, "ffprobe", "probe\noutput=12.5")
            .display()
            .to_string();
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
        let ep = episodes::create(
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
                source_audio_track: Some("id:alt".into()),
                source_description: None,
            },
        )
        .await
        .unwrap();
        sqlx::query("UPDATE episodes SET status='running', stage='assembling' WHERE uuid=?1")
            .bind(&ep.uuid)
            .execute(&pool)
            .await
            .unwrap();
        let ep = episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        let dir = root.join("tmp").join(&ep.uuid);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("source.media"), b"source").unwrap();
        (cfg, pool, ep)
    }

    #[tokio::test]
    async fn database_failure_can_retry_without_incrementing_version() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("fixture source");
        std::fs::write(&source, b"audio").unwrap();
        let (cfg, pool, ep) = setup(
            root.path(),
            &format!("conversion-copy\nsource={}", source.display()),
        )
        .await;
        let audio_dir = root.path().join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let old = audio_dir.join(format!("{}-1.mp3", ep.uuid));
        std::fs::write(&old, b"historical").unwrap();
        sqlx::query("UPDATE episodes SET audio_path=?1, published_at=123 WHERE uuid=?2")
            .bind(old.to_str().unwrap())
            .bind(&ep.uuid)
            .execute(&pool)
            .await
            .unwrap();
        let ep = episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        sqlx::query("CREATE TRIGGER fail_audio BEFORE UPDATE OF audio_path ON episodes BEGIN SELECT RAISE(FAIL, 'test failure'); END").execute(&pool).await.unwrap();
        assert!(assemble_episode(&pool, &cfg, &ep).await.is_err());
        let failed = episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(failed.audio_version, ep.audio_version);
        assert_eq!(failed.audio_path.as_deref(), old.to_str());
        assert_eq!(failed.published_at, Some(123));
        assert_eq!(std::fs::read(&old).unwrap(), b"historical");
        sqlx::query("DROP TRIGGER fail_audio")
            .execute(&pool)
            .await
            .unwrap();
        assemble_episode(&pool, &cfg, &failed).await.unwrap();
        let completed = episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(completed.stage, stage::PUBLISHING);
        assert_eq!(completed.audio_version, 2);
        assert_eq!(std::fs::read(&old).unwrap(), b"historical");
        assert_eq!(completed.audio_bytes, Some(5));
        assert_eq!(completed.source_audio_track.as_deref(), Some("id:alt"));
    }

    #[tokio::test]
    async fn missing_temporary_source_returns_to_fetch_with_saved_track() {
        let root = tempfile::tempdir().unwrap();
        let (cfg, pool, ep) = setup(root.path(), "fail").await;
        std::fs::remove_file(root.path().join("tmp").join(&ep.uuid).join("source.media")).unwrap();
        assemble_episode(&pool, &cfg, &ep).await.unwrap();
        let resumed = episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(resumed.stage, stage::FETCHING);
        assert_eq!(resumed.source_audio_track.as_deref(), Some("id:alt"));
    }

    #[tokio::test]
    async fn cancellation_kills_conversion_and_does_not_attach_audio() {
        let root = tempfile::tempdir().unwrap();
        let started = root.path().join("started");
        let finished = root.path().join("finished");
        let script = format!(
            "delayed-write\nstarted={}\nfinished={}",
            started.display(),
            finished.display()
        );
        let (cfg, pool, ep) = setup(root.path(), &script).await;
        let uuid = ep.uuid.clone();
        let task_pool = pool.clone();
        let task = tokio::spawn(async move { assemble_episode(&task_pool, &cfg, &ep).await });
        for _ in 0..500 {
            if started.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(started.exists());
        episodes::cancel(&pool, &uuid).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
        assert!(!finished.exists());
        let ep = episodes::get(&pool, &uuid).await.unwrap().unwrap();
        assert!(ep.audio_path.is_none());
        assert_eq!(ep.status, "cancelled");
    }
}
