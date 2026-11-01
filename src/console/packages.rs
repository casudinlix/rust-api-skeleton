use std::{path::Path, process::Command};

use anyhow::{Context, ensure};

pub fn fetch_and_build(root: &Path) -> anyhow::Result<()> {
    execute(root, &["fetch", "--locked"])?;
    execute(root, &["build", "--locked", "--bins"])
}

pub fn require(
    root: &Path,
    package: &str,
    features: Option<&str>,
    dev: bool,
) -> anyhow::Result<()> {
    validate_package(package, true)?;
    let mut args = vec!["add"];
    if let Some(features) = features {
        ensure!(
            !features.is_empty()
                && features.len() <= 1024
                && features.split(',').all(|feature| {
                    !feature.is_empty()
                        && feature
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"_-/".contains(&b))
                }),
            "FEATURES must be a comma-separated list of Cargo feature names"
        );
        args.extend(["--features", features]);
    }
    if dev {
        args.push("--dev");
    }
    args.push(package);
    execute(root, &args)
}

pub fn remove(root: &Path, package: &str, dev: bool) -> anyhow::Result<()> {
    validate_package(package, false)?;
    let mut args = vec!["remove"];
    if dev {
        args.push("--dev");
    }
    args.push(package);
    execute(root, &args)
}

pub fn update(root: &Path, package: Option<&str>) -> anyhow::Result<()> {
    let mut args = vec!["update"];
    if let Some(package) = package {
        validate_package(package, true)?;
        args.extend(["--package", package]);
    }
    execute(root, &args)
}

fn validate_package(package: &str, version_allowed: bool) -> anyhow::Result<()> {
    ensure!(package.len() <= 192, "PACKAGE is too long");
    let (name, version) = match package.split_once('@') {
        Some((name, version)) => (name, Some(version)),
        None => (package, None),
    };
    ensure!(
        !name.is_empty()
            && name.len() <= 64
            && name.starts_with(|c: char| c.is_ascii_alphabetic())
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
        "PACKAGE must be a crates.io package name, optionally followed by @version"
    );
    if let Some(version) = version {
        ensure!(
            version_allowed,
            "remove accepts a package name without @version"
        );
        ensure!(
            !version.is_empty()
                && version
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b".+*^~=<>,-".contains(&b)),
            "invalid PACKAGE version requirement"
        );
    }
    Ok(())
}

fn execute(root: &Path, args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new("cargo")
        .args(args)
        .current_dir(root)
        .status()
        .context(
            "could not start Cargo; install the pinned Rust toolchain and add Cargo to PATH",
        )?;
    ensure!(
        status.success(),
        "Cargo {} failed ({status}); inspect its output above",
        args[0]
    );
    Ok(())
}
