use crate::error::{AppError, AppResult};
use crate::services::tokens::new_feed_token;
use serde::Serialize;
use sqlx::SqlitePool;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct DeviceGrant {
    pub device_id: String,
    pub credential: String,
}

pub async fn issue(pool: &SqlitePool, now: i64) -> AppResult<(String, i64)> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE pair_tickets SET state = 'cancelled' WHERE state = 'pending'")
        .execute(&mut *tx)
        .await?;
    let ticket = new_feed_token();
    let expires_at = now + 300;
    sqlx::query("INSERT INTO pair_tickets (ticket, expires_at) VALUES (?, ?)")
        .bind(&ticket)
        .bind(expires_at)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok((ticket, expires_at))
}

pub async fn redeem(
    pool: &SqlitePool,
    ticket: &str,
    label: &str,
    now: i64,
) -> AppResult<DeviceGrant> {
    let mut tx = pool.begin().await?;
    let result = sqlx::query("UPDATE pair_tickets SET state = 'used' WHERE ticket = ? AND state = 'pending' AND expires_at > ?")
        .bind(ticket).bind(now).execute(&mut *tx).await?;
    if result.rows_affected() == 0 {
        let row: Option<(String, i64)> =
            sqlx::query_as("SELECT state, expires_at FROM pair_tickets WHERE ticket = ?")
                .bind(ticket)
                .fetch_optional(&mut *tx)
                .await?;
        let code = match row {
            Some((state, _)) if state == "used" => "pair_used",
            Some((state, _)) if state == "cancelled" => "pair_cancelled",
            Some((_, expiry)) if expiry <= now => "pair_expired",
            _ => "pair_cancelled",
        };
        return Err(AppError::Conflict(code.into()));
    }
    let grant = DeviceGrant {
        device_id: Uuid::new_v4().to_string(),
        credential: new_feed_token(),
    };
    sqlx::query("INSERT INTO devices (id, label, credential) VALUES (?, ?, ?)")
        .bind(&grant.device_id)
        .bind(label.trim())
        .bind(&grant.credential)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(grant)
}

pub async fn revoke(pool: &SqlitePool, device_id: &str) -> AppResult<()> {
    sqlx::query("UPDATE devices SET revoked = 1 WHERE id = ?")
        .bind(device_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn rename(pool: &SqlitePool, device_id: &str, label: &str) -> AppResult<()> {
    sqlx::query("UPDATE devices SET label = ? WHERE id = ? AND revoked = 0")
        .bind(label.trim())
        .bind(device_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn authorized(pool: &SqlitePool, credential: &str) -> AppResult<bool> {
    let found: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM devices WHERE credential = ? AND revoked = 0)",
    )
    .bind(credential)
    .fetch_one(pool)
    .await?;
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn pair_redeems_once_concurrently() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::connect(&dir.path().join("test.db"))
            .await
            .unwrap();
        crate::db::migrate(&pool).await.unwrap();
        let (ticket, _) = issue(&pool, 10).await.unwrap();
        let (a, b) = tokio::join!(
            redeem(&pool, &ticket, "a", 11),
            redeem(&pool, &ticket, "b", 11)
        );
        assert_ne!(a.is_ok(), b.is_ok());
    }
    #[tokio::test]
    async fn concurrently_issued_tickets_leave_only_one_redeemable() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::connect(&dir.path().join("test.db"))
            .await
            .unwrap();
        crate::db::migrate(&pool).await.unwrap();
        let (a, b) = tokio::join!(issue(&pool, 10), issue(&pool, 10));
        let a = a.unwrap().0;
        let b = b.unwrap().0;
        let (a, b) = tokio::join!(redeem(&pool, &a, "a", 11), redeem(&pool, &b, "b", 11));
        assert_ne!(a.is_ok(), b.is_ok());
    }
    #[tokio::test]
    async fn pair_rejects_expired_or_cancelled_ticket() {
        let pool = crate::db::test_pool().await;
        let (expired, _) = issue(&pool, 10).await.unwrap();
        assert!(
            matches!(redeem(&pool, &expired, "a", 310).await, Err(AppError::Conflict(e)) if e == "pair_expired")
        );
        let (old, _) = issue(&pool, 311).await.unwrap();
        issue(&pool, 312).await.unwrap();
        assert!(
            matches!(redeem(&pool, &old, "a", 313).await, Err(AppError::Conflict(e)) if e == "pair_cancelled")
        );
    }
    #[tokio::test]
    async fn revoking_one_device_preserves_other_device() {
        let pool = crate::db::test_pool().await;
        let (ticket, _) = issue(&pool, 10).await.unwrap();
        let a = redeem(&pool, &ticket, "a", 11).await.unwrap();
        let (ticket, _) = issue(&pool, 12).await.unwrap();
        let b = redeem(&pool, &ticket, "b", 13).await.unwrap();
        revoke(&pool, &a.device_id).await.unwrap();
        assert!(!authorized(&pool, &a.credential).await.unwrap());
        assert!(authorized(&pool, &b.credential).await.unwrap());
    }
}
