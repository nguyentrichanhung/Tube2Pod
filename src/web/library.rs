use crate::db::episodes::{self, EpisodeFilter};
use crate::db::shows;
use crate::error::AppResult;
use crate::models::Episode;
use crate::state::AppState;
use crate::web::csrf::CsrfToken;
use askama::Template;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse};
use axum::Extension;
use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
pub struct LibraryQuery {
    pub show_id: Option<i64>,
    pub status: Option<String>,
    pub q: Option<String>,
}

impl LibraryQuery {
    pub fn filter(&self) -> EpisodeFilter {
        EpisodeFilter {
            show_id: self.show_id,
            status: self.status.clone().filter(|value| !value.is_empty()),
            query: self.q.clone().filter(|value| !value.trim().is_empty()),
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
    shows: Vec<crate::models::Show>,
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
    let rows = load_rows(&state, &query).await?;
    let shows = shows::list(&state.pool).await?;
    Ok(Html(
        LibraryTemplate {
            csrf: token.0,
            rows,
            shows,
            selected_show: query.show_id,
            selected_status: query.status.clone().unwrap_or_default(),
            query: query.q.clone().unwrap_or_default(),
        }
        .render()?,
    ))
}

pub async fn rows(
    State(state): State<AppState>,
    Query(query): Query<LibraryQuery>,
) -> AppResult<impl IntoResponse> {
    let rows = load_rows(&state, &query).await?;
    Ok(Html(RowsTemplate { rows }.render()?))
}

async fn load_rows(state: &AppState, query: &LibraryQuery) -> AppResult<Vec<LibraryRow>> {
    let episodes = episodes::list(&state.pool, &query.filter()).await?;
    let shows = shows::list(&state.pool).await?;
    let names: std::collections::HashMap<i64, String> = shows
        .into_iter()
        .map(|show| (show.id, show.title))
        .collect();
    Ok(episodes
        .into_iter()
        .map(|episode| LibraryRow {
            show_title: names
                .get(&episode.show_id)
                .cloned()
                .unwrap_or_else(|| "Show đã xoá".into()),
            episode,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
