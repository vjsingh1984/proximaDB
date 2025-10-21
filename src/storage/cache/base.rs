use crate::storage::cache::backend::{
    CacheTier, MemoryBackend, NetworkBackend, NvmeBackend, StorageBackend,
};
// Note: Using new eviction system through CrossCacheOrchestrator
use crate::storage::cache::traits::{BaseCache, CacheEntry, CacheKey, CacheValue};
use crate::storage::traits::{MetricsOperationType, UnifiedMetricsCollector};
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

    // Note: Eviction now handled by global CrossCacheOrchestrator

    // Metrics
    metrics: Arc<UnifiedMetricsCollector>,

    // Configuration
    promotion_threshold: u32,
    max_entry_size_for_l1: usize,
}

impl<K, V> std::fmt::Debug for BaseCacheImpl<K, V>
where
    K: CacheKey,
    V: CacheValue,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BaseCacheImpl")
            .field("l1_backend", &"<MemoryBackend>")
            .field(
                "l2_backend",
                &self.l2_backend.as_ref().map(|_| "<NvmeBackend>"),
            )
            .field(
                "l3_backend",
                &self.l3_backend.as_ref().map(|_| "<NetworkBackend>"),
            )
            .field("metrics", &"<UnifiedMetricsCollector>")
            .field("promotion_threshold", &self.promotion_threshold)
            .field("max_entry_size_for_l1", &self.max_entry_size_for_l1)
            .finish()
    }
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
            // eviction_strategy now handled by global orchestrator
            metrics: Arc::new(UnifiedMetricsCollector::new()),
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

    // Note: Eviction strategy now handled by global CrossCacheOrchestrator

    /// Get the number of entries in the cache
    pub async fn size(&self) -> usize {
        self.l1_backend.size().await
    }

    /// Remove a specific entry from the cache
    pub async fn remove(&self, key: &K) -> Option<V> {
        if let Some(entry) = self.l1_backend.remove_and_get(key).await {
            // Record eviction in unified metrics
            let metrics = self.metrics.clone();
            tokio::spawn(async move {
                metrics
                    .record_operation(
                        MetricsOperationType::Delete,
                        true,
                        0,
                        std::time::Duration::from_secs(0),
                    )
                    .await;
            });
            Some(entry.value)
        } else {
            None
        }
    }

    /// Get memory usage in bytes
    pub async fn memory_usage(&self) -> usize {
        self.l1_backend.memory_usage().await
    }

    /// Get cache metrics
    pub fn metrics(&self) -> &UnifiedMetricsCollector {
        &self.metrics
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
            let _ = self.l1_backend.put(key.clone(), entry.clone()).await;
            Some(entry.value)
        } else {
            None
        }
    }

    async fn check_l2(&self, key: &Self::Key) -> Option<Self::Value> {
        if let Some(ref l2) = self.l2_backend {
            if let Some(mut entry) = l2.get(key).await {
                entry.touch();
                let _ = l2.put(key.clone(), entry.clone()).await;
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
                let _ = l3.put(key.clone(), entry.clone()).await;
                Some(entry.value)
            } else {
                None
            }
        } else {
            None
        }
    }

    async fn put_l1(&self, key: Self::Key, value: Self::Value) -> () {
        let entry = CacheEntry::new(value);

        // Try to insert
        match self.l1_backend.put(key.clone(), entry.clone()).await {
            Ok(_) => {
                // Success - update metrics (eviction handled by global orchestrator)
                self.metrics
                    .record_operation(
                        MetricsOperationType::Write,
                        true,
                        0,
                        std::time::Duration::from_secs(0),
                    )
                    .await;
            }
            Err(crate::storage::cache::backend::StorageError::CapacityExceeded) => {
                // Cache is full - trigger eviction through global orchestrator
                if let Some(orchestrator) =
                    crate::storage::cache::orchestrator::CrossCacheOrchestrator::global()
                {
                    // Try to trigger eviction
                    if let Err(e) = orchestrator.trigger_eviction_if_needed().await {
                        tracing::warn!("Failed to trigger cache eviction: {:?}", e);
                    }

                    // Retry the insert after eviction
                    match self.l1_backend.put(key.clone(), entry.clone()).await {
                        Ok(_) => {
                            let metrics = self.metrics.clone();
                            tokio::spawn(async move {
                                metrics
                                    .record_operation(
                                        MetricsOperationType::Write,
                                        true,
                                        0,
                                        std::time::Duration::from_secs(0),
                                    )
                                    .await;
                            });
                        }
                        Err(_) => {
                            // Still fails after eviction - cache is critically full
                            tracing::error!("Cache capacity exceeded even after eviction attempt");
                            return;
                        }
                    }
                } else {
                    // No global orchestrator available
                    tracing::error!(
                        "Cache capacity exceeded and no orchestrator available for eviction"
                    );
                    return;
                }
            }
            Err(_) => {
                // Other error - just record the put attempt
                let metrics = self.metrics.clone();
                tokio::spawn(async move {
                    metrics
                        .record_operation(
                            MetricsOperationType::Write,
                            false,
                            0,
                            std::time::Duration::from_secs(0),
                        )
                        .await;
                });
            }
        }
    }

    async fn put_l2(&self, key: Self::Key, value: Self::Value) {
        if let Some(ref l2) = self.l2_backend {
            let entry = CacheEntry::new(value);
            let _ = l2.put(key, entry).await;
            let metrics = self.metrics.clone();
            tokio::spawn(async move {
                metrics
                    .record_operation(
                        MetricsOperationType::Write,
                        true,
                        0,
                        std::time::Duration::from_secs(0),
                    )
                    .await;
            });
        }
    }

    async fn put_l3(&self, key: Self::Key, value: Self::Value) {
        if let Some(ref l3) = self.l3_backend {
            let entry = CacheEntry::new(value);
            let _ = l3.put(key, entry).await;
            let metrics = self.metrics.clone();
            tokio::spawn(async move {
                metrics
                    .record_operation(
                        MetricsOperationType::Write,
                        true,
                        0,
                        std::time::Duration::from_secs(0),
                    )
                    .await;
            });
        }
    }

    async fn invalidate_l1(&self, key: &Self::Key) -> bool {
        // Use the trait remove method that returns bool
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
            self.put_l1(key.clone(), value.clone()).await;
        }
    }

    async fn promote_to_l2(&self, key: &Self::Key, value: &Self::Value) {
        // First try to promote to L1
        self.promote_to_l1(key, value).await;

        // Also ensure it's in L2
        if self.l2_backend.is_some() {
            self.put_l2(key.clone(), value.clone()).await;
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

    fn metrics(&self) -> &UnifiedMetricsCollector {
        &self.metrics
    }
}
