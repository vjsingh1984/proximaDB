// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Unified Memtable for Recent Data
//! 
//! In-memory structure that serves recent writes before they are flushed
//! to persistent storage. Works with both VIPER and regular layouts.

use std::collections::{HashMap, BTreeMap};
use std::sync::Arc;
use tokio::sync::RwLock;
use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};

use crate::core::{VectorRecord, CollectionId, VectorId};
use crate::storage::Result;

/// In-memory table for recent vector operations
pub struct Memtable {
    /// Vector data organized by collection
    data: Arc<RwLock<HashMap<CollectionId, CollectionMemtable>>>,
    
    /// Maximum size before flush is triggered
    max_size_bytes: usize,
    
    /// Current size estimate
    current_size: Arc<RwLock<usize>>,
    
    /// Sequence number for ordering
    sequence: Arc<RwLock<u64>>,
}

/// Per-collection memtable data
#[derive(Debug)]
struct CollectionMemtable {
    /// Vector records ordered by sequence number
    vectors: BTreeMap<u64, MemtableEntry>,
    
    /// Vector ID to sequence mapping for quick lookups
    id_to_sequence: HashMap<VectorId, u64>,
    
    /// Deleted vector IDs (tombstones)
    deleted: HashMap<VectorId, u64>, // VectorId -> deletion_sequence
    
    /// Collection statistics
    stats: CollectionStats,
}

/// Entry in the memtable
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemtableEntry {
    /// Sequence number for ordering
    pub sequence: u64,
    
    /// Operation type
    pub operation: MemtableOperation,
    
    /// Timestamp of operation
    pub timestamp: DateTime<Utc>,
}

/// Types of operations in memtable
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MemtableOperation {
    /// Insert or update a vector
    Put { record: VectorRecord },
    
    /// Delete a vector
    Delete { vector_id: VectorId },
}

/// Statistics for a collection in memtable
#[derive(Debug, Default)]
struct CollectionStats {
    /// Number of active vectors (puts - deletes)
    active_count: usize,
    
    /// Number of operations
    operation_count: usize,
    
    /// Estimated size in bytes
    size_bytes: usize,
    
    /// Last update timestamp
    last_updated: DateTime<Utc>,
}

