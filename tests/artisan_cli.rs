#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Output},
};

struct Fixture(tempfile::TempDir);

impl Fixture {
    fn new() -> Self {
        let fixture = Self(tempfile::tempdir().unwrap());
        fs::create_dir(fixture.root().join("src")).unwrap();
        fs::create_dir(fixture.root().join("bin")).unwrap();
        fs::write(
            fixture.root().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::write(fixture.root().join("src/lib.rs"), "").unwrap();
        fs::write(
            fixture.root().join(".env.example"),
            "DB_DRIVER=mysql\nJWT_SECRET=replace-with-generated-key\n",
        )
        .unwrap();
        let cargo = fixture.root().join("bin/cargo");
        fs::write(&cargo, "#!/bin/sh\nprintf '%s\\000' \"$@\" >> cargo-arguments\nexit \"${FAKE_CARGO_EXIT:-0}\"\n").unwrap();
        fs::set_permissions(cargo, fs::Permissions::from_mode(0o755)).unwrap();
        fixture
    }

    fn root(&self) -> &Path {
        self.0.path()
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_artisan"));
        let mut paths = vec![self.root().join("bin")];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        command
            .current_dir(self.root())
            .env("PATH", std::env::join_paths(paths).unwrap());
        for name in [
            "ARTISAN_NAME",
            "ARTISAN_PACKAGE",
            "ARTISAN_FEATURES",
            "ARTISAN_DEV",
            "ARTISAN_FORCE",
            "ARTISAN_SEED",
            "ARTISAN_STEP",
            "ARTISAN_CLASS",
            "ARTISAN_SCHEDULER",
            "ARTISAN_SHUTDOWN_TIMEOUT",
        ] {
            command.env_remove(name);
        }
        command
    }

    fn cargo_arguments(&self) -> Vec<String> {
        fs::read(self.root().join("cargo-arguments"))
            .unwrap()
            .split(|b| *b == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| String::from_utf8(arg.into()).unwrap())
            .collect()
    }
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn help_works_without_project_or_application_configuration() {
    let outside = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_artisan"))
        .arg("--help")
        .current_dir(outside.path())
        .output()
        .unwrap();
    success(&output);
    let help = String::from_utf8(output.stdout).unwrap();
    assert!(
        help.contains("make:controller")
            && help.contains("install")
            && help.contains("doctor")
            && help.contains("hooks:install")
            && help.contains("migrate:rollback")
            && help.contains("migrate:fresh")
            && help.contains("db:seed")
            && help.contains("make:seeder")
            && help.contains("serve")
    );
}

#[test]
fn install_is_repeatable_uses_locked_cargo_and_never_runs_migrations() {
    let fixture = Fixture::new();
    let first = fixture.command().arg("install").output().unwrap();
    success(&first);
    let env = fs::read_to_string(fixture.root().join(".env")).unwrap();
    let secret = env
        .lines()
        .find_map(|l| l.strip_prefix("JWT_SECRET="))
        .unwrap();
    assert_eq!(secret.len(), 64);
    assert!(!String::from_utf8_lossy(&first.stdout).contains(secret));
    assert!(!String::from_utf8_lossy(&first.stderr).contains(secret));
    assert_eq!(
        fixture.cargo_arguments(),
        ["fetch", "--locked", "build", "--locked", "--bins"]
    );
    success(&fixture.command().arg("install").output().unwrap());
    assert_eq!(
        fs::read_to_string(fixture.root().join(".env")).unwrap(),
        env
    );
    assert_eq!(
        fs::metadata(fixture.root().join(".env"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(!fixture.root().join("migrations").exists());
}

#[test]
fn key_rotation_requires_explicit_force_including_make_boolean_environment() {
    let fixture = Fixture::new();
    success(&fixture.command().arg("key:generate").output().unwrap());
    let original = fs::read_to_string(fixture.root().join(".env")).unwrap();
    let refused = fixture
        .command()
        .arg("key:generate")
        .env("ARTISAN_FORCE", "0")
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert_eq!(
        fs::read_to_string(fixture.root().join(".env")).unwrap(),
        original
    );
    success(
        &fixture
            .command()
            .arg("key:generate")
            .env("ARTISAN_FORCE", "1")
            .output()
            .unwrap(),
    );
    assert_ne!(
        fs::read_to_string(fixture.root().join(".env")).unwrap(),
        original
    );
    assert!(!fixture.root().join("cargo-arguments").exists());
}

#[test]
fn storage_link_creates_public_symlink_and_refuses_regular_file_overwrite() {
    let fixture = Fixture::new();
    success(&fixture.command().arg("storage:link").output().unwrap());
    let link = fixture.root().join("public/storage");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(fixture.root().join("storage/app/public").is_dir());

    success(&fixture.command().arg("storage:link").output().unwrap());

    fs::remove_file(&link).unwrap();
    fs::write(&link, b"keep").unwrap();
    let refused = fixture
        .command()
        .arg("storage:link")
        .env("ARTISAN_FORCE", "1")
        .output()
        .unwrap();
    assert!(!refused.status.success());
    assert_eq!(fs::read(&link).unwrap(), b"keep");
}

#[test]
fn dependency_commands_pass_data_as_arguments_and_propagate_cargo_failures() {
    let fixture = Fixture::new();
    success(
        &fixture
            .command()
            .arg("require")
            .env("ARTISAN_PACKAGE", "serde@1")
            .env("ARTISAN_FEATURES", "derive")
            .env("ARTISAN_DEV", "1")
            .output()
            .unwrap(),
    );
    assert_eq!(
        fixture.cargo_arguments(),
        ["add", "--features", "derive", "--dev", "serde@1"]
    );
    fs::remove_file(fixture.root().join("cargo-arguments")).unwrap();
    success(
        &fixture
            .command()
            .args(["remove", "serde", "--dev"])
            .output()
            .unwrap(),
    );
    success(
        &fixture
            .command()
            .args(["update", "serde"])
            .output()
            .unwrap(),
    );
    assert_eq!(
        fixture.cargo_arguments(),
        ["remove", "--dev", "serde", "update", "--package", "serde"]
    );
    let failed = fixture
        .command()
        .args(["require", "serde"])
        .env("FAKE_CARGO_EXIT", "19")
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("Cargo add failed"));
}

#[test]
fn invalid_names_packages_and_options_cannot_run_commands() {
    let fixture = Fixture::new();
    for (command, value) in [
        ("require", "serde;touch injected"),
        ("require", "$(touch injected)"),
        ("require", "--path=../outside"),
        ("make:controller", "../outside"),
        ("make:controller", "$(touch injected)"),
    ] {
        let output = fixture.command().args([command, value]).output().unwrap();
        assert!(!output.status.success(), "accepted {command} {value}");
        assert!(!fixture.root().join("injected").exists());
        assert!(!fixture.root().join("cargo-arguments").exists());
    }
    let output = fixture
        .command()
        .args(["require", "serde", "--features", "derive;touch injected"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!fixture.root().join("cargo-arguments").exists());
}
