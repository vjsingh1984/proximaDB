// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Isolatable Skip List Memtable - Example Implementation
//! 
//! Shows how to extend an existing memtable to support collection isolation.

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc, Duration};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use crossbeam_skiplist::SkipMap;

use crate::core::{CollectionId, VectorId};
use super::{
    MemTableStrategy, MemTableStats, MemTableMaintenanceStats,
    IsolatableMemTableStrategy, CollectionMemTableData, IsolatedCollectionMemTable,
    IsolationReason,
};
use crate::storage::wal::{WalEntry, WalOperation, WalConfig};

/// Isolatable skip list memtable implementation
#[derive(Debug)]
pub struct IsolatableSkipListMemTable {
    /// Per-collection skip lists for ordered storage
    collections: Arc<RwLock<HashMap<CollectionId, Arc<IsolatableCollectionSkipList>>>>,
    
    /// Configuration
    config: Option<WalConfig>,
    
    /// Global sequence counter
    global_sequence: Arc<tokio::sync::Mutex<u64>>,
    
    /// Total memory tracking
    total_memory_size: Arc<tokio::sync::Mutex<usize>>,
}

/// Collection-specific skip list that supports isolation
#[derive(Debug)]
pub struct IsolatableCollectionSkipList {
    /// Main skip list ordered by sequence number
    entries: SkipMap<u64, WalEntry>,
    
    /// Vector ID index for MVCC (vector_id -> list of sequence numbers)
    vector_index: Arc<RwLock<HashMap<VectorId, Vec<u64>>>>,
    
    /// Collection metadata
    collection_id: CollectionId,
    sequence_counter: Arc<tokio::sync::Mutex<u64>>,
    memory_size: Arc<tokio::sync::Mutex<usize>>,
    last_flush_sequence: Arc<tokio::sync::Mutex<u64>>,
    total_entries: Arc<tokio::sync::Mutex<u64>>,
    last_write_time: Arc<tokio::sync::RwLock<DateTime<Utc>>>,
    oldest_entry_time: Arc<tokio::sync::RwLock<Option<DateTime<Utc>>>>,
}

impl IsolatableCollectionSkipList {
    fn new(collection_id: CollectionId) -> Self {
        Self {
            entries: SkipMap::new(),
            vector_index: Arc::new(RwLock::new(HashMap::new())),
            collection_id,
            sequence_counter: Arc::new(tokio::sync::Mutex::new(0)),
            memory_size: Arc::new(tokio::sync::Mutex::new(0)),
            last_flush_sequence: Arc::new(tokio::sync::Mutex::new(0)),
            total_entries: Arc::new(tokio::sync::Mutex::new(0)),
            last_write_time: Arc::new(tokio::sync::RwLock::new(Utc::now())),
            oldest_entry_time: Arc::new(tokio::sync::RwLock::new(None)),
        }
    }
    
    /// Insert entry with optimized skip list performance
    async fn insert_entry(&self, mut entry: WalEntry) -> u64 {
        let mut sequence_counter = self.sequence_counter.lock().await;
        *sequence_counter += 1;
        entry.sequence = *sequence_counter;
        let sequence = *sequence_counter;
        drop(sequence_counter);
        
        // Update oldest entry time if this is the first entry
        {
            let mut oldest_time = self.oldest_entry_time.write().await;
            if oldest_time.is_none() {
                *oldest_time = Some(entry.timestamp);
            }
        }
        
        // Update vector index for MVCC
        if let WalOperation::Insert { vector_id, .. } 
         | WalOperation::Update { vector_id, .. } 
         | WalOperation::Delete { vector_id, .. } = &entry.operation {
            let mut vector_index = self.vector_index.write().await;
            vector_index
                .entry(vector_id.clone())
                .or_insert_with(Vec::new)
                .push(sequence);
        }
        
        // Estimate memory usage
        let entry_size = Self::estimate_entry_size(&entry);
        {
            let mut memory_size = self.memory_size.lock().await;
            *memory_size += entry_size;
        }
        
        // Insert into skip list (concurrent-safe)
        self.entries.insert(sequence, entry);
        
        {
            let mut total_entries = self.total_entries.lock().await;
            *total_entries += 1;
        }
        
        {
            let mut last_write_time = self.last_write_time.write().await;
            *last_write_time = Utc::now();
        }
        
        sequence
    }
    
