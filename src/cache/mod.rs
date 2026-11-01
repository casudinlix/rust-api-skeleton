//! Optional, namespaced JSON cache for disposable application data.

mod redis;
mod store;

pub use store::{CacheStore, DisabledCacheStore};

use std::{future::Future, io::Write, sync::Arc, time::Duration};

use serde::{Serialize, de::DeserializeOwned};

use crate::config::cache::{CacheConfig, CacheDriver};

use self::redis::RedisCacheStore;

pub type CacheResult<T> = Result<T, CacheError>;

/// Safe categories only: backend messages can contain credentials or payloads.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CacheError {
    #[error("invalid cache configuration")]
    Configuration,
    #[error("invalid cache key")]
    InvalidKey,
    #[error("cache TTL must be between one millisecond and thirty days")]
    InvalidTtl,
    #[error("cache value exceeds its configured size limit")]
    ValueTooLarge,
    #[error("cache value could not be serialized or decoded")]
    Serialization,
    #[error("cache is unavailable")]
    Unavailable,
    #[error("cache operation timed out")]
    Timeout,
}

/// An explicitly injected, optional cache. Authentication must continue reading
/// the current database record rather than relying on disposable cached data.
///
/// ```no_run
/// use rust_api_skeleton::{cache::Cache, config::cache::CacheConfig};
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct CatalogSummary { count: u64 }
///
/// # async fn example() -> Result<(), String> {
/// let cache = Cache::from_config(&CacheConfig::default()).map_err(|e| e.to_string())?;
/// let summary: CatalogSummary = cache.remember("tenant:42:catalog:v1", None, || async {
///     // Call an authorized repository/service here on cache miss or failure.
///     Ok::<_, String>(CatalogSummary { count: 12 })
/// }).await?;
/// assert_eq!(summary.count, 12);
/// # Ok(()) }
/// ```
#[derive(Clone)]
pub struct Cache {
    store: Arc<dyn CacheStore>,
    prefix: String,
    default_ttl: Duration,
    timeout: Duration,
    max_value_bytes: usize,
}

impl Cache {
    /// Validate settings without connecting or requiring a Tokio runtime.
    pub fn from_config(config: &CacheConfig) -> CacheResult<Self> {
        config.validate().map_err(|_| CacheError::Configuration)?;
        let store: Arc<dyn CacheStore> = match config.driver {
            CacheDriver::None => Arc::new(DisabledCacheStore),
            CacheDriver::Redis => Arc::new(RedisCacheStore::new(config)?),
        };
        Self::with_store(config, store)
    }

    /// Inject an alternative byte store; the caller chooses the backend here.
    /// Prefix, payload, TTL and deadline rules still apply at the facade.
    pub fn with_store(config: &CacheConfig, store: Arc<dyn CacheStore>) -> CacheResult<Self> {
        config.validate().map_err(|_| CacheError::Configuration)?;
        Ok(Self {
            store,
            prefix: config.prefix.clone(),
            default_ttl: Duration::from_secs(config.default_ttl_secs),
            timeout: Duration::from_millis(config.timeout_ms),
            max_value_bytes: config.max_value_bytes,
        })
    }

    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> CacheResult<Option<T>> {
        let key = self.key(key)?;
        let bytes = self
            .bounded(self.store.get(&key, self.max_value_bytes))
            .await?;
        bytes
            .map(|bytes| {
                if bytes.len() > self.max_value_bytes {
                    return Err(CacheError::ValueTooLarge);
                }
                serde_json::from_slice(&bytes).map_err(|_| CacheError::Serialization)
            })
            .transpose()
    }

    /// Write JSON with a mandatory expiry; None selects the configured default.
    pub async fn put<T: Serialize + ?Sized>(
        &self,
        key: &str,
        value: &T,
        ttl: Option<Duration>,
    ) -> CacheResult<()> {
        let key = self.key(key)?;
        let ttl = ttl.unwrap_or(self.default_ttl);
        ttl_millis(ttl)?;
        let bytes = serialize_bounded(value, self.max_value_bytes)?;
        self.bounded(self.store.put(&key, &bytes, ttl)).await
    }

    /// Forget exactly one namespaced key. Global flush operations are not exposed.
    pub async fn forget(&self, key: &str) -> CacheResult<bool> {
        let key = self.key(key)?;
        self.bounded(self.store.forget(&key)).await
    }

    pub async fn ping(&self) -> CacheResult<()> {
        self.bounded(self.store.ping()).await
    }

