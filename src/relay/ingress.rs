use super::{read_line, registration::Registrations, write_json, Command, Kind};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, Mutex},
};

type Queues = HashMap<(String, Kind), Arc<Mutex<mpsc::Receiver<TcpStream>>>>;
#[derive(Clone)]
pub struct Broker {
    registrations: Arc<Registrations>,
    senders: Arc<Mutex<HashMap<(String, Kind), mpsc::Sender<TcpStream>>>>,
    queues: Arc<Mutex<Queues>>,
    domain: String,
}
impl Broker {
    pub fn new(registrations: Registrations, domain: String) -> Self {
        Self {
            registrations: Arc::new(registrations),
            senders: Default::default(),
            queues: Default::default(),
            domain,
        }
    }
    async fn sender(&self, id: &str, kind: Kind) -> mpsc::Sender<TcpStream> {
        let key = (id.to_owned(), kind);
        let mut senders = self.senders.lock().await;
        if let Some(tx) = senders.get(&key) {
            return tx.clone();
        }
        let (tx, rx) = mpsc::channel(8);
        self.queues
            .lock()
            .await
            .insert(key.clone(), Arc::new(Mutex::new(rx)));
        senders.insert(key, tx.clone());
        tx
    }
    pub async fn open_route(&self, id: &str, kind: Kind) -> anyhow::Result<TcpStream> {
        if !self.registrations.contains(id).await {
            anyhow::bail!("unknown registration");
        }
        self.sender(id, kind).await;
        let queue = self
            .queues
            .lock()
            .await
            .get(&(id.to_owned(), kind))
            .unwrap()
            .clone();
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let mut stream = queue.lock().await.recv().await.ok_or_else(|| anyhow::anyhow!("route closed"))?;
                if stream.write_all(b"go\n").await.is_ok()
                    && matches!(tokio::time::timeout(Duration::from_secs(2), read_line(&mut stream)).await, Ok(Ok(reply)) if reply == "ready")
                {
                    return Ok(stream);
                }
            }
        }).await?
    }
    pub async fn control(&self, mut stream: TcpStream) -> anyhow::Result<()> {
        match serde_json::from_str::<Command>(&read_line(&mut stream).await?)? {
            Command::Register => {
                write_json(&mut stream, &self.registrations.register().await?).await?
            }
            Command::Connect {
                id,
                credential,
                kind,
            } => {
                if !self.registrations.authorized(&id, &credential).await {
                    anyhow::bail!("unauthorized");
                }
                self.sender(&id, kind).await.send(stream).await?;
            }
        }
        Ok(())
    }
    fn id_for_host<'a>(&self, host: &'a str, kind: Kind) -> Option<&'a str> {
        let suffix = format!(
            ".{}.{}",
            match kind {
                Kind::Public => "feed",
                Kind::Admin => "admin",
            },
            self.domain
        );
        let host = host.split(':').next()?;
        let id = host.strip_suffix(&suffix)?;
        if id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit()) {
            Some(id)
        } else {
            None
        }
    }
    pub async fn public(&self, mut stream: TcpStream) -> anyhow::Result<()> {
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            if head.len() >= 32768 {
                anyhow::bail!("request headers too large");
            }
            head.push(stream.read_u8().await?);
        }
        let header = std::str::from_utf8(&head)?;
        let host = header
            .lines()
            .skip(1)
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("host").then(|| value.trim())
            })
            .ok_or_else(|| anyhow::anyhow!("missing host"))?;
        let id = self
            .id_for_host(host, Kind::Public)
            .ok_or_else(|| anyhow::anyhow!("unknown host"))?;
        let mut home = self.open_route(id, Kind::Public).await?;
        home.write_all(&head).await?;
        tokio::io::copy_bidirectional(&mut stream, &mut home).await?;
        Ok(())
    }
    pub async fn admin(&self, mut stream: TcpStream) -> anyhow::Result<()> {
        let mut acceptor = rustls::server::Acceptor::default();
        let mut raw = Vec::new();
        let host = loop {
            let mut chunk = [0; 4096];
            let n = stream.read(&mut chunk).await?;
            if n == 0 || raw.len() + n > 65536 {
                anyhow::bail!("invalid TLS hello");
            }
            raw.extend_from_slice(&chunk[..n]);
            acceptor.read_tls(&mut &chunk[..n])?;
            if let Some(hello) = acceptor.accept().map_err(|(e, _)| e)? {
                break hello
                    .client_hello()
                    .server_name()
                    .ok_or_else(|| anyhow::anyhow!("missing SNI"))?
                    .to_owned();
            }
        };
        let id = self
            .id_for_host(&host, Kind::Admin)
            .ok_or_else(|| anyhow::anyhow!("unknown host"))?;
        let mut home = self.open_route(id, Kind::Admin).await?;
        home.write_all(&raw).await?;
        tokio::io::copy_bidirectional(&mut stream, &mut home).await?;
        Ok(())
    }
}
pub async fn serve(
    listener: TcpListener,
    broker: Broker,
    kind: Option<Kind>,
) -> anyhow::Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let broker = broker.clone();
        tokio::spawn(async move {
            let result = match kind {
                None => broker.control(stream).await,
                Some(Kind::Public) => broker.public(stream).await,
                Some(Kind::Admin) => broker.admin(stream).await,
            };
            if let Err(e) = result {
                tracing::debug!(%e, "relay connection ended");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::registration::RelayRegistration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn fixture() -> (
        tempfile::TempDir,
        Broker,
        RelayRegistration,
        RelayRegistration,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("registrations.json");
        let registrations = Registrations::load(path.clone(), "example.test".into())
            .await
            .unwrap();
        let first = registrations.register().await.unwrap();
        let second = registrations.register().await.unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                tokio::fs::metadata(&path)
                    .await
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o077,
                0
            );
        }
        let restored = Registrations::load(path, "example.test".into())
            .await
            .unwrap();
        assert!(restored.authorized(&first.id, &first.credential).await);
        assert!(!restored.authorized(&first.id, &second.credential).await);
        (
            dir,
            Broker::new(restored, "example.test".into()),
            first,
            second,
        )
    }
    async fn connected(broker: &Broker, registration: &RelayRegistration, kind: Kind) -> TcpStream {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let broker = broker.clone();
        tokio::spawn(async move {
            broker.control(server).await.unwrap();
        });
        let mut client = client;
        write_json(
            &mut client,
            &Command::Connect {
                id: registration.id.clone(),
                credential: registration.credential.clone(),
                kind,
            },
        )
        .await
        .unwrap();
        client
    }
    async fn ingress(broker: &Broker, admin: bool) -> TcpStream {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let (server, _) = listener.accept().await.unwrap();
        let broker = broker.clone();
        tokio::spawn(async move {
            let _ = if admin {
                broker.admin(server).await
            } else {
                broker.public(server).await
            };
        });
        client
    }
    #[tokio::test]
    async fn registration_survives_reconnect_and_public_is_independent() {
        let (_dir, broker, first, second) = fixture().await;
        let mut home1 = connected(&broker, &first, Kind::Public).await;
        let mut home2 = connected(&broker, &second, Kind::Public).await;
        let mut phone = ingress(&broker, false).await;
        phone.write_all(format!("HEAD /feed/test.xml HTTP/1.1\r\nHost: {}.feed.example.test\r\nRange: bytes=1-3\r\n\r\n", second.id).as_bytes()).await.unwrap();
        assert_eq!(read_line(&mut home2).await.unwrap(), "go");
        home2.write_all(b"ready\n").await.unwrap();
        let mut request = [0; 256];
        let count = home2.read(&mut request).await.unwrap();
        let request = std::str::from_utf8(&request[..count]).unwrap();
        assert!(request.contains("HEAD /feed/test.xml"));
        assert!(request.contains("Range: bytes=1-3"));
        home2
            .write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
        let mut response = [0; 64];
        let count = phone.read(&mut response).await.unwrap();
        assert!(std::str::from_utf8(&response[..count])
            .unwrap()
            .contains("206 Partial Content"));
        let mut next_home = connected(&broker, &second, Kind::Public).await;
        let mut next_listener = ingress(&broker, false).await;
        next_listener
            .write_all(
                format!(
                    "GET /feed/test.xml HTTP/1.1\r\nHost: {}.feed.example.test\r\n\r\n",
                    second.id
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(read_line(&mut next_home).await.unwrap(), "go");
        next_home.write_all(b"ready\n").await.unwrap();
        let mut next_request = [0; 256];
        let count = next_home.read(&mut next_request).await.unwrap();
        assert!(std::str::from_utf8(&next_request[..count])
            .unwrap()
            .contains("GET /feed/test.xml"));
        let mut other = ingress(&broker, false).await;
        other
            .write_all(
                format!(
                    "GET /cover/a HTTP/1.1\r\nHost: {}.feed.example.test\r\n\r\n",
                    first.id
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(read_line(&mut home1).await.unwrap(), "go");
        home1.write_all(b"ready\n").await.unwrap();
        let mut other_request = [0; 256];
        let count = home1.read(&mut other_request).await.unwrap();
        assert!(std::str::from_utf8(&other_request[..count])
            .unwrap()
            .contains("GET /cover/a"));
    }
    #[tokio::test]
    async fn stale_home_socket_is_skipped_after_reconnect() {
        let (_dir, broker, first, _) = fixture().await;
        let old = connected(&broker, &first, Kind::Public).await;
        let key = (first.id.clone(), Kind::Public);
        loop {
            if let Some(queue) = broker.queues.lock().await.get(&key).cloned() {
                if !queue.lock().await.is_empty() {
                    break;
                }
            }
            tokio::task::yield_now().await;
        }
        drop(old);
        let mut home = connected(&broker, &first, Kind::Public).await;
        let mut listener = ingress(&broker, false).await;
        listener
            .write_all(
                format!(
                    "GET / HTTP/1.1\r\nHost: {}.feed.example.test\r\n\r\n",
                    first.id
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), read_line(&mut home))
                .await
                .unwrap()
                .unwrap(),
            "go"
        );
        home.write_all(b"ready\n").await.unwrap();
        let mut request = [0; 256];
        assert!(home.read(&mut request).await.unwrap() > 0);
    }
    #[tokio::test]
    async fn admin_tunnel_tls_ends_at_home_and_rejects_wrong_identity() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio_rustls::{TlsAcceptor, TlsConnector};
        let (_dir, broker, first, _) = fixture().await;
        let host = format!("{}.admin.example.test", first.id);
        let expected = rcgen::generate_simple_self_signed(vec![host.clone()]).unwrap();
        let other = rcgen::generate_simple_self_signed(vec![host.clone()]).unwrap();
        let mut roots = rustls::RootCertStore::empty();
        roots.add(expected.cert.der().clone()).unwrap();
        let client = TlsConnector::from(Arc::new(
            rustls::ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        ));
        for (identity, succeeds) in [(expected, true), (other, false)] {
            let key =
                rustls::pki_types::PrivatePkcs8KeyDer::from(identity.key_pair.serialize_der());
            let server = rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(vec![identity.cert.der().clone()], key.into())
                .unwrap();
            let mut home = connected(&broker, &first, Kind::Admin).await;
            let seen = Arc::new(AtomicBool::new(false));
            let observed = seen.clone();
            let home_task = tokio::spawn(async move {
                assert_eq!(read_line(&mut home).await.unwrap(), "go");
                home.write_all(b"ready\n").await.unwrap();
                if let Ok(mut tls) = TlsAcceptor::from(Arc::new(server)).accept(home).await {
                    let mut byte = [0];
                    if tls.read_exact(&mut byte).await.is_ok() {
                        observed.store(true, Ordering::SeqCst);
                    }
                }
            });
            let socket = ingress(&broker, true).await;
            let result = client
                .connect(
                    rustls::pki_types::ServerName::try_from(host.clone()).unwrap(),
                    socket,
                )
                .await;
            if succeeds {
                result.unwrap().write_all(b"x").await.unwrap();
            } else {
                assert!(result.is_err());
            }
            home_task.await.unwrap();
            assert_eq!(seen.load(Ordering::SeqCst), succeeds);
        }
    }
}
