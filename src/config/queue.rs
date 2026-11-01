use std::{fmt, str::FromStr};

use anyhow::{Result, bail, ensure};

/// Selects where NEW jobs are recorded. Existing backlogs retain their backend.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum QueueDriver {
    #[default]
    Database,
    Redis,
}

impl fmt::Display for QueueDriver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Database => "database",
            Self::Redis => "redis",
        })
    }
}

impl FromStr for QueueDriver {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "database" => Ok(Self::Database),
            "redis" => Ok(Self::Redis),
            _ => bail!("QUEUE_DRIVER must be database or redis"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct QueueConfig {
    pub driver: QueueDriver,
    pub poll_interval_ms: u64,
    pub visibility_timeout_secs: u64,
    pub job_timeout_secs: u64,
    pub max_attempts: u32,
    pub retry_base_secs: u64,
    pub concurrency: usize,
    pub redis: RedisQueueConfig,
    pub relay_enabled: bool,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            driver: QueueDriver::Database,
            poll_interval_ms: 1_000,
            visibility_timeout_secs: 90,
            job_timeout_secs: 30,
            max_attempts: 5,
            retry_base_secs: 5,
            concurrency: 1,
            redis: RedisQueueConfig::default(),
            relay_enabled: true,
        }
    }
}

impl QueueConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (100..=60_000).contains(&self.poll_interval_ms),
            "QUEUE_POLL_INTERVAL_MS must be 100..60000"
        );
        ensure!(
            (1..=300).contains(&self.job_timeout_secs),
            "QUEUE_JOB_TIMEOUT_SECS must be 1..300"
        );
        ensure!(
            self.visibility_timeout_secs >= self.job_timeout_secs + 10
                && self.visibility_timeout_secs <= 3_600,
            "QUEUE_VISIBILITY_TIMEOUT_SECS must exceed the job timeout by at least 10 seconds, and be <=3600"
        );
        ensure!(
            (1..=100).contains(&self.max_attempts),
            "QUEUE_MAX_ATTEMPTS must be 1..100"
        );
        ensure!(
            (1..=3_600).contains(&self.retry_base_secs),
            "QUEUE_RETRY_BASE_SECS must be 1..3600"
        );
        ensure!(
            (1..=64).contains(&self.concurrency),
            "QUEUE_CONCURRENCY must be 1..64"
        );
        if self.driver == QueueDriver::Redis {
            self.redis.validate()?;
            ensure!(
                self.redis.timeout_ms < self.visibility_timeout_secs * 1_000,
                "QUEUE_REDIS_TIMEOUT_MS must be shorter than the queue visibility timeout"
            );
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct RedisQueueConfig {
    pub url: String,
    pub prefix: String,
    pub timeout_ms: u64,
}

impl Default for RedisQueueConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            prefix: "rust-api-skeleton:queue:v1".into(),
            timeout_ms: 2_000,
        }
    }
}

impl fmt::Debug for RedisQueueConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RedisQueueConfig")
            .field("timeout_ms", &self.timeout_ms)
            .finish_non_exhaustive()
    }
}

impl RedisQueueConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.prefix.is_empty()
                && self.prefix.len() <= 96
                && self.prefix.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-')
                }),
            "QUEUE_REDIS_PREFIX must contain 1..96 ASCII letters, digits, colons, underscores or hyphens"
        );
        ensure!(
            (100..=30_000).contains(&self.timeout_ms),
            "QUEUE_REDIS_TIMEOUT_MS must be 100..30000"
        );
        let url = url::Url::parse(&self.url)
            .map_err(|_| anyhow::anyhow!("QUEUE_REDIS_URL must be a valid Redis URL"))?;
        ensure!(
            matches!(url.scheme(), "redis" | "rediss")
                && url.host_str().is_some_and(|host| !host.is_empty())
                && url.port() != Some(0)
                && url.query().is_none()
                && url.fragment().is_none(),
            "QUEUE_REDIS_URL must use redis:// or verified rediss:// with a host and without query/fragment options"
        );
        let database = url.path().strip_prefix('/').unwrap_or(url.path());
        ensure!(
            database.is_empty()
                || (database.bytes().all(|byte| byte.is_ascii_digit())
                    && database.parse::<u32>().is_ok()),
            "QUEUE_REDIS_URL database index must be a nonnegative integer"
        );
        redis::Client::open(self.url.as_str())
            .map_err(|_| anyhow::anyhow!("QUEUE_REDIS_URL has invalid connection options"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_default_does_not_require_redis() {
        let config = QueueConfig {
            redis: RedisQueueConfig {
                url: "unused-secret".into(),
                ..RedisQueueConfig::default()
            },
            ..QueueConfig::default()
        };
        config.validate().unwrap();
        assert_eq!(config.driver, QueueDriver::Database);
        assert!(!format!("{config:?}").contains("unused-secret"));
    }

    #[test]
    fn redis_queue_requires_its_own_url_and_valid_limits() {
        let mut config = QueueConfig {
            driver: QueueDriver::Redis,
            ..QueueConfig::default()
        };
        assert!(config.validate().is_err());
        config.redis.url = "redis://127.0.0.1:6379/0".into();
        config.validate().unwrap();
        for count in [0, 65, usize::MAX] {
            config.concurrency = count;
            assert!(config.validate().is_err());
        }
    }

    #[test]
    fn queue_driver_and_urls_do_not_expose_credentials() {
        let error = "sensitive-token".parse::<QueueDriver>().unwrap_err();
        assert!(!error.to_string().contains("sensitive-token"));
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
            let config = RedisQueueConfig {
                url: value.into(),
                ..RedisQueueConfig::default()
            };
            let error = config.validate().unwrap_err();
            assert!(!format!("{error:#}").contains("secret-value"));
            assert!(!format!("{config:?}").contains("secret-value"));
        }
    }

    #[test]
    fn queue_namespaces_are_bounded_and_cannot_inject_redis_patterns() {
        for prefix in [
            "".to_owned(),
            "x".repeat(97),
            "queue:*".into(),
            "queue?".into(),
            "queue{tenant}".into(),
        ] {
            assert!(
                RedisQueueConfig {
                    url: "redis://127.0.0.1/0".into(),
                    prefix,
                    ..RedisQueueConfig::default()
                }
                .validate()
                .is_err()
            );
        }
        for url in [
            "redis://:encoded%40password@localhost/1",
            "rediss://user:password@queue.example/0",
        ] {
            RedisQueueConfig {
                url: url.into(),
                ..RedisQueueConfig::default()
            }
            .validate()
            .unwrap();
        }
    }
}
