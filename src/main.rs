mod cleanup;
mod config;
mod db;
mod domain;
#[cfg(test)]
mod e2e;
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
    let admin_addr = parse_loopback_bind("BIND_ADMIN", &cfg.bind_admin)?;
    let public_addr = parse_loopback_bind("BIND_PUBLIC", &cfg.bind_public)?;
    for dir in ["tmp", "audio", "transcripts", "shows"] {
        tokio::fs::create_dir_all(cfg.data_dir.join(dir)).await?;
    }
    let _lock = lock::acquire(&cfg.data_dir.join("tube2pod.lock"))?;

    let pool = db::connect(&cfg.database_path()).await?;
    db::migrate(&pool).await?;
    let capped = db::episodes::cap_legacy_summaries(&pool)
        .await
        .context("cap legacy display summaries")?;
    if capped > 0 {
        tracing::info!(capped, "capped legacy display summaries");
    }
    db::episodes::recover_running(&pool)
        .await
        .context("recover running episodes")?;

    let notify = Arc::new(Notify::new());
    worker::spawn(pool.clone(), cfg.clone(), notify.clone());
    cleanup::spawn(pool.clone(), cfg.clone());

    let state = AppState {
        pool,
        cfg: cfg.clone(),
        notify,
    };
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

fn parse_loopback_bind(name: &str, value: &str) -> anyhow::Result<SocketAddr> {
    let addr: SocketAddr = value.parse().with_context(|| format!("invalid {name}"))?;
    if !addr.ip().is_loopback() {
        anyhow::bail!("{name} must bind to loopback (127.0.0.1 or ::1), got {addr}");
    }
    Ok(addr)
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(error) => {
                tracing::warn!(%error, "failed to install SIGTERM handler");
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_loopback_binds_and_rejects_public_interfaces() {
        assert!(parse_loopback_bind("BIND_ADMIN", "127.0.0.1:8400").is_ok());
        assert!(parse_loopback_bind("BIND_PUBLIC", "[::1]:8401").is_ok());

        for value in ["0.0.0.0:8400", "192.168.1.5:8400", "not-an-address"] {
            let error = parse_loopback_bind("BIND_ADMIN", value).unwrap_err();
            assert!(error.to_string().contains("BIND_ADMIN"), "{value}: {error}");
        }
    }
}
