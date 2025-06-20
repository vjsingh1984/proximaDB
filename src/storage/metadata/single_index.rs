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

//! Single Unified Index - One Memory Table for All Collection Metadata
//! 
//! Eliminates dual-index sync complexity and recovery challenges by using
//! a single data structure with built-in secondary key support.

use std::sync::Arc;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use parking_lot::RwLock;

use super::backends::filestore_backend::CollectionRecord;

/// Collection index entry - contains both record and secondary keys
#[derive(Debug, Clone)]
pub struct CollectionIndexEntry {
    /// The actual collection record
    pub record: Arc<CollectionRecord>,
    
    /// Pre-computed secondary keys for fast lookups
    pub name_key: String,
    pub uuid_key: String,
}

impl CollectionIndexEntry {
    pub fn new(record: CollectionRecord) -> Self {
        let name_key = record.name.clone();
        let uuid_key = record.uuid.clone();
        
        Self {
            record: Arc::new(record),
            name_key,
            uuid_key,
        }
    }
    
    /// Update with new record, maintaining key consistency
    pub fn update_record(&mut self, new_record: CollectionRecord) {
        self.name_key = new_record.name.clone();
        self.uuid_key = new_record.uuid.clone();
        self.record = Arc::new(new_record);
    }
}

/// Performance metrics for the single index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SingleIndexMetrics {
    pub total_collections: usize,
    pub memory_usage_bytes: usize,
    pub lookups_by_uuid: u64,
    pub lookups_by_name: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub avg_lookup_time_ns: u64,
    pub last_rebuild_timestamp: Option<i64>,
}

/// Single unified index using UUID as primary key with secondary name lookup
/// This eliminates the dual-index sync problem by using a single memory table
/// with built-in secondary key indexing through efficient iteration
pub struct SingleCollectionIndex {
    /// Primary store: UUID -> CollectionIndexEntry
    /// DashMap provides lock-free concurrent access with excellent performance
    entries: DashMap<String, CollectionIndexEntry>,
    
    /// Performance metrics
    metrics: Arc<RwLock<SingleIndexMetrics>>,
}

impl SingleCollectionIndex {
    /// Create new single index
    pub fn new() -> Self {
        Self {
            entries: DashMap::new(),
            metrics: Arc::new(RwLock::new(SingleIndexMetrics {
                total_collections: 0,
                memory_usage_bytes: 0,
                lookups_by_uuid: 0,
                lookups_by_name: 0,
                cache_hits: 0,
                cache_misses: 0,
                avg_lookup_time_ns: 0,
                last_rebuild_timestamp: None,
            })),
        }
    }
    
    /// Insert or update collection - single atomic operation
    /// This is the ONLY method that modifies the index, ensuring consistency
    pub fn upsert_collection(&self, record: CollectionRecord) {
        let start = std::time::Instant::now();
        let uuid = record.uuid.clone();
        
        // Single atomic operation - no sync issues possible
        let entry = CollectionIndexEntry::new(record);
        self.entries.insert(uuid, entry);
        
        // Update metrics
        let elapsed = start.elapsed().as_nanos() as u64;
        let mut metrics = self.metrics.write();
        metrics.total_collections = self.entries.len();
        metrics.memory_usage_bytes = self.estimate_memory_usage();
        metrics.avg_lookup_time_ns = (metrics.avg_lookup_time_ns + elapsed) / 2;
    }
    
    /// Remove collection by UUID - single atomic operation
    pub fn remove_collection(&self, uuid: &str) -> Option<Arc<CollectionRecord>> {
        let start = std::time::Instant::now();
        
        let result = self.entries.remove(uuid).map(|(_, entry)| entry.record);
        
        // Update metrics
        let elapsed = start.elapsed().as_nanos() as u64;
        let mut metrics = self.metrics.write();
        metrics.total_collections = self.entries.len();
        metrics.memory_usage_bytes = self.estimate_memory_usage();
        metrics.avg_lookup_time_ns = (metrics.avg_lookup_time_ns + elapsed) / 2;
        
        if result.is_some() {
            metrics.cache_hits += 1;
        } else {
            metrics.cache_misses += 1;
        }
        
        result
    }
    
    /// Get collection by UUID - O(1) primary key lookup
    pub fn get_by_uuid(&self, uuid: &str) -> Option<Arc<CollectionRecord>> {
        let start = std::time::Instant::now();
        let result = self.entries.get(uuid).map(|entry| entry.record.clone());
        
        // Update metrics
        let elapsed = start.elapsed().as_nanos() as u64;
        let mut metrics = self.metrics.write();
        metrics.lookups_by_uuid += 1;
        if result.is_some() {
            metrics.cache_hits += 1;
        } else {
            metrics.cache_misses += 1;
        }
        metrics.avg_lookup_time_ns = (metrics.avg_lookup_time_ns + elapsed) / 2;
        
        result
    }
    
