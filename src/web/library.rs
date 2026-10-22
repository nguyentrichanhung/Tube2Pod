use crate::db::episodes::{self, EpisodeFilter};
use crate::db::shows;
use crate::error::AppResult;
use crate::models::{status, Episode, Show};
use crate::state::AppState;
use crate::web::csrf::CsrfToken;
use askama::Template;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse};
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
        for value in [status::PUBLISHED, status::READY, status::FAILED, status::CANCELLED] {
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
}
