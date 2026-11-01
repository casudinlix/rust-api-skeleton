use std::{fmt, str::FromStr};

use anyhow::{Result, bail, ensure};
use serde::Deserialize;
use url::Url;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseDriver {
    #[serde(alias = "postgresql")]
    Postgres,
    Mysql,
}

impl fmt::Display for DatabaseDriver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Postgres => "postgres",
            Self::Mysql => "mysql",
        })
    }
}

impl FromStr for DatabaseDriver {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "postgres" | "postgresql" => Ok(Self::Postgres),
            "mysql" => Ok(Self::Mysql),
            _ => bail!("DB_DRIVER must be postgres or mysql"),
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct DatabaseConfig {
    pub driver: DatabaseDriver,
    pub url: String,
    pub max_connections: u32,
    pub min_connections: u32,
    pub acquire_timeout_secs: u64,
    pub idle_timeout_secs: u64,
}

// A database URL can contain credentials. Never include it in configuration logs.
impl fmt::Debug for DatabaseConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DatabaseConfig")
            .field("driver", &self.driver)
            .field("max_connections", &self.max_connections)
            .field("min_connections", &self.min_connections)
            .field("acquire_timeout_secs", &self.acquire_timeout_secs)
            .field("idle_timeout_secs", &self.idle_timeout_secs)
            .finish_non_exhaustive()
    }
}

impl DatabaseConfig {
    pub fn validate(&self) -> Result<()> {
        match self.driver {
            DatabaseDriver::Postgres => {
                ensure!(
                    cfg!(feature = "postgres"),
                    "DB_DRIVER=postgres requires the postgres Cargo feature"
                );
            }
            DatabaseDriver::Mysql => {
                ensure!(
                    cfg!(feature = "mysql"),
                    "DB_DRIVER=mysql requires the mysql Cargo feature"
                );
            }
        }

        let url = Url::parse(&self.url)
            .map_err(|_| anyhow::anyhow!("DATABASE_URL must be a valid database URL"))?;
        let scheme_matches = match self.driver {
            DatabaseDriver::Postgres => matches!(url.scheme(), "postgres" | "postgresql"),
            DatabaseDriver::Mysql => url.scheme() == "mysql",
        };
        ensure!(scheme_matches, "DATABASE_URL scheme must match DB_DRIVER");
        ensure!(
            !url.path().trim_matches('/').is_empty(),
            "DATABASE_URL must include a database name"
        );
        ensure!(
            url.fragment().is_none(),
            "DATABASE_URL must not contain a fragment; URL-encode credentials"
        );
        ensure!(
            self.max_connections > 0,
            "DB_MAX_CONNECTIONS must be positive"
        );
        ensure!(
            self.min_connections <= self.max_connections,
            "DB_MIN_CONNECTIONS must not exceed DB_MAX_CONNECTIONS"
        );
        ensure!(
            self.acquire_timeout_secs > 0,
            "DB_ACQUIRE_TIMEOUT_SECS must be positive"
        );
        ensure!(
            self.idle_timeout_secs > 0,
            "DB_IDLE_TIMEOUT_SECS must be positive"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(driver: DatabaseDriver) -> DatabaseConfig {
        DatabaseConfig {
            driver,
            url: format!("{driver}://example:very-secret@localhost/test_db"),
            max_connections: 5,
            min_connections: 1,
            acquire_timeout_secs: 5,
            idle_timeout_secs: 60,
        }
    }

    fn compiled_driver() -> DatabaseDriver {
        if cfg!(feature = "postgres") {
            DatabaseDriver::Postgres
        } else {
            DatabaseDriver::Mysql
        }
    }

    #[test]
    fn parses_known_drivers_without_echoing_unknown_input() {
        assert_eq!(
            "postgresql".parse::<DatabaseDriver>().unwrap(),
            DatabaseDriver::Postgres
        );
        assert_eq!(
            "MYSQL".parse::<DatabaseDriver>().unwrap(),
            DatabaseDriver::Mysql
        );
        let error = "very-secret".parse::<DatabaseDriver>().unwrap_err();
        assert!(!error.to_string().contains("very-secret"));
    }

    #[test]
    fn debug_does_not_expose_url_or_credentials() {
        let debug = format!("{:?}", config(compiled_driver()));
        assert!(!debug.contains("very-secret"));
        assert!(!debug.contains("example"));
        assert!(!debug.contains("localhost"));
    }

    #[test]
    fn validates_matching_driver_and_pool_bounds() {
        let mut config = config(compiled_driver());
        config.validate().unwrap();
        config.min_connections = 6;
        assert!(config.validate().is_err());
        config.min_connections = 0;
        config.max_connections = 0;
        assert!(config.validate().is_err());
        config.max_connections = 5;
        config.acquire_timeout_secs = 0;
        assert!(config.validate().is_err());
        config.acquire_timeout_secs = 5;
        config.idle_timeout_secs = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_driver_url_mismatch_and_missing_database() {
        let mut config = config(compiled_driver());
        config.url = "sqlite://example/test_db".to_owned();
        assert!(config.validate().is_err());
        config.url = format!("{}://example:very-secret@localhost/", config.driver);
        assert!(config.validate().is_err());
        config.url = format!(
            "{}://example:very-secret@localhost/test_db#oops",
            config.driver
        );
        assert!(config.validate().is_err());
    }

    #[test]
    fn errors_do_not_expose_malformed_credentials() {
        let mut config = config(compiled_driver());
        config.url = "very-secret".to_owned();
        assert!(
            !config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("very-secret")
        );
    }

    #[cfg(not(feature = "postgres"))]
    #[test]
    fn rejects_postgres_when_feature_is_absent() {
        assert!(config(DatabaseDriver::Postgres).validate().is_err());
    }

    #[cfg(not(feature = "mysql"))]
    #[test]
    fn rejects_mysql_when_feature_is_absent() {
        assert!(config(DatabaseDriver::Mysql).validate().is_err());
    }
}
