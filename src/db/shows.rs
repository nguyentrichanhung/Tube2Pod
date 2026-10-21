use crate::db::{now_ts, AppResult};
use crate::error::AppError;
use crate::models::{Show, VoicePreset};
use crate::services::tokens::new_feed_token;
use sqlx::SqlitePool;

#[derive(Debug, Clone)]
pub struct NewShow {
    pub title: String,
    pub author: String,
    pub description: String,
    pub category: String,
    pub owner_name: String,
    pub owner_email: String,
    pub explicit: bool,
    pub language: String,
    pub website_url: String,
    pub default_minutes: i64,
    pub auto_publish: bool,
    pub voice_preset: VoicePreset,
}

impl Default for NewShow {
    fn default() -> Self {
        Self {
            title: String::new(),
            author: String::new(),
            description: String::new(),
            category: "Technology".into(),
            owner_name: String::new(),
            owner_email: String::new(),
            explicit: false,
            language: "vi".into(),
            website_url: String::new(),
            default_minutes: 10,
            auto_publish: true,
            voice_preset: VoicePreset::default(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ShowPatch {
    pub title: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub owner_name: Option<String>,
    pub owner_email: Option<String>,
    pub explicit: Option<bool>,
    pub language: Option<String>,
    pub website_url: Option<String>,
    pub default_minutes: Option<i64>,
    pub auto_publish: Option<bool>,
    pub voice_preset_json: Option<String>,
}

pub async fn create(pool: &SqlitePool, new: NewShow) -> AppResult<Show> {
    let now = now_ts();
    let row = sqlx::query_as::<_, Show>(
        "INSERT INTO shows (
            title, author, description, category, owner_name, owner_email, explicit,
            language, website_url, feed_token, default_minutes, auto_publish,
            voice_preset_json, created_at, updated_at
         ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?14)
         RETURNING *",
    )
    .bind(new.title.trim())
    .bind(new.author.trim())
    .bind(new.description.trim())
    .bind(new.category.trim())
    .bind(new.owner_name.trim())
    .bind(new.owner_email.trim())
    .bind(new.explicit)
    .bind(new.language)
    .bind(new.website_url.trim())
    .bind(new_feed_token())
    .bind(new.default_minutes)
    .bind(new.auto_publish)
    .bind(serde_json::to_string(&new.voice_preset)?)
    .bind(now)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn list(pool: &SqlitePool) -> AppResult<Vec<Show>> {
    Ok(
        sqlx::query_as::<_, Show>("SELECT * FROM shows ORDER BY title COLLATE NOCASE")
            .fetch_all(pool)
            .await?,
    )
}

pub async fn get(pool: &SqlitePool, id: i64) -> AppResult<Option<Show>> {
    Ok(
        sqlx::query_as::<_, Show>("SELECT * FROM shows WHERE id = ?1")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn get_by_feed_token(pool: &SqlitePool, token: &str) -> AppResult<Option<Show>> {
    Ok(
        sqlx::query_as::<_, Show>("SELECT * FROM shows WHERE feed_token = ?1")
            .bind(token)
            .fetch_optional(pool)
            .await?,
    )
}

pub async fn update(pool: &SqlitePool, id: i64, patch: ShowPatch) -> AppResult<Show> {
    let row = sqlx::query_as::<_, Show>(
        "UPDATE shows SET
            title = COALESCE(?1, title),
            author = COALESCE(?2, author),
            description = COALESCE(?3, description),
            category = COALESCE(?4, category),
            owner_name = COALESCE(?5, owner_name),
            owner_email = COALESCE(?6, owner_email),
            explicit = COALESCE(?7, explicit),
            language = COALESCE(?8, language),
            website_url = COALESCE(?9, website_url),
            default_minutes = COALESCE(?10, default_minutes),
            auto_publish = COALESCE(?11, auto_publish),
            voice_preset_json = COALESCE(?12, voice_preset_json),
            updated_at = ?13
         WHERE id = ?14
         RETURNING *",
    )
    .bind(patch.title.map(|v| v.trim().to_string()))
    .bind(patch.author.map(|v| v.trim().to_string()))
    .bind(patch.description.map(|v| v.trim().to_string()))
    .bind(patch.category.map(|v| v.trim().to_string()))
    .bind(patch.owner_name.map(|v| v.trim().to_string()))
    .bind(patch.owner_email.map(|v| v.trim().to_string()))
    .bind(patch.explicit)
    .bind(patch.language)
    .bind(patch.website_url.map(|v| v.trim().to_string()))
    .bind(patch.default_minutes)
    .bind(patch.auto_publish)
    .bind(patch.voice_preset_json)
    .bind(now_ts())
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn sync_owner_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    owner_name: &str,
    owner_email: &str,
) -> AppResult<()> {
    sqlx::query("UPDATE shows SET owner_name = ?1, owner_email = ?2, updated_at = ?3")
        .bind(owner_name.trim())
        .bind(owner_email.trim())
        .bind(now_ts())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub async fn set_cover(
    pool: &SqlitePool,
    id: i64,
    path: &str,
    custom: bool,
    bump_version: bool,
) -> AppResult<Show> {
    let row = sqlx::query_as::<_, Show>(
        "UPDATE shows SET cover_path = ?1, cover_custom = ?2,
            cover_version = cover_version + ?3, updated_at = ?4
         WHERE id = ?5 RETURNING *",
    )
    .bind(path)
    .bind(custom)
    .bind(if bump_version { 1 } else { 0 })
    .bind(now_ts())
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn set_feed_enabled(pool: &SqlitePool, id: i64, enabled: bool) -> AppResult<Show> {
    if enabled {
        let show = get(pool, id).await?.ok_or(AppError::NotFound)?;
        if !show.feed_complete() {
            return Err(AppError::BadRequest(
                "show is incomplete; cannot enable feed".into(),
            ));
        }
    }
    let row = sqlx::query_as::<_, Show>(
        "UPDATE shows SET feed_enabled = ?1, updated_at = ?2 WHERE id = ?3 RETURNING *",
    )
    .bind(enabled)
    .bind(now_ts())
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

pub async fn rotate_feed_token(pool: &SqlitePool, id: i64) -> AppResult<String> {
    let token = new_feed_token();
    sqlx::query("UPDATE shows SET feed_token = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(&token)
        .bind(now_ts())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(token)
}

pub async fn upsert_voice(
    pool: &SqlitePool,
    show_id: i64,
    language: &str,
    voice_id: i64,
    voice_name: &str,
) -> AppResult<()> {
    sqlx::query(
        "INSERT INTO show_voices (show_id, language, omnivoice_voice_id, voice_name, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(show_id, language) DO UPDATE SET
            omnivoice_voice_id = excluded.omnivoice_voice_id,
            voice_name = excluded.voice_name,
            created_at = excluded.created_at",
    )
    .bind(show_id)
    .bind(language)
    .bind(voice_id)
    .bind(voice_name)
    .bind(now_ts())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_voice(
    pool: &SqlitePool,
    show_id: i64,
    language: &str,
) -> AppResult<Option<(i64, String)>> {
    Ok(sqlx::query_as(
        "SELECT omnivoice_voice_id, voice_name FROM show_voices
         WHERE show_id = ?1 AND language = ?2",
    )
    .bind(show_id)
    .bind(language)
    .fetch_optional(pool)
    .await?)
}

pub async fn delete_voice(pool: &SqlitePool, show_id: i64, language: &str) -> AppResult<()> {
    sqlx::query("DELETE FROM show_voices WHERE show_id = ?1 AND language = ?2")
        .bind(show_id)
        .bind(language)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete(pool: &SqlitePool, id: i64) -> AppResult<()> {
    sqlx::query("DELETE FROM shows WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_show() -> NewShow {
        NewShow {
            title: "Công nghệ mỗi ngày".into(),
            language: "vi".into(),
            ..NewShow::default()
        }
    }

    fn complete_show() -> NewShow {
        NewShow {
            title: "Công nghệ mỗi ngày".into(),
            author: "Tube2Pod".into(),
            description: "Bản tin công nghệ hằng ngày.".into(),
            category: "Technology".into(),
            owner_name: "Người dẫn".into(),
            owner_email: "owner@example.com".into(),
            language: "vi".into(),
            ..NewShow::default()
        }
    }

    #[tokio::test]
    async fn creates_lists_and_updates_shows() {
        let pool = crate::db::test_pool().await;
        let show = create(&pool, new_show()).await.unwrap();
        assert!(!show.feed_token.is_empty());
        assert!(!show.feed_enabled);

        let listed = list(&pool).await.unwrap();
        assert_eq!(listed.len(), 1);

        let updated = update(
            &pool,
            show.id,
            ShowPatch {
                title: Some("Tên mới".into()),
                ..ShowPatch::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(updated.title, "Tên mới");
    }

    #[tokio::test]
    async fn rotates_feed_tokens_and_looks_up_by_token() {
        let pool = crate::db::test_pool().await;
        let show = create(&pool, new_show()).await.unwrap();
        let old = show.feed_token.clone();
        let new_token = rotate_feed_token(&pool, show.id).await.unwrap();
        assert_ne!(old, new_token);
        assert!(get_by_feed_token(&pool, &old).await.unwrap().is_none());
        assert_eq!(
            get_by_feed_token(&pool, &new_token)
                .await
                .unwrap()
                .unwrap()
                .id,
            show.id
        );
    }

    #[tokio::test]
    async fn enabling_feed_requires_complete_show() {
        let pool = crate::db::test_pool().await;

        let incomplete = create(&pool, new_show()).await.unwrap();
        let err = set_feed_enabled(&pool, incomplete.id, true)
            .await
            .unwrap_err();
        assert!(matches!(err, crate::error::AppError::BadRequest(_)));
        assert!(
            !get(&pool, incomplete.id)
                .await
                .unwrap()
                .unwrap()
                .feed_enabled
        );

        let complete = create(&pool, complete_show()).await.unwrap();
        let complete = set_cover(&pool, complete.id, "covers/1.jpg", true, true)
            .await
            .unwrap();
        assert!(complete.feed_complete());

        let enabled = set_feed_enabled(&pool, complete.id, true).await.unwrap();
        assert!(enabled.feed_enabled);

        let disabled = set_feed_enabled(&pool, complete.id, false).await.unwrap();
        assert!(!disabled.feed_enabled);
    }

    #[tokio::test]
    async fn caches_voice_per_show_and_language() {
        let pool = crate::db::test_pool().await;
        let show = create(&pool, new_show()).await.unwrap();
        upsert_voice(&pool, show.id, "vi", 42, "Tube2Pod · vi")
            .await
            .unwrap();
        assert_eq!(
            get_voice(&pool, show.id, "vi").await.unwrap().unwrap().0,
            42
        );
    }
}
