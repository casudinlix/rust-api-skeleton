use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions, Permissions},
    io::{ErrorKind, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, bail, ensure};
use tempfile::NamedTempFile;

/// A relative-path change with an optimistic snapshot. `None` means create only.
pub struct FileChange {
    pub relative: PathBuf,
    pub before: Option<String>,
    pub after: String,
    pub private: bool,
}

pub fn discover(explicit: Option<&Path>) -> anyhow::Result<PathBuf> {
    let start = match explicit {
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir().context("could not find the working directory")?,
    }
    .canonicalize()
    .context("project directory does not exist")?;
    for path in start.ancestors() {
        if path.join("Cargo.toml").is_file() && path.join("src/lib.rs").is_file() {
            checked_path(path, Path::new("Cargo.toml"))?;
            checked_path(path, Path::new("src/lib.rs"))?;
            return Ok(path.to_path_buf());
        }
        if explicit.is_some() {
            break;
        }
    }
    bail!("run artisan inside the skeleton project or provide --root PATH")
}

/// Refuse traversal and existing symlink components, including the destination.
/// This protects ordinary local edits, not a directory concurrently controlled
/// by an adversarial process. Cooperating artisan commands also hold `lock`.
pub fn checked_path(root: &Path, relative: &Path) -> anyhow::Result<PathBuf> {
    ensure!(!relative.as_os_str().is_empty(), "empty project path");
    let mut path = root.to_path_buf();
    let components: Vec<_> = relative.components().collect();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            bail!("project paths must be relative and cannot traverse directories");
        };
        path.push(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                ensure!(
                    !metadata.file_type().is_symlink(),
                    "refusing symlink at {}",
                    relative.display()
                );
                if index + 1 < components.len() {
                    ensure!(metadata.is_dir(), "project path has a non-directory parent");
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("could not inspect project path"),
        }
    }
    Ok(path)
}

