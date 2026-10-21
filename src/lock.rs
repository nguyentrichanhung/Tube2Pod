use crate::error::{AppError, AppResult};
use fs2::FileExt;
use std::fs::File;
use std::path::Path;

pub struct InstanceLock {
    file: File,
}

impl InstanceLock {
    pub fn acquire(path: &Path) -> AppResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        FileExt::try_lock_exclusive(&file).map_err(|_| {
            AppError::Conflict("Tube2Pod đang chạy với cùng thư mục dữ liệu.".into())
        })?;
        Ok(Self { file })
    }
}

pub fn acquire(path: &Path) -> AppResult<InstanceLock> {
    InstanceLock::acquire(path)
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_second_process_lock() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tube2pod.lock");
        let _first = acquire(&path).unwrap();
        assert!(acquire(&path).is_err());
    }

    #[test]
    fn dropping_the_lock_releases_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tube2pod.lock");
        let first = acquire(&path).unwrap();
        drop(first);
        assert!(acquire(&path).is_ok());
    }
}