    /// Estimate memory usage of an entry
    fn estimate_entry_size(entry: &WalEntry) -> usize {
        // Basic estimation - in production, this would be more accurate
        std::mem::size_of::<WalEntry>() + 
        match &entry.operation {
            WalOperation::Insert { record, .. } | WalOperation::Update { record, .. } => {
                record.vector.len() * std::mem::size_of::<f32>() + 
                record.metadata.len() * 64 // estimate for metadata
            },
            _ => 64, // basic size for other operations
        }
    }
    
    /// Create a frozen copy of this collection's data for isolation
    async fn create_frozen_copy(&self, up_to_sequence: u64) -> Result<IsolatableCollectionSkipList> {
        let frozen = IsolatableCollectionSkipList::new(self.collection_id.clone());
        
        // Copy entries up to the sequence fence
        for entry_ref in self.entries.iter() {
            let sequence = entry_ref.key();
            let entry = entry_ref.value();
            if *sequence <= up_to_sequence {
                frozen.entries.insert(*sequence, entry.clone());
                
                // Update frozen collection's metadata
                if let WalOperation::Insert { vector_id, .. } 
                 | WalOperation::Update { vector_id, .. } 
                 | WalOperation::Delete { vector_id, .. } = &entry.operation {
                    let mut vector_index = frozen.vector_index.write().await;
                    vector_index
                        .entry(vector_id.clone())
                        .or_insert_with(Vec::new)
                        .push(*sequence);
                }
            }
        }
        
        // Update frozen collection's counters
        {
            let mut sequence_counter = frozen.sequence_counter.lock().await;
            *sequence_counter = up_to_sequence;
        }
        
        {
            let mut total_entries = frozen.total_entries.lock().await;
            *total_entries = frozen.entries.len() as u64;
        }
        
        // Copy oldest entry time
        {
            let oldest_time = self.oldest_entry_time.read().await;
            let mut frozen_oldest_time = frozen.oldest_entry_time.write().await;
            *frozen_oldest_time = *oldest_time;
        }
        
        Ok(frozen)
    }
    
    /// Remove entries up to a sequence number (for cleanup after flush)
    async fn remove_entries_up_to(&self, up_to_sequence: u64) -> Result<()> {
        // Collect sequences to remove
        let sequences_to_remove: Vec<u64> = self.entries
            .iter()
            .filter_map(|entry_ref| {
                let sequence = entry_ref.key();
                if *sequence <= up_to_sequence {
                    Some(*sequence)
                } else {
                    None
                }
            })
            .collect();
        
        // Remove entries and update metadata
        let mut memory_freed = 0;
        for sequence in sequences_to_remove {
            if let Some(entry) = self.entries.remove(&sequence) {
                let wal_entry = entry.value();
                memory_freed += Self::estimate_entry_size(wal_entry);
                
                // Update vector index
                if let WalOperation::Insert { vector_id, .. } 
                 | WalOperation::Update { vector_id, .. } 
                 | WalOperation::Delete { vector_id, .. } = &wal_entry.operation {
                    let mut vector_index = self.vector_index.write().await;
                    if let Some(sequences) = vector_index.get_mut(vector_id) {
                        sequences.retain(|&s| s > up_to_sequence);
                        if sequences.is_empty() {
                            vector_index.remove(vector_id);
                        }
                    }
                }
            }
        }
        
        // Update memory and entry counts
        {
            let mut memory_size = self.memory_size.lock().await;
            *memory_size = memory_size.saturating_sub(memory_freed);
        }
        
        {
            let mut total_entries = self.total_entries.lock().await;
            *total_entries = self.entries.len() as u64;
        }
        
        // Update last flush sequence
        {
            let mut last_flush_sequence = self.last_flush_sequence.lock().await;
            *last_flush_sequence = up_to_sequence;
        }
        
        // Update oldest entry time if we removed the oldest entries
        if self.entries.is_empty() {
            let mut oldest_time = self.oldest_entry_time.write().await;
            *oldest_time = None;
        } else if let Some(first_entry) = self.entries.front() {
            let mut oldest_time = self.oldest_entry_time.write().await;
            *oldest_time = Some(first_entry.value().timestamp);
        }
        
        Ok(())
    }
}

#[async_trait]
impl CollectionMemTableData for IsolatableCollectionSkipList {
    async fn get_all_entries(&self) -> Result<Vec<WalEntry>> {
        let entries: Vec<WalEntry> = self.entries
            .iter()
            .map(|entry_ref| entry_ref.value().clone())
            .collect();
        Ok(entries)
    }
    
