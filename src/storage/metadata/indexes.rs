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

//! Optimized Memory Indexes for Metadata Fast Lookups
//!
//! Designed for multi-cloud and serverless support where state is in cloud object stores
//! but compute needs fast in-memory lookups for metadata queries.

use dashmap::DashMap;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::proto::proximadb_v1::Collection;

/// Fast lookup result for metadata queries
#[derive(Debug, Clone)]
pub struct CollectionLookupResult {
    /// Collection UUID
    pub uuid: String,
    /// Collection name
    pub name: String,
    /// Vector dimension
    pub dimension: i32,
    /// Distance metric type
    pub distance_metric: String,
    /// Indexing algorithm used
    pub indexing_algorithm: String,
    /// Storage engine name
    pub storage_engine: String,
    /// Number of vectors in collection
    pub vector_count: i64,
    /// Total size in bytes
    pub total_size_bytes: i64,
    /// Creation timestamp
    pub timestamp: i64,
    /// Last update timestamp
    pub updated_at: i64,
}

impl From<&Collection> for CollectionLookupResult {
    fn from(record: &Collection) -> Self {
        Self {
            uuid: record.id.clone(),
            name: record
                .config
                .as_ref()
                .map_or_else(|| "unknown".to_string(), |c| c.name.clone()),
            dimension: record.config.as_ref().map_or(0, |c| c.dimension as i32),
            distance_metric: format!("{:?}", record.config.as_ref().map(|c| c.distance_metric)),
            indexing_algorithm: record
                .config
                .as_ref()
                .and_then(|c| c.primary_index.clone())
                .unwrap_or_else(|| "None".to_string()),
            storage_engine: format!("{:?}", record.config.as_ref().map(|c| c.storage_engine)),
            vector_count: record.stats.as_ref().map_or(0, |s| s.vector_count),
            total_size_bytes: record.stats.as_ref().map_or(0, |s| s.data_size_bytes),
            timestamp: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

/// Statistics for memory index performance monitoring
#[derive(Debug, Clone)]
pub struct IndexStatistics {
    /// Total number of collections indexed
    pub total_collections: usize,
    /// Memory usage in bytes
    pub memory_usage_bytes: usize,
    /// UUID index cache hits
    pub uuid_index_hits: u64,
    /// Name index cache hits
    pub name_index_hits: u64,
    /// Prefix search index hits
    pub prefix_index_hits: u64,
    /// Tag filter index hits
    pub tag_index_hits: u64,
    /// Total cache misses
    pub cache_misses: u64,
    /// Last index rebuild timestamp
    pub last_rebuild_time: Option<i64>,
    /// Average lookup time in nanoseconds
    pub avg_lookup_time_ns: u64,
}

/// High-performance memory indexes for collection metadata
/// Optimized for multi-cloud serverless scenarios where compute scales horizontally
/// and state persistence is in cloud object stores
pub struct MetadataMemoryIndexes {
    /// Primary UUID index - O(1) lookup by UUID (HashMap for 1:1 mapping)
    /// Most important for storage/WAL operations that use UUIDs
    uuid_to_record: DashMap<String, Arc<Collection>>,

    /// Name index - O(1) lookup by collection name (HashMap for 1:1 mapping)
    /// Important for user queries using collection names
    name_to_uuid: DashMap<String, String>,

    /// Prefix index for name prefix searches - O(log n) for prefix queries
    /// Supports queries like "find collections starting with 'user_'"
    secondary_indexes: Arc<RwLock<SecondaryIndexes>>,

    /// Statistics for monitoring and optimization
    stats: Arc<RwLock<IndexStatistics>>,
}

#[derive(Debug, Default)]
struct SecondaryIndexes {
    /// Prefix index for name prefix searches - O(log n) for prefix queries
    name_prefix_index: BTreeMap<String, Vec<String>>,
    /// Tag index for metadata filtering - O(1) lookup by tag
    tag_to_uuids: HashMap<String, Vec<String>>,
    /// Size-based index for capacity planning - O(log n) range queries
    size_index: BTreeMap<i64, Vec<String>>,
    /// Creation time index for lifecycle queries - O(log n) range queries
    created_time_index: BTreeMap<i64, Vec<String>>,
}

impl MetadataMemoryIndexes {
    /// Create new memory indexes
    pub fn new() -> Self {
        Self {
            uuid_to_record: DashMap::new(),
            name_to_uuid: DashMap::new(),
            secondary_indexes: Arc::new(RwLock::new(SecondaryIndexes::default())),
            stats: Arc::new(RwLock::new(IndexStatistics {
                total_collections: 0,
                memory_usage_bytes: 0,
                uuid_index_hits: 0,
                name_index_hits: 0,
                prefix_index_hits: 0,
                tag_index_hits: 0,
                cache_misses: 0,
                last_rebuild_time: None,
                avg_lookup_time_ns: 0,
            })),
        }
    }

    /// Insert or update collection in all indexes
    pub async fn upsert_collection(&self, record: Collection) {
        let start_time = std::time::Instant::now();
        let uuid = record.id.clone();
        let name = record.config.as_ref().map(|c| c.name.clone());
        let record_arc = Arc::new(record.clone());

        // Remove old record if exists (for updates)
        if let Some(old_record) = self.uuid_to_record.get(&uuid) {
            self.remove_from_secondary_indexes(old_record.value()).await;
        }

        // Primary indexes - O(1) operations
        self.uuid_to_record.insert(uuid.clone(), record_arc);
        if let Some(name) = name {
            self.name_to_uuid.insert(name, uuid.clone());
        }

        // Secondary indexes
        self.insert_into_secondary_indexes(&record).await;

        // Update statistics
        let mut stats = self.stats.write().await;
        stats.total_collections = self.uuid_to_record.len();
        stats.memory_usage_bytes = self.estimate_memory_usage();

        let elapsed = start_time.elapsed().as_nanos() as u64;
        stats.avg_lookup_time_ns = (stats.avg_lookup_time_ns + elapsed) / 2;
    }

    /// Remove collection from all indexes
    pub async fn remove_collection(&self, uuid: &str) {
        if let Some((_, record)) = self.uuid_to_record.remove(uuid) {
            // Remove from name index
            if let Some(config) = record.config.as_ref() {
                self.name_to_uuid.remove(&config.name);
            }

            // Remove from secondary indexes
            self.remove_from_secondary_indexes(&record).await;

            // Update statistics
            let mut stats = self.stats.write().await;
            stats.total_collections = self.uuid_to_record.len();
            stats.memory_usage_bytes = self.estimate_memory_usage();
        }
    }

    /// Fast UUID lookup - O(1) - Primary use case for storage/WAL operations
    pub async fn get_by_uuid(&self, uuid: &str) -> Option<Arc<Collection>> {
        let start_time = std::time::Instant::now();
        let result = self
            .uuid_to_record
            .get(uuid)
            .map(|entry| entry.value().clone());

        // Update statistics
        let mut stats = self.stats.write().await;
        if result.is_some() {
            stats.uuid_index_hits += 1;
        } else {
            stats.cache_misses += 1;
        }

        let elapsed = start_time.elapsed().as_nanos() as u64;
        stats.avg_lookup_time_ns = (stats.avg_lookup_time_ns + elapsed) / 2;

        result
    }

    /// Fast name lookup - O(1) - Primary use case for user queries
    pub async fn get_by_name(&self, name: &str) -> Option<Arc<Collection>> {
        let start_time = std::time::Instant::now();

        let result = if let Some(uuid) = self.name_to_uuid.get(name) {
            self.uuid_to_record
                .get(uuid.value())
                .map(|entry| entry.value().clone())
        } else {
            None
        };

        // Update statistics
        let mut stats = self.stats.write().await;
        if result.is_some() {
            stats.name_index_hits += 1;
        } else {
            stats.cache_misses += 1;
        }

        let elapsed = start_time.elapsed().as_nanos() as u64;
        stats.avg_lookup_time_ns = (stats.avg_lookup_time_ns + elapsed) / 2;

        result
    }

    /// Get UUID by name - O(1) - Optimized for storage operations
    pub async fn get_uuid_by_name(&self, name: &str) -> Option<String> {
        self.name_to_uuid
            .get(name)
            .map(|entry| entry.value().clone())
    }

    /// Prefix search - O(log n) - For collection discovery
    pub async fn find_by_name_prefix(&self, prefix: &str) -> Vec<CollectionLookupResult> {
        let start_time = std::time::Instant::now();
        let mut results = Vec::new();

        let secondary = self.secondary_indexes.read().await;

        // Use BTreeMap range to efficiently find all names with prefix
        for (name, uuids) in secondary.name_prefix_index.range(prefix.to_string()..) {
            if !name.starts_with(prefix) {
                break; // BTreeMap is sorted, so we can break early
            }

            for uuid in uuids {
                if let Some(record) = self.uuid_to_record.get(uuid) {
                    results.push(CollectionLookupResult::from(record.value().as_ref()));
                }
            }
        }

        // Update statistics
        let mut stats = self.stats.write().await;
        stats.prefix_index_hits += 1;

        let elapsed = start_time.elapsed().as_nanos() as u64;
        stats.avg_lookup_time_ns = (stats.avg_lookup_time_ns + elapsed) / 2;

        results
    }

    /// Tag-based search - O(1) for tag lookup + O(k) for results
    pub async fn find_by_tag(&self, tag: &str) -> Vec<CollectionLookupResult> {
        let start_time = std::time::Instant::now();
        let mut results = Vec::new();

        let secondary = self.secondary_indexes.read().await;
        if let Some(uuids) = secondary.tag_to_uuids.get(tag) {
            for uuid in uuids {
                if let Some(record) = self.uuid_to_record.get(uuid) {
                    results.push(CollectionLookupResult::from(record.value().as_ref()));
                }
            }
        }

        // Update statistics
        let mut stats = self.stats.write().await;
        stats.tag_index_hits += 1;

        let elapsed = start_time.elapsed().as_nanos() as u64;
        stats.avg_lookup_time_ns = (stats.avg_lookup_time_ns + elapsed) / 2;

        results
    }

    /// Size range query - O(log n) - For capacity planning
    pub async fn find_by_size_range(
        &self,
        min_size: i64,
        max_size: i64,
    ) -> Vec<CollectionLookupResult> {
        let mut results = Vec::new();

        let secondary = self.secondary_indexes.read().await;
        for (_size, uuids) in secondary.size_index.range(min_size..=max_size) {
            for uuid in uuids {
                if let Some(record) = self.uuid_to_record.get(uuid) {
                    results.push(CollectionLookupResult::from(record.value().as_ref()));
                }
            }
        }

        results
    }

    /// Time range query - O(log n) - For lifecycle management
    pub async fn find_by_creation_time_range(
        &self,
        start_time: i64,
        end_time: i64,
    ) -> Vec<CollectionLookupResult> {
        let mut results = Vec::new();

        let secondary = self.secondary_indexes.read().await;
        for (_time, uuids) in secondary.created_time_index.range(start_time..=end_time) {
            for uuid in uuids {
                if let Some(record) = self.uuid_to_record.get(uuid) {
                    results.push(CollectionLookupResult::from(record.value().as_ref()));
                }
            }
        }

        results
    }

    /// List all collections - O(n) but efficient iteration
    pub async fn list_all(&self) -> Vec<CollectionLookupResult> {
        self.uuid_to_record
            .iter()
            .map(|entry| CollectionLookupResult::from(entry.value().as_ref()))
            .collect()
    }

    /// Get index statistics for monitoring
    pub async fn get_statistics(&self) -> IndexStatistics {
        self.stats.read().await.clone()
    }

    /// Clear all indexes - for testing or full rebuild
    pub async fn clear(&self) {
        self.uuid_to_record.clear();
        self.name_to_uuid.clear();
        *self.secondary_indexes.write().await = SecondaryIndexes::default();

        let mut stats = self.stats.write().await;
        *stats = IndexStatistics {
            total_collections: 0,
            memory_usage_bytes: 0,
            uuid_index_hits: 0,
            name_index_hits: 0,
            prefix_index_hits: 0,
            tag_index_hits: 0,
            cache_misses: 0,
            last_rebuild_time: Some(chrono::Utc::now().timestamp()),
            avg_lookup_time_ns: 0,
        };
    }

    /// Rebuild indexes from collection records - for recovery scenarios
    pub async fn rebuild_from_records(&self, records: Vec<Collection>) {
        self.clear().await;

        for record in records {
            self.upsert_collection(record).await;
        }

        let mut stats = self.stats.write().await;
        stats.last_rebuild_time = Some(chrono::Utc::now().timestamp());
    }

    /// Insert into secondary indexes
    async fn insert_into_secondary_indexes(&self, record: &Collection) {
        let mut secondary = self.secondary_indexes.write().await;
        Self::insert_into_secondary_indexes_locked(&mut secondary, record);
    }

    fn insert_into_secondary_indexes_locked(secondary: &mut SecondaryIndexes, record: &Collection) {
        // Name prefix index - Store full names only
        if let Some(config) = record.config.as_ref() {
            secondary
                .name_prefix_index
                .entry(config.name.clone())
                .or_default()
                .push(record.id.clone());
        }

        // Tag index
        if let Some(config) = &record.config {
            for tag in &config.tags {
                secondary
                    .tag_to_uuids
                    .entry(tag.clone())
                    .or_default()
                    .push(record.id.clone());
            }
        }

        // Size index
        if let Some(stats) = &record.stats {
            secondary
                .size_index
                .entry(stats.data_size_bytes)
                .or_default()
                .push(record.id.clone());
        }

        // Time index
        secondary
            .created_time_index
            .entry(record.created_at)
            .or_default()
            .push(record.id.clone());
    }

    /// Remove from secondary indexes
    async fn remove_from_secondary_indexes(&self, record: &Collection) {
        let mut secondary = self.secondary_indexes.write().await;
        Self::remove_from_secondary_indexes_locked(&mut secondary, record);
    }

    fn remove_from_secondary_indexes_locked(secondary: &mut SecondaryIndexes, record: &Collection) {
        // Name prefix index - Remove full name only
        if let Some(config) = record.config.as_ref()
            && let Some(uuids) = secondary.name_prefix_index.get_mut(&config.name)
        {
            uuids.retain(|uuid| uuid != &record.id);
            if uuids.is_empty() {
                secondary.name_prefix_index.remove(&config.name);
            }
        }

        // Tag index
        if let Some(config) = &record.config {
            for tag in &config.tags {
                if let Some(uuids) = secondary.tag_to_uuids.get_mut(tag) {
                    uuids.retain(|uuid| uuid != &record.id);
                    if uuids.is_empty() {
                        secondary.tag_to_uuids.remove(tag);
                    }
                }
            }
        }

        // Size index
        if let Some(stats) = &record.stats
            && let Some(uuids) = secondary.size_index.get_mut(&stats.data_size_bytes)
        {
            uuids.retain(|uuid| uuid != &record.id);
            if uuids.is_empty() {
                secondary.size_index.remove(&stats.data_size_bytes);
            }
        }

        // Time index
        if let Some(uuids) = secondary.created_time_index.get_mut(&record.created_at) {
            uuids.retain(|uuid| uuid != &record.id);
            if uuids.is_empty() {
                secondary.created_time_index.remove(&record.created_at);
            }
        }
    }

    /// Estimate memory usage for monitoring
    fn estimate_memory_usage(&self) -> usize {
        // Rough estimation - would need more precise calculation in production
        let uuid_index_size = self.uuid_to_record.len() * (32 + std::mem::size_of::<Collection>());
        let name_index_size = self.name_to_uuid.len() * 64; // Approximate

        uuid_index_size + name_index_size + 1024 // Add overhead for secondary indexes
    }
}

impl Default for MetadataMemoryIndexes {
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
                enable_dual_use_embeddings: None,
            }),
            stats: Some(CollectionStats {
                vector_count: 100,
                index_size_bytes: 1024,
                data_size_bytes: 2048,
            }),
            created_at: 1000,
            updated_at: 1000,
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

    #[tokio::test]
    async fn test_uuid_lookup_performance() {
        let indexes = MetadataMemoryIndexes::new();
        let collection = create_test_collection("test-uuid-123", "test-collection");
        indexes.upsert_collection(collection.clone()).await;
        let result = indexes.get_by_uuid("test-uuid-123").await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "test-uuid-123");
        let stats = indexes.get_statistics().await;
        assert_eq!(stats.total_collections, 1);
        assert_eq!(stats.uuid_index_hits, 1);
    }

