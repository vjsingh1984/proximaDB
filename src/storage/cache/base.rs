use crate::storage::cache::backend::{
    CacheTier, MemoryBackend, NetworkBackend, NvmeBackend, StorageBackend,
};
use crate::storage::cache::eviction::{CacheState, EvictionStrategy, LRUStrategy};
use crate::storage::cache::metrics::CacheMetrics;
use crate::storage::cache::traits::{BaseCache, CacheEntry, CacheKey, CacheValue};
use async_trait::async_trait;
use std::hash::Hash;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Base implementation that specialized caches can build upon
pub struct BaseCacheImpl<K, V>
where
    K: CacheKey,
    V: CacheValue,
{
    // Storage backends for each tier
    l1_backend: Arc<MemoryBackend<K, CacheEntry<V>>>,
    l2_backend: Option<Arc<NvmeBackend<K, CacheEntry<V>>>>,
    l3_backend: Option<Arc<NetworkBackend<K, CacheEntry<V>>>>,

    // Eviction strategy
    eviction_strategy: Arc<RwLock<Box<dyn EvictionStrategy<Key = K> + Send + Sync>>>,

    // Metrics
    metrics: Arc<CacheMetrics>,

    // Configuration
    promotion_threshold: u32,
    max_entry_size_for_l1: usize,
}

impl<K, V> BaseCacheImpl<K, V>
where
    K: CacheKey + Hash,
    V: CacheValue,
{
    pub fn new(max_memory_mb: usize) -> Self {
        Self {
            l1_backend: Arc::new(MemoryBackend::new(max_memory_mb)),
            l2_backend: None,
            l3_backend: None,
            eviction_strategy: Arc::new(RwLock::new(Box::new(LRUStrategy::new()))),
            metrics: Arc::new(CacheMetrics::new()),
            promotion_threshold: 3,
            max_entry_size_for_l1: 1024 * 1024, // 1MB
        }
    }

    pub fn with_l2(mut self, path: &str, max_size_gb: usize) -> Self {
        self.l2_backend = Some(Arc::new(NvmeBackend::new(path, max_size_gb)));
        self
    }

    pub fn with_l3(mut self, endpoint: String) -> Self {
        self.l3_backend = Some(Arc::new(NetworkBackend::new(endpoint)));
        self
    }

    pub fn with_eviction_strategy<E>(mut self, strategy: E) -> Self
    where
        E: EvictionStrategy<Key = K> + Send + Sync + 'static,
    {
        self.eviction_strategy = Arc::new(RwLock::new(Box::new(strategy)));
        self
    }
}