    async fn get_entries_from(&self, from_sequence: u64, limit: Option<usize>) -> Result<Vec<WalEntry>> {
        let entries: Vec<WalEntry> = self.entries
            .range(from_sequence..)
            .take(limit.unwrap_or(usize::MAX))
            .map(|entry_ref| entry_ref.value().clone())
            .collect();
        Ok(entries)
    }
    
    async fn get_latest_entry(&self, vector_id: &VectorId) -> Result<Option<WalEntry>> {
        let vector_index = self.vector_index.read().await;
        
        if let Some(sequences) = vector_index.get(vector_id) {
            if let Some(&latest_sequence) = sequences.last() {
                if let Some(entry) = self.entries.get(&latest_sequence) {
                    return Ok(Some(entry.value().clone()));
                }
            }
        }
        
        Ok(None)
    }
    
    async fn get_entry_history(&self, vector_id: &VectorId) -> Result<Vec<WalEntry>> {
        let vector_index = self.vector_index.read().await;
        
        if let Some(sequences) = vector_index.get(vector_id) {
            let mut entries = Vec::new();
            for &sequence in sequences {
                if let Some(entry) = self.entries.get(&sequence) {
                    entries.push(entry.value().clone());
                }
            }
            Ok(entries)
        } else {
            Ok(Vec::new())
        }
    }
    
    async fn search_vector(&self, vector_id: &VectorId) -> Result<Option<WalEntry>> {
        self.get_latest_entry(vector_id).await
    }
    
    async fn get_stats(&self) -> Result<MemTableStats> {
        let total_entries = *self.total_entries.lock().await;
        let memory_bytes = *self.memory_size.lock().await;
        
        Ok(MemTableStats {
            total_entries,
            memory_bytes,
            lookup_performance_ms: 0.1, // Skip list lookup is very fast
            insert_performance_ms: 0.05, // Skip list insert is very fast
            range_scan_performance_ms: 0.2, // Skip list range scan is excellent
        })
    }
    
    fn memory_usage(&self) -> usize {
        // This is a synchronous snapshot - for exact value use async get_stats()
        self.memory_size.try_lock().map(|guard| *guard).unwrap_or(0)
    }
    
    fn entry_count(&self) -> u64 {
        // This is a synchronous snapshot - for exact value use async get_stats()
        self.total_entries.try_lock().map(|guard| *guard).unwrap_or(0)
    }
    
    fn oldest_entry_timestamp(&self) -> Option<DateTime<Utc>> {
        // This is a synchronous snapshot - for exact value use async methods
        self.oldest_entry_time.try_read().and_then(|guard| *guard).ok().flatten()
    }
    
    fn collection_id(&self) -> &CollectionId {
        &self.collection_id
    }
    
    async fn clone_data(&self) -> Result<Box<dyn CollectionMemTableData>> {
        // Create a full copy of this collection's data
        let current_sequence = *self.sequence_counter.lock().await;
        let cloned = self.create_frozen_copy(current_sequence).await?;
        Ok(Box::new(cloned))
    }
}

impl IsolatableSkipListMemTable {
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
impl MemTableStrategy for IsolatableSkipListMemTable {
    fn strategy_name(&self) -> &'static str {
        "isolatable_skiplist"
    }
    
    async fn initialize(&mut self, config: &WalConfig) -> Result<()> {
        self.config = Some(config.clone());
        tracing::info!("🚀 Initialized isolatable skip list memtable");
        Ok(())
    }
    
    async fn insert_entry(&self, entry: WalEntry) -> Result<u64> {
        let collection_id = entry.collection_id.clone();
        
        // Get or create collection
        let collection = {
            let mut collections = self.collections.write().await;
            collections.entry(collection_id.clone())
                .or_insert_with(|| Arc::new(IsolatableCollectionSkipList::new(collection_id.clone())))
                .clone()
        };
        
        // Insert into collection
        let sequence = collection.insert_entry(entry).await;
        
        // Update global sequence
        {
            let mut global_sequence = self.global_sequence.lock().await;
            *global_sequence = (*global_sequence).max(sequence);
        }
        
        Ok(sequence)
    }
    
