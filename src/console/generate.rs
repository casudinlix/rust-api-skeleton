use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{Context, ensure};
use chrono::{DateTime, NaiveDateTime, TimeDelta, Utc};
use heck::{ToSnakeCase, ToUpperCamelCase};

use super::{
    project::{self, FileChange},
    templates,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Controller,
    Model,
    Request,
    Service,
    Repository,
    Job,
    Middleware,
    Migration,
    Seeder,
}

pub fn run(root: &Path, kind: Kind, name: &str) -> anyhow::Result<Vec<PathBuf>> {
    run_at(root, kind, name, Utc::now())
}

fn run_at(root: &Path, kind: Kind, name: &str, now: DateTime<Utc>) -> anyhow::Result<Vec<PathBuf>> {
    let name = Name::parse(kind, name)?;
    if kind == Kind::Migration {
        return migration(root, &name.module, now);
    }
    if kind == Kind::Seeder {
        return seeder(root, &name);
    }

    let artifacts = artifacts(kind, &name);
    let mut changes = Vec::with_capacity(artifacts.len() * 2);
    let mut created = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        syn::parse_file(&artifact.contents).context("generated Rust source is invalid")?;
        ensure!(
            project::read_optional(root, &artifact.relative)?.is_none(),
            "{} already exists; refusing to overwrite it",
            artifact.relative.display()
        );
        let parent = artifact.relative.parent().context("output has no parent")?;
        let module_file = parent.join("mod.rs");
        let declaration = module_change(root, &module_file, &artifact.module)?;
        created.push(artifact.relative.clone());
        changes.push(FileChange {
            relative: artifact.relative,
            before: None,
            after: artifact.contents,
            private: false,
        });
        changes.push(declaration);
    }
    project::apply_changes(root, changes)?;
    Ok(created)
}

struct Name {
    module: String,
    entity: String,
}

impl Name {
    fn parse(kind: Kind, name: &str) -> anyhow::Result<Self> {
        ensure!(
            !name.is_empty()
                && name.len() <= 100
                && name.as_bytes()[0].is_ascii_alphabetic()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
            "use an ASCII PascalCase or snake_case name starting with a letter (maximum 100 characters)"
        );
        // Check the input as well as its normalized forms: `Self` and `self`
        // must not become accepted merely because a suffix is appended later.
        validate_identifier(name)?;
        let normalized = name.to_snake_case();
        let suffixes: &[&str] = match kind {
            Kind::Controller => &["controller", "handler"],
            Kind::Model => &["model"],
            Kind::Request => &["request", "dto"],
            Kind::Service => &["service"],
            Kind::Repository => &["repository", "repo"],
            Kind::Job => &["job"],
            Kind::Middleware => &["middleware"],
            Kind::Migration => &["migration"],
            Kind::Seeder => &["seeder"],
        };
        ensure!(
            !suffixes.contains(&normalized.as_str()),
            "provide a meaningful name before the generator suffix"
        );
        let module = suffixes
            .iter()
            .find_map(|suffix| normalized.strip_suffix(&format!("_{suffix}")))
            .unwrap_or(&normalized)
            .to_owned();
        ensure!(module.len() <= 100, "normalized module name is too long");
        let entity = module.to_upper_camel_case();
        // For models/repositories, NAME supplies the singular entity explicitly.
        // Do not infer table names: Person and Status remain person and status.
        validate_identifier(&module)?;
        validate_identifier(&entity)?;
        Ok(Self { module, entity })
    }
}

fn validate_identifier(name: &str) -> anyhow::Result<()> {
    // `gen` is reserved by Rust 2024; reserve future keywords as well, regardless
    // of which edition the syn parser currently understands.
    const RESERVED: &[&str] = &[
        "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate",
        "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl",
        "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub",
        "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "try",
        "type", "typeof", "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
    ];
    ensure!(
        !RESERVED.contains(&name),
        "{name:?} is a reserved Rust identifier"
    );
    syn::parse_str::<syn::Ident>(name)
        .with_context(|| format!("{name:?} is not a valid Rust identifier"))?;
    Ok(())
}

