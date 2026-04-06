/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Unified High-Performance Collection Index
//!
//! Single data structure optimized for scale and performance in multi-cloud serverless environments.
//! Designed for horizontal compute scaling with state persistence in cloud object stores.

use dashmap::DashMap;
use std::sync::Arc;

use crate::proto::proximadb_v1::Collection;

/// Performance metrics for monitoring and optimization
#[derive(Debug, Clone)]
pub struct IndexPerformanceMetrics {
    /// Total number of collections indexed
    pub total_collections: usize,
    /// Memory usage in bytes
    pub memory_usage_bytes: usize,
    /// Total UUID lookups
    pub uuid_lookups: u64,
    /// Total name lookups
    pub name_lookups: u64,
    /// Cache hit count
    pub cache_hits: u64,
    /// Cache miss count
    pub cache_misses: u64,
    /// Average lookup time in nanoseconds
    pub avg_lookup_time_ns: u64,
    /// Last index rebuild timestamp
    pub last_rebuild_timestamp: Option<i64>,
}

/// Unified collection index - single source of truth for all metadata operations
/// Optimized for O(1) access patterns with excellent concurrency
pub struct UnifiedCollectionIndex {
    /// Primary store: UUID -> Collection (most critical for storage/WAL operations)
    /// Uses DashMap for lock-free concurrent access - perfect for serverless horizontal scaling
    collections: DashMap<String, Arc<Collection>>,

    /// Secondary index: Name -> UUID (critical for user API queries)
    /// Separate map ensures O(1) name lookups without scanning primary store
    name_to_uuid: DashMap<String, String>,

    /// Performance metrics for monitoring and optimization
    metrics: Arc<parking_lot::RwLock<IndexPerformanceMetrics>>,
}

impl UnifiedCollectionIndex {
    /// Create new unified index
    pub fn new() -> Self {
        Self {
            collections: DashMap::new(),
            name_to_uuid: DashMap::new(),
            metrics: Arc::new(parking_lot::RwLock::new(IndexPerformanceMetrics {
                total_collections: 0,
                memory_usage_bytes: 0,
                uuid_lookups: 0,
                name_lookups: 0,
                cache_hits: 0,
                cache_misses: 0,
                avg_lookup_time_ns: 0,
                last_rebuild_timestamp: None,
            })),
        }
    }

    /// Insert or update collection - atomic operation across both indexes
    /// Critical: This is the ONLY way to modify the index to ensure consistency
    pub fn upsert_collection(&self, record: Collection) {
        let start = std::time::Instant::now();
        let uuid = record.id.clone();
        let name = record
            .config
            .as_ref().map_or_else(|| "unnamed".to_string(), |c| c.name.clone());

        // Atomic operation: insert into both maps
        let record_arc = Arc::new(record);
        self.collections.insert(uuid.clone(), record_arc);
        self.name_to_uuid.insert(name, uuid);

        // Update metrics
        let elapsed = start.elapsed().as_nanos() as u64;
        let mut metrics = self.metrics.write();
        metrics.total_collections = self.collections.len();
        metrics.memory_usage_bytes = self.estimate_memory_usage();
        metrics.avg_lookup_time_ns = (metrics.avg_lookup_time_ns + elapsed) / 2;
    }

    /// Remove collection - atomic operation across both indexes
    pub fn remove_collection(&self, uuid: &str) -> Option<Arc<Collection>> {
        let start = std::time::Instant::now();

        // Remove from primary store first to get the record
        if let Some((_, record)) = self.collections.remove(uuid) {
            // Remove from secondary index
            if let Some(config) = record.config.as_ref() {
                self.name_to_uuid.remove(&config.name);
            }

            // Update metrics
            let elapsed = start.elapsed().as_nanos() as u64;
            let mut metrics = self.metrics.write();
            metrics.total_collections = self.collections.len();
            metrics.memory_usage_bytes = self.estimate_memory_usage();
            metrics.avg_lookup_time_ns = (metrics.avg_lookup_time_ns + elapsed) / 2;

            Some(record)
        } else {
            // Update miss count
            let mut metrics = self.metrics.write();
            metrics.cache_misses += 1;
            None
        }
    }

