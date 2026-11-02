use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::Arc,
};

use thiserror::Error;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::config::filesystem::{FilesystemConfig, validate_relative_path};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageDisk {
    Private,
    Public,
}

impl StorageDisk {
    const fn directory(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Public => "public",
        }
    }
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("invalid storage path")]
    InvalidPath,
    #[error("storage object exceeds configured size limit")]
    TooLarge,
    #[error("storage object not found")]
    NotFound,
    #[error("storage I/O failed")]
    Io,
}

#[derive(Clone)]
pub struct LocalStorage {
    root: Arc<PathBuf>,
    max_file_bytes: usize,
}

impl LocalStorage {
    pub fn new(config: &FilesystemConfig) -> anyhow::Result<Self> {
        config.validate()?;
        Ok(Self {
            root: Arc::new(config.root.clone()),
            max_file_bytes: config.max_file_bytes,
        })
    }

    pub async fn prepare(&self) -> Result<(), StorageError> {
        ensure_directory(&self.root).await?;
        ensure_directory(&self.root.join(StorageDisk::Private.directory())).await?;
        ensure_directory(&self.root.join(StorageDisk::Public.directory())).await?;
        Ok(())
    }

    pub async fn put(
        &self,
        disk: StorageDisk,
        relative: impl AsRef<Path>,
        bytes: &[u8],
    ) -> Result<(), StorageError> {
        if bytes.len() > self.max_file_bytes {
            return Err(StorageError::TooLarge);
        }
        let relative = checked_relative(relative.as_ref())?;
        self.prepare().await?;
        let base = self.root.join(disk.directory());
        let path = checked_destination(&base, &relative).await?;
        let parent = path.parent().ok_or(StorageError::InvalidPath)?;
        ensure_directory(parent).await?;
        reject_symlink_chain(&base, &relative).await?;

        if let Ok(metadata) = tokio::fs::symlink_metadata(&path).await {
            if metadata.file_type().is_symlink() || metadata.is_dir() {
                return Err(StorageError::InvalidPath);
            }
        }

        let temporary = parent.join(format!(".storage-{}.tmp", Uuid::now_v7()));
        let mut options = tokio::fs::OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = options
            .open(&temporary)
            .await
            .map_err(|_| StorageError::Io)?;
        if let Err(error) = async {
            file.write_all(bytes).await?;
            file.flush().await?;
            drop(file);
            tokio::fs::rename(&temporary, &path).await
        }
        .await
        {
            let _ = tokio::fs::remove_file(&temporary).await;
            let _ = error;
            return Err(StorageError::Io);
        }
        Ok(())
    }

    pub async fn get(
        &self,
        disk: StorageDisk,
        relative: impl AsRef<Path>,
    ) -> Result<Vec<u8>, StorageError> {
        let relative = checked_relative(relative.as_ref())?;
        let base = self.root.join(disk.directory());
        reject_symlink_chain(&base, &relative).await?;
        let path = base.join(&relative);
        let metadata = match tokio::fs::symlink_metadata(&path).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Err(StorageError::NotFound);
            }
            Err(_) => return Err(StorageError::Io),
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(StorageError::InvalidPath);
        }
        if metadata.len() > self.max_file_bytes as u64 {
            return Err(StorageError::TooLarge);
        }
        tokio::fs::read(path).await.map_err(|_| StorageError::Io)
    }

    pub async fn exists(
        &self,
        disk: StorageDisk,
        relative: impl AsRef<Path>,
    ) -> Result<bool, StorageError> {
        let relative = checked_relative(relative.as_ref())?;
        let base = self.root.join(disk.directory());
        reject_symlink_chain(&base, &relative).await?;
        match tokio::fs::symlink_metadata(base.join(relative)).await {
            Ok(metadata) if metadata.file_type().is_symlink() => Err(StorageError::InvalidPath),
            Ok(metadata) => Ok(metadata.is_file()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(_) => Err(StorageError::Io),
        }
    }

    pub async fn delete(
        &self,
        disk: StorageDisk,
        relative: impl AsRef<Path>,
    ) -> Result<bool, StorageError> {
        let relative = checked_relative(relative.as_ref())?;
        let base = self.root.join(disk.directory());
        reject_symlink_chain(&base, &relative).await?;
        let path = base.join(relative);
        match tokio::fs::symlink_metadata(&path).await {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                Err(StorageError::InvalidPath)
            }
            Ok(_) => {
                tokio::fs::remove_file(path)
                    .await
                    .map_err(|_| StorageError::Io)?;
                Ok(true)
            }
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
            Err(_) => Err(StorageError::Io),
        }
    }

    pub fn root(&self) -> &Path {
        self.root.as_path()
    }

    pub fn public_root(&self) -> PathBuf {
        self.root.join(StorageDisk::Public.directory())
    }

    pub fn private_root(&self) -> PathBuf {
        self.root.join(StorageDisk::Private.directory())
    }
}

