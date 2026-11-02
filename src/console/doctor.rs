use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::Context;
use uuid::Uuid;

use rust_api_skeleton::{
    cache::Cache,
    config::{QueueDriver, Settings, cache::CacheDriver},
    database::DatabasePool,
    filesystem::{LocalStorage, StorageDisk},
    jobs::redis::RedisTransport,
};

use super::project;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Status {
    Ok,
    Skip,
    Warn,
    Fail,
}

impl Status {
    const fn label(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Skip => "SKIP",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        }
    }

    const fn blocks(self) -> bool {
        matches!(self, Self::Fail)
    }
}

#[derive(Debug)]
struct Check {
    name: &'static str,
    status: Status,
    detail: String,
}

impl Check {
    fn new(name: &'static str, status: Status, detail: impl Into<String>) -> Self {
        Self {
            name,
            status,
            detail: detail.into(),
        }
    }
}

pub async fn run(root: &Path) -> anyhow::Result<()> {
    std::env::set_current_dir(root).context("could not enter project root for doctor")?;

    let mut checks = Vec::new();
    checks.push(toolchain_check());
    checks.push(environment_file_check(root));

    let settings = match Settings::load() {
        Ok(settings) => {
            checks.push(Check::new("Configuration", Status::Ok, "validated"));
            settings
        }
        Err(_) => {
            checks.push(Check::new(
                "Configuration",
                Status::Fail,
                "invalid .env/configuration",
            ));
            print_report(&checks);
            anyhow::bail!("doctor found blocking checks");
        }
    };

    let database = match DatabasePool::connect(&settings.database).await {
        Ok(database) => {
            checks.push(Check::new(
                "Database",
                Status::Ok,
                settings.database.driver.to_string(),
            ));
            Some(database)
        }
        Err(_) => {
            checks.push(Check::new(
                "Database",
                Status::Fail,
                format!("{} unavailable", settings.database.driver),
            ));
            None
        }
    };

    if let Some(database) = &database {
        checks.push(match database.ready().await {
            Ok(()) => Check::new("Migrations", Status::Ok, "ledger/checksum ready"),
            Err(_) => Check::new(
                "Migrations",
                Status::Fail,
                "not applied or checksum/history mismatch",
            ),
        });
    } else {
        checks.push(Check::new(
            "Migrations",
            Status::Fail,
            "database unavailable",
        ));
    }

    checks.push(cache_check(&settings).await);
    checks.push(queue_check(&settings).await);
    checks.push(storage_check(&settings).await);
    checks.push(storage_link_check(root, &settings));
    checks.push(logging_check(&settings));
    checks.push(if settings.swagger.enabled {
        Check::new(
            "Swagger",
            Status::Ok,
            format!("ON {}", settings.swagger.ui_path),
        )
    } else {
        Check::new("Swagger", Status::Skip, "OFF")
    });
    checks.push(Check::new(
        "Observability",
        Status::Ok,
        format!(
            "api:{} worker:{} scheduler:{} relay:{}",
            settings.observability.api_port,
            settings.observability.worker_port,
            settings.observability.scheduler_port,
            settings.observability.relay_port
        ),
    ));
    checks.push(git_hook_check(root));

    if let Some(database) = database {
        database.close().await;
    }

    let failed = checks.iter().any(|check| check.status.blocks());
    print_report(&checks);
    if failed {
        anyhow::bail!("doctor found blocking checks");
    }
    Ok(())
}

fn toolchain_check() -> Check {
    let rustc = Command::new("rustc").arg("--version").output();
    let cargo = Command::new("cargo").arg("--version").output();
    match (rustc, cargo) {
        (Ok(rustc), Ok(cargo)) if rustc.status.success() && cargo.status.success() => {
            let rustc = String::from_utf8_lossy(&rustc.stdout).trim().to_owned();
            let cargo = String::from_utf8_lossy(&cargo.stdout).trim().to_owned();
            if rustc.starts_with("rustc 1.99.1 ") {
                Check::new("Toolchain", Status::Ok, format!("{rustc}; {cargo}"))
            } else {
                Check::new(
                    "Toolchain",
                    Status::Fail,
                    format!("expected rustc 1.99.1, found {rustc}"),
                )
            }
        }
        _ => Check::new("Toolchain", Status::Fail, "rustc/cargo unavailable"),
    }
}

fn environment_file_check(root: &Path) -> Check {
    let relative = Path::new(".env");
    match project::read_optional(root, relative) {
        Ok(Some(_)) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                match fs::metadata(root.join(relative)) {
                    Ok(metadata) => {
                        let mode = metadata.permissions().mode() & 0o777;
                        if mode & 0o077 != 0 {
                            return Check::new(
                                "Environment",
                                Status::Fail,
                                format!(
                                    ".env permissions are {mode:o}; expected private permissions"
                                ),
                            );
                        }
                    }
                    Err(_) => {
                        return Check::new("Environment", Status::Fail, "could not inspect .env");
                    }
                }
            }
            Check::new("Environment", Status::Ok, ".env present and private")
        }
        Ok(None) => Check::new(
            "Environment",
            Status::Fail,
            ".env missing; run make install",
        ),
        Err(_) => Check::new(
            "Environment",
            Status::Fail,
            ".env is not a safe regular project file",
        ),
    }
}

