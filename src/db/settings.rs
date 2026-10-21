use crate::db::{now_ts, AppResult};
use sqlx::SqlitePool;

pub async fn get(pool: &SqlitePool, key: &str) -> AppResult<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT value FROM settings WHERE key = ?1")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.0))
}

pub async fn get_or(pool: &SqlitePool, key: &str, fallback: &str) -> AppResult<String> {
    Ok(get(pool, key).await?.unwrap_or_else(|| fallback.to_string()))
}

pub async fn read_i64(pool: &SqlitePool, key: &str, default: i64) -> AppResult<i64> {
    Ok(get_or(pool, key, &default.to_string())
        .await?
        .parse()
        .unwrap_or(default))
}

pub async fn set(pool: &SqlitePool, key: &str, value: &str) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(key)
    .bind(value)
    .bind(now_ts())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_many(pool: &SqlitePool, pairs: &[(&str, &str)]) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    for (key, value) in pairs {
        sqlx::query(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(value)
        .bind(now_ts())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn settings_round_trip_and_defaults() {
        let pool = crate::db::test_pool().await;
        assert_eq!(get(&pool, "default_minutes").await.unwrap(), None);
        set(&pool, "default_minutes", "15").await.unwrap();
        assert_eq!(get(&pool, "default_minutes").await.unwrap().as_deref(), Some("15"));
        assert_eq!(get_or(&pool, "missing", "fallback").await.unwrap(), "fallback");
    }
}
