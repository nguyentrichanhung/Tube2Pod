use crate::config::Config;
use crate::db::{self, settings};
use crate::error::AppResult;
use crate::models::status;
use sqlx::SqlitePool;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::JoinHandle;

const CLEANUP_INTERVAL_SECS: u64 = 24 * 60 * 60;
const BACKUP_RETENTION_DAYS: i64 = 7;
const EVENT_RETENTION_DAYS: i64 = 30;

static BACKUP_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    purge_stage_events(pool, db::now_ts()).await?;
    backup_database(pool, cfg).await?;
    Ok(())
}

async fn purge_stage_events(pool: &SqlitePool, now: i64) -> AppResult<()> {
    let cutoff = now.saturating_sub(EVENT_RETENTION_DAYS * 86_400);
    match db::stage_events::purge_before(pool, cutoff).await {
        Ok(deleted) if deleted > 0 => {
            tracing::info!(deleted, "purged old stage events");
            Ok(())
        }
        Ok(_) => Ok(()),
        Err(error) => {
            tracing::warn!(%error, "failed to purge stage events");
            Ok(())
        }
    }
}

async fn backup_database(pool: &SqlitePool, cfg: &Config) -> AppResult<()> {
    let _guard = BACKUP_LOCK.lock().await;
    let dir = cfg.data_dir.join("backups");
    tokio::fs::create_dir_all(&dir).await?;
    let today = chrono::Utc::now().date_naive();
    let target = dir.join(format!("tube2pod-{}.db", today.format("%Y-%m-%d")));
    if !tokio::fs::try_exists(&target).await.unwrap_or(false) {
        let sql = format!("VACUUM INTO {}", sql_string_literal(&target));
        sqlx::query(&sql).execute(pool).await?;
    }
    prune_backups(&dir, today).await?;
    Ok(())
}

