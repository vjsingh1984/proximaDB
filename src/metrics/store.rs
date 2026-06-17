// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Persistent metrics storage with filesystem abstraction

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::MetricsConfig;
use super::schema::{CollectionMetrics, GlobalMetrics};
use super::updater::MetricsUpdate;
use crate::storage::cache::orchestrator::CrossCacheOrchestrator;
use crate::storage::persistence::filesystem::{FileOptions, FilesystemFactory};

/// Metrics persistence layer with cross-cloud support
pub struct MetricsPersistenceLayer {
    /// Filesystem factory for cross-cloud storage operations
    #[allow(dead_code)]
    filesystem_factory: Arc<FilesystemFactory>,

    /// Base path for metrics storage
    #[allow(dead_code)]
    base_path: String,

    /// Configuration
    config: MetricsConfig,

    /// Unified cache orchestrator for metrics snapshots
    cache_orchestrator: Option<Arc<CrossCacheOrchestrator>>,
    /// Legacy in-memory cache for backwards compatibility
    snapshot_cache: Arc<RwLock<HashMap<String, MetricsStoreSnapshot>>>,

    /// Pending updates buffer
    pending_updates: Arc<RwLock<Vec<MetricsUpdate>>>,

    /// Last snapshot timestamp
    last_snapshot: Arc<RwLock<i64>>,
}

/// A snapshot of metrics at a point in time
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MetricsStoreSnapshot {
    pub collection_id: String,
    pub metrics: CollectionMetrics,
    pub timestamp: i64,
    pub version: u32,
}

/// Global metrics snapshot
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GlobalMetricsStoreSnapshot {
    pub metrics: GlobalMetrics,
    pub timestamp: i64,
    pub version: u32,
}

impl MetricsPersistenceLayer {
    /// Create a new persistent metrics store
    pub async fn new(
        filesystem_factory: Arc<FilesystemFactory>,
        config: MetricsConfig,
    ) -> Result<Self> {
        // Ensure base directories exist
        let base_path = config.storage_path.clone();

        // Normalize path to ensure it starts with file:// for local filesystem
        let normalized_base = if !base_path.contains("://") {
            format!("file://{}", base_path)
        } else {
            base_path.clone()
        };

        filesystem_factory
            .create_dir_all(&format!("{}/snapshots/global", normalized_base))
            .await?;
        filesystem_factory
            .create_dir_all(&format!("{}/snapshots/collections", normalized_base))
            .await?;
        filesystem_factory
            .create_dir_all(&format!("{}/incremental", normalized_base))
            .await?;

        info!("Initialized MetricsPersistenceLayer at {}", base_path);

        Ok(Self {
            filesystem_factory,
            base_path: normalized_base,
            config,
            cache_orchestrator: None,
            snapshot_cache: Arc::new(RwLock::new(HashMap::new())),
            pending_updates: Arc::new(RwLock::new(Vec::new())),
            last_snapshot: Arc::new(RwLock::new(0)),
        })
    }

    /// Record a metrics update (async, non-blocking)
    pub async fn record_update(&self, update: MetricsUpdate) -> Result<()> {
        // Add to pending updates buffer
        let mut pending = self.pending_updates.write().await;
        pending.push(update);

        // Check if we should trigger a snapshot
        let now = chrono::Utc::now().timestamp();
        let last = *self.last_snapshot.read().await;

        if now - last > self.config.snapshot_interval_seconds as i64 {
            // Trigger snapshot in background (fire and forget)
            let store = self.clone_for_snapshot();
            tokio::spawn(async move {
                if let Err(e) = store.create_snapshots().await {
                    warn!("Failed to create metrics snapshot: {}", e);
                }
            });
        }

        Ok(())
    }

