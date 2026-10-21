use crate::config::Config;
use crate::db::episodes;
use crate::models::{stage, status, Episode};
use crate::services::ffmpeg;
use crate::worker::{synth::parts_dir, PipelineError};
use sqlx::SqlitePool;

pub async fn assemble_episode(
    pool: &SqlitePool,
    cfg: &Config,
    episode: &Episode,
) -> Result<(), PipelineError> {
    let dir = parts_dir(cfg, &episode.uuid);
    let count = episode
        .chunk_count
        .ok_or_else(|| PipelineError::new("assembly_failed", "Thiếu số chunk.", true))?;
    let parts: Vec<_> = (0..count)
        .map(|index| dir.join(format!("part-{index:03}.mp3")))
        .collect();
    if parts.iter().any(|path| !path.exists()) {
        return Err(PipelineError::new(
            "assembly_failed",
            "Thiếu file audio tạm.",
            true,
        ));
    }
    let out = dir.join("out.mp3");
    let probe = ffmpeg::concat_audio(cfg, &parts, &dir, &out)
        .await
        .map_err(|e| PipelineError::from_app(e, "assembly_failed"))?;
    let current = episodes::get(pool, &episode.uuid)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?
        .ok_or_else(|| PipelineError::new("internal", "episode disappeared", false))?;
    if current.status == status::CANCELLED {
        let _ = tokio::fs::remove_file(&out).await;
        return Ok(());
    }
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
    let hash = tokio::fs::read_to_string(dir.join("summary_hash.txt"))
        .await
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| episode.summary_hash.clone())
        .ok_or_else(|| PipelineError::new("internal", "summary hash missing", false))?;
    let installed = episodes::set_audio(
        pool,
        &episode.uuid,
        &target.display().to_string(),
        probe.bytes as i64,
        probe.duration_s.round() as i64,
        &hash,
        version,
    )
    .await
    .map_err(|e| PipelineError::from_app(e, "internal"))?;
    if !installed {
        let _ = tokio::fs::remove_file(&target).await;
        let _ = tokio::fs::remove_file(&out).await;
        return Ok(());
    }
    episodes::set_stage(pool, &episode.uuid, stage::PUBLISHING)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    episodes::set_progress(pool, &episode.uuid, r#"{"percent":95}"#)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::{
        self,
        shows::{self, NewShow},
    };
    use crate::models::stage;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    fn test_config(dir: &Path) -> Config {
        let mut cfg = Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner@example.com".into()),
            "DATA_DIR" => Some(dir.display().to_string()),
            _ => None,
        })
        .unwrap();
        cfg.ffmpeg_path = dir.join("ffmpeg").display().to_string();
        cfg.ffprobe_path = dir.join("ffprobe").display().to_string();
        cfg
    }

    fn fake_binary(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[tokio::test]
    async fn cancellation_after_concat_discards_the_assembly_output() {
        let root = tempfile::tempdir().unwrap();
        let cfg = test_config(root.path());
        let started = root.path().join("ffmpeg-started");
        let release = root.path().join("ffmpeg-release");
        let script = format!(
            "#!/bin/sh\nout=\nfor a in \"$@\"; do out=\"$a\"; done\ntouch '{}'\nwhile [ ! -f '{}' ]; do sleep 0.02; done\nprintf 'audio' > \"$out\"\n",
            started.display(),
            release.display()
        );
        fake_binary(root.path(), "ffmpeg", &script);
        fake_binary(root.path(), "ffprobe", "#!/bin/sh\nprintf '12.5\\n'\n");

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

        let dir = crate::worker::synth::parts_dir(&cfg, &ep.uuid);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("part-000.mp3"), b"part").unwrap();
        std::fs::write(dir.join("summary_hash.txt"), "hash-new").unwrap();
        db::episodes::set_chunk_count(&pool, &ep.uuid, 1)
            .await
            .unwrap();
        let ep = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();

        let task_pool = pool.clone();
        let task_cfg = cfg.clone();
        let task_uuid = ep.uuid.clone();
        let task = tokio::spawn(async move { assemble_episode(&task_pool, &task_cfg, &ep).await });

        for _ in 0..500 {
            if started.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(started.exists(), "ffmpeg did not start");

        db::episodes::cancel(&pool, &task_uuid).await.unwrap();
        std::fs::write(&release, b"go").unwrap();
        task.await.unwrap().unwrap();

        let got = db::episodes::get(&pool, &task_uuid).await.unwrap().unwrap();
        assert_eq!(got.status, crate::models::status::CANCELLED);
        assert_eq!(got.stage, stage::QUEUED);
        assert!(got.audio_path.is_none());
        assert!(!dir.join("out.mp3").exists());
        assert!(!cfg
            .data_dir
            .join("audio")
            .join(format!("{task_uuid}-1.mp3"))
            .exists());
    }

    #[tokio::test]
    async fn assembles_a_new_version_and_preserves_the_previous_audio() {
        let root = tempfile::tempdir().unwrap();
        let cfg = test_config(root.path());
        fake_binary(
            root.path(),
            "ffmpeg",
            "#!/bin/sh\nout=\nfor a in \"$@\"; do out=\"$a\"; done\nprintf 'audio' > \"$out\"\n",
        );
        fake_binary(root.path(), "ffprobe", "#!/bin/sh\nprintf '12.5\\n'\n");

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

        let dir = crate::worker::synth::parts_dir(&cfg, &ep.uuid);
        std::fs::create_dir_all(&dir).unwrap();
        for index in 0..2 {
            std::fs::write(dir.join(format!("part-{index:03}.mp3")), b"part").unwrap();
        }
        std::fs::write(dir.join("summary_hash.txt"), "hash-new").unwrap();

        let audio_dir = cfg.data_dir.join("audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        let old_path = audio_dir.join(format!("{}-1.mp3", ep.uuid));
        std::fs::write(&old_path, b"old").unwrap();
        db::episodes::set_audio(
            &pool,
            &ep.uuid,
            &old_path.display().to_string(),
            3,
            1,
            "hash-old",
            1,
        )
        .await
        .unwrap();
        db::episodes::set_chunk_count(&pool, &ep.uuid, 2)
            .await
            .unwrap();
        let ep = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();

        assemble_episode(&pool, &cfg, &ep).await.unwrap();

        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.stage, stage::PUBLISHING);
        assert_eq!(got.audio_version, 2);
        assert_eq!(got.audio_bytes, Some(5));
        assert_eq!(got.audio_duration_s, Some(13));
        assert_eq!(got.audio_summary_hash.as_deref(), Some("hash-new"));
        assert_eq!(
            got.audio_path.as_deref(),
            Some(
                audio_dir
                    .join(format!("{}-2.mp3", ep.uuid))
                    .to_str()
                    .unwrap()
            )
        );
        assert_eq!(std::fs::read(&old_path).unwrap(), b"old");
    }
}
