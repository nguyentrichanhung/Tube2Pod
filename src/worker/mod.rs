pub mod assemble;
pub mod fetch;
pub mod publish;
pub mod summarize;
pub mod synth;
pub mod voice;

use crate::config::Config;
use crate::db::episodes;
use crate::error::AppError;
use crate::models::{stage, status};
use crate::services::omnivoice::OmnivoiceClient;
use sqlx::SqlitePool;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

pub const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(250);

pub async fn is_cancelled(pool: &SqlitePool, uuid: &str) -> bool {
    match episodes::get(pool, uuid).await {
        Ok(Some(episode)) => episode.status == status::CANCELLED,
        Ok(None) => true,
        Err(error) => {
            tracing::warn!(%error, %uuid, "cancellation check failed");
            false
        }
    }
}

async fn wait_for_cancellation(pool: &SqlitePool, uuid: &str) {
    loop {
        tokio::time::sleep(CANCEL_POLL_INTERVAL).await;
        match episodes::get(pool, uuid).await {
            Ok(Some(episode)) if episode.status != status::CANCELLED => {}
            Ok(Some(_)) | Ok(None) => return,
            Err(error) => {
                tracing::warn!(%error, %uuid, "cancellation watch failed");
                return;
            }
        }
    }
}

pub async fn cancellable<F>(pool: &SqlitePool, uuid: &str, future: F) -> Option<F::Output>
where
    F: Future,
{
    tokio::select! {
        biased;
        _ = wait_for_cancellation(pool, uuid) => None,
        output = future => Some(output),
    }
}

#[derive(Debug, Clone)]
pub struct PipelineError {
    pub code: &'static str,
    pub message: String,
    pub retryable: bool,
}

impl PipelineError {
    pub fn new(code: &'static str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }

    pub fn from_app(error: AppError, code: &'static str) -> Self {
        let retryable = !matches!(error, AppError::BadRequest(_) | AppError::NotFound);
        Self::new(code, error.to_string(), retryable)
    }
}

#[derive(Clone)]
pub struct Pipeline {
    pub pool: SqlitePool,
    pub cfg: Arc<Config>,
    pub omni: Arc<OmnivoiceClient>,
}

impl Pipeline {
    pub fn new(pool: SqlitePool, cfg: Arc<Config>) -> Self {
        let omni = Arc::new(OmnivoiceClient::new(&cfg.omnivoice));
        Self { pool, cfg, omni }
    }