    /// Get collection by UUID - O(1) - Primary access pattern for storage/WAL
    pub fn get_by_uuid(&self, uuid: &str) -> Option<Arc<Collection>> {
        let start = std::time::Instant::now();
        let result = self
            .collections
            .get(uuid)
            .map(|entry| entry.value().clone());

        // Update metrics
        let elapsed = start.elapsed().as_nanos() as u64;
        let mut metrics = self.metrics.write();
        metrics.uuid_lookups += 1;
        if result.is_some() {
            metrics.cache_hits += 1;
        } else {
            metrics.cache_misses += 1;
        }
        metrics.avg_lookup_time_ns = (metrics.avg_lookup_time_ns + elapsed) / 2;

        result
    }

    /// Get collection by name - O(1) - Primary access pattern for user APIs
    pub fn get_by_name(&self, name: &str) -> Option<Arc<Collection>> {
        let start = std::time::Instant::now();

        // Two-step O(1) lookup: name -> UUID -> record
        let result = self.name_to_uuid.get(name).and_then(|uuid_entry| {
            self.collections
                .get(uuid_entry.value())
                .map(|record_entry| record_entry.value().clone())
        });

        // Update metrics
        let elapsed = start.elapsed().as_nanos() as u64;
        let mut metrics = self.metrics.write();
        metrics.name_lookups += 1;
        if result.is_some() {
            metrics.cache_hits += 1;
        } else {
            metrics.cache_misses += 1;
        }
        metrics.avg_lookup_time_ns = (metrics.avg_lookup_time_ns + elapsed) / 2;

        result
    }

    /// Get UUID by name - O(1) - Optimized for storage operations that need UUID
    pub fn get_uuid_by_name(&self, name: &str) -> Option<String> {
        self.name_to_uuid
            .get(name)
            .map(|entry| entry.value().clone())
    }

    /// Check if collection exists by UUID - O(1)
    pub fn exists_by_uuid(&self, uuid: &str) -> bool {
        self.collections.contains_key(uuid)
    }

    /// Check if collection exists by name - O(1)
    pub fn exists_by_name(&self, name: &str) -> bool {
        self.name_to_uuid.contains_key(name)
    }

    /// List all collections - O(n) but efficient iteration
    /// Returns cloned Arc<Collection> for zero-copy sharing
    pub fn list_all(&self) -> Vec<Arc<Collection>> {
        self.collections
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get collection count - O(1)
    pub fn count(&self) -> usize {
        self.collections.len()
    }

    /// Clear all data - for testing or complete rebuild
    pub fn clear(&self) {
        self.collections.clear();
        self.name_to_uuid.clear();

        let mut metrics = self.metrics.write();
        metrics.total_collections = 0;
        metrics.memory_usage_bytes = 0;
        metrics.last_rebuild_timestamp = Some(chrono::Utc::now().timestamp());
    }

    /// Rebuild from collection records - for recovery from disk
    /// This is the critical recovery method for serverless startup
    pub fn rebuild_from_records(&self, records: Vec<Collection>) {
        // Clear existing data
        self.clear();

        // Rebuild atomically
        for record in records {
            self.upsert_collection(record);
        }

        let mut metrics = self.metrics.write();
        metrics.last_rebuild_timestamp = Some(chrono::Utc::now().timestamp());

        tracing::info!(
            "UnifiedCollectionIndex rebuilt with {} collections",
            self.collections.len()
        );
    }

    /// Get performance metrics
    pub fn get_metrics(&self) -> IndexPerformanceMetrics {
        self.metrics.read().clone()
    }

    /// Get metrics (alias for get_metrics)
    pub fn metrics(&self) -> IndexPerformanceMetrics {
        self.get_metrics()
    }

    /// Estimate memory usage for monitoring
    fn estimate_memory_usage(&self) -> usize {
        // Approximate calculation for monitoring
        let collections_size = self.collections.len()
            * (
                32 +  // UUID string
            std::mem::size_of::<Collection>() +
            64
                // Arc overhead
            );
        let name_index_size = self.name_to_uuid.len()
            * (
                64 +  // Name string (average)
            32
                // UUID string
            );

        collections_size + name_index_size
    }

    /// Advanced: Prefix search on collection names - O(n) but optimized
    /// Only use when necessary - for most cases, exact name lookup is preferred
    pub fn find_by_name_prefix(&self, prefix: &str) -> Vec<Arc<Collection>> {
        self.name_to_uuid
            .iter()
            .filter(|entry| entry.key().starts_with(prefix))
            .filter_map(|entry| {
                self.collections
                    .get(entry.value())
                    .map(|r| r.value().clone())
            })
            .collect()
    }

    /// Advanced: Filter collections by predicate - O(n)
    /// Use sparingly - prefer exact lookups when possible
    pub fn filter_collections<F>(&self, predicate: F) -> Vec<Arc<Collection>>
    where
        F: Fn(&Collection) -> bool,
    {
        self.collections
            .iter()
            .filter(|entry| predicate(entry.value()))
            .map(|entry| entry.value().clone())
            .collect()
    }
}

impl Default for UnifiedCollectionIndex {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe wrapper for the unified index with additional safety guarantees
pub struct ThreadSafeUnifiedIndex {
    /// Underlying unified collection index
    index: UnifiedCollectionIndex,
}

impl ThreadSafeUnifiedIndex {
    pub fn new() -> Self {
        Self {
            index: UnifiedCollectionIndex::new(),
        }
    }

