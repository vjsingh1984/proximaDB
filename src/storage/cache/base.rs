//! Base cache implementation for multi-tier caching
//!
//! This module provides `BaseCacheImpl`, a foundational cache implementation that
//! supports hierarchical storage tiers (L1/L2/L3) with automatic promotion/demotion
//! and integration with the global cache orchestrator.

use crate::storage::cache::backend::{
    CacheTier, MemoryBackend, NetworkBackend, NvmeBackend, StorageBackend,
};
// Note: Using new eviction system through CrossCacheOrchestrator
use crate::storage::cache::traits::{BaseCache, CacheEntry, CacheKey, CacheValue};
use crate::storage::traits::{MetricsOperationType, UnifiedMetricsCollector};
use async_trait::async_trait;
use std::hash::Hash;
use std::sync::Arc;

/// Base implementation that specialized caches can build upon
///
/// Provides a three-tier cache hierarchy:
/// - **L1**: In-memory cache (fastest, smallest capacity)
/// - **L2**: NVMe/SSD cache (fast, medium capacity)
/// - **L3**: Network cache (slower, largest capacity)
///
/// # Type Parameters
///
/// - `K`: Cache key type (must implement `CacheKey` + `Hash`)
/// - `V`: Cache value type (must implement `CacheValue`)
///
/// # Example
///
/// ```rust,ignore
/// use proximadb::storage::cache::base::BaseCacheImpl;
///
/// let cache = BaseCacheImpl::<String, Vec<f32>>::new(1024) // 1GB L1
///     .with_l2("/var/cache/proximadb", 10)  // 10GB L2
///     .with_l3("redis://localhost:6379".to_string());  // L3
/// ```
pub struct BaseCacheImpl<K, V>
where
    K: CacheKey,
    V: CacheValue,
{
    /// In-memory cache backend (L1 tier)
    l1_backend: Arc<MemoryBackend<K, CacheEntry<V>>>,
    /// Optional NVMe/SSD backend (L2 tier)
    l2_backend: Option<Arc<NvmeBackend<K, CacheEntry<V>>>>,
    /// Optional network backend (L3 tier)
    l3_backend: Option<Arc<NetworkBackend<K, CacheEntry<V>>>>,

    // Note: Eviction now handled by global CrossCacheOrchestrator
    /// Metrics collector for cache operations
    metrics: Arc<UnifiedMetricsCollector>,

    /// Number of accesses before promoting to higher tier
    promotion_threshold: u32,
    /// Maximum entry size (bytes) allowed in L1 cache
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
    /// Create a new base cache with specified L1 memory limit
    ///
    /// # Arguments
    ///
    /// * `max_memory_mb`: Maximum memory for L1 cache in megabytes
    ///
    /// # Returns
    ///
    /// A new `BaseCacheImpl` with only L1 (memory) backend configured
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

    /// Add an L2 (NVMe/SSD) cache backend
    ///
    /// # Arguments
    ///
    /// * `path`: File system path for L2 cache storage
    /// * `max_size_gb`: Maximum size in gigabytes
    pub fn with_l2(mut self, path: &str, max_size_gb: usize) -> Self {
        self.l2_backend = Some(Arc::new(NvmeBackend::new(path, max_size_gb)));
        self
    }

    /// Add an L3 (network) cache backend
    ///
    /// # Arguments
    ///
    /// * `endpoint`: Network endpoint (e.g., "redis://localhost:6379")
    pub fn with_l3(mut self, endpoint: String) -> Self {
        self.l3_backend = Some(Arc::new(NetworkBackend::new(endpoint)));
        self
    }

    // Note: Eviction strategy now handled by global CrossCacheOrchestrator

    /// Get the number of entries in the L1 cache
    pub async fn size(&self) -> usize {
        self.l1_backend.size().await
    }

    /// Remove a specific entry from the L1 cache
    ///
    /// Returns the value if found and removed, or `None` if the key doesn't exist.
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

    /// Remove every L1 entry whose key matches `predicate`; returns the
    /// count removed. Backs per-collection cache invalidation on writes.
    pub async fn remove_where<F>(&self, predicate: F) -> usize
    where
        F: Fn(&K) -> bool,
    {
        let removed = self.l1_backend.remove_where(predicate).await;
        if removed > 0 {
            let metrics = self.metrics.clone();
            tokio::spawn(async move {
                for _ in 0..removed {
                    metrics
                        .record_operation(
                            MetricsOperationType::Delete,
                            true,
                            0,
                            std::time::Duration::from_secs(0),
                        )
                        .await;
                }
            });
        }
        removed
    }

    /// Get current L1 memory usage in bytes
    pub async fn memory_usage(&self) -> usize {
        self.l1_backend.memory_usage().await
    }

    /// Get reference to the metrics collector
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
            Err(e)
                if e.kind == crate::storage::cache::backend::StorageErrorKind::CapacityExceeded =>
            {
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
                            // L1 still full after eviction — spill to L2 (NVMe/SSD)
                            // instead of dropping the entry (TD-034: graceful degradation)
                            if let Some(ref l2) = self.l2_backend {
                                if let Err(e) = l2.put(key, entry).await {
                                    tracing::warn!(
                                        "L2 spill failed after L1 capacity exceeded: {:?}",
                                        e
                                    );
                                } else {
                                    tracing::debug!(
                                        "Spilled entry to L2 after L1 capacity exceeded"
                                    );
                                }
                            } else {
                                tracing::error!(
                                    "Cache capacity exceeded after eviction, no L2 backend for spillover"
                                );
                            }
                            return;
                        }
                    }
                } else {
                    // No global orchestrator — try L2 spillover directly (TD-034)
                    if let Some(ref l2) = self.l2_backend {
                        if let Err(e) = l2.put(key, entry).await {
                            tracing::warn!("L2 spill failed (no orchestrator): {:?}", e);
                        } else {
                            tracing::debug!("Spilled entry to L2 (no orchestrator available)");
                        }
                    } else {
                        tracing::error!(
                            "Cache capacity exceeded: no orchestrator and no L2 backend for spillover"
                        );
                    }
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