#[async_trait]
impl<K, V> BaseCache for BaseCacheImpl<K, V>
where
    K: CacheKey + Hash,
    V: CacheValue,
{
    type Key = K;
    type Value = V;

    async fn check_l1(&self, key: &Self::Key) -> Option<Self::Value> {
        if let Some(mut entry) = self.l1_backend.get(key).await {
            entry.touch();
            // Update the entry with new access time
            let _ = self.l1_backend.put(item.clone(), entry.clone()).await;
            Some(entry.value)
        } else {
            None
        }
    }

    async fn check_l2(&self, key: &Self::Key) -> Option<Self::Value> {
        if let Some(ref l2) = self.l2_backend {
            if let Some(mut entry) = l2.get(key).await {
                entry.touch();
                let _ = l2.put(item.clone(), entry.clone()).await;
                Some(entry.value)
            } else {
                None
            }
        } else {
            None
        }
    }

    async fn check_l3(&self, key: &Self::Key) -> Option<Self::Value> {
        if let Some(ref l3) = self.l3_backend {
            if let Some(mut entry) = l3.get(key).await {
                entry.touch();
                let _ = l3.put(item.clone(), entry.clone()).await;
                Some(entry.value)
            } else {
                None
            }
        } else {
            None
        }
    }

    async fn put_l1(&self, key: Self::Key, value: Self::Value) {
        let entry = CacheEntry::new(item.clone());

        // Try to insert
        match self.l1_backend.put(item.clone(), entry.clone()).await {
            Ok(_) => {
                // Success - update eviction strategy and metrics
                let mut strategy = self.eviction_strategy.write().await;
                strategy.update_on_insert(&key, 0);
                self.metrics.record_put();
            }
            Err(crate::storage::cache::backend::StorageError::CapacityExceeded) => {
                // Cache is full - need to evict
                // Try to select a victim using the eviction strategy
                let victim_key = {
                    let strategy = self.eviction_strategy.read().await;
                    let cache_state = CacheState {
                        total_capacity: self.l1_backend.size_bytes().await,
                        current_size: self.l1_backend.size_bytes().await,
                        entry_count: self.l1_backend.entry_count().await,
                    };
                    strategy.select_victim(&cache_state)
                };

                // If we found a victim, evict it
                if let Some(victim) = victim_key {
                    // Remove the victim
                    if self.l1_backend.remove(&victim).await {
                        // Update eviction strategy
                        let mut strategy = self.eviction_strategy.write().await;
                        strategy.update_on_evict(&victim);
                        self.metrics.record_eviction();

                        // Now try to insert the new entry
                        if self.l1_backend.put(item.clone(), entry).await.is_ok() {
                            strategy.update_on_insert(&key, 0);
                            self.metrics.record_put();
                        } else {
                            // Still failed after eviction
                            self.metrics.record_put();
                        }
                    } else {
                        // Eviction failed
                        self.metrics.record_put();
                    }
                } else {
                    // No victim found - this means the LRU doesn't have any keys tracked
                    // Record eviction to maintain metrics consistency
                    self.metrics.record_eviction();
                    self.metrics.record_put();
                }
            }
            Err(_) => {
                // Other error - just record the put attempt
                self.metrics.record_put();
            }
        }
    }

    async fn put_l2(&self, key: Self::Key, value: Self::Value) {
        if let Some(ref l2) = self.l2_backend {
            let entry = CacheEntry::new(value);
            let _ = l2.put(key, entry).await;
            self.metrics.record_put();
        }
    }

    async fn put_l3(&self, key: Self::Key, value: Self::Value) {
        if let Some(ref l3) = self.l3_backend {
            let entry = CacheEntry::new(value);
            let _ = l3.put(key, entry).await;
            self.metrics.record_put();
        }
    }

    async fn invalidate_l1(&self, key: &Self::Key) -> bool {
        self.l1_backend.remove(key).await
    }

    async fn invalidate_l2(&self, key: &Self::Key) -> bool {
        if let Some(ref l2) = self.l2_backend {
            l2.remove(key).await
        } else {
            false
        }
    }

    async fn invalidate_l3(&self, key: &Self::Key) -> bool {
        if let Some(ref l3) = self.l3_backend {
            l3.remove(key).await
        } else {
            false
        }
    }

    async fn promote_to_l1(&self, key: &Self::Key, value: &Self::Value) {
        // Check if value is small enough for L1
        if value.size_bytes() <= self.max_entry_size_for_l1 {
            self.put_l1(item.clone(), item.clone()).await;
        }
    }

    async fn promote_to_l2(&self, key: &Self::Key, value: &Self::Value) {
        // First try to promote to L1
        self.promote_to_l1(key, value).await;

        // Also ensure it's in L2
        if self.l2_backend.is_some() {
            self.put_l2(item.clone(), item.clone()).await;
        }
    }

    async fn select_tier(&self, _key: &Self::Key, value: &Self::Value) -> CacheTier {
        // Simple tier selection based on size
        let size = value.size_bytes();

        if size <= self.max_entry_size_for_l1 {
            CacheTier::L1
        } else if self.l2_backend.is_some() && size <= 10 * 1024 * 1024 {
            CacheTier::L2
        } else if self.l3_backend.is_some() {
            CacheTier::L3
        } else {
            CacheTier::L1 // Fallback to L1
        }
    }

    fn metrics(&self) -> &CacheMetrics {
        &self.metrics
    }
}