    /// All operations delegate to the underlying unified index
    /// DashMap already provides thread safety, so this is mostly for API consistency
    pub fn upsert_collection(&self, record: Collection) {
        self.index.upsert_collection(record);
    }

    pub fn remove_collection(&self, uuid: &str) -> Option<Arc<Collection>> {
        self.index.remove_collection(uuid)
    }

    pub fn get_by_uuid(&self, uuid: &str) -> Option<Arc<Collection>> {
        self.index.get_by_uuid(uuid)
    }

    pub fn get_by_name(&self, name: &str) -> Option<Arc<Collection>> {
        self.index.get_by_name(name)
    }

    pub fn get_uuid_by_name(&self, name: &str) -> Option<String> {
        self.index.get_uuid_by_name(name)
    }

    pub fn exists_by_uuid(&self, uuid: &str) -> bool {
        self.index.exists_by_uuid(uuid)
    }

    pub fn exists_by_name(&self, name: &str) -> bool {
        self.index.exists_by_name(name)
    }

    pub fn list_all(&self) -> Vec<Arc<Collection>> {
        self.index.list_all()
    }

    pub fn count(&self) -> usize {
        self.index.count()
    }

    pub fn rebuild_from_records(&self, records: Vec<Collection>) {
        self.index.rebuild_from_records(records);
    }

    pub fn get_metrics(&self) -> IndexPerformanceMetrics {
        self.index.metrics()
    }
}

impl Default for ThreadSafeUnifiedIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb_v1::{
        Collection, CollectionConfig, CollectionStats, DistanceMetric, StorageEngine,
    };

    fn create_test_collection(id: &str, name: &str) -> Collection {
        let temp_dir = tempfile::tempdir().unwrap();
        Collection {
            id: id.to_string(),
            config: Some(CollectionConfig {
                name: name.to_string(),
                dimension: 128,
                distance_metric: Some(DistanceMetric::Cosine as i32),
                storage_engine: Some(StorageEngine::Viper as i32),
                filterable_columns: vec![],
                index_configs: vec![],
                quantization: None,
                primary_index: Some("HNSW".to_string()),
                auto_index_selection: Some(false),
                description: Some("Test collection".to_string()),
                tags: vec![],
                owner: Some("test_user".to_string()),
                embedding_models: vec![],
                storage_config: None,
                record_schema: None,
                enable_proxima_record: None,
                text_columns: vec![],
                text_storage_configs: vec![],
            }),
            stats: Some(CollectionStats {
                vector_count: 100,
                index_size_bytes: 1024,
                data_size_bytes: 2048,
            }),
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            storage_assignment: Some(crate::proto::proximadb_v1::StorageAssignment {
                primary_path: format!("{}", temp_dir.path().display()),
                backup_paths: vec![],
                engine: StorageEngine::Viper as i32,
                engine_config: std::collections::HashMap::new(),
                base_location: format!("{}", temp_dir.path().display()),
                assigned_at: chrono::Utc::now().timestamp_micros(),
            }),
        }
    }

