// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! HashMap Memtable - High Performance Unordered Access for Point Lookups

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::core::{CollectionId, VectorId};
use super::{MemTableStrategy, MemTableStats, MemTableMaintenanceStats};
use crate::storage::wal::{WalEntry, WalOperation, WalConfig};

/// HashMap-based memtable for maximum write performance and point lookups
#[derive(Debug)]
pub struct HashMapMemTable {
    /// Per-collection hash maps for unordered high-speed storage
    collections: Arc<RwLock<HashMap<CollectionId, CollectionHashMap>>>,
    
    /// Configuration
    config: Option<WalConfig>,
    
    /// Global sequence counter
    global_sequence: Arc<tokio::sync::Mutex<u64>>,
    
    /// Total memory tracking
    total_memory_size: Arc<tokio::sync::Mutex<usize>>,
}

/// Collection-specific hash map with MVCC and TTL support
#[derive(Debug)]
struct CollectionHashMap {
    /// Main hash map for O(1) access by sequence
    entries_by_sequence: HashMap<u64, WalEntry>,
    
    /// Vector ID index for MVCC (vector_id -> list of sequence numbers)
    vector_index: HashMap<VectorId, Vec<u64>>,
    
    /// Collection metadata
    collection_id: CollectionId,
    sequence_counter: u64,
    memory_size: usize,
    last_flush_sequence: u64,
    total_entries: u64,
    last_write_time: DateTime<Utc>,
}

impl CollectionHashMap {
    fn new(collection_id: CollectionId) -> Self {
        Self {
            entries_by_sequence: HashMap::new(),
            vector_index: HashMap::new(),
            collection_id,
            sequence_counter: 0,
            memory_size: 0,
            last_flush_sequence: 0,
            total_entries: 0,
            last_write_time: Utc::now(),
        }
    }
    
    /// Insert entry with maximum write performance
    fn insert_entry(&mut self, mut entry: WalEntry) -> u64 {
        self.sequence_counter += 1;
        entry.sequence = self.sequence_counter;
        
        // Update vector index for MVCC (O(1) hash map access)
        if let WalOperation::Insert { vector_id, .. } 
         | WalOperation::Update { vector_id, .. } 
         | WalOperation::Delete { vector_id, .. } = &entry.operation {
            self.vector_index
                .entry(vector_id.clone())
                .or_insert_with(Vec::new)
                .push(self.sequence_counter);
        }
        
        // Estimate memory usage
        self.memory_size += Self::estimate_entry_size(&entry);
        
        // Insert into hash map (O(1) performance)
        self.entries_by_sequence.insert(self.sequence_counter, entry);
        self.total_entries += 1;
        self.last_write_time = Utc::now();
        
        self.sequence_counter
    }
    
    /// Get latest entry for vector ID (O(1) hash map lookup)
    fn get_latest_entry_for_vector(&self, vector_id: &VectorId) -> Option<&WalEntry> {
        self.vector_index
            .get(vector_id)?
            .last() // Get latest sequence number
            .and_then(|seq| self.entries_by_sequence.get(seq))
    }
    
    /// Get entries from sequence (requires sorting - not optimal for range scans)
    fn get_entries_from(&self, from_sequence: u64, limit: Option<usize>) -> Vec<&WalEntry> {
        let mut entries: Vec<_> = self.entries_by_sequence
            .iter()
            .filter(|(&seq, _)| seq >= from_sequence)
            .collect();
        
        // Sort by sequence for ordered output (expensive for hash map)
        entries.sort_by_key(|(&seq, _)| seq);
        
        let mut result = Vec::new();
        for (_, entry) in entries {
            if let Some(limit) = limit {
                if result.len() >= limit {
                    break;
                }
            }
            result.push(entry);
        }
        
        result
    }
    
    /// Get all entries (for flushing) - requires sorting
    fn get_all_entries(&self) -> Vec<&WalEntry> {
        let mut entries: Vec<_> = self.entries_by_sequence.values().collect();
        entries.sort_by_key(|entry| entry.sequence);
        entries
    }
    