    async fn insert_batch(&self, entries: Vec<WalEntry>) -> Result<Vec<u64>> {
        let mut sequences = Vec::with_capacity(entries.len());
        
        for entry in entries {
            let sequence = self.insert_entry(entry).await?;
            sequences.push(sequence);
        }
        
        Ok(sequences)
    }
    
    async fn get_latest_entry(&self, collection_id: &CollectionId, vector_id: &VectorId) -> Result<Option<WalEntry>> {
        let collections = self.collections.read().await;
        if let Some(collection) = collections.get(collection_id) {
            collection.get_latest_entry(vector_id).await
        } else {
            Ok(None)
        }
    }
    
    async fn get_entry_history(&self, collection_id: &CollectionId, vector_id: &VectorId) -> Result<Vec<WalEntry>> {
        let collections = self.collections.read().await;
        if let Some(collection) = collections.get(collection_id) {
            collection.get_entry_history(vector_id).await
        } else {
            Ok(Vec::new())
        }
    }
    
    async fn get_entries_from(&self, collection_id: &CollectionId, from_sequence: u64, limit: Option<usize>) -> Result<Vec<WalEntry>> {
        let collections = self.collections.read().await;
        if let Some(collection) = collections.get(collection_id) {
            collection.get_entries_from(from_sequence, limit).await
        } else {
            Ok(Vec::new())
        }
    }
    
    async fn get_all_entries(&self, collection_id: &CollectionId) -> Result<Vec<WalEntry>> {
        let collections = self.collections.read().await;
        if let Some(collection) = collections.get(collection_id) {
            collection.get_all_entries().await
        } else {
            Ok(Vec::new())
        }
    }
    
    async fn search_vector(&self, collection_id: &CollectionId, vector_id: &VectorId) -> Result<Option<WalEntry>> {
        self.get_latest_entry(collection_id, vector_id).await
    }
    
    async fn clear_flushed(&self, collection_id: &CollectionId, up_to_sequence: u64) -> Result<()> {
        let collections = self.collections.read().await;
        if let Some(collection) = collections.get(collection_id) {
            collection.remove_entries_up_to(up_to_sequence).await?;
        }
        Ok(())
    }
    
    async fn drop_collection(&self, collection_id: &CollectionId) -> Result<()> {
        let mut collections = self.collections.write().await;
        collections.remove(collection_id);
        tracing::info!("🗑️ Dropped collection {} from isolatable skip list memtable", collection_id);
        Ok(())
    }
    
    async fn collections_needing_flush(&self) -> Result<Vec<CollectionId>> {
        // This would implement flush logic based on size/count thresholds
        Ok(Vec::new())
    }
    
    async fn needs_global_flush(&self) -> Result<bool> {
        let total_memory = *self.total_memory_size.lock().await;
        let config = self.config.as_ref().unwrap();
        Ok(total_memory >= config.performance.global_flush_threshold)
    }
    
    async fn get_stats(&self) -> Result<HashMap<CollectionId, MemTableStats>> {
        let collections = self.collections.read().await;
        let mut stats = HashMap::new();
        
        for (collection_id, collection) in collections.iter() {
            let collection_stats = collection.get_stats().await?;
            stats.insert(collection_id.clone(), collection_stats);
        }
        
        Ok(stats)
    }
    
    async fn maintenance(&self) -> Result<MemTableMaintenanceStats> {
        // Implement maintenance operations (MVCC cleanup, TTL expiration)
        Ok(MemTableMaintenanceStats::default())
    }
    
    async fn get_collection_ages(&self) -> Result<HashMap<CollectionId, Duration>> {
        let collections = self.collections.read().await;
        let mut ages = HashMap::new();
        let now = Utc::now();
        
        for (collection_id, collection) in collections.iter() {
            let oldest_time = collection.oldest_entry_time.read().await;
            if let Some(oldest) = *oldest_time {
                let age = now.signed_duration_since(oldest);
                ages.insert(collection_id.clone(), age);
            }
        }
        
        Ok(ages)
    }
    
    async fn get_oldest_unflushed_timestamp(&self, collection_id: &CollectionId) -> Result<Option<DateTime<Utc>>> {
        let collections = self.collections.read().await;
        if let Some(collection) = collections.get(collection_id) {
            let oldest_time = collection.oldest_entry_time.read().await;
            Ok(*oldest_time)
        } else {
            Ok(None)
        }
    }
    