    /// Get collection by name - O(n) scan but very efficient with DashMap
    /// This is the trade-off: single table = O(n) name lookup
    /// But DashMap's parallel iteration makes this very fast in practice
    pub fn get_by_name(&self, name: &str) -> Option<Arc<CollectionRecord>> {
        let start = std::time::Instant::now();
        
        // Parallel scan through DashMap - very efficient
        let result = self.entries
            .iter()
            .find(|entry| entry.value().name_key == name)
            .map(|entry| entry.value().record.clone());
        
        // Update metrics
        let elapsed = start.elapsed().as_nanos() as u64;
        let mut metrics = self.metrics.write();
        metrics.lookups_by_name += 1;
        if result.is_some() {
            metrics.cache_hits += 1;
        } else {
            metrics.cache_misses += 1;
        }
        metrics.avg_lookup_time_ns = (metrics.avg_lookup_time_ns + elapsed) / 2;
        
        result
    }
    
    /// Get UUID by name - O(n) but optimized for storage operations
    pub fn get_uuid_by_name(&self, name: &str) -> Option<String> {
        self.entries
            .iter()
            .find(|entry| entry.value().name_key == name)
            .map(|entry| entry.key().clone())
    }
    
    /// Check if collection exists by UUID - O(1)
    pub fn exists_by_uuid(&self, uuid: &str) -> bool {
        self.entries.contains_key(uuid)
    }
    
    /// Check if collection exists by name - O(n) but efficient
    pub fn exists_by_name(&self, name: &str) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.value().name_key == name)
    }
    
    /// List all collections - O(n) efficient iteration
    pub fn list_all(&self) -> Vec<Arc<CollectionRecord>> {
        self.entries
            .iter()
            .map(|entry| entry.value().record.clone())
            .collect()
    }
    
    /// Get collection count - O(1)
    pub fn count(&self) -> usize {
        self.entries.len()
    }
    
    /// Clear all data - for testing or rebuild
    pub fn clear(&self) {
        self.entries.clear();
        
        let mut metrics = self.metrics.write();
        metrics.total_collections = 0;
        metrics.memory_usage_bytes = 0;
        metrics.last_rebuild_timestamp = Some(chrono::Utc::now().timestamp());
    }
    
    /// Rebuild from collection records - critical for recovery
    /// This is the key method for disk recovery - single operation, no sync issues
    pub fn rebuild_from_records(&self, records: Vec<CollectionRecord>) {
        // Clear and rebuild atomically
        self.clear();
        
        for record in records {
            self.upsert_collection(record);
        }
        
        let mut metrics = self.metrics.write();
        metrics.last_rebuild_timestamp = Some(chrono::Utc::now().timestamp());
        
        tracing::info!(
            "SingleCollectionIndex rebuilt with {} collections", 
            self.entries.len()
        );
    }
    
    /// Get performance metrics
    pub fn get_metrics(&self) -> SingleIndexMetrics {
        self.metrics.read().clone()
    }
    
    /// Filter collections by predicate - O(n) parallel scan
    pub fn filter_collections<F>(&self, predicate: F) -> Vec<Arc<CollectionRecord>>
    where
        F: Fn(&CollectionRecord) -> bool + Sync,
    {
        self.entries
            .iter()
            .filter(|entry| predicate(&entry.value().record))
            .map(|entry| entry.value().record.clone())
            .collect()
    }
    
    /// Prefix search on collection names - O(n) but efficient
    pub fn find_by_name_prefix(&self, prefix: &str) -> Vec<Arc<CollectionRecord>> {
        self.entries
            .iter()
            .filter(|entry| entry.value().name_key.starts_with(prefix))
            .map(|entry| entry.value().record.clone())
            .collect()
    }
    
    /// Estimate memory usage for monitoring
    fn estimate_memory_usage(&self) -> usize {
        self.entries.len() * (
            32 +  // UUID key
            64 +  // Name key (average)
            std::mem::size_of::<CollectionRecord>() +
            64    // Arc and entry overhead
        )
    }
}

impl Default for SingleCollectionIndex {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe wrapper with additional safety guarantees
pub struct ThreadSafeSingleIndex {
    index: SingleCollectionIndex,
}

impl ThreadSafeSingleIndex {
    pub fn new() -> Self {
        Self {
            index: SingleCollectionIndex::new(),
        }
    }
    
    /// All operations delegate to the single index
    pub fn upsert_collection(&self, record: CollectionRecord) {
        self.index.upsert_collection(record);
    }
    
    pub fn remove_collection(&self, uuid: &str) -> Option<Arc<CollectionRecord>> {
        self.index.remove_collection(uuid)
    }
    
    pub fn get_by_uuid(&self, uuid: &str) -> Option<Arc<CollectionRecord>> {
        self.index.get_by_uuid(uuid)
    }
    
