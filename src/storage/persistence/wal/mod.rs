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

use crate::core::{CollectionId, VectorId, VectorRecord};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::traits::{UnifiedStorageEngine, FlushParameters, CompactionParameters};

// Sub-modules
pub mod avro;
pub mod background_manager;
pub mod bincode;
pub mod config;
pub mod disk;
pub mod factory;
pub mod memtable;
// Note: age_monitor removed - using size-based flush on write operations only

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
pub use memtable::WalMemTable;

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
    CreateCollection {
        collection_id: CollectionId,
        config: serde_json::Value,
    },
    DropCollection {
        collection_id: CollectionId,
    },
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    fn set_storage_engine(&mut self, storage_engine: Arc<dyn UnifiedStorageEngine>);

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
    async fn delegate_to_storage_engine_flush(&self, collection_id: &CollectionId) -> Result<crate::storage::traits::FlushResult> {
        // Default implementation - override in implementing strategies
        Err(anyhow::anyhow!("Storage engine not available for flush delegation"))
    }
    
    /// Delegate compaction to storage engine (common implementation for all strategies)  
    async fn delegate_to_storage_engine_compact(&self, collection_id: &CollectionId) -> Result<crate::storage::traits::CompactionResult> {
        // Default implementation - override in implementing strategies
        Err(anyhow::anyhow!("Storage engine not available for compaction delegation"))
    }

    /// Compact entries for a collection (MVCC cleanup)
    async fn compact_collection(&self, collection_id: &CollectionId) -> Result<u64>;

    /// Drop all entries for a collection
    async fn drop_collection(&self, collection_id: &CollectionId) -> Result<()>;

    /// Get WAL statistics
    async fn get_stats(&self) -> Result<WalStats>;

    /// Recover from disk on startup
    async fn recover(&self) -> Result<u64>;

    /// Close and cleanup resources
    async fn close(&self) -> Result<()>;
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
    pub fn set_storage_engine(&mut self, storage_engine: Arc<dyn UnifiedStorageEngine>) {
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

        self.strategy.write_entry(entry).await
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

    /// Create collection
    pub async fn create_collection(
        &self,
        collection_id: CollectionId,
        config: serde_json::Value,
    ) -> Result<u64> {
        let entry = WalEntry {
            entry_id: collection_id.clone(),
            collection_id: collection_id.clone(),
            operation: WalOperation::CreateCollection {
                collection_id: collection_id.clone(),
                config,
            },
            timestamp: Utc::now(),
            sequence: 0,
            global_sequence: 0,
            expires_at: None,
            version: 1,
        };

        self.strategy.write_entry(entry).await
    }

    /// Drop collection and all its data
    pub async fn drop_collection(&self, collection_id: &CollectionId) -> Result<()> {
        // Record the drop operation
        let entry = WalEntry {
            entry_id: collection_id.clone(),
            collection_id: collection_id.clone(),
            operation: WalOperation::DropCollection {
                collection_id: collection_id.clone(),
            },
            timestamp: Utc::now(),
            sequence: 0,
            global_sequence: 0,
            expires_at: None,
            version: 1,
        };

        let _ = self.strategy.write_entry(entry).await?;

        // Actually drop the collection data
        self.strategy.drop_collection(collection_id).await
    }

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
}

impl std::fmt::Debug for WalManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WalManager")
            .field("strategy", &self.strategy.strategy_name())
            .field("config", &self.config)
            .finish()
    }
}
