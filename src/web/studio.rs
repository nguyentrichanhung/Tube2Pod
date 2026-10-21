use crate::db::{episodes, shows};
use crate::error::{AppError, AppResult};
use crate::models::Episode;
use crate::services::ytdlp;
use crate::state::AppState;
use crate::web::csrf::{self, CsrfToken};
use askama::Template;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Json, Redirect};
use axum::Extension;
use axum::Form;
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

#[derive(Template)]
#[template(path = "studio.html")]
pub struct StudioTemplate {
    csrf: String,
    shows: Vec<crate::models::Show>,
    recent: Vec<Episode>,
    selected_show: Option<i64>,
}

#[derive(Template)]
#[template(path = "partials/queue.html")]
pub struct QueueTemplate {
    episodes: Vec<Episode>,
}

#[derive(Debug, Deserialize)]
pub struct StudioQuery {
    pub show: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct CreateForm {
    pub _csrf: String,
    pub url: String,
    pub show_id: i64,
    pub minutes: i64,
}

#[derive(Debug, Deserialize)]
pub struct PreviewForm {
    pub _csrf: String,
    pub url: String,
}

pub async fn index(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Query(query): Query<StudioQuery>,
) -> AppResult<Html<String>> {
    let shows = shows::list(&state.pool).await?;
    let recent = episodes::list(
        &state.pool,
        &episodes::EpisodeFilter {
            limit: Some(8),
            ..episodes::EpisodeFilter::default()
        },
    )
    .await?;
    Ok(Html(
        StudioTemplate {
            csrf: token.0,
            shows,
            recent,
            selected_show: query.show,
        }
        .render()?,
    ))
}

pub async fn create(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Form(form): Form<CreateForm>,
) -> AppResult<Redirect> {
    csrf::verify(&token, &Default::default(), &form._csrf)?;
    validate_minutes(form.minutes)?;
    let video = ytdlp::validate_url(&form.url)?;
    let show = shows::get(&state.pool, form.show_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if let Some(existing) = episodes::get_by_video_id(&state.pool, &video.id).await? {
        return Ok(Redirect::to(&format!("/episodes/{}", existing.uuid)));
    }
    let episode = episodes::create(
        &state.pool,
        episodes::NewEpisode {
            show_id: show.id,
            video_id: video.id.clone(),
            source_url: format!("https://www.youtube.com/watch?v={}", video.id),
            source_title: "Đang tải thông tin…".into(),
            source_channel: String::new(),
            source_duration_s: None,
            source_thumbnail_url: None,
            source_upload_date: None,
            language: show.language.clone(),
            target_minutes: form.minutes,
        },
    )
    .await?;
    state.notify.notify_one();
    Ok(Redirect::to(&format!("/episodes/{}", episode.uuid)))
}

pub async fn preview(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Form(form): Form<PreviewForm>,
) -> AppResult<Json<serde_json::Value>> {
    csrf::verify(&token, &Default::default(), &form._csrf)?;
    let video = ytdlp::validate_url(&form.url)?;
    let meta = tokio::time::timeout(Duration::from_secs(15), ytdlp::probe(&state.cfg, &video))
        .await
        .map_err(|_| AppError::Upstream("preview timed out".into()))?
        .map_err(|e| AppError::Upstream(e.to_string()))?;
    let language = ytdlp::source_language(&meta).ok();
    let has_captions = language
        .and_then(|lang| ytdlp::choose_caption(&meta, lang))
        .is_some();
    Ok(Json(json!({
        "ok": true,
        "title": meta.title,
        "channel": meta.channel,
        "duration_s": meta.duration,
        "language": language,
        "has_captions": has_captions,
        "thumbnail": meta.thumbnail,
    })))
}

pub async fn queue(State(state): State<AppState>) -> AppResult<impl IntoResponse> {
    let active = episodes::list(
        &state.pool,
        &episodes::EpisodeFilter {
            status: Some(crate::models::status::RUNNING.into()),
            limit: Some(20),
            ..episodes::EpisodeFilter::default()
        },
    )
    .await?;
    let recent = episodes::list(
        &state.pool,
        &episodes::EpisodeFilter {
            limit: Some(10),
            ..episodes::EpisodeFilter::default()
        },
    )
    .await?;
    let mut episodes = active;
    for episode in recent {
        if !episodes
            .iter()
            .any(|existing| existing.uuid == episode.uuid)
        {
            episodes.push(episode);
        }
    }
    Ok(Html(QueueTemplate { episodes }.render()?))
}

fn validate_minutes(minutes: i64) -> AppResult<()> {
    if matches!(minutes, 5 | 10 | 15 | 20) {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "Thời lượng phải là 5, 10, 15 hoặc 20 phút.".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_accepts_the_four_length_presets() {
        assert!(validate_minutes(5).is_ok());
        assert!(validate_minutes(10).is_ok());
        assert!(validate_minutes(15).is_ok());
        assert!(validate_minutes(20).is_ok());
        assert!(validate_minutes(7).is_err());
    }
}
