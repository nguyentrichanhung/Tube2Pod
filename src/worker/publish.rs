use crate::db::{episodes, shows};
use crate::models::{stage, status, Episode};
use crate::worker::PipelineError;
use sqlx::SqlitePool;

pub async fn publish_episode(pool: &SqlitePool, episode: &Episode) -> Result<(), PipelineError> {
    let show = shows::get(pool, episode.show_id)
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?
        .ok_or_else(|| PipelineError::new("internal", "show not found", false))?;
    if show.auto_publish && show.feed_enabled && show.feed_complete() {
        episodes::finish(
            pool,
            &episode.uuid,
            stage::PUBLISHED,
            status::PUBLISHED,
            true,
        )
        .await
        .map_err(|e| PipelineError::from_app(e, "internal"))?;
    } else {
        episodes::finish(pool, &episode.uuid, stage::READY, status::READY, false)
            .await
            .map_err(|e| PipelineError::from_app(e, "internal"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{
        self,
        shows::{self, NewShow},
    };

    async fn complete_show(
        pool: &SqlitePool,
        auto_publish: bool,
        feed_enabled: bool,
    ) -> crate::models::Show {
        let show = shows::create(
            pool,
            NewShow {
                title: "Show".into(),
                description: "Mô tả".into(),
                category: "Technology".into(),
                owner_name: "Owner".into(),
                owner_email: "owner@example.com".into(),
                auto_publish,
                ..NewShow::default()
            },
        )
        .await
        .unwrap();
        let show = shows::set_cover(pool, show.id, "/tmp/cover.jpg", false, false)
            .await
            .unwrap();
        if feed_enabled {
            shows::set_feed_enabled(pool, show.id, true).await.unwrap()
        } else {
            show
        }
    }

    async fn episode_for(pool: &SqlitePool, show_id: i64, video_id: &str) -> Episode {
        db::episodes::create(
            pool,
            db::episodes::NewEpisode {
                show_id,
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
    async fn publishes_only_when_show_is_ready_and_auto_publish_is_on() {
        let pool = db::test_pool().await;
        let show = complete_show(&pool, true, true).await;
        assert!(show.feed_complete());

        let ep = episode_for(&pool, show.id, "v1").await;
        publish_episode(&pool, &ep).await.unwrap();
        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.status, crate::models::status::PUBLISHED);
        assert!(got.published_at.is_some());
    }

    #[tokio::test]
    async fn leaves_episodes_ready_when_auto_publish_is_off() {
        let pool = db::test_pool().await;
        let show = complete_show(&pool, false, true).await;
        let ep = episode_for(&pool, show.id, "v1").await;

        publish_episode(&pool, &ep).await.unwrap();

        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.status, crate::models::status::READY);
        assert_eq!(got.stage, stage::READY);
        assert!(got.published_at.is_none());
    }

    #[tokio::test]
    async fn leaves_episodes_ready_when_the_feed_is_disabled() {
        let pool = db::test_pool().await;
        let show = complete_show(&pool, true, false).await;
        let ep = episode_for(&pool, show.id, "v1").await;

        publish_episode(&pool, &ep).await.unwrap();

        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.status, crate::models::status::READY);
        assert_eq!(got.stage, stage::READY);
        assert!(got.published_at.is_none());
    }

    #[tokio::test]
    async fn repeated_publish_preserves_the_first_published_at() {
        let pool = db::test_pool().await;
        let show = complete_show(&pool, true, true).await;
        let ep = episode_for(&pool, show.id, "v1").await;

        publish_episode(&pool, &ep).await.unwrap();
        let first = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        let published_at = first.published_at.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        publish_episode(&pool, &ep).await.unwrap();

        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.status, crate::models::status::PUBLISHED);
        assert_eq!(got.published_at, Some(published_at));
    }

    #[tokio::test]
    async fn publish_does_not_resurrect_a_cancelled_episode() {
        let pool = db::test_pool().await;
        let show = complete_show(&pool, true, true).await;
        let ep = episode_for(&pool, show.id, "v1").await;
        db::episodes::cancel(&pool, &ep.uuid).await.unwrap();

        publish_episode(&pool, &ep).await.unwrap();

        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.status, crate::models::status::CANCELLED);
        assert_eq!(got.stage, stage::QUEUED);
        assert!(got.published_at.is_none());
    }
}
