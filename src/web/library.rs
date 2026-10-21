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

pub struct LibraryRow {
    pub episode: Episode,
    pub show_title: String,
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
}

impl LibraryTemplate {
    pub fn status_selected(&self, status: &str) -> bool {
        self.selected_status == status
    }
}

#[derive(Template)]
#[template(path = "partials/episode_rows.html")]
pub struct RowsTemplate {
    rows: Vec<LibraryRow>,
}

pub async fn index(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    Query(query): Query<LibraryQuery>,
) -> AppResult<Html<String>> {
    let filter = query.filter();
    let selected_status = filter.status.clone().unwrap_or_default();
    let (rows, shows) = load_rows(&state, &filter).await?;
    Ok(Html(
        LibraryTemplate {
            csrf: token.0,
            rows,
            shows,
            selected_show: query.show_id,
            selected_status,
            query: query.q.clone().unwrap_or_default(),
        }
        .render()?,
    ))
}

pub async fn rows(
    State(state): State<AppState>,
    Query(query): Query<LibraryQuery>,
) -> AppResult<impl IntoResponse> {
    let (rows, _) = load_rows(&state, &query.filter()).await?;
    Ok(Html(RowsTemplate { rows }.render()?))
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
