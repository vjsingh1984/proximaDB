// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! WAL Memory Tables - Collection-organized in-memory WAL storage

use chrono::{DateTime, Utc};
use std::collections::{HashMap, BTreeMap};
use std::sync::Arc;
use tokio::sync::RwLock;
use anyhow::Result;

use crate::core::{CollectionId, VectorId};
use super::{WalEntry, WalOperation};
use super::config::{WalConfig, CompressionConfig};

/// Collection-specific memory table for WAL entries
#[derive(Debug)]
struct CollectionMemTable {
    /// Collection ID
    collection_id: CollectionId,
    
    /// Ordered entries by sequence number
    entries: BTreeMap<u64, WalEntry>,
    
    /// Index by vector ID for fast lookups (MVCC support)
    vector_index: HashMap<VectorId, Vec<u64>>, // Vector ID -> sequence numbers (for MVCC)
    
    /// Collection sequence counter
    sequence_counter: u64,
    
    /// Memory size estimation (bytes)
    memory_size: usize,
    
    /// Last flush sequence
    last_flush_sequence: u64,
    
    /// Statistics
    total_entries: u64,
    last_write_time: DateTime<Utc>,
}

impl CollectionMemTable {
    fn new(collection_id: CollectionId) -> Self {
        Self {
            collection_id,
            entries: BTreeMap::new(),
            vector_index: HashMap::new(),
            sequence_counter: 0,
            memory_size: 0,
            last_flush_sequence: 0,
            total_entries: 0,
            last_write_time: Utc::now(),
        }
    }
    
    /// Insert entry and return assigned sequence number
    fn insert_entry(&mut self, mut entry: WalEntry) -> u64 {
        self.sequence_counter += 1;
        entry.sequence = self.sequence_counter;
        
        // Update vector index for MVCC support
        if let WalOperation::Insert { vector_id, .. } 
         | WalOperation::Update { vector_id, .. } 
         | WalOperation::Delete { vector_id, .. } = &entry.operation {
            self.vector_index
                .entry(vector_id.clone())
                .or_insert_with(Vec::new)
                .push(self.sequence_counter);
        }
        
        // Estimate memory usage (rough calculation)
        self.memory_size += Self::estimate_entry_size(&entry);
        
        self.entries.insert(self.sequence_counter, entry);
        self.total_entries += 1;
        self.last_write_time = Utc::now();
        
        self.sequence_counter
    }
    
    /// Get latest entry for a vector ID (MVCC)
    fn get_latest_entry_for_vector(&self, vector_id: &VectorId) -> Option<&WalEntry> {
        self.vector_index
            .get(vector_id)?
            .last() // Get latest sequence number
            .and_then(|seq| self.entries.get(seq))
    }
    
    /// Get all entries for a vector ID (MVCC history)
    fn get_all_entries_for_vector(&self, vector_id: &VectorId) -> Vec<&WalEntry> {
        self.vector_index
            .get(vector_id)
            .map(|sequences| {
                sequences
                    .iter()
                    .filter_map(|seq| self.entries.get(seq))
                    .collect()
            })
            .unwrap_or_default()
    }
    
    /// Get entries from a sequence number
    fn get_entries_from(&self, from_sequence: u64, limit: Option<usize>) -> Vec<&WalEntry> {
        let mut result = Vec::new();
        let mut count = 0;
        
        for (seq, entry) in self.entries.range(from_sequence..) {
            if let Some(limit) = limit {
                if count >= limit {
                    break;
                }
            }
            result.push(entry);
            count += 1;
        }
        
        result
    }
    
    /// Get all entries for flushing
    fn get_all_entries(&self) -> Vec<&WalEntry> {
        self.entries.values().collect()
    }
    