fn checked_relative(path: &Path) -> Result<PathBuf, StorageError> {
    validate_relative_path(path).map_err(|_| StorageError::InvalidPath)?;
    Ok(path.to_path_buf())
}

async fn checked_destination(base: &Path, relative: &Path) -> Result<PathBuf, StorageError> {
    reject_symlink_chain(base, relative).await?;
    Ok(base.join(relative))
}

async fn ensure_directory(path: &Path) -> Result<(), StorageError> {
    tokio::fs::create_dir_all(path)
        .await
        .map_err(|_| StorageError::Io)?;
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|_| StorageError::Io)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(StorageError::InvalidPath);
    }
    Ok(())
}

async fn reject_symlink_chain(base: &Path, relative: &Path) -> Result<(), StorageError> {
    let base_meta = match tokio::fs::symlink_metadata(base).await {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(_) => return Err(StorageError::Io),
    };
    if base_meta.is_some_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(StorageError::InvalidPath);
    }

    let mut current = base.to_path_buf();
    let components: Vec<_> = relative.components().collect();
    for (index, component) in components.iter().enumerate() {
        current.push(component.as_os_str());
        match tokio::fs::symlink_metadata(&current).await {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(StorageError::InvalidPath);
                }
                if index + 1 < components.len() && !metadata.is_dir() {
                    return Err(StorageError::InvalidPath);
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => break,
            Err(_) => return Err(StorageError::Io),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage(root: &Path) -> LocalStorage {
        LocalStorage::new(&FilesystemConfig {
            root: root.join("storage/app"),
            max_file_bytes: 1024,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn private_and_public_disks_round_trip_and_delete() {
        let root = tempfile::tempdir().unwrap();
        let storage = storage(root.path());

        storage
            .put(StorageDisk::Private, "exports/a.txt", b"private")
            .await
            .unwrap();
        storage
            .put(StorageDisk::Public, "avatars/a.txt", b"public")
            .await
            .unwrap();

        assert_eq!(
            storage
                .get(StorageDisk::Private, "exports/a.txt")
                .await
                .unwrap(),
            b"private"
        );
        assert!(storage
            .exists(StorageDisk::Public, "avatars/a.txt")
            .await
            .unwrap());
        assert!(storage
            .delete(StorageDisk::Public, "avatars/a.txt")
            .await
            .unwrap());
        assert!(!storage
            .exists(StorageDisk::Public, "avatars/a.txt")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn rejects_traversal_and_oversized_files() {
        let root = tempfile::tempdir().unwrap();
        let storage = storage(root.path());
        assert_eq!(
            storage
                .put(StorageDisk::Private, "../secret", b"x")
                .await
                .unwrap_err()
                .to_string(),
            "invalid storage path"
        );
        assert!(matches!(
            storage
                .put(StorageDisk::Private, "large.bin", &vec![0_u8; 1025])
                .await,
            Err(StorageError::TooLarge)
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refuses_symlink_components() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let storage = storage(root.path());
        storage.prepare().await.unwrap();
        symlink(
            outside.path(),
            storage.private_root().join("linked"),
        )
        .unwrap();

        assert!(matches!(
            storage
                .put(StorageDisk::Private, "linked/file.txt", b"blocked")
                .await,
            Err(StorageError::InvalidPath)
        ));
        assert!(!outside.path().join("file.txt").exists());
    }
}
