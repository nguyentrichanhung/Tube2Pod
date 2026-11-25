mod cleanup;
mod config;
mod db;
mod device;
#[cfg(test)]
mod e2e;
mod error;
mod health;
mod launch;
mod lock;
mod mobile_api;
mod models;
mod relay;
mod services;
mod state;
#[cfg(test)]
mod test_tools;
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
    let mut args = env::args_os().skip(1);
    let file = match args.next() {
        None => None,
        Some(flag) if flag == "--config" => {
            let path = args.next().context("--config requires a path")?;
            if args.next().is_some() {
                anyhow::bail!("unexpected command line argument");
            }
            Some(launch::load_file(std::path::Path::new(&path))?)
        }
        Some(_) => anyhow::bail!("unsupported command line argument"),
    };
    let get = |key: &str| {
        let value = env::var(key).ok();
        if let Some(file) = &file {
            value
                .filter(|v| !v.trim().is_empty())
                .or_else(|| file.get(key).cloned())
        } else {
            value
        }
    };
    let filter = tracing_subscriber::EnvFilter::new(get("RUST_LOG").unwrap_or_default());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let data_dir = if file.is_some() && get("DATA_DIR").is_none_or(|v| v.trim().is_empty()) {
        Some(launch::portable_data_dir(|key| env::var(key).ok())?)
    } else {
        None
    };
    let cfg = Arc::new(Config::from_lookup(|key| {
        if key == "DATA_DIR" {
            data_dir
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .or_else(|| get(key))
        } else {
            get(key)
        }
    })?);
    if file.is_some() {
        launch::check_tools(&cfg).await?;
    }
    let admin_addr = parse_loopback_bind("BIND_ADMIN", &cfg.bind_admin)?;
    let owner_addr = parse_loopback_bind("BIND_OWNER", &cfg.bind_owner)?;
    let public_addr = parse_loopback_bind("BIND_PUBLIC", &cfg.bind_public)?;
    for dir in ["tmp", "audio", "transcripts", "shows"] {
        tokio::fs::create_dir_all(cfg.data_dir.join(dir)).await?;
    }
    let _lock = lock::acquire(&cfg.data_dir.join("tube2pod.lock"))?;

    let pool = db::connect(&cfg.database_path()).await?;
    db::migrate(&pool).await?;
    cleanup::remove_legacy_pending_episodes(&pool, &cfg.data_dir)
        .await
        .context("purge legacy pending episodes")?;
    db::episodes::recover_running(&pool)
        .await
        .context("recover running episodes")?;

    let notify = Arc::new(Notify::new());
    worker::spawn(pool.clone(), cfg.clone(), notify.clone());
    cleanup::spawn(pool.clone(), cfg.clone());

    let identity = device::identity::Identity::load(&cfg.data_dir).await?;
    let lan_addr: SocketAddr = get("BIND_LAN")
        .unwrap_or_else(|| "0.0.0.0:8403".into())
        .parse()
        .context("invalid BIND_LAN")?;
    let _mdns = match device::lan::advertise(&identity.server_id, lan_addr.port()) {
        Ok(daemon) => Some(daemon),
        Err(error) => {
            tracing::warn!(%error, "LAN discovery unavailable");
            None
        }
    };
    let tls =
        axum_server::tls_rustls::RustlsConfig::from_pem_file(&identity.cert, &identity.key).await?;

    let remote_listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    remote_listener.set_nonblocking(true)?;
    let remote_addr = remote_listener.local_addr()?;
    let relay_endpoint = get("TUBE2POD_RELAY_ENDPOINT")
        .map(|s| url::Url::parse(&s))
        .transpose()?;
    let relay = Arc::new(
        relay::client::Runtime::load(
            relay_endpoint,
            cfg.data_dir.join("relay.json"),
            public_addr,
            remote_addr,
        )
        .await?,
    );
    let state = AppState {
        pool,
        cfg: cfg.clone(),
        notify,
        relay,
    };
    let admin_listener = tokio::net::TcpListener::bind(admin_addr)
        .await
        .context("bind admin listener")?;
    let public_listener = tokio::net::TcpListener::bind(public_addr)
        .await
        .context("bind public listener")?;

    let owner_listener = tokio::net::TcpListener::bind(owner_addr)
        .await
        .context("bind owner listener")?;
    let owner_addr = owner_listener.local_addr()?;
    let owner_origin = url::Url::parse(&format!("http://localhost:{}", owner_addr.port()))?;
    println!("Owner: {owner_origin}");
    tracing::info!(%admin_addr, %public_addr, %owner_addr, "tube2pod listening");
    let admin = axum::serve(admin_listener, web::admin_router(state.clone()))
        .with_graceful_shutdown(shutdown_signal());
    let owner = axum::serve(
        owner_listener,
        web::owner_connection_router(state.clone(), identity.clone(), lan_addr, owner_origin),
    )
    .with_graceful_shutdown(shutdown_signal());
    let public = axum::serve(public_listener, web::public::public_router(state.clone()))
        .with_graceful_shutdown(shutdown_signal());
    let mobile_state = mobile_api::MobileState {
        app: state.clone(),
        server_id: Arc::new(identity.server_id),
    };
    let remote_handle = axum_server::Handle::new();
    let remote_shutdown = remote_handle.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        remote_shutdown.graceful_shutdown(Some(std::time::Duration::from_secs(10)));
    });
    let remote = axum_server::from_tcp_rustls(remote_listener, tls.clone())?
        .handle(remote_handle)
        .serve(
            mobile_api::router(mobile_state.clone(), mobile_api::RouteKind::Relay)
                .into_make_service(),
        );
    let lan_handle = axum_server::Handle::new();
    let shutdown_handle = lan_handle.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        shutdown_handle.graceful_shutdown(Some(std::time::Duration::from_secs(10)));
    });
    let lan = axum_server::bind_rustls(lan_addr, tls)
        .handle(lan_handle)
        .serve(mobile_api::router(mobile_state, mobile_api::RouteKind::Lan).into_make_service());
    tokio::try_join!(admin, public, owner, lan, remote)?;
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