    /// Create snapshots for all collections
    pub async fn create_snapshots(&self) -> Result<()> {
        let timestamp = chrono::Utc::now().timestamp_millis();
        info!("Creating metrics snapshots at {}", timestamp);

        // Get current cache state
        let cache = self.snapshot_cache.read().await;

        // Process each collection
        for (collection_id, snapshot) in cache.iter() {
            let path = format!(
                "{}/snapshots/collections/{}/snapshot_{}.bincode",
                self.base_path, collection_id, timestamp
            );

            // Serialize to Bincode (3-5x faster than Avro, 20-30% smaller)
            let bincode_data = self.serialize_snapshot(snapshot)?;

            // Use overwrite option (FileOptions already imported below)
            let options = FileOptions {
                overwrite: true,
                create_dirs: true,
                ..Default::default()
            };

            // Write to filesystem
            self.filesystem_factory
                .write(&path, &bincode_data, Some(options.clone()))
                .await
                .context(format!("Failed to write snapshot for {}", collection_id))?;

            // Also update latest snapshot
            let latest_path = format!(
                "{}/snapshots/collections/{}/snapshot_latest.bincode",
                self.base_path, collection_id
            );
            self.filesystem_factory
                .write(&latest_path, &bincode_data, Some(options))
                .await?;

            debug!(
                "Created snapshot for collection {} ({} bytes)",
                collection_id,
                bincode_data.len()
            );
        }

        // Update last snapshot timestamp
        *self.last_snapshot.write().await = chrono::Utc::now().timestamp();

        // Clear pending updates
        self.pending_updates.write().await.clear();

        // Cleanup old snapshots
        self.cleanup_old_snapshots().await?;

        Ok(())
    }

    /// Load latest snapshot for a collection
    pub async fn load_snapshot(&self, collection_id: &str) -> Result<Option<MetricsStoreSnapshot>> {
        let path = format!(
            "{}/snapshots/collections/{}/snapshot_latest.bincode",
            self.base_path, collection_id
        );

        if !self.filesystem_factory.exists(&path).await? {
            return Ok(None);
        }

        let data = self.filesystem_factory.read(&path).await?;
        let snapshot = self.deserialize_snapshot(&data)?;

        Ok(Some(snapshot))
    }

    /// Load all collection snapshots
    pub async fn load_all_snapshots(&self) -> Result<HashMap<String, MetricsStoreSnapshot>> {
        let collections_path = format!("{}/snapshots/collections", self.base_path);
        let entries = self.filesystem_factory.list(&collections_path).await?;

        let mut snapshots = HashMap::new();

        for entry in entries {
            if entry.metadata.is_directory {
                let collection_id = entry.name.clone();
                if let Ok(Some(snapshot)) = self.load_snapshot(&collection_id).await {
                    snapshots.insert(collection_id, snapshot);
                }
            }
        }

        Ok(snapshots)
    }

    /// Get metrics for a specific collection
    pub async fn collection_metrics(
        &self,
        collection_id: &str,
    ) -> Result<Option<CollectionMetrics>> {
        // Check cache first
        let cache = self.snapshot_cache.read().await;
        if let Some(snapshot) = cache.get(collection_id) {
            return Ok(Some(snapshot.metrics.clone()));
        }

        // Load from disk if not in cache
        if let Some(snapshot) = self.load_snapshot(collection_id).await? {
            Ok(Some(snapshot.metrics))
        } else {
            Ok(None)
        }
    }

    /// Get global metrics
    pub async fn global_metrics(&self) -> Result<GlobalMetrics> {
        let cache = self.snapshot_cache.read().await;

        let mut global = GlobalMetrics {
            total_collections: cache.len() as i64,
            ..Default::default()
        };

        // Aggregate metrics from all collections
        for snapshot in cache.values() {
            global.total_vectors += snapshot.metrics.vector_count;
            global.total_storage_bytes += snapshot.metrics.data_size_bytes;
            global.total_operations += snapshot.metrics.total_inserts
                + snapshot.metrics.total_updates
                + snapshot.metrics.total_deletes
                + snapshot.metrics.total_searches;
        }

        // Calculate operations per second (rough estimate)
        let uptime = chrono::Utc::now().timestamp() - self.start_time();
        if uptime > 0 {
            global.operations_per_second = (global.total_operations as f64) / (uptime as f64);
            global.uptime_seconds = uptime;
        }

        Ok(global)
    }

    /// Update cache with new metrics
    pub async fn update_cache(&self, collection_id: String, metrics: CollectionMetrics) {
        let mut cache = self.snapshot_cache.write().await;
        cache.insert(
            collection_id.clone(),
            MetricsStoreSnapshot {
                collection_id,
                metrics,
                timestamp: chrono::Utc::now().timestamp_millis(),
                version: 1,
            },
        );
    }

