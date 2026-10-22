use crate::db::episodes::{self, EpisodeFilter};
use crate::db::shows;
use crate::error::{AppError, AppResult};
use crate::models::{status, Episode, Show};
use crate::services::omnivoice::OmnivoiceClient;
use crate::state::AppState;
use crate::web::csrf::{self, CsrfToken};
use crate::web::shows::purge_episode_files;
use askama::Template;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect};
use axum::Extension;
use serde::Deserialize;

const STATUSES: [&str; 6] = [
    status::QUEUED,
    status::RUNNING,
    status::READY,
    status::PUBLISHED,
    status::FAILED,
    status::CANCELLED,
];

fn empty_as_none<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    let value = Option::<String>::deserialize(deserializer)?;
    match value.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(raw) => raw.parse::<T>().map(Some).map_err(serde::de::Error::custom),
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct LibraryQuery {
    #[serde(default, deserialize_with = "empty_as_none")]
    pub show_id: Option<i64>,
    #[serde(default, deserialize_with = "empty_as_none")]
    pub status: Option<String>,
    #[serde(default, deserialize_with = "empty_as_none")]
    pub q: Option<String>,
    #[serde(default, deserialize_with = "empty_as_none")]
    pub deleted: Option<i64>,
    #[serde(default, deserialize_with = "empty_as_none")]
    pub skipped: Option<i64>,
}

impl LibraryQuery {
    pub fn filter(&self) -> EpisodeFilter {
        EpisodeFilter {
            show_id: self.show_id,
            status: self
                .status
                .as_deref()
                .map(str::trim)
                .filter(|value| STATUSES.contains(value))
                .map(str::to_string),
            query: self
                .q
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            limit: Some(200),
            offset: None,
        }
    }
}

pub fn deletable_status(value: &str) -> bool {
    matches!(
        value,
        status::PUBLISHED | status::READY | status::FAILED | status::CANCELLED
    )
}

pub struct LibraryRow {
    pub episode: Episode,
    pub show_title: String,
}

impl LibraryRow {
    pub fn deletable(&self) -> bool {
        deletable_status(&self.episode.status)
    }
}

#[derive(Template)]
#[template(path = "library.html")]
pub struct LibraryTemplate {
    csrf: String,
    rows: Vec<LibraryRow>,
    shows: Vec<Show>,
    selected_show: Option<i64>,
    selected_status: String,
    query: String,
    selectable: bool,
    bulk_message: Option<String>,
}

impl LibraryTemplate {
    pub fn status_selected(&self, status: &str) -> bool {
        self.selected_status == status
    }

    pub fn status_label(&self, status: &str) -> &'static str {
        crate::models::status_label_for(status)
    }
}

#[derive(Template)]
#[template(path = "partials/episode_rows.html")]
pub struct RowsTemplate {
    rows: Vec<LibraryRow>,
    selectable: bool,
}

pub async fn index(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Query(query): Query<LibraryQuery>,
) -> AppResult<Html<String>> {
    let filter = query.filter();
    let selected_status = filter.status.clone().unwrap_or_default();
    let (rows, shows) = load_rows(&state, &filter).await?;
    let bulk_message = query.deleted.map(|deleted| {
        let skipped = query.skipped.unwrap_or(0);
        if skipped > 0 {
            format!("Đã xoá {deleted} tập, bỏ qua {skipped} tập.")
        } else {
            format!("Đã xoá {deleted} tập.")
        }
    });
    Ok(Html(
        LibraryTemplate {
            csrf: token.0,
            rows,
            shows,
            selected_show: query.show_id,
            selected_status,
            query: query.q.clone().unwrap_or_default(),
            selectable: true,
            bulk_message,
        }
        .render()?,
    ))
}

pub async fn rows(
    State(state): State<AppState>,
    Query(query): Query<LibraryQuery>,
) -> AppResult<impl IntoResponse> {
    let (rows, _) = load_rows(&state, &query.filter()).await?;
    Ok(Html(
        RowsTemplate {
            rows,
            selectable: true,
        }
        .render()?,
    ))
}

