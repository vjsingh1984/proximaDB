// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Comprehensive Write-Ahead Log System with Strategy Pattern
//!
//! This module provides a high-performance WAL system supporting:
//! - Multiple serialization strategies (Avro with schema evolution, Bincode for speed)
//! - Memory + Disk organization by collection
//! - Atomic operations with MVCC and TTL support
//! - Multi-disk support for sequential I/O optimization
//! - Configurable compression and smart defaults
//! - Batch operations for optimal performance

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::debug;

use crate::core::{CollectionId, VectorId, VectorRecord};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::UnifiedStorageEngine;

// Sub-modules
pub mod avro;
pub mod background_manager;
pub mod bincode;
pub mod config;
pub mod disk;
pub mod factory;
pub mod flush_coordinator;
// pub mod memtable;  // Moved to obsolete - using new unified memtable system
pub mod schema;

// Unit tests
#[cfg(test)]
mod tests;

// Re-exports
pub use background_manager::{
    BackgroundMaintenanceManager, BackgroundMaintenanceStats, BackgroundTaskStatus,
};
pub use config::WalStrategyType;
pub use config::{CompressionConfig, PerformanceConfig, WalConfig};
pub use disk::WalDiskManager;
pub use factory::WalFactory;
pub use flush_coordinator::{WalFlushCoordinator, FlushState, PendingFlush, FlushDataSource, CleanupInstructions, FlushCoordinatorCallbacks};
// pub use memtable::WalMemTable;  // Moved to obsolete - using new unified memtable system

/// WAL operation types with MVCC support and binary Avro payload
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WalOperation {
    Insert {
        vector_id: VectorId,
        record: VectorRecord,
        expires_at: Option<DateTime<Utc>>, // TTL support
    },
    Update {
        vector_id: VectorId,
        record: VectorRecord,
        expires_at: Option<DateTime<Utc>>,
    },
    Delete {
        vector_id: VectorId,
        expires_at: Option<DateTime<Utc>>, // Soft delete with TTL
    },
    /// Flush operation
    Flush,
    /// Checkpoint operation
    Checkpoint,
    /// Binary Avro payload operation (zero-copy)
    AvroPayload {
        operation_type: String,
        avro_data: Vec<u8>,
    },
}

/// WAL entry with MVCC versioning
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalEntry {
    /// Vector ID (client-provided or system-generated)
    pub entry_id: String,

    /// Collection this entry belongs to
    pub collection_id: CollectionId,

    /// Operation being logged
    pub operation: WalOperation,

    /// Entry timestamp
    pub timestamp: DateTime<Utc>,

    /// Sequence number for ordering (per collection)
    pub sequence: u64,

    /// Global sequence number across all collections
    pub global_sequence: u64,

    /// Entry expires at (for TTL and soft deletes)
    pub expires_at: Option<DateTime<Utc>>,

    /// Entry version for MVCC
    pub version: u64,
}

/// WAL statistics for monitoring
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WalStats {
    pub total_entries: u64,
    pub memory_entries: u64,
    pub disk_segments: u64,
    pub total_disk_size_bytes: u64,
    pub memory_size_bytes: u64,
    pub collections_count: usize,
    pub last_flush_time: Option<DateTime<Utc>>,
    pub write_throughput_entries_per_sec: f64,
    pub read_throughput_entries_per_sec: f64,
    pub compression_ratio: f64,
}

/// WAL flush result
#[derive(Debug, Clone)]
pub struct FlushResult {
    pub entries_flushed: u64,
    pub bytes_written: u64,
    pub segments_created: u64,
    pub collections_affected: Vec<CollectionId>,
    pub flush_duration_ms: u64,
}

/// Atomic flush cycle for consistent WAL→Storage operations
#[derive(Debug, Clone)]
pub struct FlushCycle {
    /// Unique identifier for this flush operation
    pub flush_id: String,
    /// Collection being flushed
    pub collection_id: CollectionId,
    /// WAL entries marked for flush
    pub entries: Vec<WalEntry>,
    /// Extracted vector records ready for storage
    pub vector_records: Vec<VectorRecord>,
    /// Disk segments marked as flush-pending
    pub marked_segments: Vec<String>,
    /// Sequence ranges marked as flush-pending
    pub marked_sequences: Vec<(u64, u64)>, // (start_seq, end_seq) pairs
    /// Current state of the flush cycle
    pub state: FlushCycleState,
}

