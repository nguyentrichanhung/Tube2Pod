use super::{read_line, registration::RelayRegistration, write_json, Command, Kind};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{net::TcpStream, sync::watch};
use tokio_rustls::{client::TlsStream, TlsConnector};
use url::Url;

fn default_tls() -> Arc<rustls::ClientConfig> {
    let roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}
async fn connect(
    endpoint: &Url,
    cfg: Arc<rustls::ClientConfig>,
) -> anyhow::Result<TlsStream<TcpStream>> {
    if endpoint.scheme() != "https" {
        anyhow::bail!("relay endpoint must use HTTPS");
    }
    let host = endpoint
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("missing relay host"))?;
    let socket = TcpStream::connect((host, endpoint.port_or_known_default().unwrap())).await?;
    let name = rustls::pki_types::ServerName::try_from(host.to_owned())?;
    Ok(TlsConnector::from(cfg).connect(name, socket).await?)
}
async fn register_with_tls(
    endpoint: &Url,
    cfg: Arc<rustls::ClientConfig>,
) -> anyhow::Result<RelayRegistration> {
    let mut stream = connect(endpoint, cfg).await?;
    write_json(&mut stream, &Command::Register).await?;
    Ok(serde_json::from_str(&read_line(&mut stream).await?)?)
}
fn spawn_with_tls(
    endpoint: Url,
    registration: RelayRegistration,
    public: SocketAddr,
    admin: SocketAddr,
    enabled: watch::Receiver<u64>,
    tls: Arc<rustls::ClientConfig>,
) {
    for (kind, target) in [(Kind::Public, public), (Kind::Admin, admin)] {
        let endpoint = endpoint.clone();
        let registration = registration.clone();
        let mut enabled = enabled.clone();
        let tls = tls.clone();
        tokio::spawn(async move {
            loop {
                let generation = *enabled.borrow_and_update();
                if generation % 2 == 0 {
                    if enabled.changed().await.is_err() {
                        break;
                    }
                    continue;
                }
                let mut active = enabled.clone();
                let attempt = async {
                    let mut stream = connect(&endpoint, tls.clone()).await?;
                    write_json(
                        &mut stream,
                        &Command::Connect {
                            id: registration.id.clone(),
                            credential: registration.credential.clone(),
                            kind,
                        },
                    )
                    .await?;
                    if read_line(&mut stream).await? != "go" {
                        anyhow::bail!("relay refused connection");
                    }
                    let mut local = TcpStream::connect(target).await?;
                    use tokio::io::AsyncWriteExt;
                    stream.write_all(b"ready\n").await?;
                    tokio::spawn(async move {
                        tokio::select! {
                            _ = tokio::io::copy_bidirectional(&mut stream, &mut local) => {},
                            _ = async { while *active.borrow_and_update() == generation && active.changed().await.is_ok() {} } => {},
                        }
                    });
                    Ok::<(), anyhow::Error>(())
                };
                tokio::select! {
                    _ = enabled.changed() => {},
                    result = attempt => if let Err(error) = result {
                        tracing::debug!(%error, "relay connection failed");
                        tokio::select! { _ = tokio::time::sleep(Duration::from_secs(2)) => {}, _ = enabled.changed() => {} }
                    },
                }
            }
        });
    }
}