const BULK_DELETE_MAX: usize = 200;

fn form_values(body: &[u8], key: &str) -> Vec<String> {
    url::form_urlencoded::parse(body)
        .filter(|(name, _)| name == key)
        .map(|(_, value)| value.into_owned())
        .collect()
}

fn form_value(body: &[u8], key: &str) -> Option<String> {
    form_values(body, key).into_iter().next()
}

fn trimmed_form_value(body: &[u8], key: &str) -> Option<String> {
    form_value(body, key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub async fn bulk_delete(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Redirect> {
    let provided = form_value(&body, "_csrf").unwrap_or_default();
    csrf::verify(&token, &headers, &provided)?;
    let confirm = form_value(&body, "confirm").unwrap_or_default();
    if !confirm.trim().eq_ignore_ascii_case("XOA") {
        return Err(AppError::BadRequest("Gõ XOA để xác nhận xoá.".into()));
    }
    let filter_show_id =
        trimmed_form_value(&body, "show_id").and_then(|value| value.parse::<i64>().ok());
    let filter_status = trimmed_form_value(&body, "status");
    let filter_q = trimmed_form_value(&body, "q");
    let mut seen = std::collections::HashSet::new();
    let uuids: Vec<String> = form_values(&body, "uuid")
        .into_iter()
        .map(|uuid| uuid.trim().to_ascii_lowercase())
        .filter(|uuid| !uuid.is_empty() && seen.insert(uuid.clone()))
        .collect();
    if uuids.is_empty() {
        return Err(AppError::BadRequest("Chưa chọn tập nào.".into()));
    }
    if uuids.len() > BULK_DELETE_MAX {
        return Err(AppError::BadRequest(
            "Chỉ xoá được tối đa 200 tập mỗi lần.".into(),
        ));
    }
    let mut take_ids = Vec::new();
    let mut deleted = 0usize;
    let mut skipped = 0usize;
    for uuid in uuids {
        let Some(episode) = episodes::delete_if_terminal(&state.pool, &uuid).await? else {
            skipped += 1;
            continue;
        };
        if let Some(take_id) = episode.active_take_id {
            take_ids.push(take_id);
        }
        purge_episode_files(&state.cfg.data_dir, &episode).await;
        deleted += 1;
    }
    if !take_ids.is_empty() {
        let cfg = state.cfg.clone();
        tokio::spawn(async move {
            let client = OmnivoiceClient::new(&cfg.omnivoice);
            for take_id in take_ids {
                let _ = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    client.delete_speech(take_id),
                )
                .await;
            }
        });
    }
    if deleted > 0 {
        state.notify.notify_one();
    }
    Ok(library_redirect(
        filter_show_id,
        filter_status.as_deref(),
        filter_q.as_deref(),
        deleted,
        skipped,
    ))
}

fn library_redirect(
    show_id: Option<i64>,
    status: Option<&str>,
    q: Option<&str>,
    deleted: usize,
    skipped: usize,
) -> Redirect {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    if let Some(show_id) = show_id {
        serializer.append_pair("show_id", &show_id.to_string());
    }
    if let Some(value) = status.filter(|value| !value.is_empty()) {
        serializer.append_pair("status", value);
    }
    if let Some(value) = q.filter(|value| !value.is_empty()) {
        serializer.append_pair("q", value);
    }
    serializer.append_pair("deleted", &deleted.to_string());
    serializer.append_pair("skipped", &skipped.to_string());
    Redirect::to(&format!("/library?{}", serializer.finish()))
}

async fn load_rows(
    state: &AppState,
    filter: &EpisodeFilter,
) -> AppResult<(Vec<LibraryRow>, Vec<Show>)> {
    let episodes = episodes::list(&state.pool, filter).await?;
    let shows = shows::list(&state.pool).await?;
    Ok((build_rows(episodes, &shows), shows))
}

fn build_rows(episodes: Vec<Episode>, shows: &[Show]) -> Vec<LibraryRow> {
    let names: std::collections::HashMap<i64, String> = shows
        .iter()
        .map(|show| (show.id, show.title.clone()))
        .collect();
    episodes
        .into_iter()
        .map(|episode| LibraryRow {
            show_title: names
                .get(&episode.show_id)
                .cloned()
                .unwrap_or_else(|| "Show đã xoá".into()),
            episode,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::db::episodes;
    use crate::models::{status, Episode};
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Method, Request, StatusCode};
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

    async fn send(context: &Ctx, method: Method, path: &str) -> axum::response::Response {
        crate::web::admin_router(context.state.clone())
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header(header::ACCEPT, "text/html")
                    .header(header::COOKIE, format!("t2p_csrf={TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn body_string(response: axum::response::Response) -> String {
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        String::from_utf8(body.to_vec()).unwrap()
    }

    async fn create_show(context: &Ctx) -> i64 {
        crate::db::shows::create(
            &context.state.pool,
            crate::db::shows::NewShow {
                title: "Show".into(),
                ..crate::db::shows::NewShow::default()
            },
        )
        .await
        .unwrap()
        .id
    }

    async fn create_episode(context: &Ctx, show_id: i64, video_id: &str, title: &str) -> Episode {
        episodes::create(
            &context.state.pool,
            episodes::NewEpisode {
                show_id,
                video_id: video_id.into(),
                source_url: format!("https://www.youtube.com/watch?v={video_id}"),
                source_title: title.into(),
                source_channel: "Channel".into(),
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

    async fn set_status(context: &Ctx, uuid: &str, new_status: &str) {
        sqlx::query("UPDATE episodes SET status = ?1, stage = ?2 WHERE uuid = ?3")
            .bind(new_status)
            .bind(new_status)
            .bind(uuid)
            .execute(&context.state.pool)
            .await
            .unwrap();
    }

    fn form(pairs: &[(&str, &str)]) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in pairs {
            serializer.append_pair(key, value);
        }
        serializer.finish()
    }

    async fn send_form(context: &Ctx, path: &str, body: String) -> axum::response::Response {
        crate::web::admin_router(context.state.clone())
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri(path)
                    .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .header(header::COOKIE, format!("t2p_csrf={TOKEN}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn library_renders_bulk_selection_ui() {
        let context = ctx().await;
        let show_id = create_show(&context).await;
        let published = create_episode(&context, show_id, "pub", "Published").await;
        set_status(&context, &published.uuid, status::PUBLISHED).await;
        let running = create_episode(&context, show_id, "run", "Running").await;
        set_status(&context, &running.uuid, status::RUNNING).await;

        let response = send(&context, Method::GET, "/library").await;
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(html.contains("bulk-form"));
        assert!(html.contains("name=\"uuid\""));
        assert!(html.contains("Gõ XOA"));
        assert!(html.contains("Xoá đã chọn"));
        assert!(html.contains("disabled"));
    }

    #[tokio::test]
    async fn show_detail_rows_are_not_selectable() {
        let context = ctx().await;
        let show_id = create_show(&context).await;
        create_episode(&context, show_id, "one", "One").await;

        let response = send(
            &context,
            Method::GET,
            &format!("/shows/{show_id}?tab=episodes"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(!html.contains("name=\"uuid\""));
    }

    #[test]
    fn deletable_status_covers_finished_work_only() {
        for value in [
            status::PUBLISHED,
            status::READY,
            status::FAILED,
            status::CANCELLED,
        ] {
            assert!(deletable_status(value), "{value} should be deletable");
        }
        for value in [status::QUEUED, status::RUNNING] {
            assert!(!deletable_status(value), "{value} must not be deletable");
        }
    }

    #[tokio::test]
    async fn library_rows_use_labels_and_relative_time() {
        let context = ctx().await;
        let show = crate::db::shows::create(
            &context.state.pool,
            crate::db::shows::NewShow {
                title: "Show".into(),
                ..crate::db::shows::NewShow::default()
            },
        )
        .await
        .unwrap();
        let episode = crate::db::episodes::create(
            &context.state.pool,
            crate::db::episodes::NewEpisode {
                show_id: show.id,
                video_id: "video".into(),
                source_url: "https://www.youtube.com/watch?v=video".into(),
                source_title: "Tập mẫu".into(),
                source_channel: "Kênh".into(),
                source_duration_s: None,
                source_thumbnail_url: None,
                source_upload_date: None,
                language: "vi".into(),
                target_minutes: 10,
            },
        )
        .await
        .unwrap();
        let response = send(&context, Method::GET, "/library").await;
        assert_eq!(response.status(), StatusCode::OK);
        let html = body_string(response).await;
        assert!(html.contains("Đang chờ"));
        assert!(html.contains("vừa xong"));
        assert!(html.contains("aria-label=\"Tiến trình Tập mẫu\""));
        assert!(!html.contains("<td>queued</td>"));
        assert!(!html.contains(&format!("<td>{}</td>", episode.created_at)));
    }

    fn parse_query(raw: &str) -> LibraryQuery {
        let uri: axum::http::Uri = raw.parse().unwrap();
        Query::<LibraryQuery>::try_from_uri(&uri).unwrap().0
    }

    #[test]
    fn builds_filters_from_query() {
        let query = LibraryQuery {
            show_id: Some(3),
            status: Some("failed".into()),
            q: Some("rust".into()),
            deleted: None,
            skipped: None,
        };
        let filter = query.filter();
        assert_eq!(filter.show_id, Some(3));
        assert_eq!(filter.status.as_deref(), Some("failed"));
        assert_eq!(filter.query.as_deref(), Some("rust"));
    }

    #[test]
    fn treats_blank_query_values_as_none() {
        let query = parse_query("/library?show_id=&status=&q=");
        assert_eq!(query.show_id, None);
        assert_eq!(query.status, None);
        assert_eq!(query.q, None);

        let filter = query.filter();
        assert_eq!(filter.show_id, None);
        assert_eq!(filter.status, None);
        assert_eq!(filter.query, None);

        let absent = parse_query("/library");
        assert_eq!(absent.show_id, None);
        assert_eq!(absent.status, None);
        assert_eq!(absent.q, None);
    }

    #[test]
    fn parses_populated_query_values() {
        let query = parse_query("/library?show_id=3&status=failed&q=rust");
        assert_eq!(query.show_id, Some(3));
        assert_eq!(query.status.as_deref(), Some("failed"));
        assert_eq!(query.q.as_deref(), Some("rust"));

        let filter = query.filter();
        assert_eq!(filter.show_id, Some(3));
        assert_eq!(filter.status.as_deref(), Some("failed"));
        assert_eq!(filter.query.as_deref(), Some("rust"));
    }

    #[test]
    fn trims_search_and_ignores_unsupported_status() {
        let query = parse_query("/library?status=bogus&q=%20%20rust%20%20");
        let filter = query.filter();
        assert_eq!(filter.status, None);
        assert_eq!(filter.query.as_deref(), Some("rust"));
    }

    #[tokio::test]
    async fn bulk_delete_removes_terminal_episodes_and_skips_active() {
        let context = ctx().await;
        let show_id = create_show(&context).await;
        let mut deletable = Vec::new();
        for (index, value) in [
            status::PUBLISHED,
            status::READY,
            status::FAILED,
            status::CANCELLED,
        ]
        .into_iter()
        .enumerate()
        {
            let episode = create_episode(
                &context,
                show_id,
                &format!("del{index}"),
                &format!("Delete {index}"),
            )
            .await;
            set_status(&context, &episode.uuid, value).await;
            deletable.push(episode.uuid);
        }
        let running = create_episode(&context, show_id, "run", "Running").await;
        set_status(&context, &running.uuid, status::RUNNING).await;
        let queued = create_episode(&context, show_id, "que", "Queued").await;

        let audio_path = context
            .state
            .cfg
            .data_dir
            .join("audio")
            .join(format!("{}-1.mp3", deletable[0]));
        tokio::fs::create_dir_all(audio_path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&audio_path, b"ID3fake").await.unwrap();
        episodes::set_audio(
            &context.state.pool,
            &deletable[0],
            &audio_path.display().to_string(),
            7,
            1,
            "hash",
            1,
        )
        .await
        .unwrap();

        let show_value = show_id.to_string();
        let mut body = form(&[
            ("_csrf", TOKEN),
            ("confirm", "XOA"),
            ("show_id", &show_value),
            ("status", "published"),
            ("q", "delete"),
        ]);
        for uuid in deletable.iter().chain([&running.uuid, &queued.uuid]) {
            body.push('&');
            body.push_str(&form(&[("uuid", &format!(" {uuid} "))]));
        }

        let response = send_form(&context, "/library/bulk-delete", body).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(location.contains("deleted=4"), "{location}");
        assert!(location.contains("skipped=2"), "{location}");
        assert!(location.contains("status=published"), "{location}");
        assert!(
            location.contains(&format!("show_id={show_id}")),
            "{location}"
        );
        assert!(location.contains("q=delete"), "{location}");

        for uuid in &deletable {
            assert!(episodes::get(&context.state.pool, uuid)
                .await
                .unwrap()
                .is_none());
        }
        assert!(episodes::get(&context.state.pool, &running.uuid)
            .await
            .unwrap()
            .is_some());
        assert!(episodes::get(&context.state.pool, &queued.uuid)
            .await
            .unwrap()
            .is_some());
        assert!(!audio_path.exists());
    }

    #[tokio::test]
    async fn bulk_delete_dedupes_uuid_case_variants() {
        let context = ctx().await;
        let show_id = create_show(&context).await;
        let episode = create_episode(&context, show_id, "one", "One").await;
        set_status(&context, &episode.uuid, status::PUBLISHED).await;

        let body = form(&[
            ("_csrf", TOKEN),
            ("confirm", "XOA"),
            ("uuid", &episode.uuid),
            ("uuid", &episode.uuid.to_ascii_uppercase()),
        ]);
        let response = send_form(&context, "/library/bulk-delete", body).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert!(location.contains("deleted=1"), "{location}");
        assert!(location.contains("skipped=0"), "{location}");
        assert!(episodes::get(&context.state.pool, &episode.uuid)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn bulk_delete_rejects_bad_csrf_confirm_and_empty_selection() {
        let context = ctx().await;
        let show_id = create_show(&context).await;
        let episode = create_episode(&context, show_id, "one", "One").await;
        set_status(&context, &episode.uuid, status::PUBLISHED).await;

        let bad_csrf = send_form(
            &context,
            "/library/bulk-delete",
            form(&[
                ("_csrf", "deadbeef"),
                ("confirm", "XOA"),
                ("uuid", &episode.uuid),
            ]),
        )
        .await;
        assert_eq!(bad_csrf.status(), StatusCode::FORBIDDEN);

        let bad_confirm = send_form(
            &context,
            "/library/bulk-delete",
            form(&[
                ("_csrf", TOKEN),
                ("confirm", "sai"),
                ("uuid", &episode.uuid),
            ]),
        )
        .await;
        assert_eq!(bad_confirm.status(), StatusCode::BAD_REQUEST);

        let empty = send_form(
            &context,
            "/library/bulk-delete",
            form(&[("_csrf", TOKEN), ("confirm", "XOA")]),
        )
        .await;
        assert_eq!(empty.status(), StatusCode::BAD_REQUEST);

        assert!(episodes::get(&context.state.pool, &episode.uuid)
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn bulk_delete_rejects_more_than_two_hundred_uuids() {
        let context = ctx().await;
        let mut body = form(&[("_csrf", TOKEN), ("confirm", "XOA")]);
        for index in 0..201 {
            body.push('&');
            body.push_str(&form(&[("uuid", &format!("missing-{index}"))]));
        }
        let response = send_form(&context, "/library/bulk-delete", body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