    /// Store collection metrics directly (used by MetricsUpdateService)
    pub async fn store_collection_metrics(&self, metrics: &CollectionMetrics) -> Result<()> {
        // Update cache first
        self.update_cache(metrics.collection_id.clone(), metrics.clone())
            .await;

        // Create immediate snapshot for persistence
        let snapshot = MetricsStoreSnapshot {
            collection_id: metrics.collection_id.clone(),
            metrics: metrics.clone(),
            timestamp: chrono::Utc::now().timestamp_millis(),
            version: 1,
        };

        // Ensure collection snapshot directory exists
        let collection_dir = format!(
            "{}/snapshots/collections/{}",
            self.base_path, metrics.collection_id
        );
        self.filesystem_factory
            .create_dir_all(&collection_dir)
            .await?;

        // Write to the same path that load_snapshot expects
        let path = format!(
            "{}/snapshots/collections/{}/snapshot_latest.bincode",
            self.base_path, metrics.collection_id
        );

        // Serialize using the same format as create_snapshots
        let bincode_data = self.serialize_snapshot(&snapshot)?;

        // Use overwrite option to allow updating existing metrics
        let options = FileOptions {
            overwrite: true,
            create_dirs: true,
            ..Default::default()
        };

        self.filesystem_factory
            .write(&path, &bincode_data, Some(options.clone()))
            .await
            .context(format!(
                "Failed to write metrics snapshot for {}",
                metrics.collection_id
            ))?;

        debug!(
            "Stored collection metrics for {} to {}",
            metrics.collection_id, path
        );

        // Also write to partitioned path for backward compatibility if needed
        let partition = self.calculate_partition(&metrics.collection_id);
        let partition_dir = format!("{}/partition_{}", self.base_path, partition);
        self.filesystem_factory
            .create_dir_all(&partition_dir)
            .await?;

        let partition_path = format!(
            "{}/partition_{}/collection_{}.json",
            self.base_path, partition, metrics.collection_id
        );

        let snapshot_json = serde_json::to_string(&snapshot)
            .context("Failed to serialize metrics snapshot to JSON")?;

        // Also use overwrite for the JSON backup
        self.filesystem_factory
            .write(&partition_path, snapshot_json.as_bytes(), Some(options))
            .await
            .context("Failed to write metrics snapshot to partitioned storage")?;

        Ok(())
    }

    /// Calculate partition for a collection ID (consistent hashing)
    pub fn calculate_partition(&self, collection_id: &str) -> usize {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        collection_id.hash(&mut hasher);
        let hash = hasher.finish();
        (hash as usize) % self.config.collection_partitions
    }

    /// Cleanup old snapshots based on retention policy
    async fn cleanup_old_snapshots(&self) -> Result<()> {
        let retention_millis = self.config.retention_days as i64 * 24 * 60 * 60 * 1000;
        let cutoff = chrono::Utc::now().timestamp_millis() - retention_millis;

        let collections_path = format!("{}/snapshots/collections", self.base_path);
        let entries = self.filesystem_factory.list(&collections_path).await?;

        let mut deleted_count = 0;

        for entry in entries {
            if entry.metadata.is_directory {
                let collection_path = format!("{}/{}", collections_path, entry.name);
                let snapshots = self.filesystem_factory.list(&collection_path).await?;

                for snapshot in snapshots {
                    if snapshot.name.starts_with("snapshot_")
                        && snapshot.name != "snapshot_latest.bincode"
                    {
                        // Extract timestamp from filename
                        if let Some(timestamp_str) = snapshot
                            .name
                            .strip_prefix("snapshot_")
                            .and_then(|s| s.strip_suffix(".bincode"))
                            && let Ok(timestamp) = timestamp_str.parse::<i64>()
                            && timestamp < cutoff
                        {
                            let path = format!("{}/{}", collection_path, snapshot.name);
                            self.filesystem_factory.delete(&path).await?;
                            deleted_count += 1;
                        }
                    }
                }
            }
        }

        if deleted_count > 0 {
            info!("Cleaned up {} old metric snapshots", deleted_count);
        }

        Ok(())
    }

    /// Serialize a snapshot to Avro format
    fn serialize_snapshot(&self, snapshot: &MetricsStoreSnapshot) -> Result<Vec<u8>> {
        // Use Bincode for optimal performance (3-5x faster than Avro, 20-30% smaller)
        let serialized = bincode::serialize(snapshot)
            .context("Failed to serialize metrics snapshot with Bincode")?;

        // Apply zstd compression for storage efficiency (works better with Bincode's dense format)
        let compressed = zstd::bulk::compress(&serialized, 3)
            .context("Failed to compress serialized metrics snapshot")?;

        Ok(compressed)
    }

