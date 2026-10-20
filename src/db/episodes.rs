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