struct Artifact {
    relative: PathBuf,
    module: String,
    contents: String,
}

impl Artifact {
    fn new(directory: &str, module: String, contents: String) -> Self {
        Self {
            relative: Path::new(directory).join(format!("{module}.rs")),
            module,
            contents,
        }
    }
}

fn artifacts(kind: Kind, name: &Name) -> Vec<Artifact> {
    let module = &name.module;
    let entity = &name.entity;
    match kind {
        Kind::Controller => vec![Artifact::new(
            "src/handlers",
            format!("{module}_handler"),
            templates::controller(entity),
        )],
        Kind::Model => vec![Artifact::new(
            "src/domain/models",
            module.clone(),
            templates::model(module, entity),
        )],
        Kind::Request => vec![Artifact::new(
            "src/domain/dtos",
            format!("{module}_request"),
            templates::request(entity),
        )],
        Kind::Service => vec![Artifact::new(
            "src/services",
            format!("{module}_service"),
            templates::service(entity),
        )],
        Kind::Repository => vec![
            Artifact::new(
                "src/repositories",
                format!("{module}_repository"),
                templates::repository(module, entity),
            ),
            Artifact::new(
                "src/repositories/postgres",
                format!("{module}_pg_repo"),
                templates::postgres_repository(module, entity),
            ),
            Artifact::new(
                "src/repositories/mysql",
                format!("{module}_mysql_repo"),
                templates::mysql_repository(module, entity),
            ),
        ],
        Kind::Job => vec![Artifact::new(
            "src/jobs",
            module.clone(),
            templates::job(module, entity),
        )],
        Kind::Middleware => vec![Artifact::new(
            "src/middleware",
            module.clone(),
            templates::middleware(),
        )],
        Kind::Migration | Kind::Seeder => Vec::new(),
    }
}

fn seeder(root: &Path, name: &Name) -> anyhow::Result<Vec<PathBuf>> {
    let module = format!("{}_seeder", name.module);
    let class = format!("{}Seeder", name.entity);
    let relative = Path::new("src/seeders").join(format!("{module}.rs"));
    ensure!(
        project::read_optional(root, &relative)?.is_none(),
        "{} already exists; refusing to overwrite it",
        relative.display()
    );

    let contents = templates::seeder(&class);
    syn::parse_file(&contents).context("generated seeder source is invalid")?;

    let registry_path = Path::new("src/seeders/mod.rs");
    let before = project::read_optional(root, registry_path)?
        .context("src/seeders/mod.rs is missing; restore the seeder registry")?;
    let parsed = syn::parse_file(&before).context("src/seeders/mod.rs has invalid Rust syntax")?;
    ensure!(
        !parsed.items.iter().any(|item| item_binds(item, &module)),
        "src/seeders/mod.rs already declares {module}"
    );
    ensure!(
        !before.contains(&format!("\"{class}\"")),
        "seeder class {class} is already registered"
    );
    const MARKER: &str = "        // ARTISAN_SEEDER_REGISTRY\n";
    ensure!(
        before.matches(MARKER).count() == 1,
        "src/seeders/mod.rs registry marker is missing or duplicated"
    );

    let mut after = before.clone();
    let declaration = format!("pub mod {module};\n");
    if let Some(index) = after.find("\n\nuse anyhow::bail;") {
        after.insert_str(index + 1, &declaration);
    } else {
        anyhow::bail!("src/seeders/mod.rs module insertion anchor is missing");
    }
    let entry = format!("        \"{class}\" => {module}::run(database).await,\n");
    after = after.replacen(MARKER, &(entry + MARKER), 1);

    project::apply_changes(
        root,
        vec![
            FileChange {
                relative: relative.clone(),
                before: None,
                after: contents,
                private: false,
            },
            FileChange {
                relative: registry_path.to_path_buf(),
                before: Some(before),
                after,
                private: false,
            },
        ],
    )?;
    Ok(vec![relative])
}