    #[tokio::test]
    async fn test_name_lookup_performance() {
        let indexes = MetadataMemoryIndexes::new();
        let collection = create_test_collection("test-uuid-456", "another-collection");
        indexes.upsert_collection(collection.clone()).await;
        let result = indexes.get_by_name("another-collection").await;
        assert!(result.is_some());
        assert_eq!(result.unwrap().id, "test-uuid-456");
        let uuid = indexes.get_uuid_by_name("another-collection").await;
        assert_eq!(uuid.unwrap(), "test-uuid-456");
    }

    #[tokio::test]
    async fn test_prefix_search() {
        let indexes = MetadataMemoryIndexes::new();
        let collections = vec![
            create_test_collection("uuid-1", "user_data_v1"),
            create_test_collection("uuid-2", "user_data_v2"),
            create_test_collection("uuid-3", "user_logs_v1"),
            create_test_collection("uuid-4", "system_config"),
        ];
        for collection in collections {
            indexes.upsert_collection(collection).await;
        }
        let user_results = indexes.find_by_name_prefix("user_").await;
        assert_eq!(user_results.len(), 3);
        let data_results = indexes.find_by_name_prefix("user_data").await;
        assert_eq!(data_results.len(), 2);
        let system_results = indexes.find_by_name_prefix("system").await;
        assert_eq!(system_results.len(), 1);
        let nonexistent_results = indexes.find_by_name_prefix("nonexistent").await;
        assert_eq!(nonexistent_results.len(), 0);
    }

