// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Metadata Storage System with WAL backing and filesystem abstraction
//!
//! This module provides a robust metadata storage system that:
//! - Uses Avro WAL for durability and schema evolution
//! - Supports atomic operations with MVCC
//! - Uses B+Tree memtable for sorted access patterns
//! - Abstracts storage backends (file:, s3:, adls:, gcs:)
//! - Enables compute-storage separation for serverless deployment

pub mod atomic;
pub mod backends;
pub mod compaction;
// filestore_backend moved to backends/filestore_backend.rs
pub mod indexes;
pub mod single_index;
pub mod store;
pub mod unified_index;
pub mod wal;

use crate::core::CollectionId;
use crate::storage::strategy::CollectionStrategyConfig;
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Re-exports
pub use atomic::{AtomicMetadataStore, MetadataTransaction, TransactionId};
pub use store::{MetadataStore, MetadataStoreConfig};
pub use wal::{MetadataWalConfig, MetadataWalManager, SystemMetadata, VersionedCollectionMetadata};

/// Collection metadata with comprehensive information and migration support
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionMetadata {
    // Core identification
    pub id: CollectionId,
    pub name: String,

    // Vector configuration
    pub dimension: usize,
    pub distance_metric: String,
    pub indexing_algorithm: String,

    // Timestamps
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,

    // Statistics
    pub vector_count: u64,
    pub total_size_bytes: u64,

    // Configuration and user-defined metadata
    pub config: HashMap<String, serde_json::Value>,

    // Access patterns for optimization
    pub access_pattern: AccessPattern,

    // Retention and lifecycle management
    pub retention_policy: Option<RetentionPolicy>,

    // Tags for organization
    pub tags: Vec<String>,

    // Ownership and permissions
    pub owner: Option<String>,
    pub description: Option<String>,

    // Strategy configuration for storage, indexing, and search
    pub strategy_config: CollectionStrategyConfig,

    // Strategy change tracking
    pub strategy_change_history: Vec<StrategyChangeStatus>,

    // WAL flush configuration (None = use global defaults)
    pub flush_config: Option<CollectionFlushConfig>,
}

/// Access pattern hints for storage optimization
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AccessPattern {
    /// Very frequent access - keep in memory, replicate
    Hot,
    /// Normal access pattern - standard caching
    Normal,
    /// Infrequent access - can be moved to slower storage
    Cold,
    /// Very rare access - archive to cheapest storage
    Archive,
}

impl Default for AccessPattern {
    fn default() -> Self {
        Self::Normal
    }
}

/// Data retention and lifecycle policy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// Days to retain data
    pub retain_days: u32,

    /// Automatically move to archive storage
    pub auto_archive: bool,

    /// Automatically delete after retention period
    pub auto_delete: bool,

    /// Move to cold storage after days
    pub cold_storage_days: Option<u32>,

    /// Backup configuration
    pub backup_config: Option<BackupConfig>,
}

/// Backup configuration for collections
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupConfig {
    /// Enable automatic backups
    pub enabled: bool,

    /// Backup frequency in hours
    pub frequency_hours: u32,

    /// Number of backups to retain
    pub retain_count: u32,

    /// Storage location for backups
    pub backup_location: String, // filesystem URL
}

/// Strategy change tracking (since migration is just metadata update)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyChangeStatus {
    /// Change ID for tracking
    pub change_id: String,

    /// Timestamp of strategy change
    pub changed_at: DateTime<Utc>,

    /// Previous strategy configuration
    pub previous_strategy: CollectionStrategyConfig,

    /// Current strategy configuration
    pub current_strategy: CollectionStrategyConfig,

    /// What was changed (storage, indexing, search, or combination)
    pub change_type: StrategyChangeType,

    /// User who initiated the change
    pub changed_by: Option<String>,

    /// Reason for the change
    pub change_reason: Option<String>,
}

/// Type of strategy change
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum StrategyChangeType {
    /// Only storage layout strategy changed
    StorageOnly,
    /// Only indexing strategy changed
    IndexingOnly,
    /// Only search strategy changed
    SearchOnly,
    /// Storage and indexing changed
    StorageAndIndexing,
    /// Storage and search changed
    StorageAndSearch,
    /// Indexing and search changed
    IndexingAndSearch,
    /// All three strategies changed
    Complete,
}

impl Default for CollectionMetadata {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            name: "Default Collection".to_string(),
            dimension: 128,
            distance_metric: "cosine".to_string(),
            indexing_algorithm: "hnsw".to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            vector_count: 0,
            total_size_bytes: 0,
            config: HashMap::new(),
            access_pattern: AccessPattern::default(),
            retention_policy: None,
            tags: Vec::new(),
            owner: None,
            description: None,
            strategy_config: CollectionStrategyConfig::default(),
            strategy_change_history: Vec::new(),
            flush_config: None, // Use global defaults
        }
    }
}