    /// Clear entries up to sequence (after flush)
    fn clear_up_to(&mut self, sequence: u64) {
        let keys_to_remove: Vec<u64> = self.entries_by_sequence
            .keys()
            .filter(|&&seq| seq <= sequence)
            .copied()
            .collect();
        
        let mut memory_freed = 0;
        
        for key in keys_to_remove {
            if let Some(entry) = self.entries_by_sequence.remove(&key) {
                memory_freed += Self::estimate_entry_size(&entry);
                
                // Update vector index
                if let WalOperation::Insert { vector_id, .. } 
                 | WalOperation::Update { vector_id, .. } 
                 | WalOperation::Delete { vector_id, .. } = &entry.operation {
                    if let Some(sequences) = self.vector_index.get_mut(vector_id) {
                        sequences.retain(|s| *s > sequence);
                        if sequences.is_empty() {
                            self.vector_index.remove(vector_id);
                        }
                    }
                }
            }
        }
        
        self.memory_size = self.memory_size.saturating_sub(memory_freed);
        self.last_flush_sequence = sequence;
    }
    
    /// MVCC cleanup with hash map efficiency
    fn compact_mvcc(&mut self, keep_versions: usize) -> u64 {
        let mut cleaned_entries = 0;
        let mut keys_to_remove = Vec::new();
        
        for (vector_id, sequences) in &mut self.vector_index {
            if sequences.len() > keep_versions {
                sequences.sort_unstable();
                let to_remove: Vec<u64> = sequences.drain(..sequences.len() - keep_versions).collect();
                
                for seq in to_remove {
                    keys_to_remove.push(seq);
                    cleaned_entries += 1;
                }
            }
        }
        
        // Remove old entries from hash map (O(1) per removal)
        for key in keys_to_remove {
            if let Some(entry) = self.entries_by_sequence.remove(&key) {
                self.memory_size = self.memory_size.saturating_sub(Self::estimate_entry_size(&entry));
            }
        }
        
        cleaned_entries
    }
    
    /// TTL cleanup with efficient hash map iteration
    fn cleanup_expired(&mut self, now: DateTime<Utc>) -> u64 {
        let mut cleaned_entries = 0;
        let mut keys_to_remove = Vec::new();
        
        // Hash map iteration is very efficient
        for (seq, entry) in &self.entries_by_sequence {
            if let Some(expires_at) = entry.expires_at {
                if expires_at <= now {
                    keys_to_remove.push(*seq);
                    cleaned_entries += 1;
                }
            }
        }
        
        // Remove expired entries (O(1) per removal)
        for key in keys_to_remove {
            if let Some(entry) = self.entries_by_sequence.remove(&key) {
                self.memory_size = self.memory_size.saturating_sub(Self::estimate_entry_size(&entry));
                
                // Update vector index
                if let WalOperation::Insert { vector_id, .. } 
                 | WalOperation::Update { vector_id, .. } 
                 | WalOperation::Delete { vector_id, .. } = &entry.operation {
                    if let Some(sequences) = self.vector_index.get_mut(vector_id) {
                        sequences.retain(|s| *s != key);
                        if sequences.is_empty() {
                            self.vector_index.remove(vector_id);
                        }
                    }
                }
            }
        }
        
        cleaned_entries
    }
    
    /// Check if flush is needed
    fn needs_flush(&self, entry_threshold: usize, size_threshold: usize) -> bool {
        self.entries_by_sequence.len() >= entry_threshold || self.memory_size >= size_threshold
    }
    
    /// Estimate entry memory size
    fn estimate_entry_size(entry: &WalEntry) -> usize {
        let base_size = std::mem::size_of::<WalEntry>();
        let operation_size = match &entry.operation {
            WalOperation::Insert { record, .. } | WalOperation::Update { record, .. } => {
                record.vector.len() * std::mem::size_of::<f32>() + 
                record.metadata.as_ref().map(|m| m.len()).unwrap_or(0)
            }
            _ => 64,
        };
        
        base_size + operation_size + 8 // Minimal hash map overhead
    }
}

impl HashMapMemTable {
    pub fn new() -> Self {
        Self {
            collections: Arc::new(RwLock::new(HashMap::new())),
            config: None,
            global_sequence: Arc::new(tokio::sync::Mutex::new(0)),
            total_memory_size: Arc::new(tokio::sync::Mutex::new(0)),
        }
    }
}