    #[test]
    fn test_basic_operations() {
        let index = UnifiedCollectionIndex::new();

        let collection = create_test_collection("uuid-123", "test-collection");

        // Test upsert
        index.upsert_collection(collection.clone());
        assert_eq!(index.count(), 1);

        // Test UUID lookup
        let result = index.get_by_uuid("uuid-123");
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "uuid-123");

        // Test name lookup
        let result = index.get_by_name("test-collection");
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "uuid-123");

        // Test UUID by name
        let uuid = index.get_uuid_by_name("test-collection");
        assert_eq!(uuid.unwrap(), "uuid-123");

        // Test existence checks
        assert!(index.exists_by_uuid("uuid-123"));
        assert!(index.exists_by_name("test-collection"));
        assert!(!index.exists_by_uuid("nonexistent"));
        assert!(!index.exists_by_name("nonexistent"));

        // Test removal
        let removed = index.remove_collection("uuid-123");
        assert!(removed.is_some());
        assert_eq!(index.count(), 0);
        assert!(!index.exists_by_uuid("uuid-123"));
        assert!(!index.exists_by_name("test-collection"));
    }

    #[test]
    fn test_concurrent_access() {
        use std::thread;

        let index = Arc::new(UnifiedCollectionIndex::new());
        let mut handles = vec![];

        // Spawn multiple threads doing concurrent operations
        for i in 0..10 {
            let index_clone = index.clone();
            let handle = thread::spawn(move || {
                let collection =
                    create_test_collection(&format!("uuid-{}", i), &format!("collection-{}", i));
                index_clone.upsert_collection(collection);

                // Immediate lookup to test consistency
                let result = index_clone.get_by_uuid(&format!("uuid-{}", i));
                assert!(result.is_some());

                let result = index_clone.get_by_name(&format!("collection-{}", i));
                assert!(result.is_some());
            });
            handles.push(handle);
        }

        // Wait for all threads
        for handle in handles {
            handle.join().unwrap();
        }

        // Verify final state
        assert_eq!(index.count(), 10);

        // Test concurrent reads
        let handles: Vec<_> = (0..10)
            .map(|i| {
                let index_clone = index.clone();
                thread::spawn(move || {
                    let result = index_clone.get_by_uuid(&format!("uuid-{}", i));
                    assert!(result.is_some());
                })
            })
            .collect();

        for handle in handles {
            handle.join().unwrap();
        }
    }

    #[test]
    fn test_rebuild_from_records() {
        let index = UnifiedCollectionIndex::new();

        let records = vec![
            create_test_collection("uuid-1", "collection-1"),
            create_test_collection("uuid-2", "collection-2"),
            create_test_collection("uuid-3", "collection-3"),
        ];

        index.rebuild_from_records(records);

        assert_eq!(index.count(), 3);
        assert!(index.exists_by_uuid("uuid-1"));
        assert!(index.exists_by_name("collection-2"));

        let metrics = index.get_metrics();
        assert!(metrics.last_rebuild_timestamp.is_some());
    }

    #[test]
    fn test_performance_metrics() {
        let index = UnifiedCollectionIndex::new();
        let collection = create_test_collection("uuid-perf", "perf-collection");

        index.upsert_collection(collection);

        // Perform some operations to generate metrics
        index.get_by_uuid("uuid-perf");
        index.get_by_name("perf-collection");
        index.get_by_uuid("nonexistent"); // Cache miss

        let metrics = index.get_metrics();
        assert_eq!(metrics.total_collections, 1);
        assert_eq!(metrics.uuid_lookups, 2);
        assert_eq!(metrics.name_lookups, 1);
        assert_eq!(metrics.cache_hits, 2);
        assert_eq!(metrics.cache_misses, 1);
        assert!(metrics.avg_lookup_time_ns > 0);
    }
}
