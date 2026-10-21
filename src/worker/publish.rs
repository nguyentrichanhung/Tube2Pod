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

    #[tokio::test]
    async fn publishes_only_when_show_is_ready_and_auto_publish_is_on() {
        let pool = db::test_pool().await;
        let show = shows::create(
            &pool,
            NewShow {
                title: "Show".into(),
                description: "Mô tả".into(),
                category: "Technology".into(),
                owner_name: "Owner".into(),
                owner_email: "owner@example.com".into(),
                auto_publish: true,
                ..NewShow::default()
            },
        )
        .await
        .unwrap();
        let show = shows::set_cover(&pool, show.id, "/tmp/cover.jpg", false, false)
            .await
            .unwrap();
        let show = shows::set_feed_enabled(&pool, show.id, true).await.unwrap();
        assert!(show.feed_complete());

        let ep = db::episodes::create(
            &pool,
            db::episodes::NewEpisode {
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
            },
        )
        .await
        .unwrap();
        publish_episode(&pool, &ep).await.unwrap();
        let got = db::episodes::get(&pool, &ep.uuid).await.unwrap().unwrap();
        assert_eq!(got.status, crate::models::status::PUBLISHED);
        assert!(got.published_at.is_some());
    }
}
