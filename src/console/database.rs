use std::{fs, path::Path};

use anyhow::{Context, ensure};
use sqlx::migrate::Migrator;

use rust_api_skeleton::{
    config::{Settings, database::DatabaseDriver},
    database::DatabasePool,
    seeders,
};

use super::project;

pub async fn rollback(root: &Path, step: usize, force: bool) -> anyhow::Result<()> {
    ensure!(
        (1..=100).contains(&step),
        "--step must be between 1 and 100"
    );
    let settings = Settings::load()?;
    destructive_guard(&settings, force, "migrate:rollback")?;
    let database = DatabasePool::connect(&settings.database).await?;

    // A step rollback starts only from the exact current schema. This makes the
    // target deterministic and prevents a partial/old database from silently
    // rolling back a different migration than the operator intended.
    database
        .ready()
        .await
        .map_err(|_| anyhow::anyhow!("database must be fully migrated before migrate:rollback"))?;

    let bundle = RollbackBundle::build(root, settings.database.driver).await?;
    let target = rollback_target(&bundle.versions, step);
    undo(&database, &bundle.migrator, target).await?;
    database.close().await;

    println!(
        "Rolled back {} migration(s). Target version: {}.",
        step.min(bundle.versions.len()),
        target
    );
    Ok(())
}

pub async fn fresh(
    root: &Path,
    seed: bool,
    seeder: Option<&str>,
    force: bool,
) -> anyhow::Result<()> {
    let settings = Settings::load()?;
    destructive_guard(&settings, force, "migrate:fresh")?;
    let database = DatabasePool::connect(&settings.database).await?;

    let bundle = RollbackBundle::build(root, settings.database.driver).await?;
    undo(&database, &bundle.migrator, 0)
        .await
        .context("could not roll back known migrations before fresh migration")?;
    database
        .migrate()
        .await
        .context("fresh migration failed after rollback")?;

    if seed || seeder.is_some() {
        seed_database(&database, seeder).await?;
    }
    database.close().await;

    if seed || seeder.is_some() {
        println!("Fresh migrations complete and seeders executed.");
    } else {
        println!("Fresh migrations complete.");
    }
    Ok(())
}

pub async fn seed(class: Option<&str>, force: bool) -> anyhow::Result<()> {
    let settings = Settings::load()?;
    write_guard(&settings, force, "db:seed")?;
    let database = DatabasePool::connect(&settings.database).await?;
    database
        .ready()
        .await
        .map_err(|_| anyhow::anyhow!("database must be fully migrated before db:seed"))?;
    seed_database(&database, class).await?;
    database.close().await;
    Ok(())
}

async fn seed_database(database: &DatabasePool, class: Option<&str>) -> anyhow::Result<()> {
    let class = class.unwrap_or("DatabaseSeeder");
    validate_class(class)?;
    seeders::run(database, class).await?;
    println!("Seeder {class} complete.");
    Ok(())
}

fn destructive_guard(settings: &Settings, force: bool, command: &str) -> anyhow::Result<()> {
    if settings.app.environment == "production" && !force {
        anyhow::bail!(
            "{command} is destructive and disabled in production; rerun with --force only after backup and maintenance approval"
        );
    }
    Ok(())
}

fn write_guard(settings: &Settings, force: bool, command: &str) -> anyhow::Result<()> {
    if settings.app.environment == "production" && !force {
        anyhow::bail!("{command} writes data and is disabled in production without --force");
    }
    Ok(())
}

fn validate_class(class: &str) -> anyhow::Result<()> {
    ensure!(
        !class.is_empty()
            && class.len() <= 100
            && class.as_bytes()[0].is_ascii_alphabetic()
            && class
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
        "seeder class must be an ASCII Rust-style name up to 100 characters"
    );
    Ok(())
}

fn rollback_target(versions: &[i64], step: usize) -> i64 {
    if step >= versions.len() {
        0
    } else {
        versions[versions.len() - step - 1]
    }
}

async fn undo(database: &DatabasePool, migrator: &Migrator, target: i64) -> anyhow::Result<()> {
    match database {
        #[cfg(feature = "postgres")]
        DatabasePool::Postgres(pool) => migrator
            .undo(pool, target)
            .await
            .context("PostgreSQL rollback failed"),
        #[cfg(feature = "mysql")]
        DatabasePool::Mysql(pool) => migrator
            .undo(pool, target)
            .await
            .context("MySQL rollback failed"),
    }
}

struct RollbackBundle {
    _directory: tempfile::TempDir,
    migrator: Migrator,
    versions: Vec<i64>,
}

