use std::path::{Component, Path, PathBuf};

use anyhow::ensure;

#[derive(Clone, Debug)]
pub struct FilesystemConfig {
    pub root: PathBuf,
    pub max_file_bytes: usize,
}

impl FilesystemConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            !self.root.as_os_str().is_empty(),
            "FILESYSTEM_ROOT must not be empty"
        );
        if self.root.is_relative() {
            ensure!(
                self.root
                    .components()
                    .all(|component| matches!(component, Component::Normal(_))),
                "relative FILESYSTEM_ROOT cannot traverse directories"
            );
        }
        ensure!(
            (1_024..=1_073_741_824).contains(&self.max_file_bytes),
            "FILESYSTEM_MAX_FILE_BYTES must be 1024..1073741824"
        );
        Ok(())
    }

    pub fn disk_root(&self, disk: &str) -> PathBuf {
        self.root.join(disk)
    }

    pub fn public_root(&self) -> PathBuf {
        self.disk_root("public")
    }

    pub fn private_root(&self) -> PathBuf {
        self.disk_root("private")
    }
}

pub fn validate_relative_path(path: &Path) -> anyhow::Result<()> {
    ensure!(
        !path.as_os_str().is_empty(),
        "storage path must not be empty"
    );
    ensure!(
        path.components()
            .all(|component| matches!(component, Component::Normal(_))),
        "storage path must be relative and cannot traverse directories"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_root_traversal_and_invalid_limits() {
        let mut config = FilesystemConfig {
            root: "../storage".into(),
            max_file_bytes: 1_024,
        };
        assert!(config.validate().is_err());
        config.root = "storage/app".into();
        config.max_file_bytes = 1_023;
        assert!(config.validate().is_err());
        config.max_file_bytes = 10 * 1024 * 1024;
        config.validate().unwrap();
    }

    #[test]
    fn storage_paths_must_be_relative_normal_components() {
        for path in ["", "../secret", "/tmp/secret", "a/../../b"] {
            assert!(validate_relative_path(Path::new(path)).is_err(), "{path}");
        }
        validate_relative_path(Path::new("avatars/2026/user.png")).unwrap();
    }
}