    /// Clear entries up to sequence number (after flush)
    fn clear_up_to(&mut self, sequence: u64) {
        let keys_to_remove: Vec<u64> = self.entries
            .range(..=sequence)
            .map(|(k, _)| *k)
            .collect();
        
        for key in keys_to_remove {
            if let Some(entry) = self.entries.remove(&key) {
                self.memory_size = self.memory_size.saturating_sub(Self::estimate_entry_size(&entry));
                
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
        
        self.last_flush_sequence = sequence;
    }
    
    /// Compact old MVCC versions
    fn compact_mvcc(&mut self, keep_versions: usize) -> u64 {
        let mut cleaned_entries = 0;
        let mut keys_to_remove = Vec::new();
        
        for (vector_id, sequences) in &mut self.vector_index {
            if sequences.len() > keep_versions {
                // Keep only the latest N versions
                sequences.sort_unstable();
                let to_remove = sequences.drain(..sequences.len() - keep_versions);
                
                for seq in to_remove {
                    keys_to_remove.push(seq);
                    cleaned_entries += 1;
                }
            }
        }
        
        // Remove old entries
        for key in keys_to_remove {
            if let Some(entry) = self.entries.remove(&key) {
                self.memory_size = self.memory_size.saturating_sub(Self::estimate_entry_size(&entry));
            }
        }
        
        cleaned_entries
    }
    
    /// Clean up expired entries (TTL)
    fn cleanup_expired(&mut self, now: DateTime<Utc>) -> u64 {
        let mut cleaned_entries = 0;
        let mut keys_to_remove = Vec::new();
        
        for (seq, entry) in &self.entries {
            if let Some(expires_at) = entry.expires_at {
                if expires_at <= now {
                    keys_to_remove.push(*seq);
                    cleaned_entries += 1;
                }
            }
        }
        
        // Remove expired entries
        for key in keys_to_remove {
            if let Some(entry) = self.entries.remove(&key) {
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
    
    /// Estimate memory size of an entry
    fn estimate_entry_size(entry: &WalEntry) -> usize {
        // Rough estimation - could be more precise
        let base_size = std::mem::size_of::<WalEntry>();
        let operation_size = match &entry.operation {
            WalOperation::Insert { record, .. } | WalOperation::Update { record, .. } => {
                record.vector.len() * std::mem::size_of::<f32>() + 
                record.metadata.as_ref().map(|m| m.len()).unwrap_or(0)
            }
            _ => 64, // Estimated size for other operations
        };
        
        base_size + operation_size
    }
    
    /// Check if flush is needed based on thresholds
    fn needs_flush(&self, entry_threshold: usize, size_threshold: usize) -> bool {
        self.entries.len() >= entry_threshold || self.memory_size >= size_threshold
    }
    
    /// Check if collections need flushing based on memory thresholds
    fn collections_needing_flush(&self) -> Vec<CollectionId> {
        let mut collections_to_flush = Vec::new();
        
        // Check if global memory usage is too high
        if self.entries.len() > self.config.memtable.memory_flush_threshold {
            // Get all collection IDs that have data
            for entry in self.entries.values() {
                if !collections_to_flush.contains(&entry.collection_id) {
                    collections_to_flush.push(entry.collection_id.clone());
                }
            }
        }
        
        collections_to_flush
    }
    
    /// Check if global flush is needed
    fn needs_global_flush(&self) -> bool {
        // Check if memory usage exceeds global limit
        let global_limit = self.config.memtable.global_memory_limit / std::mem::size_of::<WalEntry>();
        self.entries.len() > global_limit
    }
}

/// Global WAL memory tables manager with pluggable strategies
#[derive(Debug)]
pub struct WalMemTable {
    /// Pluggable memtable strategy
    strategy: Arc<dyn super::memtable::MemTableStrategy>,
    
    /// Configuration
    config: WalConfig,
}

impl WalMemTable {
    /// Create new WAL memory table manager with pluggable strategy
    pub async fn new(config: WalConfig) -> Result<Self> {
        let strategy = super::memtable::MemTableFactory::create_from_config(&config).await?;
        
        Ok(Self {
            strategy: strategy.into(),
            config,
        })
    }
    
    /// Insert entry using pluggable strategy
    pub async fn insert_entry(&self, entry: WalEntry) -> Result<u64> {
        self.strategy.insert_entry(entry).await
    }
    
    /// Insert batch of entries using pluggable strategy
    pub async fn insert_batch(&self, entries: Vec<WalEntry>) -> Result<Vec<u64>> {
        self.strategy.insert_batch(entries).await
    }
    
    /// Get latest entry for a vector ID using pluggable strategy
    pub async fn get_latest_entry(&self, collection_id: &CollectionId, vector_id: &VectorId) -> Result<Option<WalEntry>> {
        self.strategy.get_latest_entry(collection_id, vector_id).await
    }
    
    /// Search for vector entry using pluggable strategy
    pub async fn search_vector(&self, collection_id: &CollectionId, vector_id: &VectorId) -> Result<Option<WalEntry>> {
        self.strategy.search_vector(collection_id, vector_id).await
    }
    
    /// Get entries from sequence number using pluggable strategy
    pub async fn get_entries(&self, collection_id: &CollectionId, from_sequence: u64, limit: Option<usize>) -> Result<Vec<WalEntry>> {
        self.strategy.get_entries_from(collection_id, from_sequence, limit).await
    }
    
    /// Get all entries for a collection (for flushing) using pluggable strategy
    pub async fn get_all_entries(&self, collection_id: &CollectionId) -> Result<Vec<WalEntry>> {
        self.strategy.get_all_entries(collection_id).await
    }
    
    /// Clear entries after flush using pluggable strategy
    pub async fn clear_flushed(&self, collection_id: &CollectionId, up_to_sequence: u64) -> Result<()> {
        self.strategy.clear_flushed(collection_id, up_to_sequence).await
    }
    
    /// Drop collection from memory using pluggable strategy
    pub async fn drop_collection(&self, collection_id: &CollectionId) -> Result<()> {
        self.strategy.drop_collection(collection_id).await
    }
    
    /// Check if collections need flushing based on memory thresholds
    pub async fn collections_needing_flush(&self) -> Result<Vec<CollectionId>> {
        self.strategy.collections_needing_flush().await
    }
    
    /// Check if global flush is needed
    pub async fn needs_global_flush(&self) -> Result<bool> {
        self.strategy.needs_global_flush().await
    }
    
    /// Get memory statistics using pluggable strategy
    pub async fn get_stats(&self) -> Result<MemoryStats> {
        let strategy_stats = self.strategy.get_stats().await?;
        
        let total_entries: u64 = strategy_stats.values().map(|s| s.total_entries).sum();
        let total_memory_bytes: usize = strategy_stats.values().map(|s| s.memory_bytes).sum();
        let collections_count = strategy_stats.len();
        let largest_collection = strategy_stats
            .iter()
            .max_by_key(|(_, s)| s.memory_bytes)
            .map(|(id, s)| (id.clone(), s.memory_bytes));
        
        Ok(MemoryStats {
            total_entries,
            total_memory_bytes,
            collections_count,
            largest_collection,
        })
    }
    
    /// Background maintenance using pluggable strategy
    pub async fn maintenance(&self) -> Result<MaintenanceStats> {
        let strategy_stats = self.strategy.maintenance().await?;
        
        Ok(MaintenanceStats {
            ttl_cleaned: strategy_stats.ttl_entries_expired,
            mvcc_cleaned: strategy_stats.mvcc_versions_cleaned,
        })
    }
}

/// Memory statistics
#[derive(Debug, Clone)]
pub struct MemoryStats {
    pub total_entries: u64,
    pub total_memory_bytes: usize,
    pub collections_count: usize,
    pub largest_collection: Option<(CollectionId, usize)>,
}

/// Maintenance statistics
#[derive(Debug, Clone, Default)]
pub struct MaintenanceStats {
    pub ttl_cleaned: u64,
    pub mvcc_cleaned: u64,
}