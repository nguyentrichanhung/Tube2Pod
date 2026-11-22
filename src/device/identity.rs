use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Identity {
    pub server_id: String,
    pub server_key: String,
    pub cert: PathBuf,
    pub key: PathBuf,
}
impl Identity {
    pub async fn load(dir: &Path) -> anyhow::Result<Self> {
        let dir = dir.join("identity");
        tokio::fs::create_dir_all(&dir).await?;
        let cert = dir.join("tls-cert.pem");
        let key = dir.join("tls-key.pem");
        let id = dir.join("server-id");
        if !tokio::fs::try_exists(&key).await? {
            if tokio::fs::try_exists(&cert).await? {
                anyhow::bail!("TLS key is missing");
            }
            let key_pair = rcgen::KeyPair::generate()?;
            let tmp = dir.join("tls-key.tmp");
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                let mut f = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&tmp)?;
                std::io::Write::write_all(&mut f, key_pair.serialize_pem().as_bytes())?;
            }
            #[cfg(not(unix))]
            tokio::fs::write(&tmp, key_pair.serialize_pem()).await?;
            tokio::fs::rename(&tmp, &key).await?;
        }
        let key_pair = rcgen::KeyPair::from_pem(&tokio::fs::read_to_string(&key).await?)?;
        if !tokio::fs::try_exists(&cert).await? {
            let params = rcgen::CertificateParams::new(vec!["tube2pod.local".into()])?;
            let generated = params.self_signed(&key_pair)?;
            let tmp = dir.join("tls-cert.tmp");
            tokio::fs::write(&tmp, generated.pem()).await?;
            tokio::fs::rename(&tmp, &cert).await?;
        }
        let server_id = match tokio::fs::read_to_string(&id).await {
            Ok(id) => id,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let value = uuid::Uuid::new_v4().to_string();
                tokio::fs::write(&id, &value).await?;
                value
            }
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            server_id,
            server_key: URL_SAFE_NO_PAD.encode(key_pair.public_key_der()),
            cert,
            key,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn restart_and_missing_certificate_preserve_identity() {
        let dir = tempfile::tempdir().unwrap();
        let first = Identity::load(dir.path()).await.unwrap();
        let second = Identity::load(dir.path()).await.unwrap();
        assert_eq!(first.server_id, second.server_id);
        assert_eq!(first.server_key, second.server_key);
        tokio::fs::remove_file(&second.cert).await.unwrap();
        let recovered = Identity::load(dir.path()).await.unwrap();
        assert_eq!(first.server_id, recovered.server_id);
        assert_eq!(first.server_key, recovered.server_key);
        axum_server::tls_rustls::RustlsConfig::from_pem_file(&recovered.cert, &recovered.key)
            .await
            .unwrap();
    }
}
