use crate::db::{now_ts, AppResult};
use crate::models::{stage, status, Episode};
use sqlx::SqlitePool;

#[derive(Debug, Clone)]
pub struct NewEpisode {
    pub show_id: i64,
    pub video_id: String,
    pub source_url: String,
    pub source_title: String,
    pub source_channel: String,
    pub source_duration_s: Option<i64>,
    pub source_thumbnail_url: Option<String>,
    pub source_upload_date: Option<String>,
    pub language: String,
    pub target_minutes: i64,
}

#[derive(Debug, Clone, Default)]
pub struct EpisodeFilter {
    pub show_id: Option<i64>,
    pub status: Option<String>,
    pub query: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

pub async fn create(pool: &SqlitePool, new: NewEpisode) -> AppResult<Episode> {
    let now = now_ts();
    let uuid = uuid::Uuid::new_v4().to_string();
    let row = sqlx::query_as::<_, Episode>(
        "INSERT INTO episodes (
            uuid, show_id, video_id, source_url, source_title, source_channel,
            source_duration_s, source_thumbnail_url, source_upload_date, language,
            target_minutes, stage, status, created_at, updated_at
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?14)
         RETURNING *",
    )
    .bind(uuid)
    .bind(new.show_id)
    .bind(new.video_id)
    .bind(new.source_url)
    .bind(new.source_title)
    .bind(new.source_channel)
    .bind(new.source_duration_s)
    .bind(new.source_thumbnail_url)
    .bind(new.source_upload_date)
    .bind(new.language)
    .bind(new.target_minutes)
    .bind(stage::QUEUED)
    .bind(status::QUEUED)
    .bind(now)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn get(pool: &SqlitePool, uuid: &str) -> AppResult<Option<Episode>> {
    Ok(
        sqlx::query_as::<_, Episode>("SELECT * FROM episodes WHERE uuid = ?1")
            .bind(uuid)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn get_by_video_id(pool: &SqlitePool, video_id: &str) -> AppResult<Option<Episode>> {
    Ok(
        sqlx::query_as::<_, Episode>("SELECT * FROM episodes WHERE video_id = ?1")
            .bind(video_id)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn list(pool: &SqlitePool, filter: &EpisodeFilter) -> AppResult<Vec<Episode>> {
    let limit = filter.limit.unwrap_or(100).clamp(1, 500);
    let offset = filter.offset.unwrap_or(0).max(0);
    let rows = sqlx::query_as::<_, Episode>(
        "SELECT * FROM episodes
         WHERE (?1 IS NULL OR show_id = ?1)
           AND (?2 IS NULL OR status = ?2)
           AND (?3 IS NULL OR source_title LIKE '%' || ?3 || '%' OR source_channel LIKE '%' || ?3 || '%')
         ORDER BY created_at DESC, uuid DESC
         LIMIT ?4 OFFSET ?5",
    )
    .bind(filter.show_id)
    .bind(filter.status.as_deref())
    .bind(filter.query.as_deref())
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn list_for_show(pool: &SqlitePool, show_id: i64, limit: i64) -> AppResult<Vec<Episode>> {
    list(
        pool,
        &EpisodeFilter {
            show_id: Some(show_id),
            limit: Some(limit),
            ..EpisodeFilter::default()
        },
    )
    .await
}

pub async fn claim_next(pool: &SqlitePool) -> AppResult<Option<Episode>> {
    let now = now_ts();
    let row = sqlx::query_as::<_, Episode>(
        "UPDATE episodes SET status = ?1, updated_at = ?2
         WHERE uuid = (
            SELECT uuid FROM episodes
            WHERE status = ?3 AND (next_attempt_at IS NULL OR next_attempt_at <= ?2)
            ORDER BY created_at ASC, uuid ASC
            LIMIT 1
         )
         RETURNING *",
    )
    .bind(status::RUNNING)
    .bind(now)
    .bind(status::QUEUED)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn recover_running(pool: &SqlitePool) -> AppResult<u64> {
    let result = sqlx::query(
        "UPDATE episodes SET status = ?1, next_attempt_at = NULL, updated_at = ?2
         WHERE status = ?3",
    )
    .bind(status::QUEUED)
    .bind(now_ts())
    .bind(status::RUNNING)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn earliest_retry_at(pool: &SqlitePool) -> AppResult<Option<i64>> {
    let value: Option<i64> = sqlx::query_scalar(
        "SELECT MIN(next_attempt_at) FROM episodes
         WHERE status = ?1 AND next_attempt_at IS NOT NULL",
    )
    .bind(status::QUEUED)
    .fetch_one(pool)
    .await?;
    Ok(value)
}

pub async fn set_stage(pool: &SqlitePool, uuid: &str, new_stage: &str) -> AppResult<()> {
    sqlx::query(
        "UPDATE episodes SET stage = ?1,
            attempts = CASE WHEN stage != ?1 THEN 0 ELSE attempts END,
            updated_at = ?2
         WHERE uuid = ?3",
    )
    .bind(new_stage)
    .bind(now_ts())
    .bind(uuid)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_progress(pool: &SqlitePool, uuid: &str, json: &str) -> AppResult<()> {
    sqlx::query("UPDATE episodes SET progress_json = ?1, updated_at = ?2 WHERE uuid = ?3")
        .bind(json)
        .bind(now_ts())
        .bind(uuid)
        .execute(pool)
        .await?;
    Ok(())
}

pub struct FetchedMeta<'a> {
    pub source_title: &'a str,
    pub source_channel: &'a str,
    pub source_duration_s: i64,
    pub source_thumbnail_url: Option<&'a str>,
    pub source_upload_date: Option<&'a str>,
    pub language: &'a str,
    pub transcript_path: &'a str,
    pub transcript_kind: &'a str,
    pub transcript_chars: i64,
    pub next_stage: &'a str,
}

pub async fn set_fetched(pool: &SqlitePool, uuid: &str, meta: FetchedMeta<'_>) -> AppResult<()> {
    sqlx::query(
        "UPDATE episodes SET
            source_title = ?1, source_channel = ?2, source_duration_s = ?3,
            source_thumbnail_url = ?4, source_upload_date = ?5, language = ?6,
            transcript_path = ?7, transcript_kind = ?8, transcript_chars = ?9,
            stage = ?10, updated_at = ?11
         WHERE uuid = ?12",
    )
    .bind(meta.source_title)
    .bind(meta.source_channel)
    .bind(meta.source_duration_s)
    .bind(meta.source_thumbnail_url)
    .bind(meta.source_upload_date)
    .bind(meta.language)
    .bind(meta.transcript_path)
    .bind(meta.transcript_kind)
    .bind(meta.transcript_chars)
    .bind(meta.next_stage)
    .bind(now_ts())
    .bind(uuid)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_summary(pool: &SqlitePool, uuid: &str, text: &str, hash: &str) -> AppResult<()> {
    sqlx::query(
        "UPDATE episodes SET summary_text = ?1, summary_hash = ?2, summary_edited_at = NULL,
            updated_at = ?3
         WHERE uuid = ?4",
    )
    .bind(text)
    .bind(hash)
    .bind(now_ts())
    .bind(uuid)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_summary_edit(
    pool: &SqlitePool,
    uuid: &str,
    text: &str,
    hash: &str,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE episodes SET summary_text = ?1, summary_hash = ?2,
            summary_edited_at = ?3, updated_at = ?3
         WHERE uuid = ?4",
    )
    .bind(text)
    .bind(hash)
    .bind(now_ts())
    .bind(uuid)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn queue_at_stage(pool: &SqlitePool, uuid: &str, new_stage: &str) -> AppResult<()> {
    sqlx::query(
        "UPDATE episodes SET stage = ?1, status = ?2, attempts = 0,
            next_attempt_at = NULL, error_code = NULL, error_message = NULL, updated_at = ?3
         WHERE uuid = ?4",
    )
    .bind(new_stage)
    .bind(status::QUEUED)
    .bind(now_ts())
    .bind(uuid)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_voice(pool: &SqlitePool, uuid: &str, voice_id: i64) -> AppResult<()> {
    sqlx::query(
        "UPDATE episodes SET voice_id = ?1, voice_design_id = NULL, updated_at = ?2 WHERE uuid = ?3",
    )
    .bind(voice_id)
    .bind(now_ts())
    .bind(uuid)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_voice_design(
    pool: &SqlitePool,
    uuid: &str,
    design_id: Option<i64>,
) -> AppResult<()> {
    sqlx::query("UPDATE episodes SET voice_design_id = ?1, updated_at = ?2 WHERE uuid = ?3")
        .bind(design_id)
        .bind(now_ts())
        .bind(uuid)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_active_take(pool: &SqlitePool, uuid: &str, take_id: Option<i64>) -> AppResult<()> {
    sqlx::query("UPDATE episodes SET active_take_id = ?1, updated_at = ?2 WHERE uuid = ?3")
        .bind(take_id)
        .bind(now_ts())
        .bind(uuid)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_chunk_progress(
    pool: &SqlitePool,
    uuid: &str,
    take_ids_json: &str,
    chunk_done: i64,
    chunk_count: i64,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE episodes SET take_ids_json = ?1, chunk_done = ?2, chunk_count = ?3,
            active_take_id = NULL, updated_at = ?4
         WHERE uuid = ?5",
    )
    .bind(take_ids_json)
    .bind(chunk_done)
    .bind(chunk_count)
    .bind(now_ts())
    .bind(uuid)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_chunk_count(pool: &SqlitePool, uuid: &str, chunk_count: i64) -> AppResult<()> {
    sqlx::query("UPDATE episodes SET chunk_count = ?1, updated_at = ?2 WHERE uuid = ?3")
        .bind(chunk_count)
        .bind(now_ts())
        .bind(uuid)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_audio(
    pool: &SqlitePool,
    uuid: &str,
    path: &str,
    bytes: i64,
    duration_s: i64,
    summary_hash: &str,
    version: i64,
) -> AppResult<bool> {
    let result = sqlx::query(
        "UPDATE episodes SET audio_path = ?1, audio_bytes = ?2, audio_duration_s = ?3,
            audio_summary_hash = ?4, audio_version = ?5, active_take_id = NULL,
            take_ids_json = NULL, chunk_done = 0, chunk_count = NULL, updated_at = ?6
         WHERE uuid = ?7 AND status != ?8",
    )
    .bind(path)
    .bind(bytes)
    .bind(duration_s)
    .bind(summary_hash)
    .bind(version)
    .bind(now_ts())
    .bind(uuid)
    .bind(status::CANCELLED)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn finish(
    pool: &SqlitePool,
    uuid: &str,
    final_stage: &str,
    final_status: &str,
    set_published_at_if_null: bool,
) -> AppResult<()> {
    let now = now_ts();
    sqlx::query(
        "UPDATE episodes SET stage = ?1, status = ?2, error_code = NULL, error_message = NULL,
            next_attempt_at = NULL, updated_at = ?3,
            published_at = CASE WHEN ?4 THEN COALESCE(published_at, ?3) ELSE published_at END
         WHERE uuid = ?5 AND status != ?6",
    )
    .bind(final_stage)
    .bind(final_status)
    .bind(now)
    .bind(set_published_at_if_null)
    .bind(uuid)
    .bind(status::CANCELLED)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn record_failure(
    pool: &SqlitePool,
    uuid: &str,
    code: &str,
    message: &str,
    retry_at: Option<i64>,
) -> AppResult<()> {
    let retry = retry_at.is_some();
    sqlx::query(
        "UPDATE episodes SET status = ?1, error_code = ?2, error_message = ?3,
            attempts = attempts + 1, next_attempt_at = ?4, updated_at = ?5
         WHERE uuid = ?6 AND status != ?7",
    )
    .bind(if retry {
        status::QUEUED
    } else {
        status::FAILED
    })
    .bind(code)
    .bind(message)
    .bind(retry_at)
    .bind(now_ts())
    .bind(uuid)
    .bind(status::CANCELLED)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn retry(pool: &SqlitePool, uuid: &str) -> AppResult<()> {
    sqlx::query(
        "UPDATE episodes SET status = ?1, attempts = 0, next_attempt_at = NULL,
            error_code = NULL, error_message = NULL, updated_at = ?2
         WHERE uuid = ?3 AND status IN (?4, ?5, ?6)",
    )
    .bind(status::QUEUED)
    .bind(now_ts())
    .bind(uuid)
    .bind(status::QUEUED)
    .bind(status::FAILED)
    .bind(status::CANCELLED)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn cancel(pool: &SqlitePool, uuid: &str) -> AppResult<()> {
    sqlx::query(
        "UPDATE episodes SET status = ?1, next_attempt_at = NULL, updated_at = ?2
         WHERE uuid = ?3 AND status NOT IN (?4, ?5)",
    )
    .bind(status::CANCELLED)
    .bind(now_ts())
    .bind(uuid)
    .bind(status::PUBLISHED)
    .bind(status::READY)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn reset_for_reread(pool: &SqlitePool, uuid: &str) -> AppResult<()> {
    sqlx::query(
        "UPDATE episodes SET stage = ?1, status = ?2, active_take_id = NULL,
            take_ids_json = NULL, chunk_done = 0, chunk_count = NULL,
            attempts = 0, next_attempt_at = NULL, error_code = NULL, error_message = NULL,
            updated_at = ?3
         WHERE uuid = ?4",
    )
    .bind(stage::SYNTHESIZING)
    .bind(status::QUEUED)
    .bind(now_ts())
    .bind(uuid)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn publish_ready(pool: &SqlitePool, show_id: i64) -> AppResult<u64> {
    let result = sqlx::query(
        "UPDATE episodes SET status = ?1, stage = ?2,
            published_at = COALESCE(published_at, ?3), updated_at = ?3
         WHERE show_id = ?4 AND status = ?5",
    )
    .bind(status::PUBLISHED)
    .bind(stage::PUBLISHED)
    .bind(now_ts())
    .bind(show_id)
    .bind(status::READY)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn list_published(pool: &SqlitePool, show_id: i64) -> AppResult<Vec<Episode>> {
    Ok(sqlx::query_as::<_, Episode>(
        "SELECT * FROM episodes WHERE show_id = ?1 AND status = ?2
         ORDER BY published_at DESC, uuid DESC",
    )
    .bind(show_id)
    .bind(status::PUBLISHED)
    .fetch_all(pool)
    .await?)
}

pub async fn delete(pool: &SqlitePool, uuid: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM episodes WHERE uuid = ?1")
        .bind(uuid)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::shows::{self, NewShow};
    use crate::models::status;

    async fn setup() -> (sqlx::SqlitePool, i64) {
        let pool = crate::db::test_pool().await;
        let show = shows::create(
            &pool,
            NewShow {
                title: "Show".into(),
                ..NewShow::default()
            },
        )
        .await
        .unwrap();
        (pool, show.id)
    }

    fn episode(show_id: i64, video_id: &str) -> NewEpisode {
        NewEpisode {
            show_id,
            video_id: video_id.into(),
            source_url: format!("https://www.youtube.com/watch?v={video_id}"),
            source_title: "Video".into(),
            source_channel: "Channel".into(),
            source_duration_s: Some(600),
            source_thumbnail_url: None,
            source_upload_date: None,
            language: "vi".into(),
            target_minutes: 10,
        }
    }

    #[tokio::test]
    async fn creates_episodes_and_rejects_duplicate_video_ids() {
        let (pool, show_id) = setup().await;
        let ep = create(&pool, episode(show_id, "abc123")).await.unwrap();
        assert_eq!(ep.status, status::QUEUED);
        assert_eq!(ep.stage, stage::QUEUED);
        assert!(get_by_video_id(&pool, "abc123").await.unwrap().is_some());
        assert!(create(&pool, episode(show_id, "abc123")).await.is_err());
    }

    #[tokio::test]
    async fn claims_only_due_queued_episodes() {
        let (pool, show_id) = setup().await;
        let ep = create(&pool, episode(show_id, "abc123")).await.unwrap();
        let claimed = claim_next(&pool).await.unwrap().unwrap();
        assert_eq!(claimed.uuid, ep.uuid);
        assert_eq!(claimed.status, status::RUNNING);
        assert!(claim_next(&pool).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn recovers_running_episodes_to_queued() {
        let (pool, show_id) = setup().await;
        create(&pool, episode(show_id, "abc123")).await.unwrap();
        let claimed = claim_next(&pool).await.unwrap().unwrap();
        assert_eq!(recover_running(&pool).await.unwrap(), 1);
        let recovered = get(&pool, &claimed.uuid).await.unwrap().unwrap();
        assert_eq!(recovered.status, status::QUEUED);
        assert_eq!(recover_running(&pool).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn records_stage_progress_and_failure_retry() {
        let (pool, show_id) = setup().await;
        let ep = create(&pool, episode(show_id, "abc123")).await.unwrap();

        set_stage(&pool, &ep.uuid, stage::FETCHING).await.unwrap();
        set_progress(&pool, &ep.uuid, r#"{"percent":40}"#)
            .await
            .unwrap();
        let got = get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.stage, stage::FETCHING);
        assert_eq!(got.progress_json.as_deref(), Some(r#"{"percent":40}"#));

        record_failure(
            &pool,
            &ep.uuid,
            "source_unavailable",
            "temporary",
            Some(now_ts() + 30),
        )
        .await
        .unwrap();
        let got = get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.status, status::QUEUED);
        assert_eq!(got.attempts, 1);

        retry(&pool, &ep.uuid).await.unwrap();
        let got = get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.status, status::QUEUED);
        assert_eq!(got.attempts, 0);
        assert!(got.error_code.is_none());
    }

    #[tokio::test]
    async fn record_failure_never_requeues_a_cancelled_episode() {
        let (pool, show_id) = setup().await;
        let ep = create(&pool, episode(show_id, "abc123")).await.unwrap();
        cancel(&pool, &ep.uuid).await.unwrap();

        record_failure(
            &pool,
            &ep.uuid,
            "tts_unavailable",
            "boom",
            Some(now_ts() + 30),
        )
        .await
        .unwrap();

        let got = get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.status, status::CANCELLED);
        assert_eq!(got.attempts, 0);
        assert!(got.next_attempt_at.is_none());
        assert!(got.error_code.is_none());
    }

    #[tokio::test]
    async fn finds_the_earliest_retry_for_queued_episodes() {
        let (pool, show_id) = setup().await;
        assert_eq!(earliest_retry_at(&pool).await.unwrap(), None);

        let first = create(&pool, episode(show_id, "abc123")).await.unwrap();
        let second = create(&pool, episode(show_id, "def456")).await.unwrap();
        let later = now_ts() + 600;
        let sooner = now_ts() + 30;
        record_failure(&pool, &first.uuid, "tts_unavailable", "boom", Some(later))
            .await
            .unwrap();
        record_failure(&pool, &second.uuid, "tts_unavailable", "boom", Some(sooner))
            .await
            .unwrap();

        assert_eq!(earliest_retry_at(&pool).await.unwrap(), Some(sooner));

        cancel(&pool, &second.uuid).await.unwrap();
        assert_eq!(earliest_retry_at(&pool).await.unwrap(), Some(later));
    }

    #[tokio::test]
    async fn set_stage_resets_attempts_only_when_the_stage_changes() {
        let (pool, show_id) = setup().await;
        let ep = create(&pool, episode(show_id, "abc123")).await.unwrap();
        record_failure(
            &pool,
            &ep.uuid,
            "tts_unavailable",
            "boom",
            Some(now_ts() + 30),
        )
        .await
        .unwrap();

        set_stage(&pool, &ep.uuid, stage::QUEUED).await.unwrap();
        assert_eq!(get(&pool, &ep.uuid).await.unwrap().unwrap().attempts, 1);

        set_stage(&pool, &ep.uuid, stage::FETCHING).await.unwrap();
        let got = get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.stage, stage::FETCHING);
        assert_eq!(got.attempts, 0);
    }

    #[tokio::test]
    async fn set_audio_clears_chunk_state_and_ignores_cancelled_episodes() {
        let (pool, show_id) = setup().await;
        let ep = create(&pool, episode(show_id, "abc123")).await.unwrap();
        set_active_take(&pool, &ep.uuid, Some(77)).await.unwrap();
        set_chunk_progress(&pool, &ep.uuid, "[77]", 1, 3)
            .await
            .unwrap();

        let installed = set_audio(&pool, &ep.uuid, "/data/audio/x-1.mp3", 10, 5, "hash", 1)
            .await
            .unwrap();
        assert!(installed);
        let got = get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.active_take_id, None);
        assert_eq!(got.take_ids_json, None);
        assert_eq!(got.chunk_done, 0);
        assert_eq!(got.chunk_count, None);

        cancel(&pool, &ep.uuid).await.unwrap();
        let installed = set_audio(&pool, &ep.uuid, "/data/audio/x-2.mp3", 20, 9, "hash", 2)
            .await
            .unwrap();
        assert!(!installed);
        let got = get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.status, status::CANCELLED);
        assert_eq!(got.audio_path.as_deref(), Some("/data/audio/x-1.mp3"));
    }

    #[tokio::test]
    async fn finish_does_not_overwrite_a_cancelled_episode() {
        let (pool, show_id) = setup().await;
        let ep = create(&pool, episode(show_id, "abc123")).await.unwrap();
        cancel(&pool, &ep.uuid).await.unwrap();

        finish(&pool, &ep.uuid, stage::PUBLISHED, status::PUBLISHED, true)
            .await
            .unwrap();

        let got = get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.status, status::CANCELLED);
        assert_eq!(got.stage, stage::QUEUED);
        assert!(got.published_at.is_none());
    }

    #[tokio::test]
    async fn persists_takes_audio_and_publish_state() {
        let (pool, show_id) = setup().await;
        let ep = create(&pool, episode(show_id, "abc123")).await.unwrap();

        set_active_take(&pool, &ep.uuid, Some(77)).await.unwrap();
        set_chunk_progress(&pool, &ep.uuid, "[77]", 1, 3)
            .await
            .unwrap();
        let got = get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.active_take_id, None);
        assert_eq!(got.take_ids_json.as_deref(), Some("[77]"));
        assert_eq!(got.chunk_done, 1);
        assert_eq!(got.chunk_count, Some(3));

        set_audio(&pool, &ep.uuid, "/data/audio/x-2.mp3", 1234, 61, "hash", 2)
            .await
            .unwrap();
        finish(&pool, &ep.uuid, stage::PUBLISHED, status::PUBLISHED, true)
            .await
            .unwrap();
        let got = get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.audio_version, 2);
        assert_eq!(got.status, status::PUBLISHED);
        assert!(got.published_at.is_some());
    }

    #[tokio::test]
    async fn generated_summary_clears_manual_edit_timestamp() {
        let (pool, show_id) = setup().await;
        let ep = create(&pool, episode(show_id, "abc123")).await.unwrap();
        set_summary_edit(&pool, &ep.uuid, "Bản đã sửa", "hash-edit")
            .await
            .unwrap();
        let edited = get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert!(edited.summary_edited_at.is_some());

        set_summary(&pool, &ep.uuid, "Bản tạo mới", "hash-new")
            .await
            .unwrap();
        let got = get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.summary_text.as_deref(), Some("Bản tạo mới"));
        assert_eq!(got.summary_edited_at, None);
    }

    #[tokio::test]
    async fn reset_for_reread_keeps_old_audio_hash() {
        let (pool, show_id) = setup().await;
        let ep = create(&pool, episode(show_id, "abc123")).await.unwrap();
        set_summary(&pool, &ep.uuid, "Bản mới", "hash-new")
            .await
            .unwrap();
        set_audio(
            &pool,
            &ep.uuid,
            "/data/audio/x-1.mp3",
            100,
            10,
            "hash-old",
            1,
        )
        .await
        .unwrap();
        reset_for_reread(&pool, &ep.uuid).await.unwrap();
        let got = get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.stage, stage::SYNTHESIZING);
        assert_eq!(got.status, status::QUEUED);
        assert_eq!(got.audio_summary_hash.as_deref(), Some("hash-old"));
        assert_eq!(got.active_take_id, None);
        assert_eq!(got.chunk_done, 0);
    }

    #[tokio::test]
    async fn filters_library_by_show_and_status() {
        let (pool, show_id) = setup().await;
        create(&pool, episode(show_id, "abc123")).await.unwrap();
        create(&pool, episode(show_id, "def456")).await.unwrap();
        let all = list(&pool, &EpisodeFilter::default()).await.unwrap();
        assert_eq!(all.len(), 2);
        let queued = list(
            &pool,
            &EpisodeFilter {
                status: Some(status::QUEUED.into()),
                ..EpisodeFilter::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(queued.len(), 2);
    }
}