async fn cache_check(settings: &Settings) -> Check {
    if settings.cache.driver == CacheDriver::None {
        return Check::new("Redis Cache", Status::Skip, "CACHE_DRIVER=none");
    }
    let cache = match Cache::from_config(&settings.cache) {
        Ok(cache) => cache,
        Err(_) => {
            return Check::new("Redis Cache", Status::Fail, "invalid cache configuration");
        }
    };
    match cache.ping().await {
        Ok(()) => Check::new("Redis Cache", Status::Ok, "reachable"),
        Err(_) => Check::new("Redis Cache", Status::Fail, "unavailable"),
    }
}

async fn queue_check(settings: &Settings) -> Check {
    if settings.queue.driver == QueueDriver::Database {
        return Check::new("Redis Queue", Status::Skip, "QUEUE_DRIVER=database");
    }
    match RedisTransport::connect(&settings.queue.redis).await {
        Ok(_) => Check::new("Redis Queue", Status::Ok, "reachable"),
        Err(_) => Check::new("Redis Queue", Status::Fail, "unavailable"),
    }
}

async fn storage_check(settings: &Settings) -> Check {
    let storage = match LocalStorage::new(&settings.filesystem) {
        Ok(storage) => storage,
        Err(_) => return Check::new("Storage", Status::Fail, "invalid configuration"),
    };
    let name = format!("doctor-probe-{}.tmp", Uuid::now_v7());
    if storage
        .put(StorageDisk::Private, &name, b"doctor")
        .await
        .is_err()
    {
        return Check::new("Storage", Status::Fail, "private disk is not writable");
    }
    match storage.delete(StorageDisk::Private, &name).await {
        Ok(true) => Check::new(
            "Storage",
            Status::Ok,
            "local storage write/delete probe passed",
        ),
        _ => Check::new("Storage", Status::Fail, "write probe cleanup failed"),
    }
}

fn storage_link_check(root: &Path, settings: &Settings) -> Check {
    let link = root.join("public/storage");
    let expected = absolute(root, &settings.filesystem.public_root());
    let metadata = match fs::symlink_metadata(&link) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Check::new(
                "Storage Link",
                Status::Warn,
                "missing; run make storage-link if public runtime files are used",
            );
        }
        Err(_) => {
            return Check::new("Storage Link", Status::Warn, "could not inspect link");
        }
    };

    if !metadata.file_type().is_symlink() {
        return Check::new(
            "Storage Link",
            Status::Warn,
            "public/storage exists but is not a symlink",
        );
    }

    let target = match fs::read_link(&link) {
        Ok(target) if target.is_absolute() => target,
        Ok(target) => link.parent().unwrap_or(root).join(target),
        Err(_) => return Check::new("Storage Link", Status::Warn, "unreadable symlink"),
    };
    match (target.canonicalize(), expected.canonicalize()) {
        (Ok(actual), Ok(expected)) if actual == expected => {
            Check::new("Storage Link", Status::Ok, "public/storage ready")
        }
        _ => Check::new(
            "Storage Link",
            Status::Warn,
            "points elsewhere; inspect or rerun make storage-link",
        ),
    }
}

fn logging_check(settings: &Settings) -> Check {
    if !settings.logging.writes_file() {
        return Check::new("File Logging", Status::Skip, "LOG_OUTPUT=stdout");
    }

    if fs::create_dir_all(&settings.logging.directory).is_err() {
        return Check::new(
            "File Logging",
            Status::Fail,
            "log directory is not writable",
        );
    }
    match tempfile::NamedTempFile::new_in(&settings.logging.directory) {
        Ok(_) => Check::new(
            "File Logging",
            Status::Ok,
            format!(
                "{:?} rotation, retain {}",
                settings.logging.rotation, settings.logging.max_files
            ),
        ),
        Err(_) => Check::new(
            "File Logging",
            Status::Fail,
            "log directory is not writable",
        ),
    }
}

fn git_hook_check(root: &Path) -> Check {
    let output = Command::new("git")
        .args(["config", "--local", "--get", "core.hooksPath"])
        .current_dir(root)
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if path == ".githooks" {
                Check::new("Pre-commit", Status::Ok, ".githooks enabled")
            } else {
                Check::new(
                    "Pre-commit",
                    Status::Warn,
                    format!("custom hooksPath={path}; make hooks-install not active"),
                )
            }
        }
        _ => Check::new(
            "Pre-commit",
            Status::Warn,
            "not installed; run make hooks-install",
        ),
    }
}

fn absolute(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn print_report(checks: &[Check]) {
    println!();
    for check in checks {
        println!(
            "{:<16} {:<5} {}",
            check.name,
            check.status.label(),
            check.detail
        );
    }
    println!();
    if checks.iter().any(|check| check.status.blocks()) {
        println!("NOT READY");
    } else {
        println!("READY");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocking_status_is_only_fail() {
        assert!(Status::Fail.blocks());
        assert!(!Status::Warn.blocks());
        assert!(!Status::Skip.blocks());
        assert!(!Status::Ok.blocks());
    }

    #[test]
    fn absolute_resolves_relative_paths_against_project_root() {
        let root = Path::new("/tmp/project");
        assert_eq!(
            absolute(root, Path::new("storage/app/public")),
            PathBuf::from("/tmp/project/storage/app/public")
        );
        assert_eq!(
            absolute(root, Path::new("/data/public")),
            PathBuf::from("/data/public")
        );
    }
}