    /// Fall back to the loader on cache miss or error; cache writes are best effort.
    /// Loader errors are returned unchanged and are never cached. Concurrent misses
    /// may invoke multiple loaders; this helper does not provide a distributed lock.
    pub async fn remember<T, E, F, Fut>(
        &self,
        key: &str,
        ttl: Option<Duration>,
        loader: F,
    ) -> Result<T, E>
    where
        T: Serialize + DeserializeOwned,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        match self.get(key).await {
            Ok(Some(value)) => return Ok(value),
            Ok(None) => {}
            Err(error) => tracing::debug!(error_class = %error, "cache read failed; using loader"),
        }
        let value = loader().await?;
        if let Err(error) = self.put(key, &value, ttl).await {
            tracing::debug!(error_class = %error, "cache write failed; returning loaded value");
        }
        Ok(value)
    }

    fn key(&self, key: &str) -> CacheResult<String> {
        validate_key(key)?;
        Ok(format!("{}{key}", self.prefix))
    }

    async fn bounded<T>(&self, task: impl Future<Output = CacheResult<T>>) -> CacheResult<T> {
        tokio::time::timeout(self.timeout, task)
            .await
            .map_err(|_| CacheError::Timeout)?
    }
}

fn validate_key(key: &str) -> CacheResult<()> {
    if key.is_empty()
        || key.len() > 256
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-' | b'.'))
    {
        return Err(CacheError::InvalidKey);
    }
    Ok(())
}

fn ttl_millis(ttl: Duration) -> CacheResult<u64> {
    if ttl < Duration::from_millis(1) || ttl > Duration::from_secs(2_592_000) {
        return Err(CacheError::InvalidTtl);
    }
    u64::try_from(ttl.as_millis()).map_err(|_| CacheError::InvalidTtl)
}

fn serialize_bounded<T: Serialize + ?Sized>(value: &T, max_bytes: usize) -> CacheResult<Vec<u8>> {
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        max_bytes,
        exceeded: false,
    };
    if serde_json::to_writer(&mut writer, value).is_err() {
        return Err(if writer.exceeded {
            CacheError::ValueTooLarge
        } else {
            CacheError::Serialization
        });
    }
    Ok(writer.bytes)
}

struct BoundedWriter {
    bytes: Vec<u8>,
    max_bytes: usize,
    exceeded: bool,
}

impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.max_bytes.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(std::io::Error::other("cache value size limit reached"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use async_trait::async_trait;

    use super::*;

    #[derive(Default)]
    struct MemoryStore(Mutex<HashMap<String, Vec<u8>>>);

    #[async_trait]
    impl CacheStore for MemoryStore {
        async fn get(&self, key: &str, _max_bytes: usize) -> CacheResult<Option<Vec<u8>>> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }
        async fn put(&self, key: &str, value: &[u8], _ttl: Duration) -> CacheResult<()> {
            self.0.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
        async fn forget(&self, key: &str) -> CacheResult<bool> {
            Ok(self.0.lock().unwrap().remove(key).is_some())
        }
        async fn ping(&self) -> CacheResult<()> {
            Ok(())
        }
    }

    struct HangingStore;

    #[async_trait]
    impl CacheStore for HangingStore {
        async fn get(&self, _key: &str, _max_bytes: usize) -> CacheResult<Option<Vec<u8>>> {
            std::future::pending().await
        }
        async fn put(&self, _key: &str, _value: &[u8], _ttl: Duration) -> CacheResult<()> {
            std::future::pending().await
        }
        async fn forget(&self, _key: &str) -> CacheResult<bool> {
            std::future::pending().await
        }
        async fn ping(&self) -> CacheResult<()> {
            std::future::pending().await
        }
    }

    #[test]
    fn rejects_unbounded_or_ambiguous_key_suffixes() {
        for key in ["", "a b", "user/*", "key\n", "é", &"a".repeat(257)] {
            assert_eq!(validate_key(key), Err(CacheError::InvalidKey));
        }
        assert!(validate_key("tenant:123:catalog.v1_2-3").is_ok());
        assert!(validate_key(&"a".repeat(256)).is_ok());
    }

    #[test]
    fn rejects_ttls_that_cannot_produce_a_bounded_expiring_write() {
        for ttl in [
            Duration::ZERO,
            Duration::from_nanos(1),
            Duration::from_secs(2_592_001),
            Duration::MAX,
        ] {
            assert_eq!(ttl_millis(ttl), Err(CacheError::InvalidTtl));
        }
        assert_eq!(ttl_millis(Duration::from_millis(1)), Ok(1));
        assert_eq!(
            ttl_millis(Duration::from_secs(2_592_000)),
            Ok(2_592_000_000)
        );
    }

    #[tokio::test]
    async fn disabled_cache_never_stores_and_still_invokes_the_loader() {
        let cache = Cache::from_config(&CacheConfig::default()).unwrap();
        cache.ping().await.unwrap();
        cache.put("key", &42, None).await.unwrap();
        assert_eq!(cache.get::<u32>("key").await.unwrap(), None);
        assert!(!cache.forget("key").await.unwrap());
        assert_eq!(
            cache
                .remember("key", None, || async { Ok::<_, ()>(12_u32) })
                .await,
            Ok(12)
        );
    }

    #[tokio::test]
    async fn facade_preserves_json_null_namespaces_and_loader_errors() {
        let store = Arc::new(MemoryStore::default());
        let config = CacheConfig::default();
        let cache = Cache::with_store(&config, store.clone()).unwrap();
        let other = Cache::with_store(
            &CacheConfig {
                prefix: "other:".into(),
                ..config
            },
            store,
        )
        .unwrap();
        cache.put("null", &Option::<u32>::None, None).await.unwrap();
        assert_eq!(cache.get::<Option<u32>>("null").await.unwrap(), Some(None));
        assert_eq!(other.get::<Option<u32>>("null").await.unwrap(), None);

        let loads = AtomicUsize::new(0);
        for _ in 0..2 {
            let result = cache
                .remember("loaded", None, || async {
                    loads.fetch_add(1, Ordering::SeqCst);
                    Ok::<_, ()>(123_u32)
                })
                .await;
            assert_eq!(result, Ok(123));
        }
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert_eq!(
            cache
                .remember::<u32, _, _, _>("failure", None, || async { Err("loader failed") })
                .await,
            Err("loader failed")
        );
        assert_eq!(cache.get::<u32>("failure").await.unwrap(), None);
        assert!(cache.forget("loaded").await.unwrap());
        assert_eq!(cache.get::<u32>("loaded").await.unwrap(), None);
    }

    #[tokio::test]
    async fn bounded_serialization_rejects_large_values_and_decode_errors() {
        let store = Arc::new(MemoryStore::default());
        let config = CacheConfig {
            max_value_bytes: 64,
            ..CacheConfig::default()
        };
        let cache = Cache::with_store(&config, store.clone()).unwrap();
        cache.put("value", &"x".repeat(62), None).await.unwrap();
        assert_eq!(
            cache.put("value", &"x".repeat(63), None).await,
            Err(CacheError::ValueTooLarge)
        );
        assert_eq!(
            cache.get::<String>("value").await.unwrap(),
            Some("x".repeat(62))
        );
        store.0.lock().unwrap().insert(
            format!("{}bad", config.prefix),
            b"not-json-secret-value".to_vec(),
        );
        let error = cache.get::<u32>("bad").await.unwrap_err();
        assert_eq!(error, CacheError::Serialization);
        assert!(!format!("{error:?}: {error}").contains("secret-value"));
        store
            .0
            .lock()
            .unwrap()
            .insert(format!("{}huge", config.prefix), vec![b'0'; 65]);
        assert_eq!(
            cache.get::<String>("huge").await,
            Err(CacheError::ValueTooLarge)
        );
        assert_eq!(
            cache.put("zero", &1, Some(Duration::ZERO)).await,
            Err(CacheError::InvalidTtl)
        );
        assert_eq!(
            cache.get::<u32>("bad key").await,
            Err(CacheError::InvalidKey)
        );
        assert_eq!(cache.forget("bad key").await, Err(CacheError::InvalidKey));
    }

    #[test]
    fn redis_construction_needs_no_runtime_or_connection_and_errors_are_safe() {
        let config = CacheConfig {
            driver: CacheDriver::Redis,
            redis_url: "redis://:secret-value@127.0.0.1:1/0".into(),
            ..CacheConfig::default()
        };
        assert!(Cache::from_config(&config).is_ok());
        let bad = CacheConfig {
            redis_url: "invalid-secret-value".into(),
            ..config
        };
        let error = Cache::from_config(&bad).err().unwrap();
        assert_eq!(error, CacheError::Configuration);
        assert!(!format!("{error:?}: {error}").contains("secret-value"));
    }

    #[tokio::test]
    async fn deadlines_cover_all_backend_operations_and_remember_preserves_loader_errors() {
        let cache = Cache::with_store(
            &CacheConfig {
                timeout_ms: 10,
                ..CacheConfig::default()
            },
            Arc::new(HangingStore),
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            assert_eq!(cache.get::<u32>("key").await, Err(CacheError::Timeout));
            assert_eq!(
                cache.put("key", &1_u32, None).await,
                Err(CacheError::Timeout)
            );
            assert_eq!(cache.forget("key").await, Err(CacheError::Timeout));
            assert_eq!(cache.ping().await, Err(CacheError::Timeout));
            assert_eq!(
                cache
                    .remember("key", None, || async { Ok::<_, ()>(19_u32) })
                    .await,
                Ok(19)
            );
            assert_eq!(
                cache
                    .remember::<u32, _, _, _>("key", None, || async { Err("loader unavailable") })
                    .await,
                Err("loader unavailable")
            );
        })
        .await
        .expect("cache facade ignored its operation deadline");
    }
}
