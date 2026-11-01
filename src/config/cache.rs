use std::{fmt, str::FromStr};

use anyhow::{Result, bail, ensure};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheDriver {
    None,
    Redis,
}

impl fmt::Display for CacheDriver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::None => "none",
            Self::Redis => "redis",
        })
    }
}

impl FromStr for CacheDriver {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "none" => Ok(Self::None),
            "redis" => Ok(Self::Redis),
            _ => bail!("CACHE_DRIVER must be none or redis"),
        }
    }
}

/// Redis URLs can contain credentials; Debug intentionally excludes the URL.
#[derive(Clone)]
pub struct CacheConfig {
    pub driver: CacheDriver,
    pub redis_url: String,
    pub prefix: String,
    pub default_ttl_secs: u64,
    pub timeout_ms: u64,
    pub max_value_bytes: usize,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            driver: CacheDriver::None,
            redis_url: String::new(),
            prefix: "rust-api-skeleton:cache:".into(),
            default_ttl_secs: 300,
            timeout_ms: 500,
            max_value_bytes: 1_048_576,
        }
    }
}

impl fmt::Debug for CacheConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CacheConfig")
            .field("driver", &self.driver)
            .field("default_ttl_secs", &self.default_ttl_secs)
            .field("timeout_ms", &self.timeout_ms)
            .field("max_value_bytes", &self.max_value_bytes)
            .finish_non_exhaustive()
    }
}

impl CacheConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.prefix.is_empty()
                && self.prefix.len() <= 128
                && self.prefix.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-' | b'.')
                }),
            "CACHE_PREFIX must contain 1..128 ASCII letters, digits, colons, underscores, hyphens or dots"
        );
        ensure!(
            (1..=2_592_000).contains(&self.default_ttl_secs),
            "CACHE_DEFAULT_TTL_SECS must be 1..2592000"
        );
        ensure!(
            (10..=10_000).contains(&self.timeout_ms),
            "CACHE_TIMEOUT_MS must be 10..10000"
        );
        ensure!(
            (64..=16_777_216).contains(&self.max_value_bytes),
            "CACHE_MAX_VALUE_BYTES must be 64..16777216"
        );
        // Disabled caching must not require a Redis URL or any Redis connection.
        if self.driver == CacheDriver::None {
            return Ok(());
        }
        let url = url::Url::parse(&self.redis_url)
            .map_err(|_| anyhow::anyhow!("REDIS_URL must be a valid Redis URL"))?;
        ensure!(
            matches!(url.scheme(), "redis" | "rediss")
                && url.host_str().is_some_and(|host| !host.is_empty())
                && url.port() != Some(0)
                && url.query().is_none()
                && url.fragment().is_none(),
            "REDIS_URL must use redis:// or verified rediss:// with a host and without query/fragment options"
        );
        let database = url.path().strip_prefix('/').unwrap_or(url.path());
        ensure!(
            database.is_empty()
                || (database.bytes().all(|byte| byte.is_ascii_digit())
                    && database.parse::<u32>().is_ok()),
            "REDIS_URL database index must be a nonnegative integer"
        );
        // Redis's own parser validates credential encoding and connection format.
        // Never attach its underlying error because it may contain the raw URL.
        redis::Client::open(self.redis_url.as_str())
            .map_err(|_| anyhow::anyhow!("REDIS_URL has invalid connection options"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redis_urls_reject_unsafe_options_and_never_echo_secrets() {
        for value in [
            "secret-value",
            "https://user:secret-value@localhost/0",
            "redis://user:secret-value@localhost:0/0",
            "redis://user:secret-value@localhost/-1",
            "redis://user:secret-value@localhost/one/two",
            "redis://user:secret-value@localhost/4294967296",
            "rediss://user:secret-value@localhost/0#insecure",
            "rediss://user:secret-value@localhost/0?insecure=true",
        ] {
            let config = CacheConfig {
                driver: CacheDriver::Redis,
                redis_url: value.into(),
                ..CacheConfig::default()
            };
            let error = config.validate().expect_err("unsafe URL accepted");
            assert!(!format!("{error:#}").contains("secret-value"));
            assert!(!format!("{config:?}").contains("secret-value"));
            assert!(!format!("{config:?}").contains("localhost"));
        }
    }

    #[test]
    fn accepts_authenticated_plaintext_and_verified_tls_urls() {
        for value in [
            "redis://127.0.0.1:6379/0",
            "redis://:encoded%40password@localhost/1",
            "rediss://user:encoded%40password@cache.example.com:6380/0",
            "redis://[::1]:6379/",
        ] {
            CacheConfig {
                driver: CacheDriver::Redis,
                redis_url: value.into(),
                ..CacheConfig::default()
            }
            .validate()
            .unwrap();
        }
    }

    #[test]
    fn disabled_cache_does_not_require_redis_connection_settings() {
        CacheConfig::default().validate().unwrap();
        CacheConfig {
            redis_url: "unused-and-invalid-secret-value".into(),
            ..CacheConfig::default()
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn rejects_empty_unsafe_or_oversized_prefix_and_operational_limits() {
        for prefix in [
            "".into(),
            "invalid key".into(),
            "key:*".into(),
            "x".repeat(129),
        ] {
            assert!(
                CacheConfig {
                    prefix,
                    ..CacheConfig::default()
                }
                .validate()
                .is_err()
            );
        }
        let mut config = CacheConfig::default();
        for ttl in [0, 2_592_001, u64::MAX] {
            config.default_ttl_secs = ttl;
            assert!(config.validate().is_err());
        }
        config.default_ttl_secs = 300;
        for timeout in [0, 9, 10_001, u64::MAX] {
            config.timeout_ms = timeout;
            assert!(config.validate().is_err());
        }
        config.timeout_ms = 500;
        for size in [0, 63, 16_777_217, usize::MAX] {
            config.max_value_bytes = size;
            assert!(config.validate().is_err());
        }
    }
}