/// Metadata operation types for atomic transactions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MetadataOperation {
    /// Create collection
    CreateCollection(CollectionMetadata),

    /// Update collection metadata
    UpdateCollection {
        collection_id: CollectionId,
        metadata: CollectionMetadata,
    },

    /// Delete collection
    DeleteCollection(CollectionId),

    /// Update statistics (atomic counter updates)
    UpdateStats {
        collection_id: CollectionId,
        vector_delta: i64,
        size_delta: i64,
    },

    /// Update access pattern
    UpdateAccessPattern {
        collection_id: CollectionId,
        pattern: AccessPattern,
    },

    /// Update tags
    UpdateTags {
        collection_id: CollectionId,
        tags: Vec<String>,
    },

    /// Update retention policy
    UpdateRetentionPolicy {
        collection_id: CollectionId,
        policy: Option<RetentionPolicy>,
    },
}

/// Metadata query filters
pub struct MetadataFilter {
    /// Filter by access pattern
    pub access_pattern: Option<AccessPattern>,

    /// Filter by tags (AND operation)
    pub tags: Vec<String>,

    /// Filter by owner
    pub owner: Option<String>,

    /// Filter by minimum vector count
    pub min_vector_count: Option<u64>,

    /// Filter by maximum age in days
    pub max_age_days: Option<u32>,

    /// Custom filter function
    pub custom_filter: Option<Box<dyn Fn(&CollectionMetadata) -> bool + Send + Sync>>,
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

/// Metadata store trait for different implementations
#[async_trait]
pub trait MetadataStoreInterface: Send + Sync {
    /// Create a new collection
    async fn create_collection(&self, metadata: CollectionMetadata) -> Result<()>;

    /// Get collection metadata
    async fn get_collection(
        &self,
        collection_id: &CollectionId,
    ) -> Result<Option<CollectionMetadata>>;

    /// Update collection metadata
    async fn update_collection(
        &self,
        collection_id: &CollectionId,
        metadata: CollectionMetadata,
    ) -> Result<()>;

    /// Delete collection
    async fn delete_collection(&self, collection_id: &CollectionId) -> Result<bool>;

    /// List collections with optional filtering
    async fn list_collections(
        &self,
        filter: Option<MetadataFilter>,
    ) -> Result<Vec<CollectionMetadata>>;

    /// Update collection statistics atomically
    async fn update_stats(
        &self,
        collection_id: &CollectionId,
        vector_delta: i64,
        size_delta: i64,
    ) -> Result<()>;

    /// Batch operations (atomic)
    async fn batch_operations(&self, operations: Vec<MetadataOperation>) -> Result<()>;

    /// Get system metadata
    async fn get_system_metadata(&self) -> Result<SystemMetadata>;

    /// Update system metadata
    async fn update_system_metadata(&self, metadata: SystemMetadata) -> Result<()>;

    /// Health check
    async fn health_check(&self) -> Result<bool>;

    /// Get storage statistics
    async fn get_storage_stats(&self) -> Result<MetadataStorageStats>;
}

/// Collection-specific WAL flush configuration
/// SIZE-BASED FLUSH ONLY - age and count-based triggers removed for stability
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionFlushConfig {
    /// Maximum WAL size before forced flush (bytes, None = use global default)
    pub max_wal_size_bytes: Option<usize>,
}

impl Default for CollectionFlushConfig {
    fn default() -> Self {
        Self {
            max_wal_size_bytes: None, // Use global default (128MB)
        }
    }
}

/// Global flush defaults for the system
/// SIZE-BASED FLUSH ONLY - simplified for write-triggered flush architecture
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalFlushDefaults {
    /// Default maximum WAL size (128MB)
    pub default_max_wal_size_bytes: usize,
}

impl Default for GlobalFlushDefaults {
    fn default() -> Self {
        Self {
            default_max_wal_size_bytes: 128 * 1024 * 1024, // 128MB for write-triggered flush
        }
    }
}

impl CollectionFlushConfig {
    /// Get effective configuration using global defaults for None values
    pub fn effective_config(&self, global_defaults: &GlobalFlushDefaults) -> EffectiveFlushConfig {
        EffectiveFlushConfig {
            max_wal_size_bytes: self
                .max_wal_size_bytes
                .unwrap_or(global_defaults.default_max_wal_size_bytes),
        }
    }
}

/// Effective flush configuration with all values resolved
/// SIZE-BASED FLUSH ONLY
#[derive(Debug, Clone)]
pub struct EffectiveFlushConfig {
    pub max_wal_size_bytes: usize,
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