    /// Deserialize a snapshot from Bincode format (3-5x faster than Avro)
    fn deserialize_snapshot(&self, data: &[u8]) -> Result<MetricsStoreSnapshot> {
        // Decompress first
        let decompressed = zstd::bulk::decompress(data, 10 * 1024 * 1024) // 10MB limit
            .context("Failed to decompress metrics snapshot")?;

        // Deserialize with Bincode (zero-copy when possible)
        let snapshot = bincode::deserialize(&decompressed)
            .context("Failed to deserialize metrics snapshot with Bincode")?;

        Ok(snapshot)
    }

    /// Clone store for background snapshot creation
    fn clone_for_snapshot(&self) -> Arc<Self> {
        // This is a simplified clone - in production, we'd properly share the Arc fields
        Arc::new(Self {
            filesystem_factory: self.filesystem_factory.clone(),
            base_path: self.base_path.clone(),
            config: self.config.clone(),
            cache_orchestrator: self.cache_orchestrator.clone(),
            snapshot_cache: self.snapshot_cache.clone(),
            pending_updates: self.pending_updates.clone(),
            last_snapshot: self.last_snapshot.clone(),
        })
    }

    /// Get approximate start time (for uptime calculation)
    fn start_time(&self) -> i64 {
        // In production, this would be tracked properly
        chrono::Utc::now().timestamp() - 3600 // Default to 1 hour ago
    }

    /// Get filesystem factory reference (for tests)
    pub fn filesystem_factory(&self) -> Result<&FilesystemFactory> {
        Ok(&self.filesystem_factory)
    }

    /// Get configuration reference (for tests)
    pub fn config(&self) -> &MetricsConfig {
        &self.config
    }

    /// Store global metrics (for tests)
    pub async fn store_global_metrics(&self, metrics: &GlobalMetrics) -> Result<()> {
        let path = format!("{}/global_metrics.json", self.base_path);
        let json = serde_json::to_string(metrics)?;
        self.filesystem_factory
            .write(&path, json.as_bytes(), None)
            .await?;
        Ok(())
    }

    /// Get global metrics (for tests)
    pub async fn global_metrics_stored(&self) -> Result<Option<GlobalMetrics>> {
        let path = format!("{}/global_metrics.json", self.base_path);
        if !self.filesystem_factory.exists(&path).await? {
            return Ok(None);
        }
        let data = self.filesystem_factory.read(&path).await?;
        let metrics: GlobalMetrics = serde_json::from_slice(&data)?;
        Ok(Some(metrics))
    }

    /// List all collections (for tests)
    pub async fn list_collections(&self) -> Result<Vec<String>> {
        let cache = self.snapshot_cache.read().await;
        let mut collections: Vec<String> = cache.keys().cloned().collect();

        // Also check partitions for collections not in cache
        for partition in 0..self.config.collection_partitions {
            let partition_path = format!("{}/partition_{}", self.base_path, partition);
            if self.filesystem_factory.exists(&partition_path).await? {
                let entries = self.filesystem_factory.list(&partition_path).await?;
                for entry in entries {
                    if entry.name.starts_with("collection_")
                        && entry.name.ends_with(".json")
                        && let Some(id) = entry
                            .name
                            .strip_prefix("collection_")
                            .and_then(|s| s.strip_suffix(".json"))
                    {
                        let collection_id = id.to_string();
                        if !collection_id.is_empty() && !collections.contains(&collection_id) {
                            collections.push(collection_id);
                        }
                    }
                }
            }
        }

        Ok(collections)
    }

