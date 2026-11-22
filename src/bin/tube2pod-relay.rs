#[path = "../relay/mod.rs"]
mod relay;
use relay::{
    ingress::{self, Broker},
    registration::Registrations,
    Kind,
};
use std::env;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let domain = env::var("RELAY_DOMAIN")?;
    let path =
        env::var("RELAY_REGISTRATIONS").unwrap_or_else(|_| "relay-registrations.json".into());
    let broker = Broker::new(
        Registrations::load(path.into(), domain.clone()).await?,
        domain,
    );
    let control = TcpListener::bind(
        env::var("RELAY_CONTROL_BIND").unwrap_or_else(|_| "127.0.0.1:8500".into()),
    )
    .await?;
    let public = TcpListener::bind(
        env::var("RELAY_PUBLIC_BIND").unwrap_or_else(|_| "127.0.0.1:8501".into()),
    )
    .await?;
    let admin =
        TcpListener::bind(env::var("RELAY_ADMIN_BIND").unwrap_or_else(|_| "127.0.0.1:8502".into()))
            .await?;
    tokio::try_join!(
        ingress::serve(control, broker.clone(), None),
        ingress::serve(public, broker.clone(), Some(Kind::Public)),
        ingress::serve(admin, broker, Some(Kind::Admin))
    )?;
    Ok(())
}
