use crate::db::{episodes, settings, shows};
use crate::error::{AppError, AppResult};
use crate::models::{Episode, Show};
use crate::services::cover;
use crate::state::AppState;
use crate::web::csrf::{self, CsrfToken};
use crate::web::library::LibraryRow;
use askama::Template;
use axum::extract::{Multipart, Path, Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, Redirect};
use axum::Extension;
use axum::Form;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct ChecklistItem {
    pub label: &'static str,
    pub ok: bool,
}

pub fn feed_checklist(show: &Show) -> Vec<ChecklistItem> {
    vec![
        ChecklistItem {
            label: "Tên show",
            ok: !show.title.trim().is_empty(),
        },
        ChecklistItem {
            label: "Mô tả",
            ok: !show.description.trim().is_empty(),
        },
        ChecklistItem {
            label: "Danh mục",
            ok: !show.category.trim().is_empty(),
        },
        ChecklistItem {
            label: "Tên chủ sở hữu",
            ok: !show.owner_name.trim().is_empty(),
        },
        ChecklistItem {
            label: "Email chủ sở hữu",
            ok: !show.owner_email.trim().is_empty(),
        },
        ChecklistItem {
            label: "Ảnh bìa",
            ok: show
                .cover_path
                .as_deref()
                .is_some_and(|path| !path.trim().is_empty()),
        },
    ]
}

pub struct ShowCard {
    pub show: Show,
    pub episode_count: i64,
    pub last_episode: Option<String>,
}

#[derive(Template)]
#[template(path = "shows.html")]
pub struct ShowsTemplate {
    csrf: String,
    cards: Vec<ShowCard>,
}

#[derive(Template)]
#[template(path = "show_detail.html")]
pub struct ShowDetailTemplate {
    csrf: String,
    show: Show,
    tab: String,
    rows: Vec<LibraryRow>,
    episode_count: usize,
    checklist: Vec<ChecklistItem>,
    can_enable: bool,
    feed_url: String,
    cover_url: String,
}

#[derive(Debug, Deserialize)]
pub struct CreateShowForm {
    pub _csrf: String,
    pub title: String,
    pub language: String,
    pub category: String,
    pub default_minutes: i64,
    pub description: String,
}

