// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Memtable Isolation for Atomic Collection Flush Operations

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::core::{CollectionId, VectorId};
use super::{MemTableStrategy, MemTableStats, MemTableMaintenanceStats};
use crate::storage::wal::{WalEntry, WalConfig};

/// Isolated collection memtable for atomic flush operations
#[derive(Debug)]
pub struct IsolatedCollectionMemTable {
    /// The isolated collection data
    collection_id: CollectionId,
    
    /// Actual memtable strategy holding the data
    memtable_data: Box<dyn CollectionMemTableData>,
    
    /// Isolation metadata
    isolation_metadata: IsolationMetadata,
}

/// Metadata about the isolation operation
#[derive(Debug, Clone)]
pub struct IsolationMetadata {
    /// When isolation started
    pub isolated_at: DateTime<Utc>,
    
    /// Sequence fence point (entries below this are isolated)
    pub sequence_fence: u64,
    
    /// Collection state when isolated
    pub snapshot_stats: MemTableStats,
    
    /// Purpose of isolation (flush, backup, etc.)
    pub isolation_reason: IsolationReason,
}

/// Reason for memtable isolation
#[derive(Debug, Clone)]
pub enum IsolationReason {
    /// Collection flush operation
    Flush,
    /// Collection backup/snapshot
    Backup,
    /// Collection migration
    Migration,
    /// Collection compaction
    Compaction,
}

/// Trait for collection-specific memtable data that can be isolated
#[async_trait]
pub trait CollectionMemTableData: Send + Sync + std::fmt::Debug {
    /// Get all entries in this collection
    async fn get_all_entries(&self) -> Result<Vec<WalEntry>>;
    
    /// Get entries from a sequence number
    async fn get_entries_from(&self, from_sequence: u64, limit: Option<usize>) -> Result<Vec<WalEntry>>;
    
    /// Get latest entry for a vector ID
    async fn get_latest_entry(&self, vector_id: &VectorId) -> Result<Option<WalEntry>>;
    
    /// Get entry history for a vector ID (MVCC)
    async fn get_entry_history(&self, vector_id: &VectorId) -> Result<Vec<WalEntry>>;
    
    /// Search for specific vector entry
    async fn search_vector(&self, vector_id: &VectorId) -> Result<Option<WalEntry>>;
    
    /// Get statistics
    async fn get_stats(&self) -> Result<MemTableStats>;
    
    /// Get memory usage in bytes
    fn memory_usage(&self) -> usize;
    
    /// Get total entry count
    fn entry_count(&self) -> u64;
    
    /// Get oldest entry timestamp (for age calculations)
    fn oldest_entry_timestamp(&self) -> Option<DateTime<Utc>>;
    
    /// Get collection ID
    fn collection_id(&self) -> &CollectionId;
    
    /// Clone the collection data (for isolation)
    async fn clone_data(&self) -> Result<Box<dyn CollectionMemTableData>>;
}

/// Extended memtable trait supporting collection isolation
#[async_trait]
pub trait IsolatableMemTableStrategy: MemTableStrategy {
    /// Isolate a collection for flush (atomic operation)
    /// Returns (isolated_memtable, new_empty_memtable_for_collection)
    async fn isolate_collection(&self, collection_id: &CollectionId) -> Result<(IsolatedCollectionMemTable, Option<Box<dyn CollectionMemTableData>>)>;
    
    /// Replace collection memtable with new empty one (for post-isolation writes)
    async fn replace_collection_memtable(&self, collection_id: &CollectionId, new_memtable: Box<dyn CollectionMemTableData>) -> Result<()>;
    
    /// Remove collection completely (after successful flush)
    async fn remove_collection(&self, collection_id: &CollectionId) -> Result<()>;
    
    /// Get collection-specific data (for direct access)
    async fn get_collection_data(&self, collection_id: &CollectionId) -> Result<Option<Arc<dyn CollectionMemTableData>>>;
    
    /// Check if collection needs isolation (age, size, entry count thresholds)
    async fn needs_isolation(&self, collection_id: &CollectionId, config: &WalConfig) -> Result<bool>;
    
    /// Get collections that need isolation based on thresholds
    async fn collections_needing_isolation(&self, config: &WalConfig) -> Result<Vec<CollectionId>>;
}