    pub async fn run_episode(&self, uuid: &str) -> Result<(), PipelineError> {
        loop {
            let episode = episodes::get(&self.pool, uuid)
                .await
                .map_err(|e| PipelineError::from_app(e, "internal"))?
                .ok_or_else(|| PipelineError::new("internal", "episode disappeared", false))?;
            if episode.status == status::CANCELLED {
                return Ok(());
            }
            if episode.is_terminal() {
                return Ok(());
            }
            match episode.stage.as_str() {
                stage::QUEUED | stage::FETCHING => {
                    fetch::fetch_episode(&self.pool, &self.cfg, &episode).await?
                }
                stage::SUMMARIZING => {
                    summarize::summarize_episode(&self.pool, &self.cfg, &episode).await?
                }
                stage::VOICING => {
                    voice::resolve_voice(&self.pool, &self.cfg, &episode, &self.omni).await?;
                }
                stage::SYNTHESIZING => {
                    synth::synthesize_episode(&self.pool, &self.cfg, &episode, &self.omni).await?
                }
                stage::ASSEMBLING => {
                    assemble::assemble_episode(&self.pool, &self.cfg, &episode).await?
                }
                stage::PUBLISHING => {
                    publish::publish_episode(&self.pool, &episode).await?;
                    let parts = synth::parts_dir(&self.cfg, &episode.uuid);
                    let _ = tokio::fs::remove_dir_all(parts).await;
                }
                stage::REVIEW => return Ok(()),
                stage::READY | stage::PUBLISHED => return Ok(()),
                other => {
                    return Err(PipelineError::new(
                        "internal",
                        format!("unknown stage: {other}"),
                        false,
                    ))
                }
            }
        }
    }
}

pub fn backoff_secs(attempts: i64) -> u64 {
    match attempts {
        0 => 30,
        1 => 120,
        _ => 600,
    }
}

async fn record_episode_failure(
    pool: &SqlitePool,
    uuid: &str,
    error: &PipelineError,
) -> Result<Option<i64>, AppError> {
    let Some(current) = episodes::get(pool, uuid).await? else {
        return Ok(None);
    };
    if current.status == status::CANCELLED {
        return Ok(None);
    }
    let retryable = error.retryable && current.attempts < 3;
    let retry_at = retryable.then(|| crate::db::now_ts() + backoff_secs(current.attempts) as i64);
    episodes::record_failure(pool, uuid, error.code, &error.message, retry_at).await?;
    Ok(retry_at)
}

pub fn spawn(
    pool: SqlitePool,
    cfg: Arc<Config>,
    notify: Arc<Notify>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        if let Err(error) = episodes::recover_running(&pool).await {
            tracing::error!(%error, "failed to recover running episodes");
        }
        let pipeline = Pipeline::new(pool.clone(), cfg);
        loop {
            match episodes::claim_next(&pool).await {
                Ok(Some(episode)) => match pipeline.run_episode(&episode.uuid).await {
                    Ok(()) => {}
                    Err(error) => {
                        match record_episode_failure(&pool, &episode.uuid, &error).await {
                            Ok(Some(at)) => {
                                let notify = notify.clone();
                                tokio::spawn(async move {
                                    let delay = (at - crate::db::now_ts()).max(0) as u64;
                                    tokio::time::sleep(Duration::from_secs(delay)).await;
                                    notify.notify_one();
                                });
                            }
                            Ok(None) => {}
                            Err(db_error) => {
                                tracing::error!(%db_error, "failed to record episode failure");
                            }
                        }
                        tracing::warn!(
                            episode = %episode.uuid,
                            code = error.code,
                            message = %error.message,
                            "episode failed"
                        );
                    }
                },
                Ok(None) => match episodes::earliest_retry_at(&pool).await {
                    Ok(Some(at)) => {
                        let delay = (at - crate::db::now_ts()).max(0) as u64;
                        tokio::select! {
                            _ = notify.notified() => {}
                            _ = tokio::time::sleep(Duration::from_secs(delay)) => {}
                        }
                    }
                    Ok(None) => notify.notified().await,
                    Err(error) => {
                        tracing::error!(%error, "failed to read retry schedule");
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                },
                Err(error) => {
                    tracing::error!(%error, "queue claim failed");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_app_marks_client_errors_as_non_retryable() {
        let error = PipelineError::from_app(AppError::BadRequest("bad url".into()), "invalid_url");
        assert_eq!(error.code, "invalid_url");
        assert_eq!(error.message, "bad url");
        assert!(!error.retryable);

        let missing = PipelineError::from_app(AppError::NotFound, "internal");
        assert!(!missing.retryable);
    }

    #[test]
    fn from_app_marks_upstream_and_internal_errors_as_retryable() {
        let upstream =
            PipelineError::from_app(AppError::Upstream("timeout".into()), "source_unavailable");
        assert!(upstream.retryable);

        let internal = PipelineError::from_app(
            AppError::Internal(anyhow::anyhow!("database busy")),
            "internal",
        );
        assert!(internal.retryable);
    }

    #[test]
    fn retry_backoff_is_30s_2m_10m() {
        assert_eq!(backoff_secs(0), 30);
        assert_eq!(backoff_secs(1), 120);
        assert_eq!(backoff_secs(2), 600);
        assert_eq!(backoff_secs(3), 600);
    }

    async fn seed_episode(pool: &SqlitePool) -> crate::models::Episode {
        let show = crate::db::shows::create(
            pool,
            crate::db::shows::NewShow {
                title: "Show".into(),
                ..crate::db::shows::NewShow::default()
            },
        )
        .await
        .unwrap();
        crate::db::episodes::create(
            pool,
            crate::db::episodes::NewEpisode {
                show_id: show.id,
                video_id: "v1".into(),
                source_url: "https://www.youtube.com/watch?v=v1".into(),
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

    #[tokio::test]
    async fn cancelled_episode_is_never_requeued_by_the_failure_handler() {
        let pool = crate::db::test_pool().await;
        let ep = seed_episode(&pool).await;
        crate::db::episodes::cancel(&pool, &ep.uuid).await.unwrap();
        let error = PipelineError::new("tts_unavailable", "boom", true);

        let retry_at = record_episode_failure(&pool, &ep.uuid, &error)
            .await
            .unwrap();

        assert!(retry_at.is_none());
        let got = crate::db::episodes::get(&pool, &ep.uuid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.status, crate::models::status::CANCELLED);
        assert_eq!(got.attempts, 0);
        assert!(got.next_attempt_at.is_none());
        assert!(got.error_code.is_none());
    }

    #[tokio::test]
    async fn failure_handler_retries_three_times_then_fails() {
        let pool = crate::db::test_pool().await;
        let ep = seed_episode(&pool).await;
        let error = PipelineError::new("tts_unavailable", "boom", true);

        for _ in 0..3 {
            assert!(record_episode_failure(&pool, &ep.uuid, &error)
                .await
                .unwrap()
                .is_some());
        }
        assert!(record_episode_failure(&pool, &ep.uuid, &error)
            .await
            .unwrap()
            .is_none());

        let got = crate::db::episodes::get(&pool, &ep.uuid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.status, crate::models::status::FAILED);
        assert_eq!(got.attempts, 4);
        assert!(got.next_attempt_at.is_none());
    }
}