#[async_trait]
impl MemTableStrategy for HashMapMemTable {
    fn strategy_name(&self) -> &'static str {
        "HashMap"
    }
    
    async fn initialize(&mut self, config: &WalConfig) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("✅ HashMap memtable initialized for maximum write performance and point lookups");
        Ok(())
    }
    
    async fn insert_entry(&self, mut entry: WalEntry) -> Result<u64> {
        // Assign global sequence
        {
            let mut global_seq = self.global_sequence.lock().await;
            *global_seq += 1;
            entry.global_sequence = *global_seq;
        }
        
        let collection_id = entry.collection_id.clone();
        let mut collections = self.collections.write().await;
        
        // Get or create collection hash map
        let collection_hashmap = collections
            .entry(collection_id.clone())
            .or_insert_with(|| CollectionHashMap::new(collection_id.clone()));
        
        let sequence = collection_hashmap.insert_entry(entry);
        
        // Update total memory size
        {
            let mut total_size = self.total_memory_size.lock().await;
            *total_size = collections.values().map(|t| t.memory_size).sum();
        }
        
        Ok(sequence)
    }
    
    async fn insert_batch(&self, entries: Vec<WalEntry>) -> Result<Vec<u64>> {
        let mut sequences = Vec::with_capacity(entries.len());
        
        // Batch optimized: group by collection for maximum efficiency
        let mut by_collection: HashMap<CollectionId, Vec<WalEntry>> = HashMap::new();
        for entry in entries {
            by_collection.entry(entry.collection_id.clone()).or_default().push(entry);
        }
        
        let mut collections = self.collections.write().await;
        
        for (collection_id, collection_entries) in by_collection {
            let collection_hashmap = collections
                .entry(collection_id.clone())
                .or_insert_with(|| CollectionHashMap::new(collection_id.clone()));
            
            for mut entry in collection_entries {
                // Assign global sequence
                {
                    let mut global_seq = self.global_sequence.lock().await;
                    *global_seq += 1;
                    entry.global_sequence = *global_seq;
                }
                
                let seq = collection_hashmap.insert_entry(entry);
                sequences.push(seq);
            }
        }
        
        // Update total memory size
        {
            let mut total_size = self.total_memory_size.lock().await;
            *total_size = collections.values().map(|t| t.memory_size).sum();
        }
        
        Ok(sequences)
    }
    
    async fn get_latest_entry(&self, collection_id: &CollectionId, vector_id: &VectorId) -> Result<Option<WalEntry>> {
        let collections = self.collections.read().await;
        
        Ok(collections
            .get(collection_id)
            .and_then(|hashmap| hashmap.get_latest_entry_for_vector(vector_id))
            .cloned())
    }
    
    async fn get_entry_history(&self, collection_id: &CollectionId, vector_id: &VectorId) -> Result<Vec<WalEntry>> {
        let collections = self.collections.read().await;
        
        Ok(collections
            .get(collection_id)
            .map(|hashmap| {
                hashmap.vector_index
                    .get(vector_id)
                    .map(|sequences| {
                        sequences
                            .iter()
                            .filter_map(|seq| hashmap.entries_by_sequence.get(seq))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default())
    }
    
    async fn get_entries_from(&self, collection_id: &CollectionId, from_sequence: u64, limit: Option<usize>) -> Result<Vec<WalEntry>> {
        let collections = self.collections.read().await;
        
        Ok(collections
            .get(collection_id)
            .map(|hashmap| {
                hashmap.get_entries_from(from_sequence, limit)
                    .into_iter()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }
    
    async fn get_all_entries(&self, collection_id: &CollectionId) -> Result<Vec<WalEntry>> {
        let collections = self.collections.read().await;
        
        Ok(collections
            .get(collection_id)
            .map(|hashmap| {
                hashmap.get_all_entries()
                    .into_iter()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }
    
    async fn search_vector(&self, collection_id: &CollectionId, vector_id: &VectorId) -> Result<Option<WalEntry>> {
        self.get_latest_entry(collection_id, vector_id).await
    }
    
    async fn clear_flushed(&self, collection_id: &CollectionId, up_to_sequence: u64) -> Result<()> {
        let mut collections = self.collections.write().await;
        
        if let Some(hashmap) = collections.get_mut(collection_id) {
            hashmap.clear_up_to(up_to_sequence);
        }
        
        // Update total memory size
        {
            let mut total_size = self.total_memory_size.lock().await;
            *total_size = collections.values().map(|t| t.memory_size).sum();
        }
        
        Ok(())
    }
    
    async fn drop_collection(&self, collection_id: &CollectionId) -> Result<()> {
        let mut collections = self.collections.write().await;
        collections.remove(collection_id);
        
        // Update total memory size
        {
            let mut total_size = self.total_memory_size.lock().await;
            *total_size = collections.values().map(|t| t.memory_size).sum();
        }
        
        Ok(())
    }
    
    async fn collections_needing_flush(&self) -> Result<Vec<CollectionId>> {
        let collections = self.collections.read().await;
        let mut result = Vec::new();
        
        let config = self.config.as_ref().unwrap();
        
        for (collection_id, hashmap) in collections.iter() {
            let effective_config = config.effective_config_for_collection(collection_id.as_str());
            
            if hashmap.needs_flush(
                effective_config.memory_flush_threshold,
                effective_config.disk_segment_size,
            ) {
                result.push(collection_id.clone());
            }
        }
        
        Ok(result)
    }
    
    async fn needs_global_flush(&self) -> Result<bool> {
        let total_size = self.total_memory_size.lock().await;
        let config = self.config.as_ref().unwrap();
        Ok(*total_size >= config.performance.global_flush_threshold)
    }
    
    async fn get_stats(&self) -> Result<HashMap<CollectionId, MemTableStats>> {
        let collections = self.collections.read().await;
        let mut stats = HashMap::new();
        
        for (collection_id, hashmap) in collections.iter() {
            stats.insert(collection_id.clone(), MemTableStats {
                total_entries: hashmap.total_entries,
                memory_bytes: hashmap.memory_size,
                lookup_performance_ms: 0.001, // HashMap excellent O(1) lookup performance
                insert_performance_ms: 0.001, // HashMap excellent O(1) insert performance
                range_scan_performance_ms: 5.0, // HashMap poor range scan (requires sorting)
            });
        }
        
        Ok(stats)
    }
    
    async fn maintenance(&self) -> Result<MemTableMaintenanceStats> {
        let mut collections = self.collections.write().await;
        let now = Utc::now();
        let mut stats = MemTableMaintenanceStats::default();
        
        for hashmap in collections.values_mut() {
            // TTL cleanup
            stats.ttl_entries_expired += hashmap.cleanup_expired(now);
            
            // MVCC cleanup (keep last 3 versions by default)
            stats.mvcc_versions_cleaned += hashmap.compact_mvcc(3);
        }
        
        // Update total memory size
        {
            let mut total_size = self.total_memory_size.lock().await;
            let new_size = collections.values().map(|t| t.memory_size).sum();
            stats.memory_compacted_bytes = total_size.saturating_sub(new_size);
            *total_size = new_size;
        }
        
        Ok(stats)
    }
    
    async fn get_collection_ages(&self) -> Result<HashMap<CollectionId, chrono::Duration>> {
        let collections = self.collections.read().await;
        let mut ages = HashMap::new();
        let now = chrono::Utc::now();
        
        for (collection_id, hash_map) in collections.iter() {
            // Get oldest entry timestamp from the first entry by insertion order
            if let Some((_, first_entry)) = hash_map.entries.iter().next() {
                let age = now.signed_duration_since(first_entry.timestamp);
                ages.insert(collection_id.clone(), age);
            }
        }
        
        Ok(ages)
    }
    
    async fn get_oldest_unflushed_timestamp(&self, collection_id: &CollectionId) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let collections = self.collections.read().await;
        
        if let Some(hash_map) = collections.get(collection_id) {
            if let Some((_, first_entry)) = hash_map.entries.iter().next() {
                Ok(Some(first_entry.timestamp))
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }
    
    async fn needs_age_based_flush(&self, collection_id: &CollectionId, max_age_secs: u64) -> Result<bool> {
        if let Some(oldest_time) = self.get_oldest_unflushed_timestamp(collection_id).await? {
            let age = chrono::Utc::now().signed_duration_since(oldest_time);
            Ok(age.num_seconds() > max_age_secs as i64)
        } else {
            Ok(false)
        }
    }

    async fn close(&self) -> Result<()> {
        tracing::info!("✅ HashMap memtable closed");
        Ok(())
    }
    
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}