pub struct Runtime {
    endpoint: Option<Url>,
    path: std::path::PathBuf,
    public: SocketAddr,
    admin: SocketAddr,
    registration: std::sync::RwLock<Option<RelayRegistration>>,
    enabled: watch::Sender<u64>,
    toggle_lock: tokio::sync::Mutex<()>,
    running: std::sync::atomic::AtomicBool,
    tls: Arc<rustls::ClientConfig>,
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            endpoint: None,
            path: Default::default(),
            public: "127.0.0.1:0".parse().unwrap(),
            admin: "127.0.0.1:0".parse().unwrap(),
            registration: Default::default(),
            enabled: watch::channel(0).0,
            toggle_lock: Default::default(),
            running: Default::default(),
            tls: default_tls(),
        }
    }
}
impl Runtime {
    pub async fn load(
        endpoint: Option<Url>,
        path: std::path::PathBuf,
        public: SocketAddr,
        admin: SocketAddr,
    ) -> anyhow::Result<Self> {
        let registration = super::registration::load_home(&path).await?;
        let enabled = endpoint.is_some()
            && registration.is_some()
            && tokio::fs::try_exists(path.with_extension("enabled")).await?;
        let runtime = Self {
            endpoint,
            path,
            public,
            admin,
            registration: std::sync::RwLock::new(registration),
            enabled: watch::channel(if enabled { 1 } else { 0 }).0,
            toggle_lock: Default::default(),
            running: Default::default(),
            tls: default_tls(),
        };
        if enabled {
            runtime.start();
        }
        Ok(runtime)
    }
    fn start(&self) {
        if self.running.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        if let (Some(endpoint), Some(registration)) =
            (&self.endpoint, self.registration.read().unwrap().clone())
        {
            spawn_with_tls(
                endpoint.clone(),
                registration,
                self.public,
                self.admin,
                self.enabled.subscribe(),
                self.tls.clone(),
            );
        }
    }
    pub fn enabled(&self) -> bool {
        *self.enabled.borrow() % 2 == 1
    }
    pub fn available(&self) -> bool {
        self.endpoint.is_some()
    }
    pub fn registration(&self) -> Option<RelayRegistration> {
        self.registration.read().unwrap().clone()
    }
    pub async fn set_enabled(&self, enabled: bool) -> anyhow::Result<()> {
        let _lock = self.toggle_lock.lock().await;
        if enabled == self.enabled() {
            return Ok(());
        }
        if enabled {
            let endpoint = self
                .endpoint
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("relay unavailable"))?;
            if self.registration().is_none() {
                let registration = register_with_tls(endpoint, self.tls.clone()).await?;
                super::registration::save_home(&self.path, &registration).await?;
                *self.registration.write().unwrap() = Some(registration);
            }
            tokio::fs::write(self.path.with_extension("enabled"), b"1").await?;
            self.enabled.send_modify(|generation| *generation += 1);
            self.start();
        } else {
            match tokio::fs::remove_file(self.path.with_extension("enabled")).await {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
            self.enabled.send_modify(|generation| *generation += 1);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::{
        ingress::{self, Broker},
        registration::Registrations,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };
    use tokio_rustls::TlsAcceptor;

    #[tokio::test]
    async fn outgoing_control_tls_uses_test_only_ca_and_carries_public_stream() {
        let dir = tempfile::tempdir().unwrap();
        let broker = Broker::new(
            Registrations::load(dir.path().join("registrations"), "example.test".into())
                .await
                .unwrap(),
            "example.test".into(),
        );
        let control = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let control_addr = control.local_addr().unwrap();
        tokio::spawn(ingress::serve(control, broker.clone(), None));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = Url::parse(&format!(
            "https://localhost:{}",
            listener.local_addr().unwrap().port()
        ))
        .unwrap();
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert.cert.der().clone()).unwrap();
        let trusted = Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let key = rustls::pki_types::PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der());
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert.cert.der().clone()], key.into())
            .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    if let Ok(mut tls) = acceptor.accept(socket).await {
                        let mut control = TcpStream::connect(control_addr).await.unwrap();
                        let _ = tokio::io::copy_bidirectional(&mut tls, &mut control).await;
                    }
                });
            }
        });
        assert!(connect(&endpoint, default_tls()).await.is_err());
        let local = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = local.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = local.accept().await.unwrap();
                let mut req = [0; 512];
                let count = socket.read(&mut req).await.unwrap();
                assert!(std::str::from_utf8(&req[..count])
                    .unwrap()
                    .contains("Range: bytes=1-3"));
                socket
                    .write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 3\r\n\r\nbcd")
                    .await
                    .unwrap();
            }
        });
        let runtime = Runtime {
            endpoint: Some(endpoint),
            path: dir.path().join("home.json"),
            public: local_addr,
            admin: local_addr,
            registration: Default::default(),
            enabled: watch::channel(0).0,
            toggle_lock: Default::default(),
            running: Default::default(),
            tls: trusted,
        };
        runtime.set_enabled(true).await.unwrap();
        let registration = runtime.registration().unwrap();
        let public = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = public.local_addr().unwrap();
        tokio::spawn(ingress::serve(public, broker, Some(Kind::Public)));
        let mut client = TcpStream::connect(addr).await.unwrap();
        client
            .write_all(
                format!(
                    "GET /audio HTTP/1.1\r\nHost: {}.feed.example.test\r\nRange: bytes=1-3\r\n\r\n",
                    registration.id
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(std::str::from_utf8(&response)
            .unwrap()
            .contains("206 Partial Content"));
        assert!(response.ends_with(b"bcd"));
        runtime.set_enabled(false).await.unwrap();
        assert!(!runtime.enabled());
        runtime.set_enabled(true).await.unwrap();
        assert_eq!(
            runtime.registration().unwrap().public_origin,
            registration.public_origin
        );
        let mut client = TcpStream::connect(addr).await.unwrap();
        client
            .write_all(
                format!(
                    "GET /audio HTTP/1.1\r\nHost: {}.feed.example.test\r\nRange: bytes=1-3\r\n\r\n",
                    registration.id
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut response))
            .await
            .unwrap()
            .unwrap();
        assert!(response.ends_with(b"bcd"));
    }
    #[tokio::test]
    async fn missing_endpoint_disables_previous_registration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relay.json");
        super::super::registration::save_home(
            &path,
            &RelayRegistration {
                id: "id".into(),
                credential: "secret".into(),
                public_origin: "https://feed.example".into(),
                admin_uri: "https://admin.example".into(),
            },
        )
        .await
        .unwrap();
        tokio::fs::write(path.with_extension("enabled"), b"1")
            .await
            .unwrap();
        let runtime = Runtime::load(
            None,
            path,
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:2".parse().unwrap(),
        )
        .await
        .unwrap();
        assert!(!runtime.enabled());
        assert!(!runtime.available());
        assert!(runtime.registration().is_some());
    }
}