pub fn read_optional(root: &Path, relative: &Path) -> anyhow::Result<Option<String>> {
    let path = checked_path(root, relative)?;
    match fs::metadata(&path) {
        Ok(metadata) => ensure!(
            metadata.is_file(),
            "{} is not a regular file",
            relative.display()
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("could not inspect project file"),
    }
    fs::read_to_string(path)
        .map(Some)
        .with_context(|| format!("could not read {} as UTF-8", relative.display()))
}

/// Keep the lock file in place: removing it would allow two distinct lock inodes.
pub fn lock(root: &Path) -> anyhow::Result<File> {
    let path = checked_path(root, Path::new(".artisan.lock"))?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .context("could not open the artisan project lock")?;
    file.try_lock()
        .context("another artisan command is modifying the project; retry after it finishes")?;
    Ok(file)
}

struct Prepared {
    change: FileChange,
    path: PathBuf,
    original_permissions: Option<Permissions>,
    temporary: NamedTempFile,
}

pub fn apply_changes(root: &Path, changes: Vec<FileChange>) -> anyhow::Result<()> {
    let mut seen = HashSet::new();
    // Complete logical preflight before creating even the first output file.
    for change in &changes {
        ensure!(
            seen.insert(change.relative.clone()),
            "duplicate output path in generator"
        );
        check_snapshot(root, change)?;
    }
    let mut prepared = Vec::with_capacity(changes.len());
    for change in changes {
        let path = checked_path(root, &change.relative)?;
        let parent = path.parent().context("project file has no parent")?;
        fs::create_dir_all(parent).context("could not create generator output directory")?;
        checked_path(root, &change.relative)?;
        let original_permissions = fs::metadata(&path).ok().map(|m| m.permissions());
        let temporary = stage_file(
            parent,
            &change.after,
            change.private,
            original_permissions.clone(),
        )?;
        prepared.push(Prepared {
            change,
            path,
            original_permissions,
            temporary,
        });
    }

    let mut applied: Vec<(FileChange, Option<Permissions>)> = Vec::new();
    for entry in prepared {
        let Prepared {
            change,
            path,
            original_permissions,
            temporary,
        } = entry;
        let result = check_snapshot(root, &change).and_then(|()| {
            let saved = if change.before.is_none() {
                temporary.persist_noclobber(&path)
            } else {
                temporary.persist(&path)
            };
            saved.map(|_| ()).map_err(|error| error.error.into())
        });
        if let Err(error) = result {
            let mut restored = true;
            for (previous, permissions) in applied.into_iter().rev() {
                restored &= rollback(root, &previous, permissions).is_ok();
            }
            return Err(error).context(if restored {
                "could not apply project files; prior file changes were restored"
            } else {
                "could not apply all project files; inspect the working tree before retrying"
            });
        }
        applied.push((change, original_permissions));
    }
    Ok(())
}

fn check_snapshot(root: &Path, change: &FileChange) -> anyhow::Result<()> {
    let current = read_optional(root, &change.relative)?;
    ensure!(
        current == change.before,
        "{} already exists or changed; refusing to overwrite it",
        change.relative.display()
    );
    Ok(())
}

fn stage_file(
    parent: &Path,
    contents: &str,
    private: bool,
    permissions: Option<Permissions>,
) -> anyhow::Result<NamedTempFile> {
    let mut file =
        NamedTempFile::new_in(parent).context("could not prepare an atomic file write")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if private {
            0o600
        } else {
            permissions.map_or(0o644, |p| p.mode() & 0o777)
        };
        file.as_file()
            .set_permissions(Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    {
        let _ = private;
        if let Some(permissions) = permissions {
            file.as_file().set_permissions(permissions)?;
        }
    }
    file.write_all(contents.as_bytes())
        .context("could not write project file")?;
    file.as_file()
        .sync_all()
        .context("could not sync project file")?;
    Ok(file)
}

fn rollback(
    root: &Path,
    change: &FileChange,
    permissions: Option<Permissions>,
) -> anyhow::Result<()> {
    let path = checked_path(root, &change.relative)?;
    ensure!(
        read_optional(root, &change.relative)?.as_deref() == Some(&change.after),
        "output changed during rollback"
    );
    if let Some(original) = &change.before {
        stage_file(
            path.parent().context("missing parent")?,
            original,
            false,
            permissions,
        )?
        .persist(path)
        .map_err(|error| error.error)?;
    } else {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create(path: &str, after: &str) -> FileChange {
        FileChange {
            relative: path.into(),
            before: None,
            after: after.into(),
            private: false,
        }
    }

    #[test]
    fn preflights_every_path_before_writing_and_preserves_unrelated_code() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("mod.rs"), "pub fn existing() {}\n").unwrap();
        assert!(
            apply_changes(
                root.path(),
                vec![create("new.rs", "new"), create("mod.rs", "bad")]
            )
            .is_err()
        );
        assert!(!root.path().join("new.rs").exists());
        let original = read_optional(root.path(), Path::new("mod.rs"))
            .unwrap()
            .unwrap();
        apply_changes(
            root.path(),
            vec![
                create("new.rs", "new"),
                FileChange {
                    relative: "mod.rs".into(),
                    before: Some(original.clone()),
                    after: format!("{original}pub mod new;\n"),
                    private: false,
                },
            ],
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(root.path().join("mod.rs")).unwrap(),
            "pub fn existing() {}\npub mod new;\n"
        );
    }

    #[test]
    fn rejects_traversal_duplicate_outputs_and_stale_snapshots() {
        let root = tempfile::tempdir().unwrap();
        for path in ["../outside.rs", "/tmp/outside.rs", ""] {
            assert!(checked_path(root.path(), Path::new(path)).is_err());
        }
        assert!(
            apply_changes(
                root.path(),
                vec![create("a.rs", "one"), create("a.rs", "two")]
            )
            .is_err()
        );
        assert!(!root.path().join("a.rs").exists());
        fs::write(root.path().join("a.rs"), "fresh").unwrap();
        assert!(
            apply_changes(
                root.path(),
                vec![FileChange {
                    relative: "a.rs".into(),
                    before: Some("stale".into()),
                    after: "bad".into(),
                    private: false,
                }]
            )
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(root.path().join("a.rs")).unwrap(),
            "fresh"
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_parents_and_serializes_mutations() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), root.path().join("linked")).unwrap();
        assert!(apply_changes(root.path(), vec![create("linked/new.rs", "bad")]).is_err());
        assert!(!outside.path().join("new.rs").exists());
        let guard = lock(root.path()).unwrap();
        assert!(lock(root.path()).is_err());
        drop(guard);
        assert!(lock(root.path()).is_ok());
    }
}
