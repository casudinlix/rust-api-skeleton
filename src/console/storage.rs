use std::{
    fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, bail, ensure};

use super::project;

const DEFAULT_FILESYSTEM_ROOT: &str = "storage/app";
const DEFAULT_PUBLIC_LINK: &str = "public/storage";

pub fn link(
    project_root: &Path,
    target_override: Option<&Path>,
    link_override: Option<&Path>,
    force: bool,
) -> anyhow::Result<()> {
    let storage_root = match target_override {
        Some(target) => normalize_target(project_root, target)?,
        None => normalize_target(project_root, &configured_filesystem_root(project_root)?)?
            .join("public"),
    };
    fs::create_dir_all(&storage_root).context("could not create public storage directory")?;
    let target_meta = fs::symlink_metadata(&storage_root)
        .context("could not inspect public storage directory")?;
    ensure!(
        target_meta.is_dir() && !target_meta.file_type().is_symlink(),
        "storage:link target must be a real directory"
    );
    let target = storage_root
        .canonicalize()
        .context("could not resolve public storage directory")?;

    let link_relative = link_override.unwrap_or_else(|| Path::new(DEFAULT_PUBLIC_LINK));
    validate_project_relative(link_relative)?;
    let link_parent_relative = link_relative
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let link_parent = if link_parent_relative == Path::new(".") {
        project_root.to_path_buf()
    } else {
        project::checked_path(project_root, link_parent_relative)?
    };
    fs::create_dir_all(&link_parent).context("could not create storage link parent")?;
    let link = project_root.join(link_relative);

    match fs::symlink_metadata(&link) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let existing =
                fs::read_link(&link).context("could not inspect existing storage link")?;
            let existing = if existing.is_absolute() {
                existing
            } else {
                link.parent().unwrap_or(project_root).join(existing)
            };
            if existing.canonicalize().ok().as_deref() == Some(target.as_path()) {
                return Ok(());
            }
            ensure!(
                force,
                "storage link already points elsewhere; rerun with --force to replace only the symlink"
            );
            fs::remove_file(&link).context("could not replace existing storage symlink")?;
        }
        Ok(_) => {
            bail!(
                "{} already exists and is not a symlink; refusing to overwrite it",
                link_relative.display()
            );
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("could not inspect storage link"),
    }

    create_symlink(&target, &link)?;
    Ok(())
}

fn configured_filesystem_root(project_root: &Path) -> anyhow::Result<PathBuf> {
    let source = project::read_optional(project_root, Path::new(".env"))?
        .or_else(|| {
            project::read_optional(project_root, Path::new(".env.example"))
                .ok()
                .flatten()
        })
        .unwrap_or_default();
    if source.is_empty() {
        return Ok(DEFAULT_FILESYSTEM_ROOT.into());
    }
    let entries: Vec<_> = dotenvy::from_read_iter(source.as_bytes())
        .collect::<Result<_, _>>()
        .map_err(|_| anyhow::anyhow!("invalid .env syntax; cannot resolve FILESYSTEM_ROOT"))?;
    Ok(entries
        .into_iter()
        .rev()
        .find_map(|(key, value)| (key == "FILESYSTEM_ROOT").then_some(PathBuf::from(value)))
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| DEFAULT_FILESYSTEM_ROOT.into()))
}

fn normalize_target(project_root: &Path, target: &Path) -> anyhow::Result<PathBuf> {
    ensure!(
        !target.as_os_str().is_empty(),
        "storage target must not be empty"
    );
    if target.is_absolute() {
        return Ok(target.to_path_buf());
    }
    validate_project_relative(target)?;
    Ok(project_root.join(target))
}

fn validate_project_relative(path: &Path) -> anyhow::Result<()> {
    ensure!(
        !path.as_os_str().is_empty()
            && path
                .components()
                .all(|component| matches!(component, Component::Normal(_))),
        "storage link paths must be relative and cannot traverse directories"
    );
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &Path, link: &Path) -> anyhow::Result<()> {
    std::os::unix::fs::symlink(target, link).context("could not create storage symlink")
}

#[cfg(not(unix))]
fn create_symlink(_target: &Path, _link: &Path) -> anyhow::Result<()> {
    bail!("storage:link currently requires Unix; use WSL on Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::write(
            root.path().join(".env.example"),
            "FILESYSTEM_ROOT=storage/app\n",
        )
        .unwrap();
        root
    }

    #[cfg(unix)]
    #[test]
    fn creates_an_idempotent_public_storage_link() {
        let root = project();
        link(root.path(), None, None, false).unwrap();
        let linked = root.path().join("public/storage");
        assert!(
            fs::symlink_metadata(&linked)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        link(root.path(), None, None, false).unwrap();
        assert!(root.path().join("storage/app/public").is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn force_replaces_only_an_existing_symlink() {
        use std::os::unix::fs::symlink;

        let root = project();
        fs::create_dir_all(root.path().join("public")).unwrap();
        let other = root.path().join("other");
        fs::create_dir_all(&other).unwrap();
        symlink(&other, root.path().join("public/storage")).unwrap();
        assert!(link(root.path(), None, None, false).is_err());
        link(root.path(), None, None, true).unwrap();

        fs::remove_file(root.path().join("public/storage")).unwrap();
        fs::write(root.path().join("public/storage"), b"keep").unwrap();
        assert!(link(root.path(), None, None, true).is_err());
        assert_eq!(
            fs::read(root.path().join("public/storage")).unwrap(),
            b"keep"
        );
    }

    #[test]
    fn rejects_link_traversal() {
        let root = project();
        assert!(
            link(
                root.path(),
                None,
                Some(Path::new("../public/storage")),
                false
            )
            .is_err()
        );
    }
}
