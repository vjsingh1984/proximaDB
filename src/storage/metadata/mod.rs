// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! # Metadata Storage System
//!
//! This module provides ProximaDB's robust metadata storage infrastructure that
//! manages collection configurations, statistics, and system metadata with
//! ACID guarantees and cloud-native storage support.
//!
//! ## Architecture Overview
//!
//! ```text
//! ┌──────────────────────────────────────────────┐
//! │            API Layer (CRUD Operations)        │
//! ├──────────────────────────────────────────────┤
//! │         Atomic Operations (MVCC)              │
//! ├──────────────────────────────────────────────┤
//! │    WAL (Write-Ahead Log) │ B+Tree MemTable   │
//! ├──────────────────────────────────────────────┤
//! │         Unified Storage Backend               │
//! │    (Local FS / S3 / Azure / GCS)             │
//! └──────────────────────────────────────────────┘
//! ```
//!
//! ## Key Features
//!
//! ### 1. **ACID Guarantees**
//! - **Atomicity**: All-or-nothing operations via WAL
//! - **Consistency**: Schema validation and invariant checks
//! - **Isolation**: MVCC (Multi-Version Concurrency Control)
//! - **Durability**: WAL ensures crash recovery
//!
//! ### 2. **Schema Evolution**
//! - Avro format for backward/forward compatibility
//! - Versioned metadata with migration support
//! - Zero-downtime schema updates
//!
//! ### 3. **Cloud-Native Storage**
//! - Unified abstraction over storage backends
//! - Support for S3, Azure Blob, Google Cloud Storage
//! - Compute-storage separation for serverless
//! - Automatic retry and failover
//!
//! ### 4. **Performance Optimizations**
//! - B+Tree memtable for sorted range queries
//! - Checkpoint mechanism to compact WAL
//! - Batch operations for efficiency
//! - Async I/O throughout
//!
//! ## Design Principles
//!
//! 1. **Write-Ahead Logging**: Every mutation goes through WAL first
//! 2. **Eventual Consistency**: Async checkpoint to persistent storage
//! 3. **Schema-First**: All metadata has defined Avro schemas
//! 4. **Cloud-First**: Designed for object storage semantics
//! 5. **Recovery-Oriented**: Built-in crash recovery and repair
//!
//! ## Module Organization
//!
//! - **`atomic/`**: MVCC transactions and isolation levels
//! - **`backends/`**: Storage backend implementations (S3, Azure, GCS)
//! - **`checkpoint/`**: WAL compaction and checkpointing
//! - **`indexes/`**: Secondary indexes for metadata queries
//! - **`store/`**: Main metadata store implementation
//! - **`write_ahead_log/`**: WAL for durability

pub use crate::storage::transaction_coordinator;
pub mod atomic;
pub mod backends;
pub mod checkpoint;
// universal_backend moved to backends/universal_backend.rs
pub mod indexes;
pub mod single_index;
pub mod store;
pub mod unified_index;
pub mod write_ahead_log;

#[cfg(test)]
mod atomic_tests;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// Re-exports
pub use store::{MetadataStore, MetadataStoreConfig};
pub use transaction_coordinator::TransactionId;
pub use write_ahead_log::{
    MetadataWALConfig, MetadataWriteAheadLog, SystemMetadata, VersionedCollectionMetadata,
};

// No conversion implementations needed - StorageEngineType is now a type alias for proto enum

/// Operations that can be performed on metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MetadataOperation {
    CreateCollection(crate::proto::proximadb_v1::Collection),
    UpdateCollection {
        collection_id: String,
        metadata: crate::proto::proximadb_v1::Collection,
    },
    DeleteCollection(String),
    UpdateStats {
        collection_id: String,
        vector_delta: i64,
        size_delta: i64,
    },
    UpdateAccessPattern {
        collection_id: String,
        access_pattern: String,
    },
    UpdateTags {
        collection_id: String,
        tags: Vec<String>,
    },
    UpdateRetentionPolicy {
        collection_id: String,
        retention_policy: String,
    },
    UpdateSystemMetadata(SystemMetadata),
}

/// Metadata query filters
pub struct MetadataFilter {
    /// Filter by access pattern
    pub access_pattern: Option<String>,