fn module_change(root: &Path, relative: &Path, module: &str) -> anyhow::Result<FileChange> {
    let before = project::read_optional(root, relative)?;
    let mut after = before.clone().unwrap_or_default();
    let parsed = syn::parse_file(&after)
        .with_context(|| format!("{} has invalid Rust syntax", relative.display()))?;
    ensure!(
        !parsed.items.iter().any(|item| item_binds(item, module)),
        "{} already declares or imports {module}; refusing to add a duplicate module",
        relative.display()
    );
    if !after.is_empty() && !after.ends_with('\n') {
        after.push('\n');
    }
    // Keep the existing module group intact: rustfmt reorders adjacent module
    // declarations, but respects a blank line separating a newly added group.
    if !after.is_empty() && !after.ends_with("\n\n") {
        after.push('\n');
    }
    after.push_str(&format!("pub mod {module};\n"));
    Ok(FileChange {
        relative: relative.to_path_buf(),
        before,
        after,
        private: false,
    })
}

fn item_binds(item: &syn::Item, module: &str) -> bool {
    match item {
        syn::Item::Const(item) => ident_binds(&item.ident, module),
        syn::Item::Enum(item) => ident_binds(&item.ident, module),
        syn::Item::ExternCrate(item) => ident_binds(
            item.rename.as_ref().map_or(&item.ident, |(_, name)| name),
            module,
        ),
        syn::Item::Fn(item) => ident_binds(&item.sig.ident, module),
        syn::Item::Macro(item) => item
            .ident
            .as_ref()
            .is_some_and(|name| ident_binds(name, module)),
        syn::Item::Mod(item) => ident_binds(&item.ident, module),
        syn::Item::Static(item) => ident_binds(&item.ident, module),
        syn::Item::Struct(item) => ident_binds(&item.ident, module),
        syn::Item::Trait(item) => ident_binds(&item.ident, module),
        syn::Item::TraitAlias(item) => ident_binds(&item.ident, module),
        syn::Item::Type(item) => ident_binds(&item.ident, module),
        syn::Item::Union(item) => ident_binds(&item.ident, module),
        syn::Item::Use(item) => use_binds(&item.tree, module, None),
        _ => false,
    }
}

fn use_binds(tree: &syn::UseTree, module: &str, parent: Option<&syn::Ident>) -> bool {
    match tree {
        syn::UseTree::Path(path) => use_binds(&path.tree, module, Some(&path.ident)),
        syn::UseTree::Name(name) if name.ident == "self" => {
            parent.is_some_and(|name| ident_binds(name, module))
        }
        syn::UseTree::Name(name) => ident_binds(&name.ident, module),
        syn::UseTree::Rename(rename) => ident_binds(&rename.rename, module),
        syn::UseTree::Group(group) => group
            .items
            .iter()
            .any(|item| use_binds(item, module, parent)),
        syn::UseTree::Glob(_) => false,
    }
}

fn ident_binds(identifier: &syn::Ident, module: &str) -> bool {
    let identifier = identifier.to_string();
    identifier.strip_prefix("r#").unwrap_or(&identifier) == module
}

