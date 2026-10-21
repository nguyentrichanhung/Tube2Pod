use crate::config::Config;
use crate::db::{self, settings};
use crate::error::AppResult;
use sqlx::SqlitePool;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;

const CLEANUP_INTERVAL_SECS: u64 = 24 * 60 * 60;

pub fn spawn(pool: SqlitePool, cfg: Arc<Config>) -> JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(error) = run_once(&pool, &cfg).await {
            tracing::warn!(%error, "initial cleanup failed");
        }
        let mut interval = tokio::time::interval(Duration::from_secs(CLEANUP_INTERVAL_SECS));
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(error) = run_once(&pool, &cfg).await {
                tracing::warn!(%error, "cleanup failed");
            }
        }
    })
}

pub async fn run_once(pool: &SqlitePool, cfg: &Config) -> AppResult<()> {
    let audio_days = settings::read_i64(pool, "retention_old_audio_days", 7).await?;
    let tmp_hours = settings::read_i64(pool, "retention_tmp_hours", 24).await?;
    cleanup_audio(pool, cfg, audio_days).await?;
    cleanup_tmp(pool, cfg, tmp_hours).await?;
    Ok(())
}

async fn cleanup_audio(pool: &SqlitePool, cfg: &Config, days: i64) -> AppResult<()> {
    let dir = cfg.data_dir.join("audio");
    let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
        return Ok(());
    };
    let now = db::now_ts();
    let ttl = days.saturating_mul(24 * 60 * 60);
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        if !file_type.is_file() {
            continue;
        }
        let modified = entry
            .metadata()
            .await
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(system_secs)
            .unwrap_or(now);
        if !is_expired(now, modified, ttl) {
            continue;
        }
        let path = entry.path();
        let referenced: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM episodes WHERE audio_path = ?1")
                .bind(path.display().to_string())
                .fetch_one(pool)
                .await?;
        if referenced == 0 {
            let _ = tokio::fs::remove_file(&path).await;
        }
    }
    Ok(())
}

async fn cleanup_tmp(pool: &SqlitePool, cfg: &Config, hours: i64) -> AppResult<()> {
    let dir = cfg.data_dir.join("tmp");
    let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
        return Ok(());
    };
    let now = db::now_ts();
    let ttl = hours.saturating_mul(60 * 60);
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        if !file_type.is_dir() {
            continue;
        }
        let uuid = entry.file_name().to_string_lossy().to_string();
        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM episodes WHERE uuid = ?1 AND status = 'running'",
        )
        .bind(&uuid)
        .fetch_one(pool)
        .await?;
        if active > 0 {
            continue;
        }
        let modified = entry
            .metadata()
            .await
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(system_secs)
            .unwrap_or(now);
        if is_expired(now, modified, ttl) {
            let _ = tokio::fs::remove_dir_all(entry.path()).await;
        }
    }
    Ok(())
}

fn system_secs(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
}

pub fn is_expired(now: i64, modified: i64, ttl_secs: i64) -> bool {
    now.saturating_sub(modified) > ttl_secs
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn expiry_uses_strictly_older_than_ttl() {
        assert!(!is_expired(1_000, 1_000, 10));
        assert!(!is_expired(1_010, 1_000, 10));
        assert!(is_expired(1_011, 1_000, 10));
    }

    fn config(dir: &Path) -> Config {
        Config::from_lookup(|key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "DATA_DIR" => Some(dir.display().to_string()),
            _ => None,
        })
        .unwrap()
    }

    fn age(path: &Path, seconds: u64) {
        let modified = SystemTime::now() - Duration::from_secs(seconds);
        std::fs::File::open(path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
    }

    fn write(path: &Path, contents: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    async fn episode(pool: &SqlitePool, video_id: &str) -> crate::models::Episode {
        let show = db::shows::create(
            pool,
            db::shows::NewShow {
                title: "Show".into(),
                ..db::shows::NewShow::default()
            },
        )
        .await
        .unwrap();
        db::episodes::create(
            pool,
            db::episodes::NewEpisode {
                show_id: show.id,
                video_id: video_id.into(),
                source_url: format!("https://www.youtube.com/watch?v={video_id}"),
                source_title: "Video".into(),
                source_channel: "".into(),
                source_duration_s: Some(600),
                source_thumbnail_url: None,
                source_upload_date: None,
                language: "vi".into(),
                target_minutes: 10,
            },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn run_once_uses_settings_and_keeps_live_files() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path());
        let pool = crate::db::test_pool().await;
        db::settings::set(&pool, "retention_old_audio_days", "1")
            .await
            .unwrap();
        db::settings::set(&pool, "retention_tmp_hours", "1")
            .await
            .unwrap();

        let audio = cfg.data_dir.join("audio");
        let referenced = episode(&pool, "referenced").await;
        let referenced_path = audio.join(format!("{}-1.mp3", referenced.uuid));
        write(&referenced_path, b"referenced");
        age(&referenced_path, 3 * 24 * 60 * 60);
        db::episodes::set_audio(
            &pool,
            &referenced.uuid,
            &referenced_path.display().to_string(),
            3,
            1,
            "hash",
            1,
        )
        .await
        .unwrap();

        let orphan = audio.join("orphan-1.mp3");
        write(&orphan, b"orphan");
        age(&orphan, 3 * 24 * 60 * 60);

        let fresh_audio = audio.join("fresh-1.mp3");
        write(&fresh_audio, b"fresh");

        let tmp = cfg.data_dir.join("tmp");
        let running = episode(&pool, "running").await;
        sqlx::query("UPDATE episodes SET status = 'running' WHERE uuid = ?1")
            .bind(&running.uuid)
            .execute(&pool)
            .await
            .unwrap();
        let run_dir = tmp.join(&running.uuid);
        write(&run_dir.join("part-000.mp3"), b"part");
        age(&run_dir, 2 * 60 * 60);

        let stale = tmp.join("stale");
        write(&stale.join("part-000.mp3"), b"part");
        age(&stale, 2 * 60 * 60);

        let fresh_tmp = tmp.join("fresh");
        std::fs::create_dir_all(&fresh_tmp).unwrap();

        run_once(&pool, &cfg).await.unwrap();

        assert!(!orphan.exists(), "unreferenced old audio should be removed");
        assert!(referenced_path.exists(), "referenced audio must be kept");
        assert!(fresh_audio.exists(), "fresh audio must be kept");
        assert!(run_dir.exists(), "active tmp dir must be kept");
        assert!(!stale.exists(), "stale tmp dir should be removed");
        assert!(fresh_tmp.exists(), "fresh tmp dir must be kept");
    }
}
