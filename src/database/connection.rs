use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::{ConnectOptions, migrate::Migrator};

use crate::{
    config::database::{DatabaseConfig, DatabaseDriver},
    errors::{AppError, AppResult},
};

#[cfg(feature = "postgres")]
static POSTGRES_MIGRATOR: Migrator = sqlx::migrate!("./migrations/postgres");
#[cfg(feature = "mysql")]
static MYSQL_MIGRATOR: Migrator = sqlx::migrate!("./migrations/mysql");

type RecordedMigration = (i64, bool, Vec<u8>);

/// Typed pools keep UUID, JSON and timestamp handling specific to each SQL dialect.
#[derive(Clone)]
pub enum DatabasePool {
    #[cfg(feature = "postgres")]
    Postgres(sqlx::PgPool),
    #[cfg(feature = "mysql")]
    Mysql(sqlx::MySqlPool),
}

impl DatabasePool {
    pub async fn connect(config: &DatabaseConfig) -> Result<Self> {
        config.validate()?;
        match config.driver {
            DatabaseDriver::Postgres => {
                #[cfg(feature = "postgres")]
                {
                    let options = config
                        .url
                        .parse::<sqlx::postgres::PgConnectOptions>()
                        .map_err(|_| anyhow::anyhow!("invalid PostgreSQL connection options"))?
                        .application_name("rust-api-skeleton")
                        .disable_statement_logging();
                    let pool = sqlx::postgres::PgPoolOptions::new()
                        .max_connections(config.max_connections)
                        .min_connections(config.min_connections)
                        .acquire_timeout(Duration::from_secs(config.acquire_timeout_secs))
                        .idle_timeout(Duration::from_secs(config.idle_timeout_secs))
                        .after_connect(|connection, _metadata| {
                            Box::pin(async move {
                                sqlx::query("SET TIME ZONE 'UTC'")
                                    .execute(connection)
                                    .await?;
                                Ok(())
                            })
                        })
                        .connect_with(options)
                        .await
                        .context("failed to connect to PostgreSQL")?;
                    Ok(Self::Postgres(pool))
                }
                #[cfg(not(feature = "postgres"))]
                {
                    anyhow::bail!("PostgreSQL support is not compiled in")
                }
            }
            DatabaseDriver::Mysql => {
                #[cfg(feature = "mysql")]
                {
                    let options = config
                        .url
                        .parse::<sqlx::mysql::MySqlConnectOptions>()
                        .map_err(|_| anyhow::anyhow!("invalid MySQL connection options"))?
                        .charset("utf8mb4")
                        .collation("utf8mb4_bin")
                        .disable_statement_logging();
                    let pool = sqlx::mysql::MySqlPoolOptions::new()
                        .max_connections(config.max_connections)
                        .min_connections(config.min_connections)
                        .acquire_timeout(Duration::from_secs(config.acquire_timeout_secs))
                        .idle_timeout(Duration::from_secs(config.idle_timeout_secs))
                        .after_connect(|connection, _metadata| {
                            Box::pin(async move {
                                sqlx::query("SET time_zone = '+00:00'")
                                    .execute(connection)
                                    .await?;
                                Ok(())
                            })
                        })
                        .connect_with(options)
                        .await
                        .context("failed to connect to MySQL")?;
                    Ok(Self::Mysql(pool))
                }
                #[cfg(not(feature = "mysql"))]
                {
                    anyhow::bail!("MySQL support is not compiled in")
                }
            }
        }
    }

