use crate::config::Config;
use crate::db::{episodes, shows};
use crate::domain::summarize::{self, SummaryOptions};
use crate::models::{stage, target_chars, Episode};
use crate::worker::PipelineError;
use sqlx::SqlitePool;

pub async fn summarize_episode(
    pool: &SqlitePool,
    _cfg: &Config,
    episode: &Episode,
) -> Result<(), PipelineError> {
    let episode = episodes::get(pool, &episode.uuid)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?
        .ok_or_else(|| PipelineError::new("internal", "episode disappeared", false))?;
    let show = shows::get(pool, episode.show_id)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?
        .ok_or_else(|| PipelineError::new("internal", "show not found", false))?;
    let path = episode.transcript_path.as_deref().ok_or_else(|| {
        PipelineError::new("transcript_too_short", "Transcript chưa sẵn sàng.", false)
    })?;
    let text = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| PipelineError::from_app(e.into(), "transcript_too_short"))?;
    let sentences: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    if sentences.len() < 3 {
        return Err(PipelineError::new(
            "transcript_too_short",
            "Transcript quá ngắn để tóm tắt.",
            false,
        ));
    }
    let target = target_chars(episode.target_minutes, &episode.language);
    let result = summarize::summarize(
        &sentences,
        &SummaryOptions {
            target_chars: target,
            title: episode.source_title.clone(),
        },
    );
    if result.selected_indices.is_empty() {
        return Err(PipelineError::new(
            "transcript_too_short",
            "Transcript quá ngắn để tóm tắt.",
            false,
        ));
    }
    let body = result
        .selected_indices
        .iter()
        .map(|index| sentences[*index].clone())
        .collect::<Vec<_>>()
        .join("\n\n");
    let script = summarize::build_script(
        &show.title,
        &episode.source_title,
        &sentences,
        &body,
        &episode.language,
    );
    let script = summarize::sanitize_for_tts(&script);
    let display_summary = summarize::cap_words(&body, summarize::DISPLAY_SUMMARY_MAX_WORDS);
    let hash = crate::models::sha256_hex(&script);
    episodes::set_summary(pool, &episode.uuid, &display_summary, &script, &hash)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    if episode.review_required {
        episodes::set_review(pool, &episode.uuid)
            .await
            .map_err(|e| PipelineError::from_app(e, "internal"))?;
    } else {
        episodes::set_stage(pool, &episode.uuid, stage::VOICING)
            .await
            .map_err(|e| PipelineError::from_app(e, "internal"))?;
        episodes::set_progress(pool, &episode.uuid, r#"{"percent":45}"#)
            .await
            .map_err(|e| PipelineError::from_app(e, "internal"))?;
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
    use crate::models::stage;

    async fn summarize_fixture(
        review_required: bool,
    ) -> (SqlitePool, Config, Episode, tempfile::TempDir) {
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
        let episode = db::episodes::create(
            &pool,
            db::episodes::NewEpisode {
                show_id: show.id,
                video_id: format!("v-{review_required}"),
                source_url: "https://www.youtube.com/watch?v=v1".into(),
                source_title: "Rust".into(),
                source_channel: String::new(),
                source_duration_s: Some(600),
                source_thumbnail_url: None,
                source_upload_date: None,
                language: "vi".into(),
                target_minutes: 10,
                review_required,
            },
        )
        .await
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tx.txt");
        std::fs::write(
            &path,
            "Rust là ngôn ngữ an toàn.\nBài nói giải thích ownership.\nBorrowing giúp tránh lỗi bộ nhớ.",
        )
        .unwrap();
        sqlx::query("UPDATE episodes SET transcript_path = ?1 WHERE uuid = ?2")
            .bind(path.display().to_string())
            .bind(&episode.uuid)
            .execute(&pool)
            .await
            .unwrap();
        let cfg = Config::from_lookup({
            let dir = dir.path().to_path_buf();
            move |key| match key {
                "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
                "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
                "DATA_DIR" => Some(dir.display().to_string()),
                _ => None,
            }
        })
        .unwrap();
        (pool, cfg, episode, dir)
    }

    #[tokio::test]
    async fn review_required_episode_stops_at_review() {
        let (pool, cfg, episode, _dir) = summarize_fixture(true).await;
        summarize_episode(&pool, &cfg, &episode).await.unwrap();
        let got = db::episodes::get(&pool, &episode.uuid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.status, crate::models::status::REVIEW);
        assert_eq!(got.stage, crate::models::stage::REVIEW);
        assert!(
            got.summary_text
                .as_deref()
                .unwrap()
                .split_whitespace()
                .count()
                <= 200
        );
        assert!(!got.audio_script.as_deref().unwrap().is_empty());
    }

    #[tokio::test]
    async fn builds_a_script_and_advances_to_voicing() {
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
                video_id: "v1".into(),
                source_url: "https://www.youtube.com/watch?v=v1".into(),
                source_title: "Rust".into(),
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
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tx.txt");
        std::fs::write(&path, "Rust là ngôn ngữ an toàn.\nBài nói giải thích ownership.\nBorrowing giúp tránh lỗi bộ nhớ.").unwrap();
        sqlx::query("UPDATE episodes SET transcript_path = ?1 WHERE uuid = ?2")
            .bind(path.display().to_string())
            .bind(&ep.uuid)
            .execute(&pool)
            .await
            .unwrap();

        let cfg = Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "DATA_DIR" => Some(dir.path().display().to_string()),
            _ => None,
        })
        .unwrap();
        summarize_episode(&pool, &cfg, &ep).await.unwrap();
        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.stage, stage::VOICING);
        assert!(!got.summary_text.unwrap().is_empty());
        assert!(got.audio_script.unwrap().contains("Chào mừng bạn"));
    }
}