    #[tokio::test]
    async fn test_concurrent_operations() {
        use std::sync::Arc;
        use tokio::task::JoinSet;

        let indexes = Arc::new(MetadataMemoryIndexes::new());
        let mut tasks = JoinSet::new();

        for i in 0..10 {
            let indexes_clone = indexes.clone();
            tasks.spawn(async move {
                let collection =
                    create_test_collection(&format!("uuid-{}", i), &format!("collection-{}", i));
                indexes_clone.upsert_collection(collection).await;
            });
        }

        while let Some(result) = tasks.join_next().await {
            result.unwrap();
        }

        let stats = indexes.get_statistics().await;
        assert_eq!(stats.total_collections, 10);

        let mut read_tasks = JoinSet::new();
        for i in 0..10 {
            let indexes_clone = indexes.clone();
            read_tasks.spawn(async move {
                let result = indexes_clone.get_by_uuid(&format!("uuid-{}", i)).await;
                assert!(result.is_some());
                result.unwrap().id.clone()
            });
        }

        while let Some(result) = read_tasks.join_next().await {
            let uuid = result.unwrap();
            assert!(uuid.starts_with("uuid-"));
        }
    }

    #[tokio::test]
    async fn test_rebuild_performance() {
        let indexes = MetadataMemoryIndexes::new();
        for i in 0..100 {
            let collection =
                create_test_collection(&format!("uuid-{:03}", i), &format!("collection-{:03}", i));
            indexes.upsert_collection(collection).await;
        }
        let initial_stats = indexes.get_statistics().await;
        assert_eq!(initial_stats.total_collections, 100);

        let collections: Vec<_> = (0..100)
            .map(|i| {
                create_test_collection(
                    &format!("new-uuid-{:03}", i),
                    &format!("new-collection-{:03}", i),
                )
            })
            .collect();

        indexes.rebuild_from_records(collections).await;
        let rebuild_stats = indexes.get_statistics().await;
        assert_eq!(rebuild_stats.total_collections, 100);
        let old_result = indexes.get_by_uuid("uuid-001").await;
        assert!(old_result.is_none());
        let new_result = indexes.get_by_uuid("new-uuid-001").await;
        assert!(new_result.is_some());
    }
}
