use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use tokio::sync::Mutex;

#[derive(Clone, Serialize, Deserialize)]
pub struct RelayRegistration {
    pub id: String,
    pub public_origin: String,
    pub admin_uri: String,
    pub credential: String,
}

pub struct Registrations {
    path: PathBuf,
    domain: String,
    entries: Mutex<HashMap<String, RelayRegistration>>,
}
impl Registrations {
    pub async fn load(path: PathBuf, domain: String) -> anyhow::Result<Self> {
        let entries = match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            path,
            domain,
            entries: Mutex::new(entries),
        })
    }
    pub async fn register(&self) -> anyhow::Result<RelayRegistration> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let registration = RelayRegistration {
            public_origin: format!("https://{id}.feed.{}", self.domain),
            admin_uri: format!("https://{id}.admin.{}", self.domain),
            credential: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
            id: id.clone(),
        };
        let mut entries = self.entries.lock().await;
        entries.insert(id, registration.clone());
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            tokio::fs::create_dir_all(parent).await?;
        }
        save_private(&self.path, &serde_json::to_vec(&*entries)?).await?;
        Ok(registration)
    }
    pub async fn authorized(&self, id: &str, credential: &str) -> bool {
        self.entries
            .lock()
            .await
            .get(id)
            .is_some_and(|r| r.credential == credential)
    }
    pub async fn contains(&self, id: &str) -> bool {
        self.entries.lock().await.contains_key(id)
    }
}

pub async fn load_home(path: &Path) -> anyhow::Result<Option<RelayRegistration>> {
    match tokio::fs::read(path).await {
        Ok(data) => Ok(Some(serde_json::from_slice(&data)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub async fn save_home(path: &Path, registration: &RelayRegistration) -> anyhow::Result<()> {
    save_private(path, &serde_json::to_vec(registration)?).await
}
async fn save_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use tokio::io::AsyncWriteExt;
    let tmp = path.with_extension("tmp");
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&tmp).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .await?;
    }
    file.write_all(bytes).await?;
    file.flush().await?;
    drop(file);
    tokio::fs::rename(tmp, path).await?;
    Ok(())
}
