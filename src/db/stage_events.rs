use crate::db::{now_ts, AppResult};
use sqlx::SqlitePool;

pub const OUTCOME_OK: &str = "ok";
pub const OUTCOME_FAILED: &str = "failed";
pub const OUTCOME_CANCELLED: &str = "cancelled";
pub const OUTCOME_INTERRUPTED: &str = "interrupted";

pub async fn open(pool: &SqlitePool, episode_uuid: &str, stage: &str) -> AppResult<()> {
    sqlx::query("INSERT INTO stage_events (episode_uuid, stage, entered_at) VALUES (?1, ?2, ?3)")
        .bind(episode_uuid)
        .bind(stage)
        .bind(now_ts())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn close_open(
    pool: &SqlitePool,
    episode_uuid: &str,
    outcome: &str,
    error_code: Option<&str>,
) -> AppResult<u64> {
    let result = sqlx::query(
        "UPDATE stage_events SET exited_at = ?1, outcome = ?2, error_code = ?3
         WHERE episode_uuid = ?4 AND exited_at IS NULL",
    )
    .bind(now_ts())
    .bind(outcome)
    .bind(error_code)
    .bind(episode_uuid)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn ensure_open(pool: &SqlitePool, episode_uuid: &str, stage: &str) -> AppResult<()> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM stage_events WHERE episode_uuid = ?1 AND exited_at IS NULL)",
    )
    .bind(episode_uuid)
    .fetch_one(pool)
    .await?;
    if !exists {
        open(pool, episode_uuid, stage).await?;
    }
    Ok(())
}

pub async fn close_for_running(pool: &SqlitePool) -> AppResult<u64> {
    let result = sqlx::query(
        "UPDATE stage_events SET exited_at = ?1, outcome = ?2
         WHERE exited_at IS NULL
           AND episode_uuid IN (SELECT uuid FROM episodes WHERE status = 'running')",
    )
    .bind(now_ts())
    .bind(OUTCOME_INTERRUPTED)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

pub async fn purge_before(pool: &SqlitePool, cutoff: i64) -> AppResult<u64> {
    let result =
        sqlx::query("DELETE FROM stage_events WHERE exited_at IS NOT NULL AND exited_at < ?1")
            .bind(cutoff)
            .execute(pool)
            .await?;
    Ok(result.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_pool;

    async fn seed_episode(pool: &SqlitePool, uuid: &str, status: &str) {
        sqlx::query(
            "INSERT OR IGNORE INTO shows (id, title, feed_token, created_at, updated_at)
             VALUES (1, 'Test Show', 'test-feed', 0, 0)",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO episodes (
                uuid, show_id, video_id, source_url, source_title, language,
                target_minutes, stage, status, created_at, updated_at
             ) VALUES (?1, 1, ?2, 'https://example.com/watch', 'Test Episode',
                       'en', 10, 'queued', ?3, 0, 0)",
        )
        .bind(uuid)
        .bind(format!("video-{uuid}"))
        .bind(status)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn opens_closes_and_ensures_stage_events() {
        let pool = test_pool().await;
        let uuid = "00000000-0000-4000-8000-000000000001";
        seed_episode(&pool, uuid, "running").await;

        open(&pool, uuid, "queued").await.unwrap();
        open(&pool, uuid, "fetching").await.unwrap();
        close_open(&pool, uuid, OUTCOME_OK, None).await.unwrap();

        let rows: Vec<(String, Option<i64>, Option<String>)> =
            sqlx::query_as("SELECT stage, exited_at, outcome FROM stage_events ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, "queued");
        assert_eq!(rows[1].0, "fetching");
        assert!(rows[1].1.is_some());
        assert_eq!(rows[1].2.as_deref(), Some(OUTCOME_OK));

        ensure_open(&pool, uuid, "summarizing").await.unwrap();
        ensure_open(&pool, uuid, "summarizing").await.unwrap();
        let open_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM stage_events WHERE episode_uuid = ?1 AND exited_at IS NULL",
        )
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(open_count, 1);
    }

    #[tokio::test]
    async fn purges_only_closed_events_before_the_cutoff() {
        let pool = test_pool().await;
        let uuid = "00000000-0000-4000-8000-000000000002";
        seed_episode(&pool, uuid, "done").await;
        sqlx::query(
            "INSERT INTO stage_events (episode_uuid, stage, entered_at, exited_at, outcome)
             VALUES (?1, 'fetching', 10, 20, 'ok'), (?1, 'fetching', 100, 110, 'ok')",
        )
        .bind(uuid)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(purge_before(&pool, 50).await.unwrap(), 1);
        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM stage_events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 1);
    }

    #[tokio::test]
    async fn closes_open_events_only_for_running_episodes() {
        let pool = test_pool().await;
        let running = "00000000-0000-4000-8000-000000000003";
        let done = "00000000-0000-4000-8000-000000000004";
        seed_episode(&pool, running, "running").await;
        seed_episode(&pool, done, "done").await;

        open(&pool, running, "fetching").await.unwrap();
        open(&pool, done, "fetching").await.unwrap();

        assert_eq!(close_for_running(&pool).await.unwrap(), 1);
        let outcomes: Vec<(String, Option<String>)> =
            sqlx::query_as("SELECT episode_uuid, outcome FROM stage_events ORDER BY id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(outcomes[0].0, running);
        assert_eq!(outcomes[0].1.as_deref(), Some(OUTCOME_INTERRUPTED));
        assert_eq!(outcomes[1].0, done);
        assert_eq!(outcomes[1].1, None);
    }

    #[tokio::test]
    async fn deleting_an_episode_cascades_its_events() {
        let pool = crate::db::test_pool().await;
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        let show = crate::db::shows::create(
            &pool,
            crate::db::shows::NewShow {
                title: "Show".into(),
                ..crate::db::shows::NewShow::default()
            },
        )
        .await
        .unwrap();
        let ep = crate::db::episodes::create(
            &pool,
            crate::db::episodes::NewEpisode {
                show_id: show.id,
                video_id: "cascade".into(),
                source_url: "https://www.youtube.com/watch?v=cascade".into(),
                source_title: "Video".into(),
                source_channel: String::new(),
                source_duration_s: None,
                source_thumbnail_url: None,
                source_upload_date: None,
                language: "vi".into(),
                target_minutes: 10,
                review_required: false,
            },
        )
        .await
        .unwrap();
        let before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM stage_events WHERE episode_uuid = ?1")
                .bind(&ep.uuid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(before, 1);
        crate::db::episodes::delete(&pool, &ep.uuid).await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM stage_events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
}
