use std::time::Duration;

use async_trait::async_trait;
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use tokio::sync::OnceCell;

use crate::config::cache::CacheConfig;

use super::{CacheError, CacheResult, CacheStore, ttl_millis};

// The length check and GET execute atomically. Oversized values never cross the
// connection, and {tag, value} distinguishes a miss from an existing empty value.
const BOUNDED_GET: &str = r#"
local length = redis.call('STRLEN', KEYS[1])
if length > tonumber(ARGV[1]) then return {2, false} end
local value = redis.call('GET', KEYS[1])
if not value then return {0, false} end
return {1, value}
"#;

pub(super) struct RedisCacheStore {
    client: redis::Client,
    config: ConnectionManagerConfig,
    manager: OnceCell<ConnectionManager>,
}

impl RedisCacheStore {
    pub(super) fn new(config: &CacheConfig) -> CacheResult<Self> {
        let client = redis::Client::open(config.redis_url.as_str())
            .map_err(|_| CacheError::Configuration)?;
        let timeout = Duration::from_millis(config.timeout_ms);
        let config = ConnectionManagerConfig::new()
            .set_connection_timeout(Some(timeout))
            .set_response_timeout(Some(timeout))
            .set_number_of_retries(2)
            .set_min_delay(Duration::from_millis(25))
            .set_max_delay(Duration::from_millis(100));
        Ok(Self {
            client,
            config,
            manager: OnceCell::new(),
        })
    }

    async fn connection(&self) -> CacheResult<ConnectionManager> {
        // Connect only during an async operation. Cache successful managers only:
        // retaining a failed initial AUTH/SELECT future would prevent recovery
        // after server configuration is corrected. The facade bounds this await.
        self.manager
            .get_or_try_init(|| async {
                ConnectionManager::new_with_config(self.client.clone(), self.config.clone())
                    .await
                    .map_err(safe_error)
            })
            .await
            .cloned()
    }
}

#[async_trait]
impl CacheStore for RedisCacheStore {
    async fn get(&self, key: &str, max_bytes: usize) -> CacheResult<Option<Vec<u8>>> {
        let mut connection = self.connection().await?;
        let (tag, value): (u8, Option<Vec<u8>>) = redis::cmd("EVAL")
            .arg(BOUNDED_GET)
            .arg(1)
            .arg(key)
            .arg(max_bytes)
            .query_async(&mut connection)
            .await
            .map_err(safe_error)?;
        match (tag, value) {
            (0, None) => Ok(None),
            (1, Some(value)) => Ok(Some(value)),
            (2, None) => Err(CacheError::ValueTooLarge),
            _ => Err(CacheError::Unavailable),
        }
    }

    async fn put(&self, key: &str, value: &[u8], ttl: Duration) -> CacheResult<()> {
        let mut connection = self.connection().await?;
        redis::cmd("SET")
            .arg(key)
            .arg(value)
            .arg("PX")
            .arg(ttl_millis(ttl)?)
            .query_async::<()>(&mut connection)
            .await
            .map_err(safe_error)
    }

    async fn forget(&self, key: &str) -> CacheResult<bool> {
        let mut connection = self.connection().await?;
        let removed: u64 = redis::cmd("DEL")
            .arg(key)
            .query_async(&mut connection)
            .await
            .map_err(safe_error)?;
        Ok(removed != 0)
    }

    async fn ping(&self) -> CacheResult<()> {
        let mut connection = self.connection().await?;
        let response: String = redis::cmd("PING")
            .query_async(&mut connection)
            .await
            .map_err(safe_error)?;
        if response == "PONG" {
            Ok(())
        } else {
            Err(CacheError::Unavailable)
        }
    }
}

fn safe_error(error: redis::RedisError) -> CacheError {
    if error.is_timeout() {
        CacheError::Timeout
    } else {
        CacheError::Unavailable
    }
}