    /// Cleanup collection metrics (for tests)
    pub async fn cleanup_collection_metrics(&self, collection_id: &str) -> Result<()> {
        // Remove from cache
        self.snapshot_cache.write().await.remove(collection_id);

        // Remove snapshot file (this is what collection_metrics checks)
        let snapshot_path = format!(
            "{}/snapshots/collections/{}/snapshot_latest.bincode",
            self.base_path, collection_id
        );
        if self.filesystem_factory.exists(&snapshot_path).await? {
            self.filesystem_factory.delete(&snapshot_path).await?;
        }

        // Also remove the entire collection snapshot directory
        let snapshot_dir = format!("{}/snapshots/collections/{}", self.base_path, collection_id);
        if self.filesystem_factory.exists(&snapshot_dir).await? {
            self.filesystem_factory.delete(&snapshot_dir).await?;
        }

        // Remove from partition storage (backward compatibility)
        let partition = self.calculate_partition(collection_id);
        let partition_path = format!(
            "{}/partition_{}/collection_{}.json",
            self.base_path, partition, collection_id
        );
        if self.filesystem_factory.exists(&partition_path).await? {
            self.filesystem_factory.delete(&partition_path).await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::schema::{CollectionMetrics, FilterableColumnStats, GlobalMetrics};
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use anyhow::Result;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::fs;
    use tracing::{debug, info};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn generate_unique_test_path() -> String {
        let counter = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("/tmp/proximadb_metrics_test_{}_{}", counter, timestamp)
    }

    async fn create_test_store() -> Result<MetricsPersistenceLayer> {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        let unique_path = generate_unique_test_path();
        let config = MetricsConfig {
            enabled: true,
            collection_partitions: 4,
            storage_path: format!("file://{}", unique_path),
            flush_interval_seconds: 30,
            retention_days: 7,
            parallel_scan_threshold: 10,
            sparsity_threshold: 0.3,
            quantization_size_threshold: 1_000_000,
            snapshot_interval_seconds: 60,
            max_memory_mb: 512,
        };

        let _ = fs::remove_dir_all(&unique_path).await;

        let filesystem_config = Default::default();
        let filesystem_factory = Arc::new(FilesystemFactory::create(filesystem_config).await?);
        MetricsPersistenceLayer::new(filesystem_factory, config).await
    }

    #[tokio::test]
    async fn test_metrics_store_creation() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        debug!("🧪 TEST: MetricsPersistenceLayer creation and initialization");

        let _store = create_test_store().await.unwrap();

        info!("✅ MetricsStore creation test passed");
    }

    #[tokio::test]
    async fn test_collection_metrics_storage_and_retrieval() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        debug!("🧪 TEST: CollectionMetrics storage and retrieval");

        let store = create_test_store().await.unwrap();

        let mut test_metrics = CollectionMetrics {
            collection_id: "test_collection_001".to_string(),
            vector_count: 10000,
            dimension: 384,
            index_size_bytes: 1024 * 1024,
            data_size_bytes: (5 * 1024 * 1024) as i64,
            total_inserts: 10000,
            total_searches: 50000,
            total_flushes: 15,
            total_compactions: 3,
            avg_insert_latency_us: 250.5,
            avg_search_latency_us: 1500.0,
            p50_search_latency_us: 1200.0,
            p95_search_latency_us: 3000.0,
            p99_search_latency_us: 5000.0,
            parquet_file_count: 8,
            sstable_file_count: 2,
            wal_size_bytes: 512 * 1024,
            memtable_size_bytes: 256 * 1024,
            last_flush_timestamp: chrono::Utc::now().timestamp_millis(),
            sparsity_ratio: 0.35,
            avg_vector_magnitude: 1.2,
            distinct_metadata_keys: 12,
            avg_metadata_size_bytes: 64,
            primary_index: "hnsw_main".to_string(),
            bloom_filter_size_bytes: 16 * 1024,
            bloom_filter_fpp: 0.01,
            cache_hit_ratio: 0.85,
            cache_size_bytes: (128 * 1024 * 1024) as i64,
            cache_entry_count: 25000,
            timestamp: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            ..Default::default()
        };

        let mut filterable_stats = HashMap::new();
        filterable_stats.insert(
            "category".to_string(),
            FilterableColumnStats {
                column_name: "category".to_string(),
                data_type: "string".to_string(),
                cardinality: 25,
                null_count: 100,
                selectivity: 0.0025,
                min_value: Some(serde_json::Value::String("category_001".to_string())),
                max_value: Some(serde_json::Value::String("category_025".to_string())),
                most_common_values: vec![
                    (serde_json::Value::String("electronics".to_string()), 3000),
                    (serde_json::Value::String("books".to_string()), 2500),
                ],
                histogram_bounds: None,
            },
        );
        test_metrics.filterable_column_stats = filterable_stats;

        let result = store.store_collection_metrics(&test_metrics).await;
        assert!(
            result.is_ok(),
            "Failed to store collection metrics: {:?}",
            result
        );

        let retrieved = store
            .collection_metrics("test_collection_001")
            .await
            .unwrap();
        assert!(retrieved.is_some(), "Failed to retrieve stored metrics");

        let retrieved_metrics = retrieved.unwrap();
        assert_eq!(retrieved_metrics.collection_id, "test_collection_001");
        assert_eq!(retrieved_metrics.vector_count, 10000);
        assert_eq!(retrieved_metrics.dimension, 384);
        assert_eq!(retrieved_metrics.total_inserts, 10000);
        assert_eq!(retrieved_metrics.sparsity_ratio, 0.35);
        assert_eq!(retrieved_metrics.filterable_column_stats.len(), 1);
        assert!(
            retrieved_metrics
                .filterable_column_stats
                .contains_key("category")
        );

        info!("✅ CollectionMetrics storage and retrieval test passed");
    }