impl RollbackBundle {
    async fn build(root: &Path, driver: DatabaseDriver) -> anyhow::Result<Self> {
        let driver = driver.to_string();
        let migration_dir = Path::new("migrations").join(&driver);
        let rollback_dir = Path::new("rollbacks").join(&driver);
        let migrations = collect_migrations(root, &migration_dir)?;
        ensure!(!migrations.is_empty(), "no migrations found for {driver}");

        let directory = tempfile::tempdir().context("could not prepare rollback bundle")?;
        let mut versions = Vec::with_capacity(migrations.len());
        for migration in migrations {
            let rollback_relative = rollback_dir.join(&migration.file_name);
            let rollback =
                project::read_optional(root, &rollback_relative)?.with_context(|| {
                    format!(
                        "missing rollback companion {}; every migration must have one",
                        rollback_relative.display()
                    )
                })?;
            ensure!(
                !rollback.trim().is_empty() && !rollback.contains("SCAFFOLD ONLY"),
                "{} is empty or still a scaffold; write explicit rollback SQL before rollback/fresh",
                rollback_relative.display()
            );

            let stem = migration
                .file_name
                .strip_suffix(".sql")
                .context("migration filename must end in .sql")?;
            fs::write(
                directory.path().join(format!("{stem}.up.sql")),
                migration.contents.as_bytes(),
            )
            .context("could not stage reversible up migration")?;
            fs::write(
                directory.path().join(format!("{stem}.down.sql")),
                rollback.as_bytes(),
            )
            .context("could not stage reversible down migration")?;
            versions.push(migration.version);
        }

        let migrator = Migrator::new(directory.path())
            .await
            .context("rollback companions do not form a valid reversible migration set")?;
        Ok(Self {
            _directory: directory,
            migrator,
            versions,
        })
    }
}

struct MigrationFile {
    version: i64,
    file_name: String,
    contents: String,
}

fn collect_migrations(root: &Path, directory: &Path) -> anyhow::Result<Vec<MigrationFile>> {
    let path = project::checked_path(root, directory)?;
    let entries = fs::read_dir(path).context("could not inspect migration directory")?;
    let mut migrations = Vec::new();
    for entry in entries {
        let entry = entry.context("could not inspect migration entry")?;
        if !entry
            .file_type()
            .context("could not inspect migration type")?
            .is_file()
        {
            continue;
        }
        let file_name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("migration filename must be UTF-8"))?;
        if !file_name.ends_with(".sql")
            || file_name.ends_with(".up.sql")
            || file_name.ends_with(".down.sql")
        {
            continue;
        }
        let (version, _) = file_name
            .strip_suffix(".sql")
            .and_then(|stem| stem.split_once('_'))
            .with_context(|| format!("invalid migration filename {file_name}"))?;
        let version: i64 = version
            .parse()
            .with_context(|| format!("invalid migration version in {file_name}"))?;
        ensure!(version > 0, "migration versions must be positive");
        let relative = directory.join(&file_name);
        let contents = project::read_optional(root, &relative)?
            .with_context(|| format!("missing migration {}", relative.display()))?;
        migrations.push(MigrationFile {
            version,
            file_name,
            contents,
        });
    }
    migrations.sort_by_key(|migration| migration.version);
    for pair in migrations.windows(2) {
        ensure!(
            pair[0].version != pair[1].version,
            "duplicate migration version {}",
            pair[0].version
        );
    }
    Ok(migrations)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        for driver in ["postgres", "mysql"] {
            fs::create_dir_all(root.path().join("migrations").join(driver)).unwrap();
            fs::create_dir_all(root.path().join("rollbacks").join(driver)).unwrap();
            fs::write(
                root.path()
                    .join("migrations")
                    .join(driver)
                    .join("20260101000000_create_example.sql"),
                "CREATE TABLE example (example_id INT PRIMARY KEY);\n",
            )
            .unwrap();
            fs::write(
                root.path()
                    .join("rollbacks")
                    .join(driver)
                    .join("20260101000000_create_example.sql"),
                "DROP TABLE example;\n",
            )
            .unwrap();
        }
        root
    }

    #[test]
    fn rollback_target_is_deterministic_and_bounded() {
        let versions = [10, 20, 30, 40];
        assert_eq!(rollback_target(&versions, 1), 30);
        assert_eq!(rollback_target(&versions, 2), 20);
        assert_eq!(rollback_target(&versions, 4), 0);
        assert_eq!(rollback_target(&versions, 99), 0);
    }

    #[test]
    fn seeder_class_is_bounded_and_cannot_be_a_path_or_shell_fragment() {
        for invalid in ["", "../AdminSeeder", "A;touch", "A B"] {
            assert!(validate_class(invalid).is_err());
        }
        assert!(validate_class(&"x".repeat(101)).is_err());
        validate_class("DatabaseSeeder").unwrap();
        validate_class("CustomerSeeder").unwrap();
    }

    #[tokio::test]
    async fn rollback_bundle_preserves_up_sql_and_requires_companions() {
        let root = fixture();
        let bundle = RollbackBundle::build(root.path(), DatabaseDriver::Postgres)
            .await
            .unwrap();
        assert_eq!(bundle.versions, [20260101000000]);
        assert_eq!(
            bundle
                .migrator
                .iter()
                .filter(|migration| migration.migration_type.is_up_migration())
                .count(),
            1
        );
        fs::remove_file(
            root.path()
                .join("rollbacks/postgres/20260101000000_create_example.sql"),
        )
        .unwrap();
        assert!(
            RollbackBundle::build(root.path(), DatabaseDriver::Postgres)
                .await
                .is_err()
        );
    }
}
