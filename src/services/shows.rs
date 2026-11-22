use crate::db::{settings, shows};
use crate::error::{AppError, AppResult};
use crate::models::Show;
use crate::services::cover;
use crate::state::AppState;

fn validate(title: Option<&str>, language: Option<&str>) -> AppResult<()> {
    if title.is_some_and(|title| title.trim().is_empty()) {
        return Err(AppError::BadRequest("Tên show là bắt buộc.".into()));
    }
    if language.is_some_and(|language| !matches!(language, "vi" | "en" | "ko")) {
        return Err(AppError::BadRequest("Chỉ hỗ trợ vi, en, ko.".into()));
    }
    Ok(())
}

async fn write_cover(
    state: &AppState,
    id: i64,
    bytes: &[u8],
    custom: bool,
    bump: bool,
) -> AppResult<Show> {
    let path = state
        .cfg
        .data_dir
        .join("shows")
        .join(id.to_string())
        .join("cover.jpg");
    tokio::fs::create_dir_all(path.parent().unwrap()).await?;
    tokio::fs::write(&path, bytes).await?;
    shows::set_cover(&state.pool, id, &path.display().to_string(), custom, bump).await
}

pub async fn create(state: &AppState, mut new: shows::NewShow) -> AppResult<Show> {
    validate(Some(&new.title), Some(&new.language))?;
    new.owner_name = settings::get_or(&state.pool, "owner_name", "").await?;
    new.owner_email = settings::get_or(&state.pool, "owner_email", "").await?;
    new.auto_publish =
        settings::get_or(&state.pool, "auto_publish_default", "true").await? == "true";
    new.explicit = settings::get_or(&state.pool, "default_explicit", "false").await? == "true";
    let show = shows::create(&state.pool, new).await?;
    let bytes = cover::render_cover(&show.title)?;
    write_cover(state, show.id, &bytes, false, false).await
}

pub async fn update(state: &AppState, id: i64, patch: shows::ShowPatch) -> AppResult<Show> {
    validate(patch.title.as_deref(), patch.language.as_deref())?;
    let current = shows::get(&state.pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let title_changed = patch
        .title
        .as_ref()
        .is_some_and(|title| current.title != title.trim());
    let mut updated = shows::update(&state.pool, id, patch).await?;
    if title_changed && !updated.cover_custom {
        let bytes = cover::render_cover(&updated.title)?;
        updated = write_cover(state, id, &bytes, false, true).await?;
    }
    if updated.feed_enabled && !updated.feed_complete() {
        updated = shows::set_feed_enabled(&state.pool, id, false).await?;
    }
    Ok(updated)
}

pub async fn upload_cover(state: &AppState, id: i64, bytes: &[u8]) -> AppResult<Show> {
    shows::get(&state.pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let bytes = cover::normalize_upload(bytes)?;
    write_cover(state, id, &bytes, true, true).await
}