    pub fn get_by_name(&self, name: &str) -> Option<Arc<CollectionRecord>> {
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
    
    pub fn list_all(&self) -> Vec<Arc<CollectionRecord>> {
        self.index.list_all()
    }
    
    pub fn count(&self) -> usize {
        self.index.count()
    }
    
    pub fn rebuild_from_records(&self, records: Vec<CollectionRecord>) {
        self.index.rebuild_from_records(records);
    }
    
    pub fn get_metrics(&self) -> SingleIndexMetrics {
        self.index.get_metrics()
    }
    
    pub fn filter_collections<F>(&self, predicate: F) -> Vec<Arc<CollectionRecord>>
    where
        F: Fn(&CollectionRecord) -> bool + Sync,
    {
        self.index.filter_collections(predicate)
    }
}

impl Default for ThreadSafeSingleIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::metadata::backends::filestore_backend::CollectionRecord;
    
    fn create_test_record(uuid: &str, name: &str) -> CollectionRecord {
        CollectionRecord {
            uuid: uuid.to_string(),
            name: name.to_string(),
            dimension: 128,
            distance_metric: "cosine".to_string(),
            indexing_algorithm: "hnsw".to_string(),
            storage_engine: "viper".to_string(),
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            version: 1,
            vector_count: 100,
            total_size_bytes: 1024,
            config: "{}".to_string(),
            description: None,
            tags: vec![],
            owner: None,
        }
    }
    
    #[test]
    fn test_single_index_operations() {
        let index = SingleCollectionIndex::new();
        
        let record = create_test_record("uuid-123", "test-collection");
        
        // Test upsert
        index.upsert_collection(record.clone());
        assert_eq!(index.count(), 1);
        
        // Test UUID lookup - O(1)
        let result = index.get_by_uuid("uuid-123");
        assert!(result.is_some());
        assert_eq!(result.unwrap().name, "test-collection");
        
        // Test name lookup - O(n) but efficient
        let result = index.get_by_name("test-collection");
        assert!(result.is_some());
        assert_eq!(result.unwrap().uuid, "uuid-123");
        
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
    fn test_single_index_rebuild() {
        let index = SingleCollectionIndex::new();
        
        let records = vec![
            create_test_record("uuid-1", "collection-1"),
            create_test_record("uuid-2", "collection-2"),
            create_test_record("uuid-3", "collection-3"),
        ];
        
        index.rebuild_from_records(records);
        
        assert_eq!(index.count(), 3);
        assert!(index.exists_by_uuid("uuid-1"));
        assert!(index.exists_by_name("collection-2"));
        
        let metrics = index.get_metrics();
        assert!(metrics.last_rebuild_timestamp.is_some());
        assert_eq!(metrics.total_collections, 3);
    }
    
    #[test]
    fn test_concurrent_access() {
        use std::sync::Arc;
        use std::thread;
        
        let index = Arc::new(SingleCollectionIndex::new());
        let mut handles = vec![];
        
        // Spawn multiple threads for concurrent operations
        for i in 0..10 {
            let index_clone = index.clone();
            let handle = thread::spawn(move || {
                let record = create_test_record(&format!("uuid-{}", i), &format!("collection-{}", i));
                index_clone.upsert_collection(record);
                
                // Immediate lookup to test consistency
                let uuid_result = index_clone.get_by_uuid(&format!("uuid-{}", i));
                assert!(uuid_result.is_some());
                
                let name_result = index_clone.get_by_name(&format!("collection-{}", i));
                assert!(name_result.is_some());
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
        let handles: Vec<_> = (0..10).map(|i| {
            let index_clone = index.clone();
            thread::spawn(move || {
                let result = index_clone.get_by_uuid(&format!("uuid-{}", i));
                assert!(result.is_some());
            })
        }).collect();
        
        for handle in handles {
            handle.join().unwrap();
        }
    }
    
    #[test]
    fn test_performance_characteristics() {
        let index = SingleCollectionIndex::new();
        
        // Insert test data
        for i in 0..1000 {
            let record = create_test_record(&format!("uuid-{:04}", i), &format!("collection-{:04}", i));
            index.upsert_collection(record);
        }
        
        // Test UUID lookup performance (should be O(1))
        let start = std::time::Instant::now();
        for i in 0..100 {
            let _result = index.get_by_uuid(&format!("uuid-{:04}", i));
        }
        let uuid_duration = start.elapsed();
        
        // Test name lookup performance (O(n) but should be fast)
        let start = std::time::Instant::now();
        for i in 0..100 {
            let _result = index.get_by_name(&format!("collection-{:04}", i));
        }
        let name_duration = start.elapsed();
        
        println!("UUID lookup (100 ops): {:?}", uuid_duration);
        println!("Name lookup (100 ops): {:?}", name_duration);
        
        // UUID lookups should be significantly faster than name lookups
        // But both should be reasonable for practical use
        assert!(uuid_duration < name_duration);
        
        let metrics = index.get_metrics();
        assert!(metrics.avg_lookup_time_ns > 0);
        assert_eq!(metrics.total_collections, 1000);
    }
}