//! Explicit, offline compiler check for generated projects. Run separately from
//! normal tests because it starts Cargo with its own target directory.

use std::{fs, path::Path, process::Command};

fn copy_directory(source: &Path, target: &Path) {
    fs::create_dir_all(target).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        assert!(
            !entry.file_type().unwrap().is_symlink(),
            "fixture source must not contain symlinks"
        );
        let destination = target.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_directory(&entry.path(), &destination);
        } else {
            fs::copy(entry.path(), destination).unwrap();
        }
    }
}

fn successful(command: &mut Command) {
    let status = command.status().expect("start fixture command");
    assert!(status.success(), "fixture command failed: {command:?}");
}

#[test]
#[ignore = "compiles a generated fixture with all database configurations; run make test-scaffolds"]
fn generated_scaffolds_compile_for_every_supported_driver() {
    let original = Path::new(env!("CARGO_MANIFEST_DIR"));
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path();
    for filename in [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "rust-toolchain.toml",
    ] {
        fs::copy(original.join(filename), root.join(filename)).unwrap();
    }
    for directory in ["src", "migrations"] {
        copy_directory(&original.join(directory), &root.join(directory));
    }
    let route = fs::read(root.join("src/routes/api.rs")).unwrap();
    let worker = fs::read(root.join("src/jobs/worker.rs")).unwrap();
    for (command, name) in [
        ("make:controller", "CustomerController"),
        ("make:model", "Customer"),
        ("make:model", "Uuid"),
        ("make:request", "CreateCustomerRequest"),
        ("make:service", "CustomerService"),
        ("make:repository", "CustomerRepository"),
        ("make:job", "SendInvoiceJob"),
        ("make:middleware", "AuditMiddleware"),
        ("make:migration", "create_customers_table"),
    ] {
        successful(
            Command::new(env!("CARGO_BIN_EXE_artisan"))
                .args([command, name])
                .current_dir(root),
        );
    }
    assert_eq!(fs::read(root.join("src/routes/api.rs")).unwrap(), route);
    assert_eq!(fs::read(root.join("src/jobs/worker.rs")).unwrap(), worker);
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let target = original.join("target/artisan-scaffold-check");
    for args in [
        vec!["fmt", "--all", "--", "--check"],
        vec![
            "clippy",
            "--locked",
            "--offline",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
        vec![
            "check",
            "--locked",
            "--offline",
            "--all-targets",
            "--no-default-features",
            "--features",
            "postgres",
        ],
        vec![
            "check",
            "--locked",
            "--offline",
            "--all-targets",
            "--no-default-features",
            "--features",
            "mysql",
        ],
    ] {
        successful(
            Command::new(&cargo)
                .args(args)
                .env("CARGO_TARGET_DIR", &target)
                .current_dir(root),
        );
    }
}