impl Memtable {
    /// Create a new memtable
    pub fn new(max_size_mb: usize) -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
            max_size_bytes: max_size_mb * 1024 * 1024,
            current_size: Arc::new(RwLock::new(0)),
            sequence: Arc::new(RwLock::new(0)),
        }
    }
    
    /// Insert or update a vector (ID-based deduplication for vector database)
    pub async fn put(&self, record: VectorRecord) -> Result<u64> {
        let sequence = {
            let mut seq = self.sequence.write().await;
            *seq += 1;
            *seq
        };
        
        let entry = MemtableEntry {
            sequence,
            operation: MemtableOperation::Put { record: record.clone() },
            timestamp: Utc::now(),
        };
        
        let entry_size = self.estimate_entry_size(&entry);
        
        let mut data = self.data.write().await;
        let collection_table = data.entry(record.collection_id.clone())
            .or_insert_with(|| CollectionMemtable {
                vectors: BTreeMap::new(),
                id_to_sequence: HashMap::new(),
                deleted: HashMap::new(),
                stats: CollectionStats::default(),
            });
        
        // Vector database behavior: IDs are unique keys, vectors are values
        // If same ID is inserted again, it's an update (not a duplicate vector)
        let _is_update = collection_table.id_to_sequence.contains_key(&record.id);
        
        // Remove old entry if updating same ID
        if let Some(&old_sequence) = collection_table.id_to_sequence.get(&record.id) {
            if let Some(old_entry) = collection_table.vectors.remove(&old_sequence) {
                let old_size = self.estimate_entry_size(&old_entry);
                collection_table.stats.size_bytes = collection_table.stats.size_bytes.saturating_sub(old_size);
            }
        } else {
            // New vector ID
            collection_table.stats.active_count += 1;
        }
        
        // Remove from deleted if it was deleted before (resurrection)
        collection_table.deleted.remove(&record.id);
        
        // Insert new entry with latest sequence
        collection_table.vectors.insert(sequence, entry);
        collection_table.id_to_sequence.insert(record.id, sequence);
        collection_table.stats.operation_count += 1;
        collection_table.stats.size_bytes += entry_size;
        collection_table.stats.last_updated = Utc::now();
        
        // Update global size
        let mut current_size = self.current_size.write().await;
        *current_size += entry_size;
        
        Ok(sequence)
    }
    
    /// Delete a vector
    pub async fn delete(&self, collection_id: CollectionId, vector_id: VectorId) -> Result<u64> {
        let sequence = {
            let mut seq = self.sequence.write().await;
            *seq += 1;
            *seq
        };
        
        let entry = MemtableEntry {
            sequence,
            operation: MemtableOperation::Delete { vector_id: vector_id.clone() },
            timestamp: Utc::now(),
        };
        
        let entry_size = self.estimate_entry_size(&entry);
        
        let mut data = self.data.write().await;
        let collection_table = data.entry(collection_id)
            .or_insert_with(|| CollectionMemtable {
                vectors: BTreeMap::new(),
                id_to_sequence: HashMap::new(),
                deleted: HashMap::new(),
                stats: CollectionStats::default(),
            });
        
        // Mark as deleted
        collection_table.deleted.insert(vector_id.clone(), sequence);
        
        // Remove from active if it was there
        if let Some(old_sequence) = collection_table.id_to_sequence.remove(&vector_id) {
            if let Some(old_entry) = collection_table.vectors.remove(&old_sequence) {
                let old_size = self.estimate_entry_size(&old_entry);
                collection_table.stats.size_bytes = collection_table.stats.size_bytes.saturating_sub(old_size);
                collection_table.stats.active_count = collection_table.stats.active_count.saturating_sub(1);
            }
        }
        
        // Insert deletion entry
        collection_table.vectors.insert(sequence, entry);
        collection_table.stats.operation_count += 1;
        collection_table.stats.size_bytes += entry_size;
        collection_table.stats.last_updated = Utc::now();
        
        // Update global size
        let mut current_size = self.current_size.write().await;
        *current_size += entry_size;
        
        Ok(sequence)
    }
    
    /// Get a vector by ID (returns None if deleted or not found)
    /// Optimized for ID-based lookups in vector database
    pub async fn get(&self, collection_id: &CollectionId, vector_id: &VectorId) -> Result<Option<VectorRecord>> {
        let data = self.data.read().await;
        
        if let Some(collection_table) = data.get(collection_id) {
            // Check if deleted (tombstone check)
            if collection_table.deleted.contains_key(vector_id) {
                return Ok(None);
            }
            
            // Direct ID-based lookup via hash map (O(1) performance)
            if let Some(&sequence) = collection_table.id_to_sequence.get(vector_id) {
                if let Some(entry) = collection_table.vectors.get(&sequence) {
                    if let MemtableOperation::Put { record } = &entry.operation {
                        return Ok(Some(record.clone()));
                    }
                }
            }
        }
        
        Ok(None)
    }
    
    /// Check if a vector ID exists in memtable (efficient existence check)
    pub async fn contains(&self, collection_id: &CollectionId, vector_id: &VectorId) -> bool {
        let data = self.data.read().await;
        
        if let Some(collection_table) = data.get(collection_id) {
            // Not deleted and exists in ID mapping
            !collection_table.deleted.contains_key(vector_id) && 
            collection_table.id_to_sequence.contains_key(vector_id)
        } else {
            false
        }
    }
    
    /// Get multiple vectors by IDs (batch lookup for efficiency)
    pub async fn get_batch(&self, collection_id: &CollectionId, vector_ids: &[VectorId]) -> Result<Vec<Option<VectorRecord>>> {
        let data = self.data.read().await;
        let mut results = Vec::with_capacity(vector_ids.len());
        
        if let Some(collection_table) = data.get(collection_id) {
            for vector_id in vector_ids {
                // Check if deleted
                if collection_table.deleted.contains_key(vector_id) {
                    results.push(None);
                    continue;
                }
                
                // ID-based lookup
                if let Some(&sequence) = collection_table.id_to_sequence.get(vector_id) {
                    if let Some(entry) = collection_table.vectors.get(&sequence) {
                        if let MemtableOperation::Put { record } = &entry.operation {
                            results.push(Some(record.clone()));
                            continue;
                        }
                    }
                }
                
                results.push(None);
            }
        } else {
            // Collection doesn't exist - return all None
            results.resize(vector_ids.len(), None);
        }
        
        Ok(results)
    }
    
    /// Filter vectors by metadata criteria (efficient metadata-based filtering)
    pub async fn filter_by_metadata(
        &self,
        collection_id: &CollectionId,
        metadata_filters: &std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<Vec<VectorRecord>> {
        let data = self.data.read().await;
        let mut filtered_vectors = Vec::new();
        
        if let Some(collection_table) = data.get(collection_id) {
            for entry in collection_table.vectors.values() {
                if let MemtableOperation::Put { record } = &entry.operation {
                    // Skip deleted vectors
                    if collection_table.deleted.contains_key(&record.id) {
                        continue;
                    }
                    
                    // Apply metadata filters
                    if self.matches_metadata_filters(&record.metadata, metadata_filters) {
                        filtered_vectors.push(record.clone());
                    }
                }
            }
        }
        
        Ok(filtered_vectors)
    }
    
    /// Combined ID and metadata filtering for efficient queries
    pub async fn get_with_metadata_filter(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
        metadata_filters: &std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<Option<VectorRecord>> {
        let data = self.data.read().await;
        
        if let Some(collection_table) = data.get(collection_id) {
            // Check if deleted
            if collection_table.deleted.contains_key(vector_id) {
                return Ok(None);
            }
            
            // ID-based lookup first (most efficient)
            if let Some(&sequence) = collection_table.id_to_sequence.get(vector_id) {
                if let Some(entry) = collection_table.vectors.get(&sequence) {
                    if let MemtableOperation::Put { record } = &entry.operation {
                        // Apply metadata filters
                        if self.matches_metadata_filters(&record.metadata, metadata_filters) {
                            return Ok(Some(record.clone()));
                        }
                    }
                }
            }
        }
        
        Ok(None)
    }
    
    /// Search vectors with metadata filters (like NoSQL query)
    pub async fn search_with_filters(
        &self,
        collection_id: &CollectionId,
        metadata_filters: Option<&std::collections::HashMap<String, serde_json::Value>>,
        limit: Option<usize>,
    ) -> Result<Vec<VectorRecord>> {
        let data = self.data.read().await;
        let mut results = Vec::new();
        
        if let Some(collection_table) = data.get(collection_id) {
            for entry in collection_table.vectors.values() {
                if let MemtableOperation::Put { record } = &entry.operation {
                    // Skip deleted vectors
                    if collection_table.deleted.contains_key(&record.id) {
                        continue;
                    }
                    
                    // Apply metadata filters if provided
                    let matches = if let Some(filters) = metadata_filters {
                        self.matches_metadata_filters(&record.metadata, filters)
                    } else {
                        true // No filters means all records match
                    };
                    
                    if matches {
                        results.push(record.clone());
                        
                        // Apply limit if specified
                        if let Some(limit_count) = limit {
                            if results.len() >= limit_count {
                                break;
                            }
                        }
                    }
                }
            }
        }
        
        Ok(results)
    }
    
    /// Get all vectors for a collection (excluding deleted)
    pub async fn get_collection_vectors(&self, collection_id: &CollectionId) -> Result<Vec<VectorRecord>> {
        let data = self.data.read().await;
        let mut vectors = Vec::new();
        
        if let Some(collection_table) = data.get(collection_id) {
            for entry in collection_table.vectors.values() {
                if let MemtableOperation::Put { record } = &entry.operation {
                    // Skip if deleted
                    if !collection_table.deleted.contains_key(&record.id) {
                        vectors.push(record.clone());
                    }
                }
            }
        }
        
        Ok(vectors)
    }
    
    /// Get all operations since a sequence number (for WAL recovery)
    pub async fn get_operations_since(&self, since_sequence: u64) -> Result<Vec<MemtableEntry>> {
        let data = self.data.read().await;
        let mut operations = Vec::new();
        
        for collection_table in data.values() {
            for (_seq, entry) in collection_table.vectors.range(since_sequence + 1..) {
                operations.push(entry.clone());
            }
        }
        
        // Sort by sequence number
        operations.sort_by_key(|entry| entry.sequence);
        
        Ok(operations)
    }
    
    /// Check if memtable should be flushed
    pub async fn should_flush(&self) -> bool {
        let current_size = self.current_size.read().await;
        *current_size >= self.max_size_bytes
    }
    
    /// Get current size in bytes
    pub async fn size_bytes(&self) -> usize {
        *self.current_size.read().await
    }
    
    /// Get number of operations
    pub async fn operation_count(&self) -> usize {
        let data = self.data.read().await;
        data.values()
            .map(|table| table.stats.operation_count)
            .sum()
    }
    
    /// Get statistics for a collection
    pub async fn get_collection_stats(&self, collection_id: &CollectionId) -> Option<MemtableCollectionStats> {
        let data = self.data.read().await;
        data.get(collection_id).map(|table| MemtableCollectionStats {
            active_vectors: table.stats.active_count,
            total_operations: table.stats.operation_count,
            size_bytes: table.stats.size_bytes,
            last_updated: table.stats.last_updated,
        })
    }
    
    /// Clear all data (used after successful flush)
    pub async fn clear(&self) {
        let mut data = self.data.write().await;
        data.clear();
        
        let mut current_size = self.current_size.write().await;
        *current_size = 0;
    }
    
    /// Clear data for a specific collection
    pub async fn clear_collection(&self, collection_id: &CollectionId) {
        let mut data = self.data.write().await;
        
        if let Some(collection_table) = data.remove(collection_id) {
            let mut current_size = self.current_size.write().await;
            *current_size = current_size.saturating_sub(collection_table.stats.size_bytes);
        }
    }
    
    /// Get all collections with data in memtable
    pub async fn get_collections(&self) -> Vec<CollectionId> {
        let data = self.data.read().await;
        data.keys().cloned().collect()
    }
    
    /// Estimate entry size in bytes
    fn estimate_entry_size(&self, entry: &MemtableEntry) -> usize {
        match &entry.operation {
            MemtableOperation::Put { record } => {
                // Rough estimate: vector dimensions * 4 bytes + metadata + overhead
                record.vector.len() * 4 + 
                serde_json::to_string(&record.metadata).unwrap_or_default().len() +
                record.collection_id.len() + 
                100 // overhead
            }
            MemtableOperation::Delete { .. } => {
                50 // Small overhead for deletion markers
            }
        }
    }
    
    /// Check if a record matches the given metadata filters
    fn matches_metadata_filters(
        &self,
        record_metadata: &std::collections::HashMap<String, serde_json::Value>,
        filters: &std::collections::HashMap<String, serde_json::Value>,
    ) -> bool {
        for (filter_key, filter_value) in filters {
            match record_metadata.get(filter_key) {
                Some(record_value) => {
                    // Support different filter operations
                    if !self.matches_filter_value(record_value, filter_value) {
                        return false;
                    }
                }
                None => {
                    // Record doesn't have this metadata field
                    return false;
                }
            }
        }
        true
    }
    
    /// Check if a record value matches a filter value with support for different operations
    fn matches_filter_value(&self, record_value: &serde_json::Value, filter_value: &serde_json::Value) -> bool {
        match filter_value {
            // Direct equality comparison
            serde_json::Value::String(_) | 
            serde_json::Value::Number(_) | 
            serde_json::Value::Bool(_) => {
                record_value == filter_value
            }
            
            // Support range queries for numbers
            serde_json::Value::Object(filter_obj) => {
                if let Some(gte) = filter_obj.get("$gte") {
                    if let (serde_json::Value::Number(record_num), serde_json::Value::Number(filter_num)) = (record_value, gte) {
                        if record_num.as_f64().unwrap_or(0.0) < filter_num.as_f64().unwrap_or(0.0) {
                            return false;
                        }
                    }
                }
                
                if let Some(lte) = filter_obj.get("$lte") {
                    if let (serde_json::Value::Number(record_num), serde_json::Value::Number(filter_num)) = (record_value, lte) {
                        if record_num.as_f64().unwrap_or(0.0) > filter_num.as_f64().unwrap_or(0.0) {
                            return false;
                        }
                    }
                }
                
                if let Some(in_values) = filter_obj.get("$in") {
                    if let serde_json::Value::Array(values) = in_values {
                        return values.contains(record_value);
                    }
                }
                
                // If no special operators, do direct comparison
                record_value == filter_value
            }
            
            // Array contains logic
            serde_json::Value::Array(filter_array) => {
                if let serde_json::Value::Array(record_array) = record_value {
                    // Check if record array contains all filter values
                    filter_array.iter().all(|filter_item| record_array.contains(filter_item))
                } else {
                    // Single value contains check
                    filter_array.contains(record_value)
                }
            }
            
            _ => record_value == filter_value,
        }
    }
    
    /// Merge another memtable into this one (for recovery)
    pub async fn merge(&self, other_entries: Vec<MemtableEntry>) -> Result<()> {
        for entry in other_entries {
            match &entry.operation {
                MemtableOperation::Put { record } => {
                    self.put(record.clone()).await?;
                }
                MemtableOperation::Delete { vector_id: _ } => {
                    // Extract collection_id from the put operation context or assume it's available
                    // For now, we'll need to track this differently or store it in the delete operation
                    // This is a design consideration for the delete operation
                    continue; // Skip deletes during merge for now
                }
            }
        }
        Ok(())
    }
}

/// Public collection statistics
#[derive(Debug, Clone)]
pub struct MemtableCollectionStats {
    pub active_vectors: usize,
    pub total_operations: usize,
    pub size_bytes: usize,
    pub last_updated: DateTime<Utc>,
}

impl CollectionMemtable {
    /// Check if collection is empty
    fn is_empty(&self) -> bool {
        self.stats.active_count == 0
    }
    
    /// Get latest sequence number
    fn latest_sequence(&self) -> Option<u64> {
        self.vectors.keys().last().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use uuid::Uuid;
    use chrono::Utc;
    
    #[tokio::test]
    async fn test_memtable_basic_operations() {
        let memtable = Memtable::new(1); // 1MB limit
        
        let record = VectorRecord {
            id: Uuid::new_v4().to_string(),
            collection_id: "test_collection".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: HashMap::new(),
            timestamp: Utc::now(),
            expires_at: None,
        };
        
        // Test put
        let seq = memtable.put(record.clone()).await.unwrap();
        assert_eq!(seq, 1);
        
        // Test get
        let retrieved = memtable.get(&record.collection_id, &record.id).await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().vector, record.vector);
        
        // Test delete
        let del_seq = memtable.delete(record.collection_id.clone(), record.id).await.unwrap();
        assert_eq!(del_seq, 2);
        
        // Test get after delete
        let retrieved = memtable.get(&record.collection_id, &record.id).await.unwrap();
        assert!(retrieved.is_none());
    }
    
    #[tokio::test]
    async fn test_memtable_size_tracking() {
        let memtable = Memtable::new(1); // 1MB limit
        
        let record = VectorRecord {
            id: Uuid::new_v4().to_string(),
            collection_id: "test_collection".to_string(),
            vector: vec![1.0; 1000], // Large vector
            metadata: HashMap::new(),
            timestamp: Utc::now(),
            expires_at: None,
        };
        
        let initial_size = memtable.size_bytes().await;
        memtable.put(record.clone()).await.unwrap();
        let after_put_size = memtable.size_bytes().await;
        
        assert!(after_put_size > initial_size);
        
        memtable.delete(record.collection_id, record.id).await.unwrap();
        let after_delete_size = memtable.size_bytes().await;
        
        assert!(after_delete_size > after_put_size); // Delete adds tombstone
    }
}