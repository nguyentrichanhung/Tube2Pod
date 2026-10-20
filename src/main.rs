mod config;
mod db;
mod error;
mod models;

use anyhow::Context;
use axum::{routing::get, Router};
use config::Config;
use std::env;
use std::net::SocketAddr;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = Config::from_lookup(|key| env::var(key).ok())?;
    tokio::fs::create_dir_all(&cfg.data_dir).await?;

    let admin = Router::new().route("/healthz", get(|| async { "ok" }));
    let addr: SocketAddr = cfg.bind_admin.parse().context("invalid BIND_ADMIN")?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "admin listening");
    axum::serve(listener, admin).await?;
    Ok(())
}
