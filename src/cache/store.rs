use std::time::Duration;

use async_trait::async_trait;

use super::CacheResult;

/// Backend boundary. Keys are already namespaced and validated by Cache.
/// Implementations must bound reads before returning payloads to the caller.
#[async_trait]
pub trait CacheStore: Send + Sync {
    async fn get(&self, key: &str, max_bytes: usize) -> CacheResult<Option<Vec<u8>>>;
    async fn put(&self, key: &str, value: &[u8], ttl: Duration) -> CacheResult<()>;
    async fn forget(&self, key: &str) -> CacheResult<bool>;
    async fn ping(&self) -> CacheResult<()>;
}

pub struct DisabledCacheStore;

#[async_trait]
impl CacheStore for DisabledCacheStore {
    async fn get(&self, _key: &str, _max_bytes: usize) -> CacheResult<Option<Vec<u8>>> {
        Ok(None)
    }
    async fn put(&self, _key: &str, _value: &[u8], _ttl: Duration) -> CacheResult<()> {
        Ok(())
    }
    async fn forget(&self, _key: &str) -> CacheResult<bool> {
        Ok(false)
    }
    async fn ping(&self) -> CacheResult<()> {
        Ok(())
    }
}
