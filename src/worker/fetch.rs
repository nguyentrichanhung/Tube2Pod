use crate::config::Config;
use crate::db::episodes::{self, FetchedMeta};
use crate::domain::transcript;
use crate::error::AppError;
use crate::models::{stage, Episode};
use crate::services::ytdlp;
use crate::worker::PipelineError;
use sqlx::SqlitePool;

const MAX_TRANSCRIPT_CHARS: usize = 500_000;

pub async fn fetch_episode(
    pool: &SqlitePool,
    cfg: &Config,
    episode: &Episode,
) -> Result<(), PipelineError> {
    episodes::set_stage(pool, &episode.uuid, stage::FETCHING)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    episodes::set_progress(pool, &episode.uuid, r#"{"percent":10}"#)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;

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
    let max_source_minutes = crate::db::settings::read_i64(pool, "max_source_minutes", 360)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    let min_source_seconds = crate::db::settings::read_i64(pool, "min_source_seconds", 60)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    let duration = meta.duration.unwrap_or(0);
    if duration > max_source_minutes.saturating_mul(60) {
        return Err(PipelineError::new(
            "source_too_long",
            "Video dài hơn giới hạn.",
            false,
        ));
    }
    if duration < min_source_seconds {
        return Err(PipelineError::new(
            "source_too_short",
            "Video quá ngắn để làm podcast.",
            false,
        ));
    }
    let language = ytdlp::source_language(&meta)
        .map_err(|e| PipelineError::new(ytdlp::classify_source_error(&e), e.to_string(), false))?;
    let caption = ytdlp::choose_caption(&meta, language).ok_or_else(|| {
        PipelineError::new(
            "no_captions",
            "Video không có phụ đề; không tự tạo transcript.",
            false,
        )
    })?;

    let tmp = cfg.data_dir.join("tmp").join(&episode.uuid);
    let download = ytdlp::download_caption(cfg, &video, &caption, &tmp, &episode.uuid);
    let raw_path = match crate::worker::cancellable(pool, &episode.uuid, download).await {
        Some(result) => result.map_err(|e| PipelineError::from_app(e, "source_unavailable"))?,
        None => {
            let _ = tokio::fs::remove_dir_all(&tmp).await;
            return Ok(());
        }
    };
    let raw = tokio::fs::read_to_string(&raw_path)
        .await
        .map_err(|e| PipelineError::from_app(e.into(), "source_unavailable"))?;
    if raw.len() > MAX_TRANSCRIPT_CHARS {
        return Err(PipelineError::new(
            "source_too_long",
            "Transcript quá dài.",
            false,
        ));
    }
    let cues = transcript::parse_by_kind(&raw, &caption.ext).map_err(|e| match e {
        AppError::Internal(_) => PipelineError::new(
            "transcript_invalid",
            "Transcript không hợp lệ; không tự tạo transcript.",
            false,
        ),
        other => PipelineError::from_app(other, "transcript_too_short"),
    })?;
    let sentences = transcript::segment(&cues, language);
    if sentences.len() < 3 {
        return Err(PipelineError::new(
            "transcript_too_short",
            "Transcript quá ngắn để tóm tắt.",
            false,
        ));
    }
    let plain = sentences.join("\n");
    if crate::worker::is_cancelled(pool, &episode.uuid).await {
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        return Ok(());
    }
    let transcript_dir = cfg.data_dir.join("transcripts");
    tokio::fs::create_dir_all(&transcript_dir)
        .await
        .map_err(|e| PipelineError::from_app(e.into(), "internal"))?;
    let canonical_raw = transcript_dir.join(format!("{}.{}", episode.uuid, caption.ext));
    let canonical_text = transcript_dir.join(format!("{}.txt", episode.uuid));
    tokio::fs::copy(&raw_path, &canonical_raw)
        .await
        .map_err(|e| PipelineError::from_app(e.into(), "internal"))?;
    tokio::fs::write(&canonical_text, &plain)
        .await
        .map_err(|e| PipelineError::from_app(e.into(), "internal"))?;
    if crate::worker::is_cancelled(pool, &episode.uuid).await {
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        return Ok(());
    }

    episodes::set_fetched(
        pool,
        &episode.uuid,
        FetchedMeta {
            source_title: &meta.title,
            source_channel: meta.channel.as_deref().unwrap_or(""),
            source_duration_s: duration,
            source_thumbnail_url: meta.thumbnail.as_deref(),
            source_upload_date: meta.upload_date.as_deref(),
            language,
            transcript_path: canonical_text
                .to_str()
                .ok_or_else(|| PipelineError::new("internal", "invalid transcript path", false))?,
            transcript_kind: &caption.ext,
            transcript_chars: plain.chars().count() as i64,
            next_stage: stage::SUMMARIZING,
        },
    )
    .await
    .map_err(|e| PipelineError::from_app(e, "internal"))?;
    episodes::set_progress(pool, &episode.uuid, r#"{"percent":35}"#)
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
    use crate::models::{stage, status};
    use std::os::unix::fs::PermissionsExt;

    fn config_with_script(data_dir: &std::path::Path, script_body: &str) -> Config {
        let script = data_dir.join("fake-ytdlp.sh");
        std::fs::write(&script, script_body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "DATA_DIR" => Some(data_dir.display().to_string()),
            "YTDLP_PATH" => Some(script.display().to_string()),
            _ => None,
        })
        .unwrap()
    }

    fn duration_config(data_dir: &std::path::Path, duration: i64) -> Config {
        config_with_script(
            data_dir,
            &format!(
                "#!/bin/sh\nif [ \"$1\" = \"--dump-single-json\" ]; then printf '%s' '{{\"id\":\"dQw4w9WgXcQ\",\"title\":\"Test\",\"duration\":{duration},\"language\":\"vi\",\"subtitles\":{{\"vi\":[{{\"ext\":\"json3\",\"url\":\"https://example.test/vi.json3\"}}]}}}}'; exit 0; fi\nexit 1\n"
            ),
        )
    }

    fn fixture_config(data_dir: &std::path::Path) -> Config {
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ytdlp_vi.json");
        let caption = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/captions_vi.json3");
        config_with_script(
            data_dir,
            &format!(
                "#!/bin/sh\nif [ \"$1\" = \"--dump-single-json\" ]; then cat '{}'; exit 0; fi\nout=''\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = \"-o\" ]; then out=\"$2\"; shift; fi\n  shift\ndone\ncp '{}' \"${{out%.*}}.json3\"\n",
                fixture.display(),
                caption.display()
            ),
        )
    }

    async fn seed_episode(pool: &sqlx::SqlitePool) -> Episode {
        let show = shows::create(
            pool,
            NewShow {
                title: "Show".into(),
                ..NewShow::default()
            },
        )
        .await
        .unwrap();
        crate::db::episodes::create(
            pool,
            crate::db::episodes::NewEpisode {
                show_id: show.id,
                video_id: "dQw4w9WgXcQ".into(),
                source_url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(),
                source_title: "placeholder".into(),
                source_channel: "".into(),
                source_duration_s: None,
                source_thumbnail_url: None,
                source_upload_date: None,
                language: "vi".into(),
                target_minutes: 10,
                review_required: false,
            },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn fetches_metadata_and_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = fixture_config(dir.path());
        let pool = db::test_pool().await;
        let ep = seed_episode(&pool).await;

        fetch_episode(&pool, &cfg, &ep).await.unwrap();
        let got = crate::db::episodes::get(&pool, &ep.uuid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.source_title, "Một video thử nghiệm");
        assert_eq!(got.transcript_kind.as_deref(), Some("json3"));
        assert_eq!(got.stage, stage::SUMMARIZING);
        let text = std::fs::read_to_string(got.transcript_path.unwrap()).unwrap();
        assert!(text.contains("Xin chào các bạn."));
    }

    #[tokio::test]
    async fn source_limits_are_read_from_settings() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = duration_config(dir.path(), 120);
        let pool = db::test_pool().await;
        let ep = seed_episode(&pool).await;

        db::settings::set(&pool, "max_source_minutes", "1")
            .await
            .unwrap();
        let error = fetch_episode(&pool, &cfg, &ep).await.unwrap_err();
        assert_eq!(error.code, "source_too_long");
        assert!(!error.retryable);

        db::settings::set(&pool, "max_source_minutes", "10")
            .await
            .unwrap();
        db::settings::set(&pool, "min_source_seconds", "180")
            .await
            .unwrap();
        let error = fetch_episode(&pool, &cfg, &ep).await.unwrap_err();
        assert_eq!(error.code, "source_too_short");
        assert!(!error.retryable);
    }

    #[tokio::test]
    async fn live_video_probe_error_is_not_retryable() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_with_script(
            dir.path(),
            "#!/bin/sh\nif [ \"$1\" = \"--dump-single-json\" ]; then printf '%s' '{\"id\":\"dQw4w9WgXcQ\",\"title\":\"Live\",\"is_live\":true}'; exit 0; fi\nexit 1\n",
        );
        let pool = db::test_pool().await;
        let ep = seed_episode(&pool).await;

        let error = fetch_episode(&pool, &cfg, &ep).await.unwrap_err();
        assert_eq!(error.code, "live_not_supported");
        assert!(
            !error.retryable,
            "live videos must not be retried: {error:?}"
        );
    }

    #[tokio::test]
    async fn upstream_probe_error_is_retryable() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config_with_script(dir.path(), "#!/bin/sh\necho 'network down' >&2\nexit 1\n");
        let pool = db::test_pool().await;
        let ep = seed_episode(&pool).await;

        let error = fetch_episode(&pool, &cfg, &ep).await.unwrap_err();
        assert_eq!(error.code, "source_unavailable");
        assert!(error.retryable, "upstream failures should retry: {error:?}");
    }

    #[tokio::test]
    async fn cancellation_kills_the_caption_download_and_cleans_tmp() {
        let dir = tempfile::tempdir().unwrap();
        let started = dir.path().join("download-started");
        let finished = dir.path().join("download-finished");
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ytdlp_vi.json");
        let caption = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/captions_vi.json3");
        let cfg = config_with_script(
            dir.path(),
            &format!(
                "#!/bin/sh\nif [ \"$1\" = \"--dump-single-json\" ]; then cat '{}'; exit 0; fi\ntouch '{}'\nsleep 1\ntouch '{}'\nout=''\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = \"-o\" ]; then out=\"$2\"; shift; fi\n  shift\ndone\ncp '{}' \"${{out%.*}}.json3\"\n",
                fixture.display(),
                started.display(),
                finished.display(),
                caption.display()
            ),
        );
        let pool = db::test_pool().await;
        let ep = seed_episode(&pool).await;
        let uuid = ep.uuid.clone();

        let task_pool = pool.clone();
        let task_cfg = cfg.clone();
        let task = tokio::spawn(async move { fetch_episode(&task_pool, &task_cfg, &ep).await });

        for _ in 0..500 {
            if started.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(started.exists(), "caption download did not start");

        crate::db::episodes::cancel(&pool, &uuid).await.unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("fetch did not stop after cancellation")
            .unwrap();
        result.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(1300)).await;
        assert!(!finished.exists(), "yt-dlp kept running after cancellation");
        let got = crate::db::episodes::get(&pool, &uuid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.status, status::CANCELLED);
        assert_ne!(got.stage, stage::SUMMARIZING);
        assert!(got.transcript_path.is_none());
        assert!(!cfg.data_dir.join("tmp").join(&uuid).exists());
        assert!(!cfg
            .data_dir
            .join("transcripts")
            .join(format!("{uuid}.txt"))
            .exists());
    }

    #[tokio::test]
    async fn malformed_transcript_is_not_retryable() {
        let dir = tempfile::tempdir().unwrap();
        let fixture =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ytdlp_vi.json");
        let cfg = config_with_script(
            dir.path(),
            &format!(
                "#!/bin/sh\nif [ \"$1\" = \"--dump-single-json\" ]; then cat '{}'; exit 0; fi\nout=''\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = \"-o\" ]; then out=\"$2\"; shift; fi\n  shift\ndone\nprintf 'not json' > \"${{out%.*}}.json3\"\n",
                fixture.display()
            ),
        );
        let pool = db::test_pool().await;
        let ep = seed_episode(&pool).await;

        let error = fetch_episode(&pool, &cfg, &ep).await.unwrap_err();
        assert_eq!(error.code, "transcript_invalid");
        assert!(
            !error.retryable,
            "malformed transcripts must not be retried: {error:?}"
        );
    }
}
