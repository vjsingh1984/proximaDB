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

use std::collections::{HashMap, BTreeMap};
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use dashmap::DashMap;

use super::backends::filestore_backend::CollectionRecord;

/// Fast lookup result for metadata queries
#[derive(Debug, Clone)]
pub struct CollectionLookupResult {
    pub uuid: String,
    pub name: String,
    pub dimension: i32,
    pub distance_metric: String,
    pub indexing_algorithm: String,
    pub storage_engine: String,
    pub vector_count: i64,
    pub total_size_bytes: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<&CollectionRecord> for CollectionLookupResult {
    fn from(record: &CollectionRecord) -> Self {
        Self {
            uuid: record.uuid.clone(),
            name: record.name.clone(),
            dimension: record.dimension,
            distance_metric: record.distance_metric.clone(),
            indexing_algorithm: record.indexing_algorithm.clone(),
            storage_engine: record.storage_engine.clone(),
            vector_count: record.vector_count,
            total_size_bytes: record.total_size_bytes,
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

/// Statistics for memory index performance monitoring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStatistics {
    pub total_collections: usize,
    pub memory_usage_bytes: usize,
    pub uuid_index_hits: u64,
    pub name_index_hits: u64,
    pub prefix_index_hits: u64,
    pub tag_index_hits: u64,
    pub cache_misses: u64,
    pub last_rebuild_time: Option<i64>,
    pub avg_lookup_time_ns: u64,
}

/// High-performance memory indexes for collection metadata
/// Optimized for multi-cloud serverless scenarios where compute scales horizontally
/// and state persistence is in cloud object stores
pub struct MetadataMemoryIndexes {
    /// Primary UUID index - O(1) lookup by UUID (HashMap for 1:1 mapping)
    /// Most important for storage/WAL operations that use UUIDs
    uuid_to_record: DashMap<String, Arc<CollectionRecord>>,
    
    /// Name index - O(1) lookup by collection name (HashMap for 1:1 mapping)
    /// Important for user queries using collection names
    name_to_uuid: DashMap<String, String>,
    
    /// Prefix index for name prefix searches - O(log n) for prefix queries
    /// Supports queries like "find collections starting with 'user_'"
    name_prefix_index: Arc<RwLock<BTreeMap<String, Vec<String>>>>,
    
    /// Tag index for metadata filtering - O(1) lookup by tag
    /// Supports tag-based collection discovery
    tag_to_uuids: Arc<RwLock<HashMap<String, Vec<String>>>>,
    
    /// Size-based index for capacity planning - O(log n) range queries
    /// Supports queries like "find collections > 1GB"
    size_index: Arc<RwLock<BTreeMap<i64, Vec<String>>>>,
    
    /// Creation time index for lifecycle queries - O(log n) range queries
    /// Supports queries like "find collections created in last 30 days"
    created_time_index: Arc<RwLock<BTreeMap<i64, Vec<String>>>>,
    
    /// Statistics for monitoring and optimization
    stats: Arc<RwLock<IndexStatistics>>,
}

impl MetadataMemoryIndexes {
    /// Create new memory indexes
    pub fn new() -> Self {
        Self {
            uuid_to_record: DashMap::new(),
            name_to_uuid: DashMap::new(),
            name_prefix_index: Arc::new(RwLock::new(BTreeMap::new())),
            tag_to_uuids: Arc::new(RwLock::new(HashMap::new())),
            size_index: Arc::new(RwLock::new(BTreeMap::new())),
            created_time_index: Arc::new(RwLock::new(BTreeMap::new())),
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
    pub async fn upsert_collection(&self, record: CollectionRecord) {
        let start_time = std::time::Instant::now();
        let uuid = record.uuid.clone();
        let name = record.name.clone();
        let record_arc = Arc::new(record.clone());
        
        // Remove old record if exists (for updates)
        if let Some(old_record) = self.uuid_to_record.get(&uuid) {
            self.remove_from_secondary_indexes(&old_record.value()).await;
        }
        
        // Primary indexes - O(1) operations
        self.uuid_to_record.insert(uuid.clone(), record_arc);
        self.name_to_uuid.insert(name.clone(), uuid.clone());
        
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
            self.name_to_uuid.remove(&record.name);
            
            // Remove from secondary indexes
            self.remove_from_secondary_indexes(&record).await;
            
            // Update statistics
            let mut stats = self.stats.write().await;
            stats.total_collections = self.uuid_to_record.len();
            stats.memory_usage_bytes = self.estimate_memory_usage();
        }
    }
    
    /// Fast UUID lookup - O(1) - Primary use case for storage/WAL operations
    pub async fn get_by_uuid(&self, uuid: &str) -> Option<Arc<CollectionRecord>> {
        let start_time = std::time::Instant::now();
        let result = self.uuid_to_record.get(uuid).map(|entry| entry.value().clone());
        
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
    pub async fn get_by_name(&self, name: &str) -> Option<Arc<CollectionRecord>> {
        let start_time = std::time::Instant::now();
        
        let result = if let Some(uuid) = self.name_to_uuid.get(name) {
            self.uuid_to_record.get(uuid.value()).map(|entry| entry.value().clone())
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
        self.name_to_uuid.get(name).map(|entry| entry.value().clone())
    }
    
    /// Prefix search - O(log n) - For collection discovery
    pub async fn find_by_name_prefix(&self, prefix: &str) -> Vec<CollectionLookupResult> {
        let start_time = std::time::Instant::now();
        let mut results = Vec::new();
        
        let prefix_index = self.name_prefix_index.read().await;
        
        // Use BTreeMap range to efficiently find all names with prefix
        for (name, uuids) in prefix_index.range(prefix.to_string()..) {
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
        
        let tag_index = self.tag_to_uuids.read().await;
        if let Some(uuids) = tag_index.get(tag) {
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
    pub async fn find_by_size_range(&self, min_size: i64, max_size: i64) -> Vec<CollectionLookupResult> {
        let mut results = Vec::new();
        
        let size_index = self.size_index.read().await;
        for (size, uuids) in size_index.range(min_size..=max_size) {
            for uuid in uuids {
                if let Some(record) = self.uuid_to_record.get(uuid) {
                    results.push(CollectionLookupResult::from(record.value().as_ref()));
                }
            }
        }
        
        results
    }
    
    /// Time range query - O(log n) - For lifecycle management
    pub async fn find_by_creation_time_range(&self, start_time: i64, end_time: i64) -> Vec<CollectionLookupResult> {
        let mut results = Vec::new();
        
        let time_index = self.created_time_index.read().await;
        for (time, uuids) in time_index.range(start_time..=end_time) {
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
        self.name_prefix_index.write().await.clear();
        self.tag_to_uuids.write().await.clear();
        self.size_index.write().await.clear();
        self.created_time_index.write().await.clear();
        
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
    pub async fn rebuild_from_records(&self, records: Vec<CollectionRecord>) {
        self.clear().await;
        
        for record in records {
            self.upsert_collection(record).await;
        }
        
        let mut stats = self.stats.write().await;
        stats.last_rebuild_time = Some(chrono::Utc::now().timestamp());
    }
    
    /// Insert into secondary indexes
    async fn insert_into_secondary_indexes(&self, record: &CollectionRecord) {
        // Name prefix index
        {
            let mut prefix_index = self.name_prefix_index.write().await;
            // Index all prefixes of the name (for efficient prefix search)
            for i in 1..=record.name.len() {
                let prefix = record.name[..i].to_string();
                prefix_index.entry(prefix).or_insert_with(Vec::new).push(record.uuid.clone());
            }
        }
        
        // Tag index
        {
            let mut tag_index = self.tag_to_uuids.write().await;
            for tag in &record.tags {
                tag_index.entry(tag.clone()).or_insert_with(Vec::new).push(record.uuid.clone());
            }
        }
        
        // Size index
        {
            let mut size_index = self.size_index.write().await;
            size_index.entry(record.total_size_bytes).or_insert_with(Vec::new).push(record.uuid.clone());
        }
        
        // Time index
        {
            let mut time_index = self.created_time_index.write().await;
            time_index.entry(record.created_at).or_insert_with(Vec::new).push(record.uuid.clone());
        }
    }
    
    /// Remove from secondary indexes
    async fn remove_from_secondary_indexes(&self, record: &CollectionRecord) {
        // Name prefix index
        {
            let mut prefix_index = self.name_prefix_index.write().await;
            for i in 1..=record.name.len() {
                let prefix = record.name[..i].to_string();
                if let Some(uuids) = prefix_index.get_mut(&prefix) {
                    uuids.retain(|uuid| uuid != &record.uuid);
                    if uuids.is_empty() {
                        prefix_index.remove(&prefix);
                    }
                }
            }
        }
        
        // Tag index
        {
            let mut tag_index = self.tag_to_uuids.write().await;
            for tag in &record.tags {
                if let Some(uuids) = tag_index.get_mut(tag) {
                    uuids.retain(|uuid| uuid != &record.uuid);
                    if uuids.is_empty() {
                        tag_index.remove(tag);
                    }
                }
            }
        }
        
        // Size index
        {
            let mut size_index = self.size_index.write().await;
            if let Some(uuids) = size_index.get_mut(&record.total_size_bytes) {
                uuids.retain(|uuid| uuid != &record.uuid);
                if uuids.is_empty() {
                    size_index.remove(&record.total_size_bytes);
                }
            }
        }
        
        // Time index
        {
            let mut time_index = self.created_time_index.write().await;
            if let Some(uuids) = time_index.get_mut(&record.created_at) {
                uuids.retain(|uuid| uuid != &record.uuid);
                if uuids.is_empty() {
                    time_index.remove(&record.created_at);
                }
            }
        }
    }
    
    /// Estimate memory usage for monitoring
    fn estimate_memory_usage(&self) -> usize {
        // Rough estimation - would need more precise calculation in production
        let uuid_index_size = self.uuid_to_record.len() * (32 + std::mem::size_of::<CollectionRecord>());
        let name_index_size = self.name_to_uuid.len() * 64; // Approximate
        
        uuid_index_size + name_index_size + 1024 // Add overhead for secondary indexes
    }
}

impl Default for MetadataMemoryIndexes {
    fn default() -> Self {
        Self::new()
    }
}

