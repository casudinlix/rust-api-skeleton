//! Destructive migration command regression tests. Each test creates and removes
//! only its own database. Run through make test-postgres / make test-mysql.

use std::{path::Path, process::Command};

use anyhow::{Context, Result, ensure};
use rust_api_skeleton::{
    config::database::{DatabaseConfig, DatabaseDriver},
    database::DatabasePool,
};
use sqlx::migrate::MigrateDatabase;
use uuid::Uuid;

#[cfg(feature = "postgres")]
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL for disposable PostgreSQL with CREATE DATABASE"]
async fn postgres_database_maintenance() -> Result<()> {
    isolated(DatabaseDriver::Postgres).await
}

#[cfg(feature = "mysql")]
#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL for disposable MySQL 8 with CREATE DATABASE"]
async fn mysql_database_maintenance() -> Result<()> {
    isolated(DatabaseDriver::Mysql).await
}

async fn isolated(driver: DatabaseDriver) -> Result<()> {
    let supplied = std::env::var("TEST_DATABASE_URL").context("set TEST_DATABASE_URL")?;
    let mut url = url::Url::parse(&supplied).context("invalid TEST_DATABASE_URL")?;
    match driver {
        DatabaseDriver::Postgres => ensure!(
            matches!(url.scheme(), "postgres" | "postgresql"),
            "PostgreSQL URL required"
        ),
        DatabaseDriver::Mysql => ensure!(url.scheme() == "mysql", "MySQL URL required"),
    }
    url.set_path(&format!("maintenance_test_{}", Uuid::now_v7().simple()));
    let database_url = url.to_string();

    match driver {
        #[cfg(feature = "postgres")]
        DatabaseDriver::Postgres => sqlx::Postgres::create_database(&database_url).await?,
        #[cfg(feature = "mysql")]
        DatabaseDriver::Mysql => sqlx::MySql::create_database(&database_url).await?,
        #[allow(unreachable_patterns)]
        _ => anyhow::bail!("database driver is not compiled"),
    }

    let result = exercise(driver, &database_url).await;
    let cleanup = match driver {
        #[cfg(feature = "postgres")]
        DatabaseDriver::Postgres => sqlx::Postgres::drop_database(&database_url).await,
        #[cfg(feature = "mysql")]
        DatabaseDriver::Mysql => sqlx::MySql::drop_database(&database_url).await,
        #[allow(unreachable_patterns)]
        _ => unreachable!("feature checked before database creation"),
    };
    result?;
    cleanup.context("remove isolated database-maintenance database")
}

async fn exercise(driver: DatabaseDriver, database_url: &str) -> Result<()> {
    run_artisan(driver, database_url, &["migrate:fresh", "--force"])?;
    assert_ready(driver, database_url, true).await?;

    run_artisan(
        driver,
        database_url,
        &["migrate:rollback", "--step", "1", "--force"],
    )?;
    assert_ready(driver, database_url, false).await?;

    run_migrate(driver, database_url)?;
    assert_ready(driver, database_url, true).await?;

    run_artisan(
        driver,
        database_url,
        &["db:seed", "--class", "DatabaseSeeder", "--force"],
    )?;
    Ok(())
}

async fn assert_ready(
    driver: DatabaseDriver,
    database_url: &str,
    expected: bool,
) -> Result<()> {
    let database = DatabasePool::connect(&DatabaseConfig {
        driver,
        url: database_url.to_owned(),
        max_connections: 2,
        min_connections: 0,
        acquire_timeout_secs: 10,
        idle_timeout_secs: 30,
    })
    .await?;
    let ready = database.ready().await.is_ok();
    database.close().await;
    ensure!(ready == expected, "unexpected migration readiness state");
    Ok(())
}

fn run_artisan(driver: DatabaseDriver, database_url: &str, args: &[&str]) -> Result<()> {
    run_binary(
        env!("CARGO_BIN_EXE_artisan"),
        driver,
        database_url,
        args,
    )
}

fn run_migrate(driver: DatabaseDriver, database_url: &str) -> Result<()> {
    run_binary(env!("CARGO_BIN_EXE_migrate"), driver, database_url, &[])
}

fn run_binary(
    binary: &str,
    driver: DatabaseDriver,
    database_url: &str,
    args: &[&str],
) -> Result<()> {
    let status = Command::new(binary)
        .args(args)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")))
        .env("APP_ENV", "test")
        .env("DB_DRIVER", driver.to_string())
        .env("DATABASE_URL", database_url)
        .env(
            "JWT_SECRET",
            "maintenance-test-only-0123456789abcdef0123456789abcdef",
        )
        .env("CACHE_DRIVER", "none")
        .env("QUEUE_DRIVER", "database")
        .env("LOG_OUTPUT", "stdout")
        .status()
        .context("start database maintenance command")?;
    ensure!(status.success(), "database maintenance command failed");
    Ok(())
}
