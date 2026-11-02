//! Offline compiler and factory-contract checks for generated projects. Run separately from
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
    for directory in ["src", "migrations", "rollbacks", "vendor"] {
        copy_directory(&original.join(directory), &root.join(directory));
    }
    let route = fs::read(root.join("src/routes/api.rs")).unwrap();
    let worker = fs::read(root.join("src/jobs/worker.rs")).unwrap();
    for (command, name) in [
        ("make:controller", "CustomerController"),
        ("make:model", "Customer"),
        ("make:model", "CompanyModel"),
        ("make:model", "Person"),
        ("make:model", "Status"),
        ("make:model", "Uuid"),
        ("make:request", "CreateCustomerRequest"),
        ("make:service", "CustomerService"),
        ("make:repository", "CustomerRepository"),
        ("make:repository", "PersonRepo"),
        ("make:repository", "StatusRepository"),
        ("make:job", "SendInvoiceJob"),
        ("make:middleware", "AuditMiddleware"),
        ("make:migration", "create_customers_table"),
        ("make:migration", "create_people_table"),
        ("make:seeder", "CustomerSeeder"),
    ] {
        successful(
            Command::new(env!("CARGO_BIN_EXE_artisan"))
                .args([command, name])
                .current_dir(root),
        );
    }
    let request =
        fs::read_to_string(root.join("src/domain/dtos/create_customer_request.rs")).unwrap();
    for required in [
        "derive(Deserialize, Validate)",
        "serde(deny_unknown_fields)",
        "message = \"must contain 1 to 100 characters\"",
        "impl Normalize for CreateCustomerRequest",
    ] {
        assert!(
            request.contains(required),
            "generated request omitted validation contract {required:?}"
        );
    }

    let seeder = fs::read_to_string(root.join("src/seeders/customer_seeder.rs")).unwrap();
    assert!(seeder.contains("pub async fn run"));
    let registry = fs::read_to_string(root.join("src/seeders/mod.rs")).unwrap();
    assert!(registry.contains("\"CustomerSeeder\" => customer_seeder::run(database).await"));

    for driver in ["postgres", "mysql"] {
        let rollback_dir = root.join("rollbacks").join(driver);
        let rollback_files: Vec<_> = fs::read_dir(&rollback_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains("create_customers_table")
            })
            .collect();
        assert_eq!(rollback_files.len(), 1);
        let contents = fs::read_to_string(rollback_files[0].path()).unwrap();
        assert!(contents.contains("SCAFFOLD ONLY"));
    }

    let controller = fs::read_to_string(root.join("src/handlers/customer_handler.rs")).unwrap();
    for required in [
        "response::ApiJson",
        "crate::response::success/created/cursor",
        "examples/module_init.rs",
        "docs/module-init.md",
        "src/swagger/mod.rs",
    ] {
        assert!(
            controller.contains(required),
            "generated controller omitted global response guidance {required:?}"
        );
    }

    assert_eq!(fs::read(root.join("src/routes/api.rs")).unwrap(), route);
    assert_eq!(fs::read(root.join("src/jobs/worker.rs")).unwrap(), worker);
    fs::create_dir(root.join("tests")).unwrap();
    fs::write(
        root.join("tests/generated_models.rs"),
        r#"use rust_api_skeleton::domain::models;

macro_rules! assert_factory {
    ($model:ty, $primary_key:ident) => {{
        let created = <$model>::new();
        let default = <$model>::default();
        assert_eq!(created.$primary_key.get_version_num(), 7);
        assert_eq!(default.$primary_key.get_version_num(), 7);
        assert_ne!(created.$primary_key, default.$primary_key);
        assert_eq!(created.created_at, created.updated_at);
        assert_eq!(default.created_at, default.updated_at);
    }};
}

#[test]
fn generated_models_create_named_uuid_v7_keys_with_consistent_timestamps() {
    assert_factory!(models::customer::Customer, customer_id);
    assert_factory!(models::company::Company, company_id);
    assert_factory!(models::person::Person, person_id);
    assert_factory!(models::status::Status, status_id);
    assert_factory!(models::uuid::Uuid, uuid_id);
}
"#,
    )
    .unwrap();
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
            "test",
            "--locked",
            "--offline",
            "--all-features",
            "--test",
            "generated_models",
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
