mod cleanup;
mod config;
mod db;
mod domain;
mod error;
mod health;
mod lock;
mod models;
mod services;
mod state;
mod web;
mod worker;

use anyhow::Context;
use config::Config;
use state::AppState;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::Notify;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = Arc::new(Config::from_lookup(|key| env::var(key).ok())?);
    for dir in ["tmp", "audio", "transcripts", "shows"] {
        tokio::fs::create_dir_all(cfg.data_dir.join(dir)).await?;
    }
    let _lock = lock::acquire(&cfg.data_dir.join("tube2pod.lock"))?;

    let pool = db::connect(&cfg.database_path()).await?;
    db::migrate(&pool).await?;

    let notify = Arc::new(Notify::new());
    worker::spawn(pool.clone(), cfg.clone(), notify.clone());
    cleanup::spawn(pool.clone(), cfg.clone());

    let state = AppState {
        pool,
        cfg: cfg.clone(),
        notify,
    };
    let admin_addr: SocketAddr = cfg.bind_admin.parse().context("invalid BIND_ADMIN")?;
    let public_addr: SocketAddr = cfg.bind_public.parse().context("invalid BIND_PUBLIC")?;
    let admin_listener = tokio::net::TcpListener::bind(admin_addr)
        .await
        .context("bind admin listener")?;
    let public_listener = tokio::net::TcpListener::bind(public_addr)
        .await
        .context("bind public listener")?;

    tracing::info!(%admin_addr, %public_addr, "tube2pod listening");
    let admin = axum::serve(admin_listener, web::admin_router(state.clone()))
        .with_graceful_shutdown(shutdown_signal());
    let public = axum::serve(public_listener, web::public::public_router(state))
        .with_graceful_shutdown(shutdown_signal());
    tokio::try_join!(admin, public)?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
