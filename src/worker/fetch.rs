use crate::config::Config;
use crate::db::episodes::{self, FetchedMeta};
use crate::domain::transcript;
use crate::models::{stage, Episode};
use crate::services::ytdlp;
use crate::worker::PipelineError;
use sqlx::SqlitePool;

const MAX_SOURCE_MINUTES: i64 = 360;
const MIN_SOURCE_SECONDS: i64 = 60;
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

    let meta = ytdlp::probe(cfg, &video)
        .await
        .map_err(|e| PipelineError::new(ytdlp::classify_source_error(&e), e.to_string(), true))?;
    let duration = meta.duration.unwrap_or(0);
    if duration > MAX_SOURCE_MINUTES * 60 {
        return Err(PipelineError::new(
            "source_too_long",
            "Video dài hơn giới hạn.",
            false,
        ));
    }
    if duration < MIN_SOURCE_SECONDS {
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
    let raw_path = ytdlp::download_caption(cfg, &video, &caption, &tmp, &episode.uuid)
        .await
        .map_err(|e| PipelineError::from_app(e, "source_unavailable"))?;
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
    let cues = transcript::parse_by_kind(&raw, &caption.ext)
        .map_err(|e| PipelineError::from_app(e, "transcript_too_short"))?;
    let sentences = transcript::segment(&cues, language);
    if sentences.len() < 3 {
        return Err(PipelineError::new(
            "transcript_too_short",
            "Transcript quá ngắn để tóm tắt.",
            false,
        ));
    }
    let plain = sentences.join("\n");
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
    use crate::models::stage;
    use std::os::unix::fs::PermissionsExt;

    async fn fixture_config(data_dir: &std::path::Path) -> Config {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/ytdlp_vi.json");
        let caption = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/captions_vi.json3");
        let script = data_dir.join("fake-ytdlp.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nif [ \"$1\" = \"--dump-single-json\" ]; then cat '{}'; exit 0; fi\nout=''\nwhile [ $# -gt 0 ]; do\n  if [ \"$1\" = \"-o\" ]; then out=\"$2\"; shift; fi\n  shift\ndone\ncp '{}' \"${{out%.*}}.json3\"\n",
                fixture.display(),
                caption.display()
            ),
        )
        .unwrap();
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

    #[tokio::test]
    async fn fetches_metadata_and_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = fixture_config(dir.path()).await;
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
        let ep = crate::db::episodes::create(
            &pool,
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
            },
        )
        .await
        .unwrap();

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
}
