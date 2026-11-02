use std::{
    path::Path,
    process::{Command, Output},
};

use anyhow::{Context, ensure};

const HOOKS_PATH: &str = ".githooks";

pub fn install(root: &Path, force: bool) -> anyhow::Result<()> {
    ensure!(
        root.join(".githooks/pre-commit").is_file(),
        ".githooks/pre-commit is missing from the repository"
    );

    let inside = git(root, &["rev-parse", "--is-inside-work-tree"])?;
    ensure!(
        inside.status.success() && String::from_utf8_lossy(&inside.stdout).trim() == "true",
        "project root is not a Git work tree"
    );

    let current = git(root, &["config", "--local", "--get", "core.hooksPath"])?;
    let current = if current.status.success() {
        Some(String::from_utf8_lossy(&current.stdout).trim().to_owned())
    } else if current.status.code() == Some(1) {
        None
    } else {
        anyhow::bail!("could not inspect existing Git hooks path");
    };

    if let Some(current) = current.as_deref() {
        if current == HOOKS_PATH {
            return Ok(());
        }
        ensure!(
            force,
            "core.hooksPath is already set; rerun hooks:install --force to replace that local setting"
        );
    }

    let configured = git(
        root,
        &["config", "--local", "core.hooksPath", HOOKS_PATH],
    )?;
    ensure!(configured.status.success(), "could not configure Git hooks path");
    Ok(())
}

fn git(root: &Path, arguments: &[&str]) -> anyhow::Result<Output> {
    Command::new("git")
        .args(arguments)
        .current_dir(root)
        .output()
        .context("could not execute git")
}