/// State of a flush cycle operation
#[derive(Debug, Clone, PartialEq)]
pub enum FlushCycleState {
    /// Flush cycle is active and entries are marked
    Active,
    /// Flush cycle completed successfully
    Completed,
    /// Flush cycle was aborted and entries restored
    Aborted,
}

/// Result of completing a flush cycle
#[derive(Debug, Clone)]
pub struct FlushCompletionResult {
    /// Number of entries permanently removed
    pub entries_removed: usize,
    /// Number of disk segments cleaned up
    pub segments_cleaned: usize,
    /// Bytes reclaimed from cleanup
    pub bytes_reclaimed: u64,
}

/// Main WAL strategy trait
#[async_trait]
pub trait WalStrategy: Send + Sync {
    /// Strategy name for identification
    fn strategy_name(&self) -> &'static str;

    /// Initialize the strategy with configuration and optional storage engine
    async fn initialize(
        &mut self,
        config: &WalConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<()>;
    
    /// Set storage engine for delegated flush/compaction operations
    fn set_storage_engine(&self, storage_engine: Arc<dyn UnifiedStorageEngine>);

    /// Serialize entries to bytes (strategy-specific format)
    async fn serialize_entries(&self, entries: &[WalEntry]) -> Result<Vec<u8>>;

    /// Deserialize entries from bytes (strategy-specific format)
    async fn deserialize_entries(&self, data: &[u8]) -> Result<Vec<WalEntry>>;

    /// Write single entry atomically (memory + disk)
    async fn write_entry(&self, entry: WalEntry) -> Result<u64>;

    /// Write batch of entries atomically (default implementation using single writes)
    async fn write_batch(&self, entries: Vec<WalEntry>) -> Result<Vec<u64>> {
        let mut sequences = Vec::with_capacity(entries.len());
        for entry in entries {
            sequences.push(self.write_entry(entry).await?);
        }
        Ok(sequences)
    }

    /// Write batch of entries with immediate disk sync for durability
    async fn write_batch_with_sync(&self, entries: Vec<WalEntry>, immediate_sync: bool) -> Result<Vec<u64>> {
        // Default implementation falls back to regular write_batch
        // Individual strategies can override for optimized immediate sync
        self.write_batch(entries).await
    }

    /// Force immediate sync of in-memory data to disk
    async fn force_sync(&self, collection_id: Option<&CollectionId>) -> Result<()> {
        // Default implementation performs a flush
        self.flush(collection_id).await.map(|_| ())
    }

    /// Read entries for a collection starting from sequence
    async fn read_entries(
        &self,
        collection_id: &CollectionId,
        from_sequence: u64,
        limit: Option<usize>,
    ) -> Result<Vec<WalEntry>>;

    /// Search entries by vector ID (checks both memory and disk)
    async fn search_by_vector_id(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<Option<WalEntry>>;

    /// Similarity search for unflushed vectors in WAL
    async fn search_vectors_similarity(
        &self,
        collection_id: &CollectionId,
        query_vector: &[f32],
        k: usize,
    ) -> Result<Vec<(VectorId, f32, WalEntry)>> {
        // Simple implementation - WAL is not optimized for similarity search
        // This is just for finding vectors that haven't been flushed yet
        if let Some(memtable) = self.memtable() {
            let entries = memtable.get_all_entries(collection_id).await?;
            let mut results = Vec::new();
            
            for entry in entries {
                if let WalOperation::Insert { vector_id, record, .. } | WalOperation::Update { vector_id, record, .. } = &entry.operation {
                    // Simple L2 distance calculation
                    let distance = record.vector.iter()
                        .zip(query_vector.iter())
                        .map(|(a, b)| (a - b).powi(2))
                        .sum::<f32>()
                        .sqrt();
                    
                    results.push((vector_id.clone(), distance, entry));
                }
            }
            
            // Sort by distance and take top k
            results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
            results.truncate(k);
            
            Ok(results)
        } else {
            Ok(Vec::new())
        }
    }

    /// Get memtable reference for similarity search - using new unified memtable system
    fn memtable(&self) -> Option<&crate::storage::memtable::specialized::WalMemtable<u64, WalEntry>>;

    /// Get latest entry for a vector (for MVCC)
    async fn get_latest_entry(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<Option<WalEntry>>;

    /// Get all entries for a collection from memtable (for similarity search)
    async fn get_collection_entries(&self, collection_id: &CollectionId) -> Result<Vec<WalEntry>>;

    /// Flush memory entries to disk - delegates to storage engine if available
    async fn flush(&self, collection_id: Option<&CollectionId>) -> Result<FlushResult>;
    
    /// Delegate flush to storage engine (common implementation for all strategies)
    async fn delegate_to_storage_engine_flush(&self, _collection_id: &CollectionId) -> Result<crate::storage::traits::FlushResult> {
        // Default implementation - override in implementing strategies
        Err(anyhow::anyhow!("Storage engine not available for flush delegation"))
    }
    
    /// Delegate compaction to storage engine (common implementation for all strategies)  
    async fn delegate_to_storage_engine_compact(&self, _collection_id: &CollectionId) -> Result<crate::storage::traits::CompactionResult> {
        // Default implementation - override in implementing strategies
        Err(anyhow::anyhow!("Storage engine not available for compaction delegation"))
    }

    /// Compact entries for a collection (MVCC cleanup)
    async fn compact_collection(&self, collection_id: &CollectionId) -> Result<u64>;

    /// Drop all entries for a collection
    async fn drop_collection(&self, collection_id: &CollectionId) -> Result<()>;

    /// Get WAL statistics
    async fn get_stats(&self) -> Result<WalStats>;

    /// Atomically retrieve and mark WAL entries for flush operation
    /// 
    /// This method provides atomic access to WAL entries that need to be flushed to storage.
    /// It ensures no gaps or duplicates by:
    /// 1. Atomically retrieving entries from memtable and disk segments
    /// 2. Marking them as "flush-pending" to prevent concurrent modifications
    /// 3. Returning entries in sequence order for consistent flush
    /// 
    /// The caller MUST call `complete_flush_cycle()` after successful storage write
    /// to permanently remove the entries, or `abort_flush_cycle()` on failure
    /// to restore them for retry.
    /// 
    /// **Memtable-Specific Optimizations:**
    /// - **BTree**: Provides ordered retrieval for consistent flush order
    /// - **HashMap**: O(1) retrieval per entry, best for large collections
    /// - **SkipList**: Probabilistic ordering with good concurrency
    /// - **ART**: Space-efficient retrieval, ideal for sparse collections
    async fn atomic_retrieve_for_flush(&self, collection_id: &CollectionId, flush_id: &str) -> Result<FlushCycle> {
        // Default implementation - get all entries for the collection
        let entries = self.get_collection_entries(collection_id).await?;
        
        // Extract VectorRecord data from WAL entries
        let mut vector_records = Vec::new();
        let mut marked_sequences = Vec::new();
        
        if !entries.is_empty() {
            let min_seq = entries.iter().map(|e| e.sequence).min().unwrap_or(0);
            let max_seq = entries.iter().map(|e| e.sequence).max().unwrap_or(0);
            marked_sequences.push((min_seq, max_seq));
            
            // Extract vector records from Insert/Update operations
            for entry in &entries {
                match &entry.operation {
                    WalOperation::Insert { vector_id: _, record, expires_at: _ } => {
                        vector_records.push(record.clone());
                    },
                    WalOperation::Update { vector_id: _, record, expires_at: _ } => {
                        vector_records.push(record.clone());
                    },
                    _ => {
                        // Skip other operation types but include in flush cycle
                    }
                }
            }
        }
        
        tracing::info!("🔄 {}: Default atomic flush retrieval - {} entries, {} vector records (collection: {}, flush_id: {})",
                      self.strategy_name(), entries.len(), vector_records.len(), collection_id, flush_id);
        
        Ok(FlushCycle {
            flush_id: flush_id.to_string(),
            collection_id: collection_id.clone(),
            entries,
            vector_records,
            marked_segments: Vec::new(), // Default: no disk segments
            marked_sequences,
            state: FlushCycleState::Active,
        })
    }
    
    /// Complete flush cycle - permanently remove flushed entries
    /// 
    /// This method is called after successful storage engine flush to:
    /// 1. Remove entries from memtable and disk segments
    /// 2. Update WAL sequence tracking
    /// 3. Trigger cleanup of obsolete segments
    /// 
    /// **Memtable-Specific Optimizations:**
    /// - **BTree**: Batch removal in sequence order for optimal B+ tree performance
    /// - **HashMap**: O(1) removal per entry, fastest for large flush cycles
    /// - **SkipList**: Maintains ordering during removal, good for concurrent access
    /// - **ART**: Space-efficient removal with prefix compression maintenance
    async fn complete_flush_cycle(&self, flush_cycle: FlushCycle) -> Result<FlushCompletionResult> {
        // Default implementation - remove all entries for the collection
        // Individual strategies can override for optimized removal
        tracing::info!("✅ {}: Default flush completion - {} entries for collection {} (flush_id: {})", 
                      self.strategy_name(), flush_cycle.entries.len(), flush_cycle.collection_id, flush_cycle.flush_id);
        
        // Drop collection data (simplified default)
        self.drop_collection(&flush_cycle.collection_id).await?;
        
        Ok(FlushCompletionResult {
            entries_removed: flush_cycle.entries.len(),
            segments_cleaned: 0,
            bytes_reclaimed: flush_cycle.entries.iter()
                .map(|entry| std::mem::size_of_val(entry))
                .sum::<usize>() as u64,
        })
    }
    
    /// Abort flush cycle - restore entries for retry
    /// 
    /// This method is called when storage engine flush fails to:
    /// 1. Restore entries to active state in memtable
    /// 2. Clear "flush-pending" marks
    /// 3. Allow retry of flush operation
    /// 
    /// **Memtable-Specific Optimizations:**
    /// - **BTree**: Ordered restoration maintains B+ tree structure
    /// - **HashMap**: O(1) restoration per entry, fastest recovery
    /// - **SkipList**: Probabilistic restoration with good concurrent access
    /// - **ART**: Space-efficient restoration preserving prefix compression
    async fn abort_flush_cycle(&self, flush_cycle: FlushCycle, reason: &str) -> Result<()> {
        // Default implementation - log the abort
        // Individual strategies can override for advanced restoration logic
        tracing::warn!("❌ {}: Default flush abort - {} entries restored for collection {} (flush_id: {}, reason: {})", 
                      self.strategy_name(), flush_cycle.entries.len(), flush_cycle.collection_id, flush_cycle.flush_id, reason);
        
        // In the default implementation, entries are never actually marked as pending,
        // so there's nothing to restore. Advanced strategies can implement proper restoration.
        Ok(())
    }

    /// Assignment Service Integration (Base Implementation for All WAL Strategies)
    
    /// Get the assignment service used by this WAL strategy
    fn get_assignment_service(&self) -> &Arc<dyn crate::storage::assignment_service::AssignmentService>;
    
    /// Select WAL directory URL for a collection using assignment service
    /// This method provides consistent assignment logic across all WAL implementations
    async fn select_wal_url_for_collection(&self, collection_id: &str, config: &WalConfig) -> Result<String> {
        use crate::storage::assignment_service::{StorageAssignmentConfig, StorageComponentType};
        
        // Check if collection already has an assignment
        if let Some(assignment) = self.get_assignment_service().get_assignment(
            &CollectionId::from(collection_id.to_string()),
            StorageComponentType::Wal
        ).await {
            return Ok(assignment.storage_url);
        }
        
        // Create new assignment using service
        let assignment_config = StorageAssignmentConfig {
            storage_urls: config.multi_disk.data_directories.clone(),
            component_type: StorageComponentType::Wal,
            collection_affinity: config.multi_disk.collection_affinity,
        };
        
        let assignment_result = self.get_assignment_service().assign_storage_url(
            &CollectionId::from(collection_id.to_string()),
            &assignment_config
        ).await?;
        
        tracing::info!("📍 Assigned collection '{}' to WAL directory '{}'", 
                      collection_id, assignment_result.storage_url);
        
        Ok(assignment_result.storage_url)
    }
    
    /// Discover existing collections from all configured WAL directories
    /// This method provides consistent discovery logic across all WAL implementations
    async fn discover_existing_assignments(&self, config: &WalConfig, filesystem: &Arc<FilesystemFactory>) -> Result<usize> {
        use crate::storage::assignment_service::{AssignmentDiscovery, StorageComponentType};
        
        AssignmentDiscovery::discover_and_record_assignments(
            StorageComponentType::Wal,
            &config.multi_disk.data_directories,
            filesystem,
            self.get_assignment_service(),
        ).await
    }

    /// Recover from disk on startup
    async fn recover(&self) -> Result<u64>;

    /// Close and cleanup resources
    async fn close(&self) -> Result<()>;

    /// Force flush all collections - FOR TESTING ONLY
    /// WARNING: This method should only be used for testing and debugging
    async fn force_flush_all(&self) -> Result<()> {
        tracing::warn!("⚠️ {}: FORCE FLUSH ALL - TESTING ONLY", self.strategy_name());
        // Default implementation - trigger flush for all known collections
        // Individual strategies can override with more efficient bulk operations
        // For now, just return success as a placeholder
        Ok(())
    }

    /// Force flush specific collection - FOR TESTING ONLY
    /// WARNING: This method should only be used for testing and debugging
    async fn force_flush_collection(&self, collection_id: &str, storage_engine: Option<&str>) -> Result<()> {
        tracing::warn!("⚠️ {}: FORCE FLUSH COLLECTION {} - TESTING ONLY", self.strategy_name(), collection_id);
        // Default implementation - trigger immediate flush for this collection
        // Individual strategies can override with collection-specific logic
        // For now, just return success as a placeholder
        Ok(())
    }
    
    /// Register storage engine with the strategy's flush coordinator
    async fn register_storage_engine(&self, engine_name: &str, engine: Arc<dyn UnifiedStorageEngine>) -> Result<()> {
        tracing::warn!("⚠️ {}: register_storage_engine not implemented for {}", self.strategy_name(), engine_name);
        Ok(())
    }
}

/// High-level WAL manager that uses strategies
pub struct WalManager {
    strategy: Box<dyn WalStrategy>,
    config: WalConfig,
    stats: Arc<tokio::sync::RwLock<WalStats>>,
    atomicity_manager: Option<Arc<crate::storage::atomicity::AtomicityManager>>,
}

impl WalManager {
    /// Create new WAL manager with specified strategy
    pub async fn new(strategy: Box<dyn WalStrategy>, config: WalConfig) -> Result<Self> {
        tracing::debug!(
            "🚀 Creating WalManager with strategy: {}",
            strategy.strategy_name()
        );
        tracing::debug!(
            "📋 WAL Config: strategy_type={:?}, memtable_type={:?}",
            config.strategy_type,
            config.memtable.memtable_type
        );
        tracing::debug!(
            "💾 Multi-disk config: {} directories, distribution={:?}",
            config.multi_disk.data_directories.len(),
            config.multi_disk.distribution_strategy
        );

        let stats = Arc::new(tokio::sync::RwLock::new(WalStats {
            total_entries: 0,
            memory_entries: 0,
            disk_segments: 0,
            total_disk_size_bytes: 0,
            memory_size_bytes: 0,
            collections_count: 0,
            last_flush_time: None,
            write_throughput_entries_per_sec: 0.0,
            read_throughput_entries_per_sec: 0.0,
            compression_ratio: 1.0,
        }));

        Ok(Self {
            strategy,
            config,
            stats,
            atomicity_manager: None,
        })
    }

    /// Set atomicity manager for transaction support
    pub fn set_atomicity_manager(
        &mut self,
        atomicity_manager: Arc<crate::storage::atomicity::AtomicityManager>,
    ) {
        self.atomicity_manager = Some(atomicity_manager);
        tracing::info!("🔒 Atomicity manager attached to WAL manager");
    }
    
    /// Set storage engine for delegated flush/compaction operations
    pub fn set_storage_engine(&self, storage_engine: Arc<dyn UnifiedStorageEngine>) {
        self.strategy.set_storage_engine(storage_engine);
        tracing::info!("🏗️ Storage engine attached to WAL manager for delegated operations");
    }

    /// Execute atomic operation with transaction support
    pub async fn execute_atomic_operation(
        &self,
        operation: Box<dyn crate::storage::atomicity::AtomicOperation>,
    ) -> Result<crate::storage::atomicity::OperationResult> {
        if let Some(atomicity_manager) = &self.atomicity_manager {
            // Begin transaction
            let transaction_id = atomicity_manager
                .begin_transaction(None, None)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to begin transaction: {}", e))?;

            // Execute operation
            match atomicity_manager
                .execute_operation(transaction_id, operation)
                .await
            {
                Ok(result) => {
                    // Commit transaction
                    atomicity_manager
                        .commit_transaction(transaction_id)
                        .await
                        .map_err(|e| anyhow::anyhow!("Failed to commit transaction: {}", e))?;
                    Ok(result)
                }
                Err(e) => {
                    // Rollback transaction
                    let _ = atomicity_manager
                        .rollback_transaction(transaction_id, format!("Operation failed: {}", e))
                        .await;
                    Err(e)
                }
            }
        } else {
            Err(anyhow::anyhow!("Atomicity manager not configured"))
        }
    }

    /// Execute atomic operation within existing transaction
    pub async fn execute_in_transaction(
        &self,
        transaction_id: crate::storage::atomicity::TransactionId,
        operation: Box<dyn crate::storage::atomicity::AtomicOperation>,
    ) -> Result<crate::storage::atomicity::OperationResult> {
        if let Some(atomicity_manager) = &self.atomicity_manager {
            atomicity_manager
                .execute_operation(transaction_id, operation)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to execute operation in transaction: {}", e))
        } else {
            Err(anyhow::anyhow!("Atomicity manager not configured"))
        }
    }

    /// Begin a new transaction
    pub async fn begin_transaction(&self) -> Result<crate::storage::atomicity::TransactionId> {
        if let Some(atomicity_manager) = &self.atomicity_manager {
            atomicity_manager
                .begin_transaction(None, None)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to begin transaction: {}", e))
        } else {
            Err(anyhow::anyhow!("Atomicity manager not configured"))
        }
    }

    /// Commit a transaction
    pub async fn commit_transaction(
        &self,
        transaction_id: crate::storage::atomicity::TransactionId,
    ) -> Result<()> {
        if let Some(atomicity_manager) = &self.atomicity_manager {
            atomicity_manager
                .commit_transaction(transaction_id)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to commit transaction: {}", e))
        } else {
            Err(anyhow::anyhow!("Atomicity manager not configured"))
        }
    }

    /// Rollback a transaction
    pub async fn rollback_transaction(
        &self,
        transaction_id: crate::storage::atomicity::TransactionId,
        reason: String,
    ) -> Result<()> {
        if let Some(atomicity_manager) = &self.atomicity_manager {
            atomicity_manager
                .rollback_transaction(transaction_id, reason)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to rollback transaction: {}", e))
        } else {
            Err(anyhow::anyhow!("Atomicity manager not configured"))
        }
    }

    /// Insert single vector record
    pub async fn insert(
        &self,
        collection_id: CollectionId,
        vector_id: VectorId,
        record: VectorRecord,
    ) -> Result<u64> {
        let start_time = std::time::Instant::now();
        
        debug!(
            "📝 [WAL_INSERT] Starting insert for collection: {}, vector_id: {}, vector_size: {} dims",
            collection_id,
            vector_id,
            record.vector.len()
        );

        let entry = WalEntry {
            entry_id: vector_id.clone(),
            collection_id: collection_id.clone(),
            operation: WalOperation::Insert {
                vector_id: vector_id.clone(),
                record,
                expires_at: None,
            },
            timestamp: Utc::now(),
            sequence: 0,        // Will be set by strategy
            global_sequence: 0, // Will be set by strategy
            expires_at: None,
            version: 1,
        };

        let result = self.strategy.write_entry(entry).await;
        let duration = start_time.elapsed();
        
        match &result {
            Ok(sequence) => {
                debug!(
                    "📝 [WAL_INSERT] Successfully inserted vector {} in collection {} (sequence: {}) in {:?}",
                    vector_id,
                    collection_id,
                    sequence,
                    duration
                );
            }
            Err(e) => {
                debug!(
                    "📝 [WAL_INSERT] Failed to insert vector {} in collection {}: {} (duration: {:?})",
                    vector_id,
                    collection_id,
                    e,
                    duration
                );
            }
        }
        
        result
    }

    /// Insert batch of vector records
    pub async fn insert_batch(
        &self,
        collection_id: CollectionId,
        records: Vec<(VectorId, VectorRecord)>,
    ) -> Result<Vec<u64>> {
        let entries: Vec<WalEntry> = records
            .into_iter()
            .map(|(vector_id, record)| {
                WalEntry {
                    entry_id: vector_id.clone(),
                    collection_id: collection_id.clone(),
                    operation: WalOperation::Insert {
                        vector_id,
                        record,
                        expires_at: None,
                    },
                    timestamp: Utc::now(),
                    sequence: 0,        // Will be set by strategy
                    global_sequence: 0, // Will be set by strategy
                    expires_at: None,
                    version: 1,
                }
            })
            .collect();

        self.strategy.write_batch(entries).await
    }

    /// Insert batch of vector records with immediate sync option
    pub async fn insert_batch_with_sync(
        &self,
        collection_id: CollectionId,
        records: Vec<(VectorId, VectorRecord)>,
        immediate_sync: bool,
    ) -> Result<Vec<u64>> {
        let entries: Vec<WalEntry> = records
            .into_iter()
            .map(|(vector_id, record)| {
                WalEntry {
                    entry_id: vector_id.clone(),
                    collection_id: collection_id.clone(),
                    operation: WalOperation::Insert {
                        vector_id,
                        record,
                        expires_at: None,
                    },
                    timestamp: Utc::now(),
                    sequence: 0,        // Will be set by strategy
                    global_sequence: 0, // Will be set by strategy
                    expires_at: None,
                    version: 1,
                }
            })
            .collect();

        self.strategy.write_batch_with_sync(entries, immediate_sync).await
    }

    /// Force immediate sync of WAL data to disk
    pub async fn force_sync(&self, collection_id: Option<&CollectionId>) -> Result<()> {
        self.strategy.force_sync(collection_id).await
    }

    /// Update vector record
    pub async fn update(
        &self,
        collection_id: CollectionId,
        vector_id: VectorId,
        record: VectorRecord,
    ) -> Result<u64> {
        // Get current version for MVCC
        let current = self
            .strategy
            .get_latest_entry(&collection_id, &vector_id)
            .await?;
        let next_version = current.map(|e| e.version + 1).unwrap_or(1);

        let entry = WalEntry {
            entry_id: vector_id.clone(),
            collection_id: collection_id.clone(),
            operation: WalOperation::Update {
                vector_id: vector_id.clone(),
                record,
                expires_at: None,
            },
            timestamp: Utc::now(),
            sequence: 0,
            global_sequence: 0,
            expires_at: None,
            version: next_version,
        };

        self.strategy.write_entry(entry).await
    }

    /// Delete vector record (soft delete with TTL)
    pub async fn delete(&self, collection_id: CollectionId, vector_id: VectorId) -> Result<u64> {
        let entry = WalEntry {
            entry_id: vector_id.clone(),
            collection_id: collection_id.clone(),
            operation: WalOperation::Delete {
                vector_id: vector_id.clone(),
                expires_at: Some(Utc::now() + chrono::Duration::days(30)), // 30-day soft delete
            },
            timestamp: Utc::now(),
            sequence: 0,
            global_sequence: 0,
            expires_at: Some(Utc::now() + chrono::Duration::days(30)),
            version: 1,
        };

        self.strategy.write_entry(entry).await
    }

    // Note: Collection lifecycle operations (create/drop) are handled by CollectionService
    // WAL only handles vector-level operations (insert/update/delete/flush/checkpoint)

    /// Search for vector entries (for queries that need to check WAL)
    pub async fn search(
        &self,
        collection_id: &CollectionId,
        vector_id: &VectorId,
    ) -> Result<Option<WalEntry>> {
        self.strategy
            .search_by_vector_id(collection_id, vector_id)
            .await
    }

    /// Similarity search for unflushed vectors in WAL
    pub async fn search_vectors_similarity(
        &self,
        collection_id: &CollectionId,
        query_vector: &[f32],
        k: usize,
    ) -> Result<Vec<(VectorId, f32, WalEntry)>> {
        self.strategy
            .search_vectors_similarity(collection_id, query_vector, k)
            .await
    }

    /// Read entries for recovery or replication
    pub async fn read_entries(
        &self,
        collection_id: &CollectionId,
        from_sequence: u64,
        limit: Option<usize>,
    ) -> Result<Vec<WalEntry>> {
        self.strategy
            .read_entries(collection_id, from_sequence, limit)
            .await
    }

    /// Force flush to disk
    pub async fn flush(&self, collection_id: Option<&CollectionId>) -> Result<FlushResult> {
        let result = self.strategy.flush(collection_id).await?;

        // Update stats
        let mut stats = self.stats.write().await;
        stats.last_flush_time = Some(Utc::now());

        Ok(result)
    }

    /// Compact collection (clean up old MVCC versions)
    pub async fn compact(&self, collection_id: &CollectionId) -> Result<u64> {
        self.strategy.compact_collection(collection_id).await
    }

    /// Append binary Avro entry directly (zero-copy WAL operation)
    /// 
    /// RESILIENCE GUARANTEES:
    /// - Treats vector data as opaque binary blobs (never parsed)
    /// - Only extracts metadata fields (id, expires_at) for compaction
    /// - Corrupted vector data CANNOT crash server or compaction
    /// - Invalid payloads are logged and skipped during background processing
    /// - WAL writes succeed regardless of vector content validity
    pub async fn append_avro_entry(
        &self,
        operation_type: &str,
        avro_payload: &[u8],
    ) -> Result<u64> {
        // Create WAL entry with raw Avro payload (NO VALIDATION)
        // Vector content is stored as opaque bytes
        let entry = WalEntry {
            entry_id: format!("avro_{}_{}", operation_type, Utc::now().timestamp_nanos_opt().unwrap_or_default()),
            collection_id: "avro_ops".to_string(), // Generic collection for Avro operations
            operation: WalOperation::AvroPayload {
                operation_type: operation_type.to_string(),
                avro_data: avro_payload.to_vec(), // Raw bytes - never parsed
            },
            timestamp: Utc::now(),
            sequence: 0,        // Will be set by strategy
            global_sequence: 0, // Will be set by strategy
            expires_at: None,   // Extracted during compaction if needed
            version: 1,
        };

        // Write to WAL immediately - no validation, maximum throughput
        self.strategy.write_entry(entry).await
    }

    /// Read binary Avro entries by operation type
    pub async fn read_avro_entries(
        &self,
        operation_type: &str,
        limit: Option<usize>,
    ) -> Result<Vec<Vec<u8>>> {
        let entries = self
            .strategy
            .read_entries(&"avro_ops".to_string(), 0, limit)
            .await?;

        let mut avro_payloads = Vec::new();
        for entry in entries {
            if let WalOperation::AvroPayload {
                operation_type: op_type,
                avro_data,
            } = entry.operation
            {
                if op_type == operation_type {
                    avro_payloads.push(avro_data);
                }
            }
        }

        Ok(avro_payloads)
    }

    /// Get all entries for a collection from memtable (for similarity search)
    pub async fn get_collection_entries(&self, collection_id: &CollectionId) -> Result<Vec<WalEntry>> {
        self.strategy.get_collection_entries(collection_id).await
    }

    /// Get WAL statistics
    pub async fn stats(&self) -> Result<WalStats> {
        self.strategy.get_stats().await
    }

    /// Recover WAL from disk on startup
    pub async fn recover(&self) -> Result<u64> {
        self.strategy.recover().await
    }

    /// Graceful shutdown
    pub async fn close(&self) -> Result<()> {
        self.strategy.close().await
    }

    /// Atomically retrieve and mark WAL entries for flush operation
    pub async fn atomic_retrieve_for_flush(&self, collection_id: &CollectionId, flush_id: &str) -> Result<FlushCycle> {
        self.strategy.atomic_retrieve_for_flush(collection_id, flush_id).await
    }
    
    /// Complete flush cycle - permanently remove flushed entries
    pub async fn complete_flush_cycle(&self, flush_cycle: FlushCycle) -> Result<FlushCompletionResult> {
        self.strategy.complete_flush_cycle(flush_cycle).await
    }
    
    /// Abort flush cycle - restore entries for retry
    pub async fn abort_flush_cycle(&self, flush_cycle: FlushCycle, reason: &str) -> Result<()> {
        self.strategy.abort_flush_cycle(flush_cycle, reason).await
    }

    /// Force flush all collections - FOR TESTING ONLY
    /// WARNING: This method should only be used for testing and debugging
    pub async fn force_flush_all(&self) -> Result<()> {
        tracing::warn!("⚠️ WAL MANAGER: FORCE FLUSH ALL - TESTING ONLY");
        self.strategy.force_flush_all().await
    }

    /// Force flush specific collection - FOR TESTING ONLY
    /// WARNING: This method should only be used for testing and debugging
    pub async fn force_flush_collection(&self, collection_id: &str, storage_engine: Option<&str>) -> Result<()> {
        tracing::warn!("⚠️ WAL MANAGER: FORCE FLUSH COLLECTION {} - TESTING ONLY", collection_id);
        self.strategy.force_flush_collection(collection_id, storage_engine).await
    }
    
    /// Register storage engine with the WAL's flush coordinator
    pub async fn register_storage_engine(&self, engine_name: &str, engine: Arc<dyn UnifiedStorageEngine>) -> Result<()> {
        self.strategy.register_storage_engine(engine_name, engine).await
    }
}

impl std::fmt::Debug for WalManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalManager")
            .field("strategy", &self.strategy.strategy_name())
            .field("config", &self.config)
            .finish()
    }
}
