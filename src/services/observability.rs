use crate::db::stage_events::{self, FailureGroup, RecentFailure};
use crate::error::AppResult;
use crate::models::{stage, status};
use sqlx::SqlitePool;
use std::collections::BTreeMap;

pub const WINDOW_24H: &str = "24h";
pub const WINDOW_7D: &str = "7d";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    Day,
    Week,
}

impl Window {
    pub fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some(WINDOW_7D) => Self::Week,
            _ => Self::Day,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Day => WINDOW_24H,
            Self::Week => WINDOW_7D,
        }
    }

    pub fn since(self, now: i64) -> i64 {
        let seconds = match self {
            Self::Day => 86_400,
            Self::Week => 604_800,
        };
        now.saturating_sub(seconds)
    }
}

#[derive(Debug, Clone)]
pub struct StageLatency {
    pub stage: &'static str,
    pub count: usize,
    pub p50_secs: Option<i64>,
    pub p95_secs: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct FailureGroupRow {
    pub label: &'static str,
    pub stage: &'static str,
    pub count: i64,
}

#[derive(Debug, Clone)]
pub struct RecentFailureRow {
    pub episode_uuid: String,
    pub source_title: String,
    pub label: &'static str,
    pub stage: &'static str,
    pub age: String,
}

#[derive(Debug, Clone, Default)]
pub struct Metrics {
    pub queued: i64,
    pub running: i64,
    pub review: i64,
    pub failed: i64,
    pub ready: i64,
    pub oldest_queued_secs: Option<i64>,
    pub created: i64,
    pub published: i64,
    pub failed_attempts: i64,
    pub latency: Vec<StageLatency>,
    pub has_latency: bool,
    pub failure_groups: Vec<FailureGroupRow>,
    pub recent_failures: Vec<RecentFailureRow>,
}

pub fn percentile(sorted: &[i64], p: u8) -> Option<i64> {
    if sorted.is_empty() {
        return None;
    }
    let n = sorted.len();
    let rank = ((p as usize * n + 99) / 100).max(1).min(n);
    Some(sorted[rank - 1])
}

fn status_count(rows: &[(String, i64)], value: &str) -> i64 {
    rows.iter()
        .find(|(status, _)| status == value)
        .map(|(_, count)| *count)
        .unwrap_or(0)
}

pub async fn load_metrics(
    pool: &SqlitePool,
    window: Window,
    now: i64,
) -> AppResult<Metrics> {
    let since = window.since(now);
    let status_rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT status, COUNT(*) FROM episodes GROUP BY status",
    )
    .fetch_all(pool)
    .await?;
    let oldest_event = stage_events::oldest_open_queued(pool).await?;
    let oldest_created: Option<i64> = sqlx::query_scalar(
        "SELECT MIN(created_at) FROM episodes WHERE status IN (?1, ?2)",
    )
    .bind(status::QUEUED)
    .bind(status::RUNNING)
    .fetch_one(pool)
    .await?;
    let oldest = match (oldest_event, oldest_created) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };

    let created: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM episodes WHERE created_at >= ?1")
            .bind(since)
            .fetch_one(pool)
            .await?;
    let published: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM episodes WHERE published_at IS NOT NULL AND published_at >= ?1",
    )
    .bind(since)
    .fetch_one(pool)
    .await?;
    let failed_attempts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM stage_events
         WHERE outcome = 'failed' AND exited_at IS NOT NULL AND exited_at >= ?1",
    )
    .bind(since)
    .fetch_one(pool)
    .await?;

    let durations = stage_events::durations_since(pool, since).await?;
    let mut by_stage: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    for (event_stage, duration) in durations {
        by_stage
            .entry(event_stage)
            .or_default()
            .push(duration.max(0));
    }
    let mut latency = Vec::new();
    for event_stage in [
        stage::FETCHING,
        stage::SUMMARIZING,
        stage::VOICING,
        stage::SYNTHESIZING,
        stage::ASSEMBLING,
        stage::PUBLISHING,
    ] {
        let mut values = by_stage.remove(event_stage).unwrap_or_default();
        values.sort_unstable();
        latency.push(StageLatency {
            stage: event_stage,
            count: values.len(),
            p50_secs: percentile(&values, 50),
            p95_secs: percentile(&values, 95),
        });
    }

    let failures: Vec<FailureGroup> = stage_events::failure_groups(pool, since).await?;
    let recent: Vec<RecentFailure> =
        stage_events::recent_failures(pool, since, 20).await?;
    let has_latency = latency.iter().any(|row| row.count > 0);

    Ok(Metrics {
        queued: status_count(&status_rows, status::QUEUED),
        running: status_count(&status_rows, status::RUNNING),
        review: status_count(&status_rows, status::REVIEW),
        failed: status_count(&status_rows, status::FAILED),
        ready: status_count(&status_rows, status::READY)
            + status_count(&status_rows, status::PUBLISHED),
        oldest_queued_secs: oldest.map(|entered| (now - entered).max(0)),
        created,
        published,
        failed_attempts,
        latency,
        has_latency,
        failure_groups: failures
            .into_iter()
            .map(|row| FailureGroupRow {
                label: crate::models::error_label_for(Some(&row.error_code))
                    .unwrap_or("Lỗi khác"),
                stage: crate::models::stage_label_for(&row.stage),
                count: row.count,
            })
            .collect(),
        recent_failures: recent
            .into_iter()
            .map(|row| RecentFailureRow {
                episode_uuid: row.episode_uuid,
                source_title: row.source_title,
                label: crate::models::error_label_for(Some(&row.error_code))
                    .unwrap_or("Lỗi khác"),
                stage: crate::models::stage_label_for(&row.stage),
                age: crate::models::created_label_for(row.exited_at),
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_percentiles_handle_small_samples() {
        let values = [10, 20, 30, 40, 50];
        assert_eq!(percentile(&values, 50), Some(30));
        assert_eq!(percentile(&values, 95), Some(50));
        assert_eq!(percentile(&[], 50), None);
        assert_eq!(percentile(&[7], 95), Some(7));
    }

    #[tokio::test]
    async fn metrics_count_queue_latency_and_failures() {
        let pool = crate::db::test_pool().await;
        let now = 1_000_000;
        let show = crate::db::shows::create(
            &pool,
            crate::db::shows::NewShow {
                title: "Show".into(),
                ..crate::db::shows::NewShow::default()
            },
        )
        .await
        .unwrap();
        let latency = crate::db::episodes::create(
            &pool,
            crate::db::episodes::NewEpisode {
                show_id: show.id,
                video_id: "latency".into(),
                source_url: "https://www.youtube.com/watch?v=latency".into(),
                source_title: "Tập nhanh".into(),
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
        let failed = crate::db::episodes::create(
            &pool,
            crate::db::episodes::NewEpisode {
                show_id: show.id,
                video_id: "failed".into(),
                source_url: "https://www.youtube.com/watch?v=failed".into(),
                source_title: "Tập lỗi".into(),
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

        sqlx::query(
            "UPDATE stage_events SET stage = 'fetching', entered_at = ?2,
                exited_at = ?3, outcome = 'ok'
             WHERE episode_uuid = ?1 AND exited_at IS NULL",
        )
        .bind(&latency.uuid)
        .bind(now - 100)
        .bind(now - 90)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO stage_events (episode_uuid, stage, entered_at, exited_at, outcome)
             VALUES (?1, 'summarizing', ?2, ?3, 'ok')",
        )
        .bind(&latency.uuid)
        .bind(now - 100)
        .bind(now - 50)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE stage_events SET stage = 'fetching', entered_at = ?2,
                exited_at = ?3, outcome = 'failed', error_code = 'tts_unavailable'
             WHERE episode_uuid = ?1 AND exited_at IS NULL",
        )
        .bind(&failed.uuid)
        .bind(now - 100)
        .bind(now - 20)
        .execute(&pool)
        .await
        .unwrap();

        let metrics = load_metrics(&pool, Window::Day, now).await.unwrap();
        assert_eq!(metrics.failed_attempts, 1);
        assert!(metrics.has_latency);
        assert_eq!(metrics.latency[0].count, 1);
        assert_eq!(metrics.latency[0].p50_secs, Some(10));
        assert_eq!(metrics.failure_groups[0].count, 1);
        assert_eq!(metrics.recent_failures[0].episode_uuid, failed.uuid);
    }
}