    pub async fn ping(&self) -> AppResult<()> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(pool) => {
                sqlx::query("SELECT 1").execute(pool).await?;
            }
            #[cfg(feature = "mysql")]
            Self::Mysql(pool) => {
                sqlx::query("SELECT 1").execute(pool).await?;
            }
        }
        Ok(())
    }

    /// Read-only readiness check against this binary's exact migration history.
    /// Unknown versions (including future migrations) are deliberately rejected:
    /// an older binary cannot assume an unrecognized schema remains compatible.
    /// This checks the migration ledger, not arbitrary manual schema changes.
    pub async fn ready(&self) -> AppResult<()> {
        let (migrator, recorded) = match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(pool) => {
                let recorded = sqlx::query_as::<_, RecordedMigration>(
                    "SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version",
                )
                .fetch_all(pool)
                .await
                .map_err(|_| AppError::Unavailable)?;
                (&POSTGRES_MIGRATOR, recorded)
            }
            #[cfg(feature = "mysql")]
            Self::Mysql(pool) => {
                let recorded = sqlx::query_as::<_, RecordedMigration>(
                    "SELECT version, success, checksum FROM _sqlx_migrations ORDER BY version",
                )
                .fetch_all(pool)
                .await
                .map_err(|_| AppError::Unavailable)?;
                (&MYSQL_MIGRATOR, recorded)
            }
        };
        validate_migration_history(migrator, &recorded)
    }

    /// Migrations run only through the explicit migration binary.
    pub async fn migrate(&self) -> Result<()> {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(pool) => POSTGRES_MIGRATOR
                .run(pool)
                .await
                .context("PostgreSQL migrations failed"),
            #[cfg(feature = "mysql")]
            Self::Mysql(pool) => MYSQL_MIGRATOR
                .run(pool)
                .await
                .context("MySQL migrations failed"),
        }
    }

    pub async fn close(&self) {
        match self {
            #[cfg(feature = "postgres")]
            Self::Postgres(pool) => pool.close().await,
            #[cfg(feature = "mysql")]
            Self::Mysql(pool) => pool.close().await,
        }
    }
}

fn validate_migration_history(
    migrator: &Migrator,
    recorded: &[RecordedMigration],
) -> AppResult<()> {
    let expected: Vec<_> = migrator
        .iter()
        .filter(|migration| migration.migration_type.is_up_migration())
        .collect();
    if recorded.len() != expected.len()
        || !recorded
            .iter()
            .zip(expected)
            .all(|((version, success, checksum), migration)| {
                *success
                    && *version == migration.version
                    && checksum.as_slice() == migration.checksum.as_ref()
            })
    {
        return Err(AppError::Unavailable);
    }
    Ok(())
}

/// MySQL marks `_bin` collated text as BINARY in result metadata. SQLx 0.8
/// therefore rejects decoding it directly as String. Decode bytes deliberately,
/// validate UTF-8, and retain the bytewise storage/comparison semantics.
#[cfg(feature = "mysql")]
pub(crate) fn mysql_text(row: &sqlx::mysql::MySqlRow, column: &str) -> Result<String, sqlx::Error> {
    use sqlx::Row;

    let bytes: &[u8] = row.try_get(column)?;
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|source| sqlx::Error::ColumnDecode {
            index: column.to_owned(),
            source: Box::new(source),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migrator() -> &'static Migrator {
        #[cfg(feature = "postgres")]
        {
            &POSTGRES_MIGRATOR
        }
        #[cfg(all(not(feature = "postgres"), feature = "mysql"))]
        {
            &MYSQL_MIGRATOR
        }
    }

    fn current_history() -> Vec<RecordedMigration> {
        migrator()
            .iter()
            .filter(|migration| migration.migration_type.is_up_migration())
            .map(|migration| (migration.version, true, migration.checksum.to_vec()))
            .collect()
    }

    #[test]
    fn readiness_accepts_only_complete_successful_matching_migrations() {
        let current = current_history();
        assert!(!current.is_empty());
        validate_migration_history(migrator(), &current).unwrap();

        let mut dirty = current.clone();
        dirty[0].1 = false;
        assert!(matches!(
            validate_migration_history(migrator(), &dirty),
            Err(AppError::Unavailable)
        ));

        let mut changed = current.clone();
        changed[0].2[0] ^= 1;
        assert!(matches!(
            validate_migration_history(migrator(), &changed),
            Err(AppError::Unavailable)
        ));

        let mut missing = current.clone();
        missing.pop();
        assert!(matches!(
            validate_migration_history(migrator(), &missing),
            Err(AppError::Unavailable)
        ));
        assert!(matches!(
            validate_migration_history(migrator(), &[]),
            Err(AppError::Unavailable)
        ));

        let mut unknown = current.clone();
        unknown[0].0 -= 1;
        assert!(matches!(
            validate_migration_history(migrator(), &unknown),
            Err(AppError::Unavailable)
        ));

        let mut future = current;
        future.push((i64::MAX, true, vec![0; 48]));
        assert!(matches!(
            validate_migration_history(migrator(), &future),
            Err(AppError::Unavailable)
        ));
    }
}