fn migration(root: &Path, name: &str, now: DateTime<Utc>) -> anyhow::Result<Vec<PathBuf>> {
    let mut largest_version = 0;
    for driver in ["postgres", "mysql"] {
        let directory = Path::new("migrations").join(driver);
        let path = project::checked_path(root, &directory)?;
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error).context("could not inspect migration directory"),
        };
        for entry in entries {
            let entry = entry.context("could not inspect existing migration")?;
            let file_name = entry.file_name();
            if Path::new(&file_name)
                .extension()
                .is_none_or(|extension| extension != "sql")
            {
                continue;
            }
            let relative = directory.join(&file_name);
            let absolute = project::checked_path(root, &relative)?;
            ensure!(
                absolute.is_file(),
                "{} is not a regular migration file",
                relative.display()
            );
            let file_name = file_name
                .to_str()
                .context("migration filename must be valid UTF-8")?;
            let (version, description) = file_name
                .strip_suffix(".sql")
                .and_then(|stem| stem.split_once('_'))
                .with_context(|| {
                    format!("{} has an invalid migration filename", relative.display())
                })?;
            let version: i64 = version
                .parse()
                .context("existing migration version is not an integer")?;
            ensure!(
                version >= 0,
                "existing migration version must be non-negative"
            );
            let description = description
                .strip_suffix(".up")
                .or_else(|| description.strip_suffix(".down"))
                .unwrap_or(description);
            ensure!(
                description != name,
                "migration {name:?} already exists; choose a new description"
            );
            largest_version = largest_version.max(version);
        }
    }
    let version = next_version(now, largest_version)?;
    let filename = format!("{version}_{name}.sql");
    let mut created = Vec::new();
    let mut changes = Vec::new();
    for (driver, display, counterpart) in [
        ("postgres", "PostgreSQL", "MySQL"),
        ("mysql", "MySQL", "PostgreSQL"),
    ] {
        let relative = Path::new("migrations").join(driver).join(&filename);
        let rollback = Path::new("rollbacks").join(driver).join(&filename);
        ensure!(
            project::read_optional(root, &relative)?.is_none(),
            "migration already exists"
        );
        ensure!(
            project::read_optional(root, &rollback)?.is_none(),
            "rollback companion already exists"
        );
        created.push(relative.clone());
        created.push(rollback.clone());
        changes.push(FileChange {
            relative,
            before: None,
            after: templates::migration(name, display, counterpart),
            private: false,
        });
        changes.push(FileChange {
            relative: rollback,
            before: None,
            after: templates::rollback(name, display, counterpart),
            private: false,
        });
    }
    project::apply_changes(root, changes)?;
    Ok(created)
}