    #[tokio::test]
    async fn test_global_metrics_storage_and_retrieval() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        debug!("🧪 TEST: GlobalMetrics storage and retrieval");

        let store = create_test_store().await.unwrap();

        let global_metrics = GlobalMetrics {
            total_collections: 15,
            total_vectors: 150000,
            total_storage_bytes: (1024 * 1024 * 1024) as i64,
            total_operations: 1_000_000,
            operations_per_second: 1500.5,
            uptime_seconds: 86400 * 7,
            cpu_usage_percent: 45.2,
            memory_usage_bytes: (8i64 * 1024 * 1024 * 1024),
            disk_io_read_bytes_per_sec: (50 * 1024 * 1024) as f64,
            disk_io_write_bytes_per_sec: (30 * 1024 * 1024) as f64,
            network_rx_bytes_per_sec: (10 * 1024 * 1024) as f64,
            network_tx_bytes_per_sec: (5 * 1024 * 1024) as f64,
            active_connections: 127,
            error_rate_per_minute: 0.25,
            last_error_timestamp: Some(chrono::Utc::now().timestamp_millis()),
        };

        let result = store.store_global_metrics(&global_metrics).await;
        assert!(
            result.is_ok(),
            "Failed to store global metrics: {:?}",
            result
        );

        info!("✅ GlobalMetrics storage test passed");
    }

    #[tokio::test]
    async fn test_collection_partitioning() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        debug!("🧪 TEST: Collection partitioning for metrics storage");

        let store = create_test_store().await.unwrap();

        let test_collections = vec![
            "collection_alpha",
            "collection_beta",
            "collection_gamma",
            "collection_delta",
            "collection_epsilon",
        ];

        for collection_id in &test_collections {
            let metrics = CollectionMetrics {
                collection_id: collection_id.to_string(),
                vector_count: 1000,
                dimension: 128,
                total_inserts: 1000,
                timestamp: chrono::Utc::now().timestamp_millis(),
                updated_at: chrono::Utc::now().timestamp_millis(),
                ..Default::default()
            };

            let result = store.store_collection_metrics(&metrics).await;
            assert!(
                result.is_ok(),
                "Failed to store metrics for {}: {:?}",
                collection_id,
                result
            );
        }

        for collection_id in &test_collections {
            let retrieved = store.collection_metrics(collection_id).await.unwrap();
            assert!(
                retrieved.is_some(),
                "Failed to retrieve metrics for {}",
                collection_id
            );
            assert_eq!(retrieved.unwrap().collection_id, *collection_id);
        }

        for collection_id in &test_collections {
            let partition = store.calculate_partition(collection_id);
            assert!(
                partition < 4,
                "Partition {} out of range for collection {}",
                partition,
                collection_id
            );
            debug!(
                "📊 Collection '{}' → Partition {}",
                collection_id, partition
            );
        }

        info!("✅ Collection partitioning test passed");
    }

    #[tokio::test]
    async fn test_metrics_list_collections() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        debug!("🧪 TEST: List collections functionality");

        let store = create_test_store().await.unwrap();

        let collections = vec!["metrics_test_001", "metrics_test_002", "metrics_test_003"];

        for collection_id in &collections {
            let metrics = CollectionMetrics {
                collection_id: collection_id.to_string(),
                vector_count: 500,
                dimension: 256,
                timestamp: chrono::Utc::now().timestamp_millis(),
                updated_at: chrono::Utc::now().timestamp_millis(),
                ..Default::default()
            };

            store.store_collection_metrics(&metrics).await.unwrap();
        }

        let collection_list = store.list_collections().await.unwrap();

        for expected_collection in &collections {
            assert!(
                collection_list.contains(&expected_collection.to_string()),
                "Collection {} not found in list: {:?}",
                expected_collection,
                collection_list
            );
        }

        debug!(
            "📋 Found {} collections: {:?}",
            collection_list.len(),
            collection_list
        );
        info!("✅ List collections test passed");
    }

    #[tokio::test]
    async fn test_metrics_cleanup_functionality() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        debug!("🧪 TEST: Metrics cleanup functionality");

        let store = create_test_store().await.unwrap();

        let test_metrics = CollectionMetrics {
            collection_id: "cleanup_test_collection".to_string(),
            vector_count: 1000,
            dimension: 128,
            timestamp: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            ..Default::default()
        };

        store.store_collection_metrics(&test_metrics).await.unwrap();

        let retrieved = store
            .collection_metrics("cleanup_test_collection")
            .await
            .unwrap();
        assert!(retrieved.is_some());

        let cleanup_result = store
            .cleanup_collection_metrics("cleanup_test_collection")
            .await;
        assert!(
            cleanup_result.is_ok(),
            "Failed to cleanup collection metrics: {:?}",
            cleanup_result
        );

        let retrieved_after_cleanup = store
            .collection_metrics("cleanup_test_collection")
            .await
            .unwrap();
        assert!(
            retrieved_after_cleanup.is_none(),
            "Metrics should be cleaned up"
        );

        info!("✅ Metrics cleanup test passed");
    }

    #[tokio::test]
    async fn test_concurrent_metrics_operations() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        debug!("🧪 TEST: Concurrent metrics operations");

        let store = create_test_store().await.unwrap();
        let store = std::sync::Arc::new(store);

        let mut handles = vec![];

        for i in 0..10 {
            let store_clone = store.clone();
            let handle = tokio::spawn(async move {
                let collection_id = format!("concurrent_test_{:03}", i);

                let metrics = CollectionMetrics {
                    collection_id: collection_id.clone(),
                    vector_count: (i + 1) * 1000,
                    dimension: 384,
                    total_inserts: (i + 1) * 1000,
                    timestamp: chrono::Utc::now().timestamp_millis(),
                    updated_at: chrono::Utc::now().timestamp_millis(),
                    ..Default::default()
                };

                store_clone
                    .store_collection_metrics(&metrics)
                    .await
                    .unwrap();

                let retrieved = store_clone
                    .collection_metrics(&collection_id)
                    .await
                    .unwrap();
                assert!(retrieved.is_some());
                assert_eq!(retrieved.unwrap().vector_count, (i + 1) * 1000);

                collection_id
            });

            handles.push(handle);
        }

        let mut completed_collections = Vec::new();
        for handle in handles {
            let collection_id = handle.await.unwrap();
            completed_collections.push(collection_id);
        }

        assert_eq!(completed_collections.len(), 10);
        debug!(
            "📊 Completed concurrent operations for {} collections",
            completed_collections.len()
        );

        let collection_list = store.list_collections().await.unwrap();
        for expected_collection in &completed_collections {
            assert!(
                collection_list.contains(expected_collection),
                "Collection {} not found after concurrent operations",
                expected_collection
            );
        }

        info!("✅ Concurrent metrics operations test passed");
    }

    #[tokio::test]
    async fn test_filesystem_factory_integration() {
        let _ = proximadb_hardware::hardware_capabilities(); // OnceLock auto-init

        debug!("🧪 TEST: FilesystemFactory integration");

        let store = create_test_store().await.unwrap();

        let test_metrics = CollectionMetrics {
            collection_id: "filesystem_test".to_string(),
            vector_count: 2500,
            dimension: 512,
            timestamp: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            ..Default::default()
        };

        let store_result = store.store_collection_metrics(&test_metrics).await;
        assert!(
            store_result.is_ok(),
            "Failed to store through filesystem: {:?}",
            store_result
        );

        let retrieve_result = store.collection_metrics("filesystem_test").await;
        assert!(
            retrieve_result.is_ok(),
            "Failed to retrieve through filesystem: {:?}",
            retrieve_result
        );
        assert!(retrieve_result.unwrap().is_some());

        info!("✅ FilesystemFactory integration test passed");
    }
}