async fn prune_backups(dir: &Path, today: chrono::NaiveDate) -> AppResult<()> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    let cutoff = today - chrono::Duration::days(BACKUP_RETENTION_DAYS);
    while let Some(entry) = entries.next_entry().await? {
        let Some(date) = entry.file_name().to_str().and_then(backup_date) else {
            continue;
        };
        if date < cutoff {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
    Ok(())
}

fn backup_date(file_name: &str) -> Option<chrono::NaiveDate> {
    let date = file_name.strip_prefix("tube2pod-")?.strip_suffix(".db")?;
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}

fn sql_string_literal(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
}

async fn cleanup_audio(pool: &SqlitePool, cfg: &Config, days: i64) -> AppResult<()> {
    let dir = cfg.data_dir.join("audio");
    let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
        return Ok(());
    };
    let now = db::now_ts();
    let ttl = retention_ttl(days, 24 * 60 * 60);
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        let path_text = path.to_string_lossy();
        let episode = match entry.file_name().to_str().and_then(audio_uuid) {
            Some(uuid) => db::episodes::get(pool, &uuid).await?,
            None => None,
        };
        if let Some(episode) = &episode {
            if episode.audio_path.as_deref() == Some(path_text.as_ref()) {
                continue;
            }
        } else {
            let referenced: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM episodes WHERE audio_path = ?1")
                    .bind(path_text.as_ref())
                    .fetch_one(pool)
                    .await?;
            if referenced > 0 {
                continue;
            }
        }
        let fallback = entry
            .metadata()
            .await
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(system_secs)
            .unwrap_or(now);
        let baseline = episode
            .map(|episode| episode.updated_at)
            .unwrap_or(fallback);
        if is_expired(now, baseline, ttl) {
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
    let ttl = retention_ttl(hours, 60 * 60);
    while let Some(entry) = entries.next_entry().await? {
        let file_type = entry.file_type().await?;
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "yt-dlp" || uuid::Uuid::parse_str(&name).is_err() {
            continue;
        }
        let current: Option<String> =
            sqlx::query_scalar("SELECT status FROM episodes WHERE uuid = ?1")
                .bind(&name)
                .fetch_optional(pool)
                .await?;
        if matches!(current.as_deref(), Some(status::QUEUED | status::RUNNING)) {
            continue;
        }
        if let Some(current) = current.as_deref() {
            if !matches!(
                current,
                status::FAILED | status::CANCELLED | status::READY | status::PUBLISHED
            ) {
                continue;
            }
        }
        let modified = entry
            .metadata()
            .await
            .ok()
            .and_then(|meta| meta.modified().ok())
            .and_then(system_secs)
            .unwrap_or(now);
        if is_expired(now, modified, ttl) {
            if current.is_some() {
                db::episodes::clear_chunk_state(pool, &name).await?;
            }
            let _ = tokio::fs::remove_dir_all(entry.path()).await;
        }
    }
    Ok(())
}

fn audio_uuid(file_name: &str) -> Option<String> {
    let stem = file_name.strip_suffix(".mp3")?;
    let (uuid, version) = stem.rsplit_once('-')?;
    let version = version.parse::<i64>().ok()?;
    if version <= 0 {
        return None;
    }
    uuid::Uuid::parse_str(uuid)
        .ok()
        .map(|uuid| uuid.to_string())
}

fn system_secs(time: SystemTime) -> Option<i64> {
    time.duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
}

fn retention_ttl(value: i64, unit: i64) -> i64 {
    value.max(1).saturating_mul(unit)
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

    #[test]
    fn backup_paths_are_safely_quoted_for_vacuum_into() {
        assert_eq!(
            sql_string_literal(Path::new("/tmp/it's.db")),
            "'/tmp/it''s.db'"
        );
    }

    #[test]
    fn retention_ttl_clamps_to_at_least_one_unit() {
        assert_eq!(retention_ttl(0, 86_400), 86_400);
        assert_eq!(retention_ttl(-5, 3_600), 3_600);
        assert_eq!(retention_ttl(2, 3_600), 7_200);
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

    fn aged_tmp_dir(cfg: &Config, name: &str, seconds: u64) -> std::path::PathBuf {
        let dir = cfg.data_dir.join("tmp").join(name);
        write(&dir.join("part-000.mp3"), b"part");
        age(&dir, seconds);
        dir
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
                review_required: false,
            },
        )
        .await
        .unwrap()
    }

    async fn set_status(pool: &SqlitePool, uuid: &str, status: &str) {
        sqlx::query("UPDATE episodes SET status = ?1 WHERE uuid = ?2")
            .bind(status)
            .bind(uuid)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn set_resume_state(pool: &SqlitePool, uuid: &str) {
        sqlx::query(
            "UPDATE episodes SET active_take_id = 11, take_ids_json = '[11]',
                chunk_done = 1, chunk_count = 3 WHERE uuid = ?1",
        )
        .bind(uuid)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn retention(pool: &SqlitePool, days: &str, hours: &str) {
        db::settings::set(pool, "retention_old_audio_days", days)
            .await
            .unwrap();
        db::settings::set(pool, "retention_tmp_hours", hours)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn run_once_creates_a_daily_backup_and_prunes_old_ones() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path());
        let pool = crate::db::connect(&cfg.database_path()).await.unwrap();
        crate::db::migrate(&pool).await.unwrap();
        let backups = cfg.data_dir.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        let today = chrono::Utc::now().date_naive();
        let name = |date: chrono::NaiveDate| format!("tube2pod-{}.db", date.format("%Y-%m-%d"));
        let today_path = backups.join(name(today));
        let old_path = backups.join(name(today - chrono::Duration::days(8)));
        let recent_path = backups.join(name(today - chrono::Duration::days(2)));
        write(&today_path, b"keep");
        write(&old_path, b"old");
        write(&recent_path, b"recent");

        run_once(&pool, &cfg).await.unwrap();

        assert_eq!(std::fs::read(&today_path).unwrap(), b"keep");
        assert!(
            !old_path.exists(),
            "backups older than 7 days must be pruned"
        );
        assert!(recent_path.exists(), "recent backups must be kept");

        std::fs::remove_file(&today_path).unwrap();
        run_once(&pool, &cfg).await.unwrap();

        let bytes = std::fs::read(&today_path).unwrap();
        assert!(
            bytes.starts_with(b"SQLite format 3\0"),
            "not a SQLite backup"
        );
        assert!(recent_path.exists(), "recent backups must be kept");
    }

    #[tokio::test]
    async fn run_once_keeps_queued_and_running_tmp_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path());
        let pool = crate::db::test_pool().await;
        retention(&pool, "1", "1").await;

        let queued = episode(&pool, "queued").await;
        let queued_dir = aged_tmp_dir(&cfg, &queued.uuid, 5 * 60 * 60);

        let running = episode(&pool, "running").await;
        set_status(&pool, &running.uuid, "running").await;
        let running_dir = aged_tmp_dir(&cfg, &running.uuid, 5 * 60 * 60);

        run_once(&pool, &cfg).await.unwrap();

        assert!(queued_dir.exists(), "queued tmp dir must be preserved");
        assert!(running_dir.exists(), "running tmp dir must be preserved");
    }

    #[tokio::test]
    async fn run_once_removes_terminal_tmp_dir_and_clears_chunk_state() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path());
        let pool = crate::db::test_pool().await;
        retention(&pool, "1", "1").await;

        let failed = episode(&pool, "failed").await;
        set_status(&pool, &failed.uuid, "failed").await;
        set_resume_state(&pool, &failed.uuid).await;
        let failed_dir = aged_tmp_dir(&cfg, &failed.uuid, 2 * 60 * 60);

        run_once(&pool, &cfg).await.unwrap();

        assert!(
            !failed_dir.exists(),
            "expired terminal tmp dir must be removed"
        );
        let got = db::episodes::get(&pool, &failed.uuid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.active_take_id, None);
        assert_eq!(got.take_ids_json, None);
        assert_eq!(got.chunk_done, 0);
        assert_eq!(got.chunk_count, None);
    }

    #[tokio::test]
    async fn run_once_preserves_shared_yt_dlp_and_non_uuid_tmp_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path());
        let pool = crate::db::test_pool().await;
        retention(&pool, "1", "1").await;

        let shared = aged_tmp_dir(&cfg, "yt-dlp", 10 * 24 * 60 * 60);
        let named = aged_tmp_dir(&cfg, "not-a-uuid", 10 * 24 * 60 * 60);

        run_once(&pool, &cfg).await.unwrap();

        assert!(shared.exists(), "tmp/yt-dlp must be preserved");
        assert!(named.exists(), "non-uuid tmp dirs must be preserved");
    }

    #[tokio::test]
    async fn run_once_removes_expired_orphan_uuid_tmp_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path());
        let pool = crate::db::test_pool().await;
        retention(&pool, "1", "1").await;

        let orphan = aged_tmp_dir(&cfg, &uuid::Uuid::new_v4().to_string(), 2 * 60 * 60);

        run_once(&pool, &cfg).await.unwrap();

        assert!(!orphan.exists(), "expired orphan uuid dir must be removed");
    }

    #[tokio::test]
    async fn audio_retention_uses_episode_updated_at_and_keeps_current_audio() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path());
        let pool = crate::db::test_pool().await;
        retention(&pool, "1", "1").await;

        let episode = episode(&pool, "reread").await;
        let audio = cfg.data_dir.join("audio");
        let superseded = audio.join(format!("{}-1.mp3", episode.uuid));
        let current = audio.join(format!("{}-2.mp3", episode.uuid));
        write(&superseded, b"old");
        write(&current, b"new");
        age(&superseded, 10 * 24 * 60 * 60);
        age(&current, 10 * 24 * 60 * 60);
        db::episodes::set_audio(
            &pool,
            &episode.uuid,
            &current.display().to_string(),
            3,
            1,
            "hash",
            2,
        )
        .await
        .unwrap();

        run_once(&pool, &cfg).await.unwrap();
        assert!(
            superseded.exists(),
            "superseded audio is young according to episode.updated_at"
        );
        assert!(current.exists(), "current audio must never be deleted");

        sqlx::query("UPDATE episodes SET updated_at = ?1 WHERE uuid = ?2")
            .bind(db::now_ts() - 2 * 24 * 60 * 60)
            .bind(&episode.uuid)
            .execute(&pool)
            .await
            .unwrap();

        run_once(&pool, &cfg).await.unwrap();
        assert!(
            !superseded.exists(),
            "superseded audio expires after updated_at ages"
        );
        assert!(current.exists(), "current audio must never be deleted");
    }

    #[tokio::test]
    async fn cleanup_removes_stage_events_older_than_thirty_days() {
        let pool = crate::db::test_pool().await;
        let now = crate::db::now_ts();
        sqlx::query(
            "INSERT INTO shows (id, title, feed_token, created_at, updated_at)
             VALUES (1, 'Test Show', 'test-feed', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO episodes (
                uuid, show_id, video_id, source_url, source_title, language,
                target_minutes, stage, status, created_at, updated_at
             ) VALUES ('u1', 1, 'v1', 'https://example.com/watch', 'Episode 1',
                       'en', 10, 'queued', 'done', 0, 0),
                      ('u2', 1, 'v2', 'https://example.com/watch', 'Episode 2',
                       'en', 10, 'queued', 'done', 0, 0)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO stage_events (episode_uuid, stage, entered_at, exited_at, outcome)
             VALUES ('u1', 'fetching', ?1, ?2, 'ok'), ('u2', 'fetching', ?3, ?4, 'ok')",
        )
        .bind(now - 31 * 86_400)
        .bind(now - 31 * 86_400 + 1)
        .bind(now - 1)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        purge_stage_events(&pool, now).await.unwrap();

        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM stage_events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 1);
    }

    #[tokio::test]
    async fn cleanup_keeps_open_stage_events_even_when_old() {
        let pool = crate::db::test_pool().await;
        let now = crate::db::now_ts();
        let episode = episode(&pool, "open-event").await;
        sqlx::query(
            "INSERT INTO stage_events (episode_uuid, stage, entered_at)
             VALUES (?1, 'fetching', ?2)",
        )
        .bind(&episode.uuid)
        .bind(now - 60 * 86_400)
        .execute(&pool)
        .await
        .unwrap();

        purge_stage_events(&pool, now).await.unwrap();

        let old_open: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM stage_events
             WHERE stage = 'fetching' AND exited_at IS NULL AND entered_at = ?1",
        )
        .bind(now - 60 * 86_400)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(old_open, 1);
    }

    #[tokio::test]
    async fn run_once_completes_when_stage_event_purge_fails() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path());
        let pool = crate::db::test_pool().await;
        sqlx::query("DROP TABLE stage_events")
            .execute(&pool)
            .await
            .unwrap();

        run_once(&pool, &cfg).await.unwrap();

        assert!(cfg.data_dir.join("backups").exists());
    }

    #[tokio::test]
    async fn zero_retention_is_clamped_and_cannot_delete_young_files() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = config(dir.path());
        let pool = crate::db::test_pool().await;
        retention(&pool, "0", "0").await;

        let orphan_audio = cfg
            .data_dir
            .join("audio")
            .join(format!("{}-1.mp3", uuid::Uuid::new_v4()));
        write(&orphan_audio, b"orphan");
        age(&orphan_audio, 30 * 60);
        let orphan_tmp = aged_tmp_dir(&cfg, &uuid::Uuid::new_v4().to_string(), 30 * 60);

        run_once(&pool, &cfg).await.unwrap();

        assert!(
            orphan_audio.exists(),
            "zero days must clamp to one day, not delete immediately"
        );
        assert!(
            orphan_tmp.exists(),
            "zero hours must clamp to one hour, not delete immediately"
        );
    }
}
