use crate::{
    db::{self, devices},
    device::{identity::Identity, lan},
    error::AppResult,
    state::AppState,
    web::csrf::{self, CsrfToken},
};
use askama::Template;
use axum::response::Html;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Extension, Form, Json,
};
use serde::{Deserialize, Serialize};

#[derive(sqlx::FromRow)]
struct Device {
    id: String,
    label: String,
    revoked: bool,
}

#[derive(Template)]
#[template(path = "connection.html")]
struct ConnectionTemplate {
    csrf: String,
    devices: Vec<Device>,
    remote_enabled: bool,
    remote_available: bool,
}

pub async fn index(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
) -> AppResult<Html<String>> {
    let devices =
        sqlx::query_as::<_, Device>("SELECT id, label, revoked FROM devices ORDER BY rowid DESC")
            .fetch_all(&state.pool)
            .await?;
    Ok(Html(
        ConnectionTemplate {
            csrf: token.0,
            devices,
            remote_enabled: state.relay.enabled(),
            remote_available: state.relay.available(),
        }
        .render()?,
    ))
}

#[derive(Deserialize)]
pub struct RemoteForm {
    _csrf: String,
    enabled: bool,
}
pub async fn remote(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Form(form): Form<RemoteForm>,
) -> AppResult<axum::response::Redirect> {
    csrf::verify(&token, &headers, &form._csrf)?;
    state.relay.set_enabled(form.enabled).await?;
    Ok(axum::response::Redirect::to("/connection"))
}

#[derive(Serialize)]
pub struct PairEnvelopeV1 {
    version: u8,
    server_id: String,
    lan_uri: String,
    server_key: String,
    ticket: String,
    expires_at: i64,
}
#[derive(Deserialize)]
pub struct CsrfForm {
    _csrf: String,
}
#[derive(Deserialize)]
pub struct TicketStatusForm {
    _csrf: String,
    ticket: String,
}
#[derive(Deserialize)]
pub struct RenameForm {
    _csrf: String,
    label: String,
}
pub async fn ticket(
    State(state): State<AppState>,
    Extension(identity): Extension<Identity>,
    Extension(bind): Extension<std::net::SocketAddr>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> AppResult<Json<PairEnvelopeV1>> {
    csrf::verify(&token, &headers, &form._csrf)?;
    let ip = if bind.ip().is_unspecified() {
        lan::address().map_err(crate::error::AppError::Internal)?
    } else {
        bind.ip()
    };
    let (ticket, expires_at) = devices::issue(&state.pool, db::now_ts()).await?;
    Ok(Json(PairEnvelopeV1 {
        version: 1,
        server_id: identity.server_id,
        lan_uri: format!("https://{}", std::net::SocketAddr::new(ip, bind.port())),
        server_key: identity.server_key,
        ticket,
        expires_at,
    }))
}
pub async fn ticket_status(
    State(state): State<AppState>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Form(form): Form<TicketStatusForm>,
) -> AppResult<Json<serde_json::Value>> {
    csrf::verify(&token, &headers, &form._csrf)?;
    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT state, expires_at FROM pair_tickets WHERE ticket = ?")
            .bind(form.ticket)
            .fetch_optional(&state.pool)
            .await?;
    let status = match row {
        Some((state, _)) if state == "used" => "used",
        Some((state, expires)) if state == "pending" && expires > db::now_ts() => "pending",
        _ => "expired",
    };
    Ok(Json(serde_json::json!({"status": status})))
}

pub async fn rename(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Form(form): Form<RenameForm>,
) -> AppResult<Json<serde_json::Value>> {
    csrf::verify(&token, &headers, &form._csrf)?;
    devices::rename(&state.pool, &id, &form.label).await?;
    Ok(Json(serde_json::json!({"ok":true})))
}
pub async fn revoke(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(token): Extension<CsrfToken>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> AppResult<Json<serde_json::Value>> {
    csrf::verify(&token, &headers, &form._csrf)?;
    devices::revoke(&state.pool, &id).await?;
    Ok(Json(serde_json::json!({"ok":true})))
}