#[derive(Debug, Deserialize)]
pub struct ShowEditForm {
    pub _csrf: String,
    pub title: String,
    pub author: String,
    pub description: String,
    pub language: String,
    pub website_url: String,
    pub category: String,
    pub owner_name: String,
    pub owner_email: String,
    pub explicit: Option<String>,
    pub default_minutes: i64,
    pub auto_publish: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ConfirmForm {
    pub _csrf: String,
    pub confirm_title: String,
}

#[derive(Debug, Deserialize)]
pub struct FeedToggleForm {
    pub _csrf: String,
    pub enabled: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TabQuery {
    pub tab: Option<String>,
}

pub async fn index(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
) -> AppResult<Html<String>> {
    let shows = shows::list(&state.pool).await?;
    let mut cards = Vec::new();
    for show in shows {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM episodes WHERE show_id = ?1")
            .bind(show.id)
            .fetch_one(&state.pool)
            .await?;
        let last_episode: Option<String> = sqlx::query_scalar(
            "SELECT source_title FROM episodes WHERE show_id = ?1
             ORDER BY created_at DESC, uuid DESC LIMIT 1",
        )
        .bind(show.id)
        .fetch_optional(&state.pool)
        .await?;
        cards.push(ShowCard {
            show,
            episode_count: count,
            last_episode,
        });
    }
    Ok(Html(
        ShowsTemplate {
            csrf: token.0,
            cards,
        }
        .render()?,
    ))
}

pub async fn create(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Form(form): Form<CreateShowForm>,
) -> AppResult<Redirect> {
    csrf::verify(&token, &Default::default(), &form._csrf)?;
    validate_language(&form.language)?;
    if form.title.trim().is_empty() {
        return Err(AppError::BadRequest("Tên show là bắt buộc.".into()));
    }
    let owner_name = settings::get_or(&state.pool, "owner_name", "").await?;
    let owner_email = settings::get_or(&state.pool, "owner_email", "").await?;
    let auto_publish =
        settings::get_or(&state.pool, "auto_publish_default", "true").await? == "true";
    let explicit = settings::get_or(&state.pool, "default_explicit", "false").await? == "true";
    let show = shows::create(
        &state.pool,
        shows::NewShow {
            title: form.title,
            description: form.description,
            category: form.category,
            owner_name,
            owner_email,
            explicit,
            language: form.language,
            default_minutes: form.default_minutes,
            auto_publish,
            ..shows::NewShow::default()
        },
    )
    .await?;
    let cover_bytes = cover::render_cover(&show.title)?;
    let path = state
        .cfg
        .data_dir
        .join("shows")
        .join(show.id.to_string())
        .join("cover.jpg");
    tokio::fs::create_dir_all(path.parent().unwrap()).await?;
    tokio::fs::write(&path, cover_bytes).await?;
    shows::set_cover(
        &state.pool,
        show.id,
        &path.display().to_string(),
        false,
        false,
    )
    .await?;
    Ok(Redirect::to(&format!("/shows/{id}", id = show.id)))
}

pub async fn detail(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Path(id): Path<i64>,
    Query(query): Query<TabQuery>,
) -> AppResult<Html<String>> {
    let show = shows::get(&state.pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let tab = match query.tab.as_deref() {
        Some("info") => "info",
        Some("feed") => "feed",
        _ => "episodes",
    };
    let episodes = episodes::list_for_show(&state.pool, id, 200).await?;
    let rows: Vec<LibraryRow> = episodes
        .into_iter()
        .map(|episode| LibraryRow {
            episode,
            show_title: show.title.clone(),
        })
        .collect();
    let episode_count = rows.len();
    let feed_url = format!("{}/feed/{}.xml", state.cfg.public_base_url, show.feed_token);
    let cover_url = show.cover_url(&state.cfg.public_base_url);
    let checklist = feed_checklist(&show);
    let can_enable = checklist.iter().all(|item| item.ok);
    Ok(Html(
        ShowDetailTemplate {
            csrf: token.0,
            show,
            tab: tab.into(),
            rows,
            episode_count,
            checklist,
            can_enable,
            feed_url,
            cover_url,
        }
        .render()?,
    ))
}

pub async fn update(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Path(id): Path<i64>,
    Form(form): Form<ShowEditForm>,
) -> AppResult<Redirect> {
    csrf::verify(&token, &Default::default(), &form._csrf)?;
    validate_language(&form.language)?;
    let current = shows::get(&state.pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let title_changed = current.title != form.title.trim();
    let updated = shows::update(
        &state.pool,
        id,
        shows::ShowPatch {
            title: Some(form.title),
            author: Some(form.author),
            description: Some(form.description),
            category: Some(form.category),
            owner_name: Some(form.owner_name),
            owner_email: Some(form.owner_email),
            explicit: Some(form.explicit.is_some()),
            language: Some(form.language),
            website_url: Some(form.website_url),
            default_minutes: Some(form.default_minutes),
            auto_publish: Some(form.auto_publish.is_some()),
            voice_preset_json: None,
        },
    )
    .await?;
    if title_changed && !updated.cover_custom {
        let bytes = cover::render_cover(&updated.title)?;
        let path = state
            .cfg
            .data_dir
            .join("shows")
            .join(updated.id.to_string())
            .join("cover.jpg");
        tokio::fs::create_dir_all(path.parent().unwrap()).await?;
        tokio::fs::write(&path, bytes).await?;
        shows::set_cover(
            &state.pool,
            updated.id,
            &path.display().to_string(),
            false,
            true,
        )
        .await?;
    }
    Ok(Redirect::to(&format!("/shows/{id}?tab=info")))
}

pub async fn upload_cover(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    mut multipart: Multipart,
) -> AppResult<Redirect> {
    let show = shows::get(&state.pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let mut bytes = None;
    let mut form_token = String::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        let name = field.name().map(str::to_string);
        match name.as_deref() {
            Some("_csrf") => {
                form_token = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(e.to_string()))?;
            }
            Some("cover") => {
                bytes = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| AppError::BadRequest(e.to_string()))?,
                );
            }
            _ => {}
        }
    }
    csrf::verify(&token, &headers, &form_token)?;
    let bytes = bytes.ok_or_else(|| AppError::BadRequest("Thiếu file ảnh bìa.".into()))?;
    let normalized = cover::normalize_upload(&bytes)?;
    let path = state
        .cfg
        .data_dir
        .join("shows")
        .join(show.id.to_string())
        .join("cover.jpg");
    tokio::fs::create_dir_all(path.parent().unwrap()).await?;
    tokio::fs::write(&path, normalized).await?;
    shows::set_cover(
        &state.pool,
        show.id,
        &path.display().to_string(),
        true,
        true,
    )
    .await?;
    Ok(Redirect::to(&format!("/shows/{id}?tab=info")))
}

pub async fn toggle_feed(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Path(id): Path<i64>,
    Form(form): Form<FeedToggleForm>,
) -> AppResult<Redirect> {
    csrf::verify(&token, &Default::default(), &form._csrf)?;
    let show = shows::get(&state.pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    let enabled = form.enabled.is_some();
    if enabled && !show.feed_complete() {
        return Err(AppError::BadRequest(
            "Feed còn thiếu thông tin bắt buộc.".into(),
        ));
    }
    shows::set_feed_enabled(&state.pool, show.id, enabled).await?;
    Ok(Redirect::to(&format!("/shows/{id}?tab=feed")))
}

pub async fn rotate_feed(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Path(id): Path<i64>,
    Form(form): Form<ConfirmForm>,
) -> AppResult<Redirect> {
    csrf::verify(&token, &Default::default(), &form._csrf)?;
    let show = shows::get(&state.pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    if show.title != form.confirm_title.trim() {
        return Err(AppError::BadRequest("Tên show xác nhận không khớp.".into()));
    }
    shows::rotate_feed_token(&state.pool, id).await?;
    Ok(Redirect::to(&format!("/shows/{id}?tab=feed")))
}

pub async fn publish_ready(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Path(id): Path<i64>,
    Form(form): Form<ConfirmForm>,
) -> AppResult<Redirect> {
    csrf::verify(&token, &Default::default(), &form._csrf)?;
    episodes::publish_ready(&state.pool, id).await?;
    state.notify.notify_one();
    Ok(Redirect::to(&format!("/shows/{id}?tab=feed")))
}

pub async fn delete(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Path(id): Path<i64>,
    Form(form): Form<ConfirmForm>,
) -> AppResult<Redirect> {
    csrf::verify(&token, &Default::default(), &form._csrf)?;
    let show = shows::get(&state.pool, id)
        .await?
        .ok_or(AppError::NotFound)?;
    if show.title != form.confirm_title.trim() {
        return Err(AppError::BadRequest("Tên show xác nhận không khớp.".into()));
    }
    let mut offset = 0;
    loop {
        let batch = episodes::list(
            &state.pool,
            &episodes::EpisodeFilter {
                show_id: Some(id),
                limit: Some(500),
                offset: Some(offset),
                ..episodes::EpisodeFilter::default()
            },
        )
        .await?;
        if batch.is_empty() {
            break;
        }
        let batch_len = batch.len();
        for episode in &batch {
            let _ = episodes::cancel(&state.pool, &episode.uuid).await;
            purge_episode_files(&state.cfg.data_dir, episode).await;
        }
        offset += batch_len as i64;
        if batch_len < 500 {
            break;
        }
    }
    shows::delete(&state.pool, id).await?;
    let _ = tokio::fs::remove_dir_all(state.cfg.data_dir.join("shows").join(id.to_string())).await;
    Ok(Redirect::to("/shows"))
}

pub async fn purge_episode_files(data_dir: &std::path::Path, episode: &Episode) {
    if let Some(path) = &episode.audio_path {
        let _ = tokio::fs::remove_file(path).await;
    }
    if let Some(path) = &episode.transcript_path {
        let _ = tokio::fs::remove_file(path).await;
    }
    let _ = tokio::fs::remove_dir_all(data_dir.join("tmp").join(&episode.uuid)).await;
    let audio_dir = data_dir.join("audio");
    if let Ok(mut entries) = tokio::fs::read_dir(&audio_dir).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(&episode.uuid)
            {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
    }
}

fn validate_language(language: &str) -> AppResult<()> {
    if matches!(language, "vi" | "en" | "ko") {
        Ok(())
    } else {
        Err(AppError::BadRequest("Chỉ hỗ trợ vi, en, ko.".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::{episodes, shows};
    use crate::models::{stage, status, Show};
    use crate::state::AppState;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Method, Request, StatusCode};
    use image::GenericImageView;
    use std::sync::Arc;
    use tokio::sync::Notify;
    use tower::ServiceExt;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    struct Ctx {
        state: AppState,
        _dir: tempfile::TempDir,
    }

    async fn ctx() -> Ctx {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let cfg = Config::from_lookup(move |key| match key {
            "PUBLIC_BASE_URL" => Some("https://pod.example.ts.net:8443".into()),
            "OMNIVOICE_TAILNET_USER" => Some("owner".into()),
            "DATA_DIR" => Some(path.display().to_string()),
            _ => None,
        })
        .unwrap();
        Ctx {
            state: AppState {
                pool: crate::db::test_pool().await,
                cfg: Arc::new(cfg),
                notify: Arc::new(Notify::new()),
            },
            _dir: dir,
        }
    }

    fn show() -> Show {
        Show {
            id: 1,
            title: "Show".into(),
            author: "".into(),
            description: "Mô tả".into(),
            category: "Technology".into(),
            owner_name: "Owner".into(),
            owner_email: "owner@example.com".into(),
            explicit: false,
            language: "vi".into(),
            website_url: "".into(),
            cover_path: Some("/data/cover.jpg".into()),
            cover_version: 1,
            cover_custom: false,
            feed_token: "token".into(),
            feed_enabled: false,
            default_minutes: 10,
            auto_publish: true,
            voice_preset_json: "{}".into(),
            created_at: 0,
            updated_at: 0,
        }
    }

    async fn complete_show(context: &Ctx, title: &str) -> Show {
        let show = shows::create(
            &context.state.pool,
            shows::NewShow {
                title: title.into(),
                description: "Mô tả".into(),
                owner_name: "Owner".into(),
                owner_email: "owner@example.com".into(),
                ..shows::NewShow::default()
            },
        )
        .await
        .unwrap();
        shows::set_cover(&context.state.pool, show.id, "/tmp/cover.jpg", false, false)
            .await
            .unwrap()
    }

    fn form(pairs: &[(&str, &str)]) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in pairs {
            serializer.append_pair(key, value);
        }
        serializer.finish()
    }

    async fn send(
        context: &Ctx,
        method: Method,
        path: &str,
        content_type: &str,
        body: Vec<u8>,
    ) -> axum::response::Response {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::COOKIE, format!("t2p_csrf={TOKEN}"));
        crate::web::admin_router(context.state.clone())
            .oneshot(request.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }

    async fn post_form(context: &Ctx, path: &str, body: String) -> axum::response::Response {
        send(
            context,
            Method::POST,
            path,
            "application/x-www-form-urlencoded",
            body.into_bytes(),
        )
        .await
    }

    fn edit_form(title: &str) -> String {
        form(&[
            ("_csrf", TOKEN),
            ("title", title),
            ("author", "Author"),
            ("description", "Mô tả"),
            ("language", "vi"),
            ("website_url", ""),
            ("category", "Technology"),
            ("owner_name", "Owner"),
            ("owner_email", "owner@example.com"),
            ("default_minutes", "10"),
        ])
    }

    #[test]
    fn checklist_requires_every_apple_field() {
        assert!(feed_checklist(&show()).iter().all(|item| item.ok));
        let mut missing = show();
        missing.owner_email.clear();
        assert!(feed_checklist(&missing)
            .iter()
            .any(|item| !item.ok && item.label == "Email chủ sở hữu"));
    }

    #[tokio::test]
    async fn create_generates_a_cover_and_inherits_owner_settings() {
        let context = ctx().await;
        crate::db::settings::set_many(
            &context.state.pool,
            &[
                ("owner_name", "Chủ show"),
                ("owner_email", "owner@example.com"),
                ("auto_publish_default", "false"),
                ("default_explicit", "true"),
            ],
        )
        .await
        .unwrap();

        let response = post_form(
            &context,
            "/shows",
            form(&[
                ("_csrf", TOKEN),
                ("title", "Show mới"),
                ("description", "Mô tả"),
                ("language", "vi"),
                ("category", "Technology"),
                ("default_minutes", "10"),
            ]),
        )
        .await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let created = shows::list(&context.state.pool).await.unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(location, format!("/shows/{}", created[0].id));
        assert_eq!(created[0].owner_name, "Chủ show");
        assert_eq!(created[0].owner_email, "owner@example.com");
        assert!(!created[0].auto_publish);
        assert!(created[0].explicit);
        assert!(!created[0].feed_enabled);
        let cover = created[0].cover_path.as_deref().unwrap();
        assert!(std::path::Path::new(cover).is_file());
    }

    #[tokio::test]
    async fn create_rejects_unsupported_language_and_blank_title() {
        let context = ctx().await;

        let bad_language = post_form(
            &context,
            "/shows",
            form(&[
                ("_csrf", TOKEN),
                ("title", "Show"),
                ("description", "Mô tả"),
                ("language", "ja"),
                ("category", "Technology"),
                ("default_minutes", "10"),
            ]),
        )
        .await;
        assert_eq!(bad_language.status(), StatusCode::BAD_REQUEST);

        let blank_title = post_form(
            &context,
            "/shows",
            form(&[
                ("_csrf", TOKEN),
                ("title", "  "),
                ("description", "Mô tả"),
                ("language", "vi"),
                ("category", "Technology"),
                ("default_minutes", "10"),
            ]),
        )
        .await;
        assert_eq!(blank_title.status(), StatusCode::BAD_REQUEST);
        assert!(shows::list(&context.state.pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn toggle_feed_refuses_incomplete_shows_and_enables_complete_ones() {
        let context = ctx().await;
        let incomplete = shows::create(
            &context.state.pool,
            shows::NewShow {
                title: "Chưa đủ".into(),
                ..shows::NewShow::default()
            },
        )
        .await
        .unwrap();
        let path = format!("/shows/{}/feed/toggle", incomplete.id);

        let rejected = post_form(
            &context,
            &path,
            form(&[("_csrf", TOKEN), ("enabled", "on")]),
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        assert!(
            !shows::get(&context.state.pool, incomplete.id)
                .await
                .unwrap()
                .unwrap()
                .feed_enabled
        );

        let complete = complete_show(&context, "Đã đủ").await;
        let accepted = post_form(
            &context,
            &format!("/shows/{}/feed/toggle", complete.id),
            form(&[("_csrf", TOKEN), ("enabled", "on")]),
        )
        .await;
        assert_eq!(accepted.status(), StatusCode::SEE_OTHER);
        assert!(
            shows::get(&context.state.pool, complete.id)
                .await
                .unwrap()
                .unwrap()
                .feed_enabled
        );

        let disabled = post_form(
            &context,
            &format!("/shows/{}/feed/toggle", complete.id),
            form(&[("_csrf", TOKEN)]),
        )
        .await;
        assert_eq!(disabled.status(), StatusCode::SEE_OTHER);
        assert!(
            !shows::get(&context.state.pool, complete.id)
                .await
                .unwrap()
                .unwrap()
                .feed_enabled
        );
    }

    #[tokio::test]
    async fn rotate_feed_requires_the_exact_show_title() {
        let context = ctx().await;
        let show = complete_show(&context, "Tên show").await;
        let path = format!("/shows/{}/feed/rotate", show.id);

        let rejected = post_form(
            &context,
            &path,
            form(&[("_csrf", TOKEN), ("confirm_title", "sai tên")]),
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            shows::get(&context.state.pool, show.id)
                .await
                .unwrap()
                .unwrap()
                .feed_token,
            show.feed_token
        );

        let accepted = post_form(
            &context,
            &path,
            form(&[("_csrf", TOKEN), ("confirm_title", "Tên show")]),
        )
        .await;
        assert_eq!(accepted.status(), StatusCode::SEE_OTHER);
        assert_ne!(
            shows::get(&context.state.pool, show.id)
                .await
                .unwrap()
                .unwrap()
                .feed_token,
            show.feed_token
        );
    }

    #[tokio::test]
    async fn delete_requires_the_exact_title_and_purges_episode_files() {
        let context = ctx().await;
        let show = complete_show(&context, "Xoá tôi").await;
        let episode = episodes::create(
            &context.state.pool,
            episodes::NewEpisode {
                show_id: show.id,
                video_id: "delete00001".into(),
                source_url: "https://www.youtube.com/watch?v=delete00001".into(),
                source_title: "Tập".into(),
                source_channel: String::new(),
                source_duration_s: None,
                source_thumbnail_url: None,
                source_upload_date: None,
                language: "vi".into(),
                target_minutes: 10,
            },
        )
        .await
        .unwrap();
        let audio_dir = context.state.cfg.data_dir.join("audio");
        tokio::fs::create_dir_all(&audio_dir).await.unwrap();
        let audio = audio_dir.join(format!("{}-1.mp3", episode.uuid));
        tokio::fs::write(&audio, b"audio").await.unwrap();
        let path = format!("/shows/{}", show.id);

        let rejected = send(
            &context,
            Method::DELETE,
            &path,
            "application/x-www-form-urlencoded",
            form(&[("_csrf", TOKEN), ("confirm_title", "sai")]).into_bytes(),
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        assert!(shows::get(&context.state.pool, show.id)
            .await
            .unwrap()
            .is_some());

        let accepted = send(
            &context,
            Method::DELETE,
            &path,
            "application/x-www-form-urlencoded",
            form(&[("_csrf", TOKEN), ("confirm_title", "Xoá tôi")]).into_bytes(),
        )
        .await;
        assert_eq!(accepted.status(), StatusCode::SEE_OTHER);
        assert!(shows::get(&context.state.pool, show.id)
            .await
            .unwrap()
            .is_none());
        assert!(episodes::get(&context.state.pool, &episode.uuid)
            .await
            .unwrap()
            .is_none());
        assert!(!audio.exists());
    }

    #[tokio::test]
    async fn renaming_regenerates_generated_covers_but_keeps_custom_ones() {
        let context = ctx().await;
        let generated = complete_show(&context, "Tên cũ").await;
        let response = send(
            &context,
            Method::PATCH,
            &format!("/shows/{}", generated.id),
            "application/x-www-form-urlencoded",
            edit_form("Tên mới").into_bytes(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let generated = shows::get(&context.state.pool, generated.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(generated.title, "Tên mới");
        assert_eq!(generated.cover_version, 2);
        assert!(!generated.cover_custom);
        assert!(std::path::Path::new(generated.cover_path.as_deref().unwrap()).is_file());

        let custom = complete_show(&context, "Bìa riêng").await;
        let custom = shows::set_cover(
            &context.state.pool,
            custom.id,
            "/tmp/custom.jpg",
            true,
            true,
        )
        .await
        .unwrap();
        let response = send(
            &context,
            Method::PATCH,
            &format!("/shows/{}", custom.id),
            "application/x-www-form-urlencoded",
            edit_form("Tên khác").into_bytes(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let custom = shows::get(&context.state.pool, custom.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(custom.title, "Tên khác");
        assert_eq!(custom.cover_version, 2);
        assert!(custom.cover_custom);
        assert_eq!(custom.cover_path.as_deref(), Some("/tmp/custom.jpg"));
    }

    #[tokio::test]
    async fn cover_upload_normalizes_the_image_and_marks_it_custom() {
        let context = ctx().await;
        let show = shows::create(
            &context.state.pool,
            shows::NewShow {
                title: "Show".into(),
                ..shows::NewShow::default()
            },
        )
        .await
        .unwrap();
        let boundary = "t2p-boundary";
        let image = image::RgbImage::from_pixel(1400, 1400, image::Rgb([20, 120, 200]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(image)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"_csrf\"\r\n\r\n{TOKEN}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"cover\"; filename=\"cover.png\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .into_bytes();
        body.extend_from_slice(&png);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());

        let response = send(
            &context,
            Method::POST,
            &format!("/shows/{}/cover", show.id),
            &format!("multipart/form-data; boundary={boundary}"),
            body,
        )
        .await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let show = shows::get(&context.state.pool, show.id)
            .await
            .unwrap()
            .unwrap();
        assert!(show.cover_custom);
        assert_eq!(show.cover_version, 2);
        let bytes = std::fs::read(show.cover_path.as_deref().unwrap()).unwrap();
        assert_eq!(&bytes[..2], &[0xff, 0xd8]);
        assert_eq!(
            image::load_from_memory(&bytes).unwrap().dimensions(),
            (3000, 3000)
        );
    }

    #[tokio::test]
    async fn publish_ready_publishes_all_ready_episodes_for_the_show() {
        let context = ctx().await;
        let show = complete_show(&context, "Show").await;
        let episode = episodes::create(
            &context.state.pool,
            episodes::NewEpisode {
                show_id: show.id,
                video_id: "publish0001".into(),
                source_url: "https://www.youtube.com/watch?v=publish0001".into(),
                source_title: "Tập".into(),
                source_channel: String::new(),
                source_duration_s: None,
                source_thumbnail_url: None,
                source_upload_date: None,
                language: "vi".into(),
                target_minutes: 10,
            },
        )
        .await
        .unwrap();
        episodes::finish(
            &context.state.pool,
            &episode.uuid,
            stage::READY,
            status::READY,
            false,
        )
        .await
        .unwrap();

        let response = post_form(
            &context,
            &format!("/shows/{}/publish-ready", show.id),
            form(&[("_csrf", TOKEN), ("confirm_title", "Show")]),
        )
        .await;

        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            episodes::get(&context.state.pool, &episode.uuid)
                .await
                .unwrap()
                .unwrap()
                .status,
            status::PUBLISHED
        );
    }

    #[tokio::test]
    async fn detail_renders_checklist_and_refuses_missing_shows() {
        let context = ctx().await;
        let show = complete_show(&context, "Show").await;
        let request = Request::builder()
            .uri(format!("/shows/{}?tab=feed", show.id))
            .header(header::COOKIE, format!("t2p_csrf={TOKEN}"));
        let response = crate::web::admin_router(context.state.clone())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Email chủ sở hữu"));
        assert!(html.contains("Feed URL"));

        let missing = crate::web::admin_router(context.state.clone())
            .oneshot(
                Request::builder()
                    .uri("/shows/9999")
                    .header(header::COOKIE, format!("t2p_csrf={TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    }
}