    /// Filter by tags (AND operation)
    pub tags: Vec<String>,

    /// Filter by owner
    pub owner: Option<String>,

    /// Filter by minimum vector count
    pub min_vector_count: Option<u64>,

    /// Filter by maximum age in days
    pub max_age_days: Option<u32>,

    /// Custom filter function
    pub custom_filter:
        Option<Box<dyn Fn(&crate::proto::proximadb_v1::Collection) -> bool + Send + Sync>>,
}

impl std::fmt::Debug for MetadataFilter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetadataFilter")
            .field("access_pattern", &self.access_pattern)
            .field("tags", &self.tags)
            .field("owner", &self.owner)
            .field("min_vector_count", &self.min_vector_count)
            .field("max_age_days", &self.max_age_days)
            .field("custom_filter", &self.custom_filter.is_some())
            .finish()
    }
}

impl Clone for MetadataFilter {
    fn clone(&self) -> Self {
        Self {
            access_pattern: self.access_pattern.clone(),
            tags: self.tags.clone(),
            owner: self.owner.clone(),
            min_vector_count: self.min_vector_count,
            max_age_days: self.max_age_days,
            custom_filter: None, // Function trait objects can't be cloned
        }
    }
}

impl Default for MetadataFilter {
    fn default() -> Self {
        Self {
            access_pattern: None,
            tags: Vec::new(),
            owner: None,
            min_vector_count: None,
            max_age_days: None,
            custom_filter: None,
        }
    }
}

/// Interface for metadata storage operations
#[async_trait]
pub trait MetadataStoreInterface: Send + Sync {
    /// Create a new collection
    async fn create_collection(&self, metadata: crate::proto::proximadb_v1::Collection) -> Result<()>;

    /// Get collection metadata by ID
    async fn get_collection(
        &self,
        collection_id: &str,
    ) -> Result<Option<crate::proto::proximadb_v1::Collection>>;

    /// Update collection metadata
    async fn update_collection(
        &self,
        collection_id: &str,
        metadata: crate::proto::proximadb_v1::Collection,
    ) -> Result<()>;

    /// Delete collection metadata
    async fn delete_collection(&self, collection_id: &str) -> Result<bool>;

    /// List collections with filtering
    async fn list_collections(
        &self,
        filter: Option<MetadataFilter>,
    ) -> Result<Vec<crate::proto::proximadb_v1::Collection>>;

    /// Update collection statistics atomically
    async fn update_stats(
        &self,
        collection_id: &str,
        vector_delta: i64,
        size_delta: i64,
    ) -> Result<()>;

    /// Batch operations (atomic if supported)
    async fn batch_operations(&self, operations: Vec<MetadataOperation>) -> Result<()>;

    /// Begin transaction (if supported)
    async fn begin_transaction(&self) -> Result<Option<String>>;

    /// Commit transaction (if supported)
    async fn commit_transaction(&self, transaction_id: &str) -> Result<()>;

    /// Rollback transaction (if supported)
    async fn rollback_transaction(&self, transaction_id: &str) -> Result<()>;

    /// Get system metadata
    async fn get_system_metadata(&self) -> Result<SystemMetadata>;

    /// Update system metadata
    async fn update_system_metadata(&self, metadata: SystemMetadata) -> Result<()>;

    /// Health check
    async fn health_check(&self) -> Result<bool>;

    /// Get metadata storage statistics
    async fn get_stats(&self) -> Result<MetadataStorageStats>;

    /// Backup metadata
    async fn backup(&self, location: &str) -> Result<String>; // Returns backup ID

    /// Restore from backup
    async fn restore(&self, backup_id: &str, location: &str) -> Result<()>;

    /// Close/cleanup metadata store
    async fn close(&self) -> Result<()>;
}

/// Storage statistics for monitoring
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataStorageStats {
    pub total_collections: u64,
    pub total_metadata_size_bytes: u64,
    pub cache_hit_rate: f64,
    pub avg_operation_latency_ms: f64,
    pub storage_backend: String,
    pub last_backup_time: Option<DateTime<Utc>>,
    pub wal_entries: u64,
    pub wal_size_bytes: u64,
}