    async fn needs_age_based_flush(&self, collection_id: &CollectionId, max_age_secs: u64) -> Result<bool> {
        if let Some(oldest_time) = self.get_oldest_unflushed_timestamp(collection_id).await? {
            let age = Utc::now().signed_duration_since(oldest_time);
            Ok(age.num_seconds() > max_age_secs as i64)
        } else {
            Ok(false)
        }
    }
    
    async fn close(&self) -> Result<()> {
        let mut collections = self.collections.write().await;
        collections.clear();
        tracing::info!("🔒 Closed isolatable skip list memtable");
        Ok(())
    }
    
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[async_trait]
impl IsolatableMemTableStrategy for IsolatableSkipListMemTable {
    async fn isolate_collection(&self, collection_id: &CollectionId) -> Result<(IsolatedCollectionMemTable, Option<Box<dyn CollectionMemTableData>>)> {
        let collections = self.collections.read().await;
        
        if let Some(collection) = collections.get(collection_id) {
            // Get current sequence as fence point
            let current_sequence = *collection.sequence_counter.lock().await;
            
            // Create frozen copy of collection data
            let frozen_collection = collection.create_frozen_copy(current_sequence).await?;
            
            // Create isolated memtable
            let isolated = IsolatedCollectionMemTable::new(
                collection_id.clone(),
                Box::new(frozen_collection),
                current_sequence,
                IsolationReason::Flush,
            );
            
            // Create new empty collection for future writes
            let new_collection = IsolatableCollectionSkipList::new(collection_id.clone());
            
            Ok((isolated, Some(Box::new(new_collection))))
        } else {
            Err(anyhow::anyhow!("Collection {} not found for isolation", collection_id))
        }
    }
    
    async fn replace_collection_memtable(&self, collection_id: &CollectionId, new_memtable: Box<dyn CollectionMemTableData>) -> Result<()> {
        // Downcast the new memtable to our concrete type
        let new_collection = new_memtable.as_any()
            .downcast_ref::<IsolatableCollectionSkipList>()
            .ok_or_else(|| anyhow::anyhow!("Invalid memtable type for replacement"))?;
        
        let mut collections = self.collections.write().await;
        collections.insert(collection_id.clone(), Arc::new(new_collection.clone()));
        
        tracing::info!("🔄 Replaced collection {} memtable with new empty one", collection_id);
        Ok(())
    }
    
    async fn remove_collection(&self, collection_id: &CollectionId) -> Result<()> {
        self.drop_collection(collection_id).await
    }
    
    async fn get_collection_data(&self, collection_id: &CollectionId) -> Result<Option<Arc<dyn CollectionMemTableData>>> {
        let collections = self.collections.read().await;
        if let Some(collection) = collections.get(collection_id) {
            Ok(Some(collection.clone() as Arc<dyn CollectionMemTableData>))
        } else {
            Ok(None)
        }
    }
    
    async fn needs_isolation(&self, collection_id: &CollectionId, config: &WalConfig) -> Result<bool> {
        let collections = self.collections.read().await;
        if let Some(collection) = collections.get(collection_id) {
            let stats = collection.get_stats().await?;
            let effective_config = config.effective_config_for_collection(collection_id);
            
            // Check size thresholds
            let needs_size_flush = stats.total_entries >= effective_config.memory_flush_threshold as u64 ||
                                  stats.memory_bytes >= effective_config.disk_segment_size;
            
            // Check age threshold
            let max_age_secs = config.effective_max_age_for_collection(collection_id);
            let needs_age_flush = self.needs_age_based_flush(collection_id, max_age_secs).await?;
            
            Ok(needs_size_flush || needs_age_flush)
        } else {
            Ok(false)
        }
    }
    
    async fn collections_needing_isolation(&self, config: &WalConfig) -> Result<Vec<CollectionId>> {
        let collections = self.collections.read().await;
        let mut needing_isolation = Vec::new();
        
        for collection_id in collections.keys() {
            if self.needs_isolation(collection_id, config).await? {
                needing_isolation.push(collection_id.clone());
            }
        }
        
        Ok(needing_isolation)
    }
}

// Note: Any trait is automatically implemented for all types
// No need to manually implement it

// Add clone capability for the collection data
impl Clone for IsolatableCollectionSkipList {
    fn clone(&self) -> Self {
        // This is a simplified clone - in production you'd want to be more careful about this
        Self::new(self.collection_id.clone())
    }
}