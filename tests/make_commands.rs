#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture {
    directory: tempfile::TempDir,
    path: std::ffi::OsString,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary Make project");
        let root = directory.path();
        fs::write(root.join("Makefile"), include_str!("../Makefile"))
            .expect("write fixture Makefile");
        let bin = root.join("bin");
        fs::create_dir(&bin).expect("create stub bin directory");
        for executable in ["cargo", "docker"] {
            let path = bin.join(executable);
            fs::write(
                &path,
                r#"#!/bin/sh
printf '%s\000' "$@" > "$CAPTURE_ARGS"
printf '%s\000' "${ARTISAN_NAME-unset}" "${ARTISAN_PACKAGE-unset}" "${ARTISAN_FEATURES-unset}" "${ARTISAN_DEV-unset}" "${ARTISAN_SCHEDULER-unset}" "${ARTISAN_FORCE-unset}" "${ARTISAN_SHUTDOWN_TIMEOUT-unset}" "${DB_DRIVER-unset}" > "$CAPTURE_ENV"
"#,
            )
            .expect("write stub executable");
            fs::set_permissions(path, fs::Permissions::from_mode(0o755))
                .expect("make stub executable");
        }
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let path = std::env::join_paths(paths).expect("join stub PATH");
        Self { directory, path }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn command(&self) -> Command {
        let mut command = Command::new("make");
        command
            .arg("--no-print-directory")
            .current_dir(self.root())
            .env_clear()
            .env("PATH", &self.path)
            .env("CAPTURE_ARGS", self.root().join("args"))
            .env("CAPTURE_ENV", self.root().join("env"));
        command
    }

    fn run(&self, arguments: &[&str]) -> Output {
        self.command()
            .args(arguments)
            .output()
            .expect("run GNU Make")
    }

    fn captured(&self, file: &str) -> Vec<String> {
        fs::read(self.root().join(file))
            .expect("read captured invocation")
            .split(|byte| *byte == 0)
            .filter(|field| !field.is_empty())
            .map(|field| String::from_utf8(field.to_vec()).expect("UTF-8 fixture argument"))
            .collect()
    }
}

fn succeeded(output: Output) {
    assert!(
        output.status.success(),
        "Make failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn artisan_wrappers_forward_the_expected_subcommands() {
    let fixture = Fixture::new();
    for (target, command) in [
        ("install", "install"),
        ("setup", "install"),
        ("serve", "serve"),
        ("key", "key:generate"),
        ("controller", "make:controller"),
        ("model", "make:model"),
        ("request", "make:request"),
        ("dto", "make:request"),
        ("service", "make:service"),
        ("repository", "make:repository"),
        ("job", "make:job"),
        ("middleware", "make:middleware"),
        ("migration", "make:migration"),
        ("require", "require"),
        ("remove", "remove"),
        ("update", "update"),
    ] {
        succeeded(fixture.run(&[target]));
        assert_eq!(
            fixture.captured("args"),
            ["run", "--locked", "--bin", "artisan", "--", command],
            "wrong wrapper for {target}"
        );
    }
}

#[test]
fn wrapper_options_are_passed_as_environment_values() {
    let fixture = Fixture::new();
    succeeded(fixture.run(&[
        "controller",
        "NAME=CustomerController",
        "PACKAGE=serde@1",
        "FEATURES=derive,rc",
        "DEV=1",
        "SCHEDULER=1",
        "FORCE=1",
        "SHUTDOWN_TIMEOUT=12",
    ]));
    assert_eq!(
        fixture.captured("env"),
        [
            "CustomerController",
            "serde@1",
            "derive,rc",
            "1",
            "1",
            "1",
            "12",
            "unset",
        ]
    );
}

#[test]
fn literal_make_and_shell_expressions_never_execute() {
    let fixture = Fixture::new();
    for (variable, target) in [
        ("NAME", "controller"),
        ("PACKAGE", "require"),
        ("FEATURES", "require"),
        ("DB_FEATURES", "docker-build"),
        ("DB_DRIVER", "docker-db"),
        ("NAME", "check"),
    ] {
        let make_marker = fixture.root().join(format!("make-marker-{variable}"));
        let shell_marker = fixture.root().join(format!("shell-marker-{variable}"));
        let expression = format!(
            "$(shell touch {})`touch {}`; touch {}",
            make_marker.display(),
            shell_marker.display(),
            shell_marker.display()
        );
        let assignment = format!("{variable}={expression}");
        succeeded(fixture.run(&[target, &assignment]));
        assert!(
            !make_marker.exists() && !shell_marker.exists(),
            "Make or shell interpreted {variable}"
        );
        let captured = if variable.starts_with("DB_") {
            fixture.captured("args")
        } else {
            fixture.captured("env")
        };
        assert!(
            captured.iter().any(|value| value.contains(&expression)),
            "literal {variable} did not survive forwarding: {captured:?}"
        );
    }
}

#[test]
fn default_database_selector_does_not_override_dotenv_for_native_processes() {
    let fixture = Fixture::new();
    succeeded(fixture.run(&["serve"]));
    assert_eq!(fixture.captured("env").last().unwrap(), "unset");

    succeeded(
        fixture
            .command()
            .arg("serve")
            .env("DB_DRIVER", "mysql")
            .output()
            .expect("run Make with explicit driver"),
    );
    assert_eq!(fixture.captured("env").last().unwrap(), "mysql");
}

#[test]
fn database_urls_are_checked_without_shell_interpolation() {
    let fixture = Fixture::new();
    let marker: PathBuf = fixture.root().join("database-url-marker");
    let assignment = format!(
        "TEST_DATABASE_URL=postgres://example/$(shell touch {})`touch {}`",
        marker.display(),
        marker.display()
    );
    succeeded(fixture.run(&["test-postgres", &assignment]));
    assert!(!marker.exists(), "database URL was evaluated as code");
}