impl IsolatedCollectionMemTable {
    /// Create new isolated collection memtable
    pub fn new(
        collection_id: CollectionId,
        memtable_data: Box<dyn CollectionMemTableData>,
        sequence_fence: u64,
        isolation_reason: IsolationReason,
    ) -> Self {
        Self {
            collection_id: collection_id.clone(),
            isolation_metadata: IsolationMetadata {
                isolated_at: Utc::now(),
                sequence_fence,
                snapshot_stats: MemTableStats {
                    total_entries: memtable_data.entry_count(),
                    memory_bytes: memtable_data.memory_usage(),
                    lookup_performance_ms: 0.0, // Will be populated by get_stats()
                    insert_performance_ms: 0.0,
                    range_scan_performance_ms: 0.0,
                },
                isolation_reason,
            },
            memtable_data,
        }
    }
    
    /// Get all entries for flushing
    pub async fn get_all_entries(&self) -> Result<Vec<WalEntry>> {
        self.memtable_data.get_all_entries().await
    }
    
    /// Get entries up to the sequence fence only
    pub async fn get_entries_for_flush(&self) -> Result<Vec<WalEntry>> {
        let all_entries = self.memtable_data.get_all_entries().await?;
        
        // Filter to only include entries up to the sequence fence
        let filtered: Vec<WalEntry> = all_entries
            .into_iter()
            .filter(|entry| entry.sequence <= self.isolation_metadata.sequence_fence)
            .collect();
            
        Ok(filtered)
    }
    
    /// Get isolation metadata
    pub fn isolation_metadata(&self) -> &IsolationMetadata {
        &self.isolation_metadata
    }
    
    /// Get collection ID
    pub fn collection_id(&self) -> &CollectionId {
        &self.collection_id
    }
    
    /// Get memory usage
    pub fn memory_usage(&self) -> usize {
        self.memtable_data.memory_usage()
    }
    
    /// Get entry count
    pub fn entry_count(&self) -> u64 {
        self.memtable_data.entry_count()
    }
}

/// Flush isolation coordinator - manages the isolation process
pub struct FlushIsolationCoordinator {
    /// Active isolated collections
    isolated_collections: Arc<RwLock<HashMap<CollectionId, IsolatedCollectionMemTable>>>,
}

impl FlushIsolationCoordinator {
    /// Create new isolation coordinator
    pub fn new() -> Self {
        Self {
            isolated_collections: Arc::new(RwLock::new(HashMap::new())),
        }
    }
    
    /// Start isolation for a collection
    pub async fn start_isolation(
        &self,
        memtable: &dyn IsolatableMemTableStrategy,
        collection_id: &CollectionId,
        reason: IsolationReason,
    ) -> Result<()> {
        // Check if already isolated
        {
            let isolated = self.isolated_collections.read().await;
            if isolated.contains_key(collection_id) {
                return Err(anyhow::anyhow!("Collection {} already isolated", collection_id));
            }
        }
        
        // Perform atomic isolation
        let (isolated_memtable, new_memtable) = memtable.isolate_collection(collection_id).await?;
        
        // Replace with new empty memtable if provided
        if let Some(new_memtable) = new_memtable {
            memtable.replace_collection_memtable(collection_id, new_memtable).await?;
        }
        
        // Track isolated collection
        {
            let mut isolated = self.isolated_collections.write().await;
            isolated.insert(collection_id.clone(), isolated_memtable);
        }
        
        tracing::info!("🔒 Collection {} isolated for {:?}", collection_id, reason);
        Ok(())
    }
    
    /// Complete isolation and cleanup
    pub async fn complete_isolation(
        &self,
        collection_id: &CollectionId,
        success: bool,
    ) -> Result<Option<IsolatedCollectionMemTable>> {
        let mut isolated = self.isolated_collections.write().await;
        let isolated_memtable = isolated.remove(collection_id);
        
        if let Some(ref memtable) = isolated_memtable {
            if success {
                tracing::info!("✅ Collection {} isolation completed successfully", collection_id);
            } else {
                tracing::warn!("⚠️ Collection {} isolation failed, cleaned up", collection_id);
            }
        }
        
        Ok(isolated_memtable)
    }
    
    /// Check if collection is isolated
    pub async fn is_collection_isolated(&self, collection_id: &CollectionId) -> Result<bool> {
        let isolated = self.isolated_collections.read().await;
        Ok(isolated.contains_key(collection_id))
    }
    
    /// Check if collection is currently isolated
    pub async fn is_isolated(&self, collection_id: &CollectionId) -> bool {
        let isolated = self.isolated_collections.read().await;
        isolated.contains_key(collection_id)
    }
    
    /// Get all currently isolated collection IDs
    pub async fn get_all_isolated_ids(&self) -> Vec<CollectionId> {
        let isolated = self.isolated_collections.read().await;
        isolated.keys().cloned().collect()
    }
}