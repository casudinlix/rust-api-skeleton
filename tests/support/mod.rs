#![allow(dead_code)]

use anyhow::{Context, Result};
use rust_api_skeleton::config::database::DatabaseDriver;

pub fn database_url(driver: DatabaseDriver) -> Result<String> {
    load_dotenv();
    let specific = match driver {
        DatabaseDriver::Postgres => "TEST_POSTGRES_URL",
        DatabaseDriver::Mysql => "TEST_MYSQL_URL",
    };
    first_nonempty(&[specific, "TEST_DATABASE_URL"]).with_context(|| {
        format!(
            "set {specific} in .env or the process environment; TEST_DATABASE_URL remains a backward-compatible fallback"
        )
    })
}

pub fn redis_url() -> Result<String> {
    load_dotenv();
    first_nonempty(&["TEST_REDIS_URL"])
        .context("set TEST_REDIS_URL in .env or the process environment")
}

fn load_dotenv() {
    // Explicit process variables keep precedence; dotenvy only fills values that
    // are not already present. This lets CI/one-off shell overrides win over .env.
    let _ = dotenvy::dotenv();
}

fn first_nonempty(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
}
