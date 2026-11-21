pub mod episodes;
pub mod settings;
pub mod shows;
pub mod stage_events;

use crate::error::AppResult;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

pub async fn connect(path: &Path) -> AppResult<SqlitePool> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;
    Ok(pool)
}

pub async fn migrate(pool: &SqlitePool) -> AppResult<()> {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .map_err(|e| crate::error::AppError::Internal(anyhow::anyhow!(e)))?;
    Ok(())
}

pub fn now_ts() -> i64 {
    chrono::Utc::now().timestamp()
}

#[cfg(test)]
pub async fn test_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    migrate(&pool).await.unwrap();
    pool
}

#[cfg(test)]
pub async fn test_pool_0004() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    let all = sqlx::migrate!("./migrations");
    let old = sqlx::migrate::Migrator {
        migrations: std::borrow::Cow::Owned(
            all.iter().filter(|m| m.version <= 4).cloned().collect(),
        ),
        ..sqlx::migrate::Migrator::DEFAULT
    };
    old.run(&pool).await.unwrap();
    pool
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn schema_drops_the_unused_summary_edited_column() {
        let pool = test_pool().await;
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('episodes')")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert!(!columns.iter().any(|name| name == "summary_edited_at"));
    }
    #[tokio::test]
    async fn migration_retires_only_operational_voice_fields() {
        let pool = test_pool_0004().await;
        migrate(&pool).await.unwrap();
        let episode_columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('episodes')")
                .fetch_all(&pool)
                .await
                .unwrap();
        for name in [
            "voice_id",
            "voice_design_id",
            "active_take_id",
            "take_ids_json",
            "chunk_count",
            "chunk_done",
            "review_required",
            "target_minutes",
        ] {
            assert!(
                !episode_columns.iter().any(|column| column == name),
                "{name}"
            );
        }
        for name in ["audio_script", "summary_text", "transcript_path"] {
            assert!(
                episode_columns.iter().any(|column| column == name),
                "{name}"
            );
        }
        let show_columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('shows')")
                .fetch_all(&pool)
                .await
                .unwrap();
        for name in ["voice_preset_json", "default_minutes"] {
            assert!(!show_columns.iter().any(|column| column == name), "{name}");
        }
        let table: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='show_voices'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(table, 0);
    }
}