fn next_version(now: DateTime<Utc>, largest_version: i64) -> anyhow::Result<String> {
    let timestamp = now.format("%Y%m%d%H%M%S").to_string();
    let current: i64 = timestamp
        .parse()
        .context("current UTC timestamp is invalid")?;
    if largest_version < current {
        return Ok(timestamp);
    }
    let latest = NaiveDateTime::parse_from_str(&largest_version.to_string(), "%Y%m%d%H%M%S")
        .context("latest migration version cannot be advanced as a UTC timestamp")?;
    let next = latest
        .checked_add_signed(TimeDelta::seconds(1))
        .context("migration timestamp overflowed")?
        .format("%Y%m%d%H%M%S")
        .to_string();
    ensure!(
        next.len() == 14,
        "migration timestamp is outside the supported calendar range"
    );
    Ok(next)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs};

    use super::*;

    fn write(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for path in [
            "src/handlers/mod.rs",
            "src/domain/models/mod.rs",
            "src/domain/dtos/mod.rs",
            "src/services/mod.rs",
            "src/jobs/mod.rs",
            "src/middleware/mod.rs",
            "src/repositories/postgres/mod.rs",
            "src/repositories/mysql/mod.rs",
            "src/seeders/mod.rs",
        ] {
            write(
                root.path(),
                path,
                "// Existing code must stay exactly as written.\npub fn existing() {}\n",
            );
        }
        write(
            root.path(),
            "src/repositories/mod.rs",
            "#[cfg(feature = \"postgres\")]\npub mod postgres;\n#[cfg(feature = \"mysql\")]\npub mod mysql;\n",
        );
        write(
            root.path(),
            "src/seeders/mod.rs",
            "pub mod database_seeder;\n\nuse anyhow::bail;\nuse crate::database::DatabasePool;\n\npub async fn run(database: &DatabasePool, class: &str) -> anyhow::Result<()> {\n    match class {\n        \"DatabaseSeeder\" => database_seeder::run(database).await,\n        // ARTISAN_SEEDER_REGISTRY\n        _ => bail!(\"unknown seeder\"),\n    }\n}\n",
        );
        root
    }

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn visit(root: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    visit(root, &entry.path(), files);
                } else {
                    files.insert(
                        entry.path().strip_prefix(root).unwrap().to_path_buf(),
                        fs::read(entry.path()).unwrap(),
                    );
                }
            }
        }
        let mut files = BTreeMap::new();
        visit(root, root, &mut files);
        files
    }

    fn now() -> DateTime<Utc> {
        "2026-11-01T10:00:00Z".parse().unwrap()
    }

    #[test]
    fn controller_creates_compilable_module_without_rewriting_existing_code() {
        let root = fixture();
        let module = root.path().join("src/handlers/mod.rs");
        let original = fs::read_to_string(&module).unwrap();
        let created = run(root.path(), Kind::Controller, "CustomerController").unwrap();
        assert_eq!(created, [PathBuf::from("src/handlers/customer_handler.rs")]);
        let contents = fs::read_to_string(root.path().join(&created[0])).unwrap();
        let parsed = syn::parse_file(&contents).unwrap();
        assert!(
            parsed.items.iter().any(
                |item| matches!(item, syn::Item::Fn(function) if function.sig.ident == "index")
            )
        );
        assert_eq!(
            fs::read_to_string(module).unwrap(),
            format!("{original}\npub mod customer_handler;\n")
        );
    }

    #[test]
    fn every_kind_generates_valid_rust_and_normalized_names() {
        let root = fixture();
        for (kind, name, expected) in [
            (Kind::Model, "Customer", "src/domain/models/customer.rs"),
            (
                Kind::Request,
                "CreateCustomerDto",
                "src/domain/dtos/create_customer_request.rs",
            ),
            (
                Kind::Service,
                "CustomerService",
                "src/services/customer_service.rs",
            ),
            (Kind::Job, "SendInvoiceJob", "src/jobs/send_invoice.rs"),
            (
                Kind::Middleware,
                "AuditMiddleware",
                "src/middleware/audit.rs",
            ),
            (
                Kind::Seeder,
                "CustomerSeeder",
                "src/seeders/customer_seeder.rs",
            ),
        ] {
            let created = run(root.path(), kind, name).unwrap();
            assert_eq!(created, [PathBuf::from(expected)]);
            syn::parse_file(&fs::read_to_string(root.path().join(expected)).unwrap()).unwrap();
        }
    }

    #[test]
    fn request_generator_enforces_validation_boundary_contract() {
        let root = fixture();
        let created = run(
            root.path(),
            Kind::Request,
            "CreateCustomerRequest",
        )
        .unwrap();
        assert_eq!(
            created,
            [PathBuf::from("src/domain/dtos/create_customer_request.rs")]
        );
        let source = fs::read_to_string(root.path().join(&created[0])).unwrap();
        for requirement in [
            "derive(Deserialize, Validate)",
            "serde(deny_unknown_fields)",
            "message = \"must contain 1 to 100 characters\"",
            "impl Normalize for CreateCustomerRequest",
        ] {
            assert!(
                source.contains(requirement),
                "generated request omitted {requirement:?}"
            );
        }
    }

    #[test]
    fn model_and_repository_keys_follow_the_explicit_singular_entity() {
        let root = fixture();
        for (entity, module, primary_key) in [
            ("Customer", "customer", "customer_id"),
            ("Company", "company", "company_id"),
            ("User", "user", "user_id"),
            ("UserRole", "user_role", "user_role_id"),
            ("Person", "person", "person_id"),
            ("Status", "status", "status_id"),
            ("Uuid", "uuid", "uuid_id"),
        ] {
            run(root.path(), Kind::Model, &format!("{entity}Model")).unwrap();
            let source =
                fs::read_to_string(root.path().join(format!("src/domain/models/{module}.rs")))
                    .unwrap();
            let parsed = syn::parse_file(&source).unwrap();
            let model = parsed
                .items
                .iter()
                .find_map(|item| match item {
                    syn::Item::Struct(model) => Some(model),
                    _ => None,
                })
                .unwrap();
            assert_eq!(model.ident, entity);
            assert!(model.fields.iter().any(|field| {
                field.ident.as_ref().is_some_and(|name| name == primary_key)
                    && matches!(field.vis, syn::Visibility::Public(_))
            }));
            assert!(!model.fields.iter().any(|field| {
                field
                    .ident
                    .as_ref()
                    .is_some_and(|name| name == "id" || name == "uuid" || name == "uid")
            }));

            let files = run(
                root.path(),
                Kind::Repository,
                &format!("{entity}Repository"),
            )
            .unwrap();
            for (index, path) in files.iter().enumerate() {
                let source = fs::read_to_string(root.path().join(path)).unwrap();
                syn::parse_file(&source).unwrap();
                let argument = if index == 0 {
                    primary_key.to_owned()
                } else {
                    format!("_{primary_key}")
                };
                assert!(
                    source.contains(&format!("async fn exists(&self, {argument}: Uuid)")),
                    "{} must use the entity's primary-key name",
                    path.display()
                );
            }
        }
    }

    #[test]
    fn duplicate_normalized_names_never_overwrite_generated_or_existing_files() {
        let root = fixture();
        run(root.path(), Kind::Controller, "CustomerController").unwrap();
        let before = snapshot(root.path());
        assert!(run(root.path(), Kind::Controller, "customer_handler").is_err());
        assert_eq!(snapshot(root.path()), before);
    }

    #[test]
    fn invalid_names_leave_the_project_unchanged() {
        let root = fixture();
        let before = snapshot(root.path());
        let too_long = "a".repeat(129);
        for name in [
            "",
            "../outside",
            "foo/bar",
            "foo\\bar",
            "bad name",
            "$(touch attack)",
            "name;false",
            "9customer",
            "mod",
            "self",
            "Self",
            "Customer-Controller",
            "Pelanggán",
            "Controller",
            &too_long,
        ] {
            assert!(
                run(root.path(), Kind::Controller, name).is_err(),
                "accepted invalid name: {name:?}"
            );
            assert_eq!(
                snapshot(root.path()),
                before,
                "changed project for {name:?}"
            );
        }
    }

    #[test]
    fn module_and_import_collisions_are_refused_before_writing() {
        let root = fixture();
        for declaration in [
            "pub mod customer_handler;\n",
            "pub mod r#customer_handler;\n",
            "pub use existing::customer_handler;\n",
            "use existing::{thing as customer_handler};\n",
            "use existing::thing as r#customer_handler;\n",
            "use existing::customer_handler::{self};\n",
            "pub fn customer_handler() {}\n",
        ] {
            write(root.path(), "src/handlers/mod.rs", declaration);
            let before = snapshot(root.path());
            assert!(run(root.path(), Kind::Controller, "Customer").is_err());
            assert_eq!(snapshot(root.path()), before);
        }
    }

    #[test]
    fn invalid_existing_module_syntax_is_preserved_and_refused() {
        let root = fixture();
        write(root.path(), "src/handlers/mod.rs", "pub fn broken(\n");
        let before = snapshot(root.path());
        assert!(run(root.path(), Kind::Controller, "Customer").is_err());
        assert_eq!(snapshot(root.path()), before);
    }

    #[test]
    fn repository_creates_trait_and_both_feature_gated_adapters() {
        let root = fixture();
        let root_module = fs::read_to_string(root.path().join("src/repositories/mod.rs")).unwrap();
        let created = run(root.path(), Kind::Repository, "CustomerRepository").unwrap();
        assert_eq!(
            created,
            [
                PathBuf::from("src/repositories/customer_repository.rs"),
                PathBuf::from("src/repositories/postgres/customer_pg_repo.rs"),
                PathBuf::from("src/repositories/mysql/customer_mysql_repo.rs"),
            ]
        );
        for path in created {
            syn::parse_file(&fs::read_to_string(root.path().join(path)).unwrap()).unwrap();
        }
        assert_eq!(
            fs::read_to_string(root.path().join("src/repositories/mod.rs")).unwrap(),
            format!("{root_module}\npub mod customer_repository;\n")
        );
        assert!(
            fs::read_to_string(root.path().join("src/repositories/postgres/mod.rs"))
                .unwrap()
                .ends_with("pub mod customer_pg_repo;\n")
        );
        assert!(
            fs::read_to_string(root.path().join("src/repositories/mysql/mod.rs"))
                .unwrap()
                .ends_with("pub mod customer_mysql_repo;\n")
        );
    }

    #[test]
    fn repository_collision_in_last_adapter_prevents_all_other_changes() {
        let root = fixture();
        write(
            root.path(),
            "src/repositories/mysql/customer_mysql_repo.rs",
            "// Existing adapter.\n",
        );
        let before = snapshot(root.path());
        assert!(run(root.path(), Kind::Repository, "Customer").is_err());
        assert_eq!(snapshot(root.path()), before);
    }

    #[test]
    fn paired_migrations_use_monotonic_versions_across_both_drivers() {
        let root = fixture();
        write(
            root.path(),
            "migrations/mysql/20270101000000_previous.sql",
            "-- Existing migration\n",
        );
        let first = run_at(
            root.path(),
            Kind::Migration,
            "create_customers_table",
            now(),
        )
        .unwrap();
        assert_eq!(
            first,
            [
                PathBuf::from("migrations/postgres/20270101000001_create_customers_table.sql"),
                PathBuf::from("rollbacks/postgres/20270101000001_create_customers_table.sql"),
                PathBuf::from("migrations/mysql/20270101000001_create_customers_table.sql"),
                PathBuf::from("rollbacks/mysql/20270101000001_create_customers_table.sql"),
            ]
        );
        let second = run_at(root.path(), Kind::Migration, "add_customer_notes", now()).unwrap();
        assert_eq!(
            second[0],
            PathBuf::from("migrations/postgres/20270101000002_add_customer_notes.sql")
        );
        assert_eq!(second[0].file_name(), second[2].file_name());
        assert_eq!(second[1].file_name(), second[3].file_name());
        for paths in [first, second] {
            for (index, path) in paths.into_iter().enumerate() {
                let contents = fs::read_to_string(root.path().join(path)).unwrap();
                assert!(
                    contents
                        .lines()
                        .all(|line| line.trim().is_empty() || line.starts_with("--"))
                );
                assert!(contents.contains("SCAFFOLD ONLY"));
                if index % 2 == 0 {
                    for requirement in [
                        "docs/design/<feature>.md",
                        "people.person_id, statuses.status_id",
                        "<entity>_id primary key",
                        "Uuid::now_v7()",
                        "CHAR(36) CHARACTER SET ascii COLLATE ascii_bin",
                        "indexes before feature code",
                        "versioned, scoped cursors",
                        "EXPLAIN plans",
                    ] {
                        assert!(
                            contents.contains(requirement),
                            "migration scaffold omitted {requirement:?}"
                        );
                    }
                } else {
                    assert!(contents.contains("rollback"));
                    assert!(contents.contains("MySQL DDL implicitly commits"));
                }
            }
        }
    }

    #[test]
    fn duplicate_migration_description_preserves_both_directories() {
        let root = fixture();
        run_at(
            root.path(),
            Kind::Migration,
            "create_customers_table",
            now(),
        )
        .unwrap();
        let before = snapshot(root.path());
        assert!(run_at(root.path(), Kind::Migration, "CreateCustomersTable", now()).is_err());
        assert_eq!(snapshot(root.path()), before);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_output_directory_is_refused_without_touching_the_target() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("src")).unwrap();
        symlink(outside.path(), root.path().join("src/handlers")).unwrap();
        assert!(run(root.path(), Kind::Controller, "Customer").is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }
}
