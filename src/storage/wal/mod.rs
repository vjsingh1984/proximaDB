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

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;
use anyhow::Result;

use crate::core::{VectorRecord, VectorId, CollectionId};
use crate::storage::filesystem::FilesystemFactory;

// Sub-modules
pub mod disk;
pub mod factory;
pub mod config;
pub mod memtable;
pub mod avro;
pub mod bincode;
pub mod age_monitor;

// Re-exports
pub use disk::WalDiskManager;
pub use factory::WalFactory;
pub use config::WalStrategyType;
pub use config::{WalConfig, CompressionConfig, PerformanceConfig};
pub use age_monitor::{WalAgeMonitor, AgeMonitorStats, CollectionAgeInfo};
pub use memtable::WalMemTable;

/// WAL operation types with MVCC support
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
    
    /// Initialize the strategy with configuration
    async fn initialize(&mut self, config: &WalConfig, filesystem: Arc<FilesystemFactory>) -> Result<()>;
    
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
    
    /// Read entries for a collection starting from sequence
    async fn read_entries(&self, collection_id: &CollectionId, from_sequence: u64, limit: Option<usize>) -> Result<Vec<WalEntry>>;
    
    /// Search entries by vector ID (checks both memory and disk)
    async fn search_by_vector_id(&self, collection_id: &CollectionId, vector_id: &VectorId) -> Result<Option<WalEntry>>;
    
    /// Get latest entry for a vector (for MVCC)
    async fn get_latest_entry(&self, collection_id: &CollectionId, vector_id: &VectorId) -> Result<Option<WalEntry>>;
    
    /// Flush memory entries to disk
    async fn flush(&self, collection_id: Option<&CollectionId>) -> Result<FlushResult>;
    
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
}

impl WalManager {
    /// Create new WAL manager with specified strategy
    pub async fn new(strategy: Box<dyn WalStrategy>, config: WalConfig) -> Result<Self> {
        tracing::debug!("🚀 Creating WalManager with strategy: {}", strategy.strategy_name());
        tracing::debug!("📋 WAL Config: strategy_type={:?}, memtable_type={:?}", 
                       config.strategy_type, config.memtable.memtable_type);
        tracing::debug!("💾 Multi-disk config: {} directories, distribution={:?}",
                       config.multi_disk.data_directories.len(), 
                       config.multi_disk.distribution_strategy);
        
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
        })
    }
    
    /// Insert single vector record
    pub async fn insert(&self, collection_id: CollectionId, vector_id: VectorId, record: VectorRecord) -> Result<u64> {
        let entry = WalEntry {
            entry_id: vector_id.clone(),
            collection_id: collection_id.clone(),
            operation: WalOperation::Insert {
                vector_id: vector_id.clone(),
                record,
                expires_at: None,
            },
            timestamp: Utc::now(),
            sequence: 0, // Will be set by strategy
            global_sequence: 0, // Will be set by strategy
            expires_at: None,
            version: 1,
        };
        
        self.strategy.write_entry(entry).await
    }
    
    /// Insert batch of vector records
    pub async fn insert_batch(&self, collection_id: CollectionId, records: Vec<(VectorId, VectorRecord)>) -> Result<Vec<u64>> {
        let entries: Vec<WalEntry> = records.into_iter().map(|(vector_id, record)| {
            WalEntry {
                entry_id: vector_id.clone(),
                collection_id: collection_id.clone(),
                operation: WalOperation::Insert {
                    vector_id,
                    record,
                    expires_at: None,
                },
                timestamp: Utc::now(),
                sequence: 0, // Will be set by strategy
                global_sequence: 0, // Will be set by strategy
                expires_at: None,
                version: 1,
            }
        }).collect();
        
        self.strategy.write_batch(entries).await
    }
    
    /// Update vector record
    pub async fn update(&self, collection_id: CollectionId, vector_id: VectorId, record: VectorRecord) -> Result<u64> {
        // Get current version for MVCC
        let current = self.strategy.get_latest_entry(&collection_id, &vector_id).await?;
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
    pub async fn create_collection(&self, collection_id: CollectionId, config: serde_json::Value) -> Result<u64> {
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
    pub async fn search(&self, collection_id: &CollectionId, vector_id: &VectorId) -> Result<Option<WalEntry>> {
        self.strategy.search_by_vector_id(collection_id, vector_id).await
    }
    
    /// Read entries for recovery or replication
    pub async fn read_entries(&self, collection_id: &CollectionId, from_sequence: u64, limit: Option<usize>) -> Result<Vec<WalEntry>> {
        self.strategy.read_entries(collection_id, from_sequence, limit).await
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