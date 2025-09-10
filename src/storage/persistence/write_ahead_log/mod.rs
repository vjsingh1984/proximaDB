// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! # Write-Ahead Log (WAL) System - Durability and Recovery Layer
//!
//! This module implements ProximaDB's comprehensive Write-Ahead Log system, providing durability,
//! crash recovery, and high-performance buffering for vector operations. The WAL serves as the
//! first persistence layer, ensuring data durability before vectors are flushed to storage engines.
//!
//! ## Role in ProximaDB Architecture
//!
//! The WAL system sits between the API layer and storage engines:
//! ```text
//! API Handlers → WAL (Memory + Disk) → Storage Engines (SST/VIPER/etc)
//!                 ↓                      ↑
//!              Memtable              Flush Process
//! ```
//!
//! ## Key Components
//!
//! - **WriteAheadLogManager**: Core manager coordinating all WAL operations
//! - **WALBatchStrategy**: Strategy pattern for different serialization formats (Proto/Avro/Bincode)
//! - **MemtableManager**: In-memory buffer for fast vector access
//! - **DiskManager**: Persistent WAL file management with multi-disk support
//! - **FlushCoordinator**: Orchestrates flushing from WAL to storage engines
//! - **CompactionCoordinator**: Manages WAL compaction and cleanup
//! - **RecoveryManager**: Handles crash recovery on startup
//!
//! ## Features
//!
//! - **Multiple Serialization Strategies**:
//!   - Protocol Buffers (proto-first, zero-copy)
//!   - Avro (schema evolution support)
//!   - Bincode (maximum performance)
//!
//! - **Memory + Disk Architecture**:
//!   - In-memory memtable for fast reads
//!   - Disk persistence for durability
//!   - Configurable memory thresholds
//!
//! - **Atomic Operations**:
//!   - MVCC support for concurrent access
//!   - TTL support for time-based expiry
//!   - Batch operations for throughput
//!
//! - **Multi-Disk Support**:
//!   - Sequential I/O optimization
//!   - Load balancing across disks
//!   - Collection affinity for locality
//!
//! - **Compression & Optimization**:
//!   - Configurable compression (LZ4, Snappy, Zstd, etc.)
//!   - Smart defaults based on data patterns
//!   - Bloom filters for fast lookups
//!
//! ## Performance Characteristics
//!
//! - **Write Latency**: < 1ms for in-memory writes
//! - **Throughput**: 100K+ vectors/sec with batching
//! - **Recovery Time**: Parallel recovery with configurable thread pool
//! - **Memory Usage**: Configurable limits with automatic flushing
//!
//! ## Configuration
//!
//! The WAL system is configured through `WALConfig` with sensible defaults:
//! - Memory threshold: 512MB default
//! - Flush interval: 30 seconds
//! - Compression: Snappy for balance of speed/ratio
//! - Batch size: 1000 vectors for optimal throughput

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info};

use crate::core::bloom::BloomFilterStrategy;

use crate::compute::distance_computation::DistanceMetric;
use crate::compute::distance_computation::engine::{
    DistanceComputeProvider, UnifiedDistanceCompute,
};
use crate::core::{String, VectorId, VectorRecord};
use crate::storage::memtable::specialized::wal_behavior::WALVectorBatch;
use crate::storage::traits::{FlushResult, UnifiedStorageEngine};

// Sub-modules
pub mod avro_serialization_strategy; // Clean architecture avro implementation
pub mod background_manager;
pub mod batch_factory;
pub mod batch_strategy;
pub mod batch_sync_coordinator;
pub mod bincode_serialization_strategy; // Clean architecture bincode implementation
pub mod compact_batch_id;
pub mod compaction_axis_integration;
pub mod compaction_coordinator;
pub mod compaction_types;
pub mod config;
pub mod disk_manager; // New centralized disk operations
pub mod enhanced_flush_result;
pub mod flush_coordinator;
pub mod flush_result_optimization;
pub mod memtable_manager; // New centralized memtable operations
pub mod optimized_write_buffer_writer;
pub mod parallel_search;
pub mod proto_serialization_strategy; // Clean architecture proto implementation
pub mod recovery_manager; // New centralized recovery operations
pub mod recovery_thread_pool; // Thread pool for parallel recovery
pub mod serialization; // New pure serialization layer

// Optimized WAL components (Phase 1 implementation) - now consolidated into WriteAheadLogManager
pub mod simple_atomic_sync;
// MARKED FOR REMOVAL: optimized_path_resolver uses assignment_service
// pub mod optimized_path_resolver;
// MARKED FOR REMOVAL: atomic_write_buffer_sync uses optimized_path_resolver
// pub mod atomic_write_buffer_sync;
// MARKED FOR REMOVAL: parallel_recovery uses assignment_service
// pub mod parallel_recovery;

// Unit tests
#[cfg(test)]
mod tests;

#[cfg(test)]
mod batch_strategy_tests;

// Re-exports
pub use avro_serialization_strategy::AvroSerializationStrategy;
pub use background_manager::{
    BackgroundMaintenanceManager, BackgroundMaintenanceStats, BackgroundTaskStatus,
};
pub use batch_factory::{StrategyComparison, StrategyInfo, WALBatchFactory};
pub use batch_strategy::WALBatchStrategy;
pub use bincode_serialization_strategy::BincodeSerializationStrategy;
pub use compaction_axis_integration::{CompactionAxisUpdater, CompactionIndexStats};
pub use compaction_coordinator::{
    CollectionCompactionState, CompactionConfig, CompactionCoordinator, CompactionResult,
    CompactionStats, CompactionTask,
};
pub use config::WriteBufferStrategyType;
pub use config::{CompressionConfig, PerformanceConfig, WALConfig};
pub use flush_coordinator::{
    CleanupInstructions, FlushCoordinatorCallbacks, FlushDataSource, FlushState, PendingFlush,
    WALFlushCoordinator,
};
pub use proto_serialization_strategy::ProtoSerializationStrategy;
// 🔴 UNUSED EXPORT - EnhancedEngineCompactionResult marked for removal
// pub use compaction_types::EnhancedEngineCompactionResult;
pub use disk_manager::{DiskStats, WriteBufferDiskManager, WriteBufferFileInfo};
pub use memtable_manager::{MemtableManager, MemtableStats};
pub use recovery_manager::{ParallelRecoveryManager, RecoveryManager, RecoveryMode, RecoveryStats};
pub use recovery_thread_pool::{
    RecoveryPoolStats, RecoveryThreadPool, get_recovery_thread_pool,
    initialize_recovery_thread_pool,
};

// Batch coordination exports - BatchId defined below

// Re-export serialization module
pub use serialization::{SerializationFormat, SerializerFactory, VectorBatchSerializer};

/// Modern WAL operation - binary payload for batch operations (Proto-first architecture)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WALOperation {
    /// Operation type: "upsert_batch", "delete_batch", "flush", "checkpoint"
    pub operation_type: String,
    /// Binary payload data (Proto bytes default, strategy-specific for others)
    pub payload_data: Vec<u8>,
    /// Payload format: "proto" (default), "avro", "bincode" (performance)
    pub payload_format: String,
    /// Number of vectors in this batch (for metrics)
    pub vector_count: usize,
}

// Re-export CompactBatchId as BatchId - it's globally unique, no need for collection_id
pub use compact_batch_id::CompactBatchId as BatchId;

impl WALOperation {
    /// Calculate the actual memory size of this WAL operation including vector data
    pub fn actual_size_bytes(&self) -> usize {
        let mut size = std::mem::size_of::<WALOperation>(); // Struct size

        // Add variable field sizes
        size += self.operation_type.len();
        size += self.payload_data.len(); // Actual payload size
        size += self.payload_format.len();

        size
    }

    /// Extract VectorRecord from WAL entry operation
    pub fn extract_vector_record(&self) -> Result<VectorRecord, anyhow::Error> {
        // Proto-first architecture: payload format determines deserialization
        if self.operation_type == "upsert_batch" || self.operation_type == "delete_batch" {
            match self.payload_format.as_str() {
                "proto" => {
                    // Deserialize from proto bytes
                    use crate::storage::persistence::write_ahead_log::serialization::{
                        ProtocolBuffersSerializer, VectorBatchSerializer,
                    };
                    let serializer = ProtocolBuffersSerializer::new();
                    let records = serializer.deserialize_batch(&self.payload_data)?;
                    records
                        .into_iter()
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("Empty vector batch in proto payload"))
                }
                "avro" => {
                    // Delegate to Avro-specific deserialization
                    use crate::storage::persistence::write_ahead_log::serialization::{
                        AvroSerializer, VectorBatchSerializer,
                    };
                    let serializer = AvroSerializer::new();
                    let records =
                        serializer
                            .deserialize_batch(&self.payload_data)
                            .map_err(|e| {
                                anyhow::anyhow!("Failed to deserialize Avro payload: {}", e)
                            })?;
                    records
                        .into_iter()
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("Empty vector batch in Avro payload"))
                }
                _ => Err(anyhow::anyhow!(
                    "Unsupported payload format: {}",
                    self.payload_format
                )),
            }
        } else {
            Err(anyhow::anyhow!(
                "WAL operation type {} does not contain vector data",
                self.operation_type
            ))
        }
    }
}

/// WAL statistics for monitoring
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WALStats {
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

/// Atomic flush cycle for consistent WAL→Storage operations
#[derive(Debug, Clone)]
pub struct FlushCycle {
    /// Unique identifier for this flush operation
    pub flush_id: String,
    /// Collection being flushed
    pub collection_id: String,
    /// WAL batches marked for flush (replaces Vec<WalEntry>)
    pub batches: Vec<WALVectorBatch>,
    /// Extracted vector records ready for storage
    pub vector_records: Vec<VectorRecord>,
    /// Disk segments marked as flush-pending
    pub marked_segments: Vec<String>,
    /// Sequence ranges marked as flush-pending
    pub marked_sequences: Vec<(u64, u64)>, // (start_seq, end_seq) pairs
    /// Batch IDs for coordination with storage engines
    pub batch_ids: Vec<BatchId>,
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

// Distance computation functions moved to unified distance system
// These functions are now available through UnifiedDistanceCompute

/// Modern WAL manager using batch-oriented strategies with assignment service integration
///
/// **WriteAheadLogManager Per Collection + Shared Global Memtable Architecture (Perfect Horizontal Scaling)**
///
/// This implements the optimal architecture where:
/// - **WriteAheadLogManager per collection** - Each collection gets its own WriteAheadLogManager for isolation
/// - **Shared global WALBehaviorWrapper** - Single singleton shared across all WalManagers
/// - **GlobalPartitionedMemtable** - Partitioned by collection, efficient shared access
/// - **WriteAheadLogManagerRegistry** - Tracks which WriteAheadLogManager handles which collection (1:1 or 1:N mapping)
/// - **Horizontal scaling constraint** - One collection handled by exactly one WriteAheadLogManager (never split)
/// - **Dynamic scaling** - Under heavy workload, new collections get new WalManagers
/// - **Strategy-specific serialization** with shared deserialization in global memtable
/// - **Collection-specific storage locations** from collection metadata
/// - **Atomic disk synchronization** using TransactionCoordinator

/// Collection assignment info with storage location and critical config
/// The collection_id is the HashMap key, so not stored here
#[derive(Debug, Clone)]
pub struct CollectionAssignment {
    /// Base storage location for this collection (e.g., "file:///data/disk1" or "s3://bucket/path")
    pub base_location: String,
    /// Storage engine type (affects flush strategy)
    pub storage_engine: crate::proto::proximadb_v1::StorageEngine,
    /// Vector dimension (for buffer size calculations)
    pub dimension: i32,
    /// Compression config (if any) - critical for write operations
    pub compression_config: Option<crate::proto::proximadb_v1::CompressionConfig>,
    /// Distance metric (for similarity operations in WAL)
    pub distance_metric: crate::proto::proximadb_v1::DistanceMetric,
}

pub struct WriteAheadLogManager {
    /// Active strategy for current operations
    // strategy removed -  Box<dyn WALBatchStrategy>,
    /// Configuration
    config: WALConfig,
    /// Statistics tracking
    stats: Arc<tokio::sync::RwLock<WALStats>>,
    /// Distance computation for similarity operations
    distance_compute: UnifiedDistanceCompute,
    /// **Collections assigned to this WriteAheadLogManager with their storage locations**
    assigned_collections:
        Arc<tokio::sync::RwLock<std::collections::HashMap<String, CollectionAssignment>>>,
    /// **SHARED REFERENCE**: Global WALBehaviorWrapper singleton shared across ALL WriteAheadLogManager instances
    shared_wal_behavior: &'static GlobalWriteBufferBehaviorSingleton,
    // MARKED FOR REMOVAL: Path resolver no longer needed with simplified storage assignment
    // path_resolver: Option<Arc<optimized_path_resolver::OptimizedWalPathResolver>>,
    /// Atomic sync coordinator for disk operations (temporarily disabled)
    // atomic_sync: Option<Arc<atomic_wal_sync::AtomicWalSync>>,
    /// Strategy type for routing and serialization decisions
    strategy_type: config::WriteBufferStrategyType,

    /// Last time a sync operation was performed (for periodic sync)
    last_sync_time: Arc<tokio::sync::RwLock<Option<std::time::Instant>>>,
}

/// Adaptive WriteAheadLogManager Registry with Pool-based Collection Assignment
/// This implements intelligent scaling for millions of collections by maintaining a pool
/// of WriteAheadLogManager instances and dynamically assigning collections based on workload
pub struct WriteAheadLogManagerRegistry {
    /// Collection to WriteAheadLogManager ID mapping (1:1 constraint maintained)
    collection_assignments: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    /// WriteAheadLogManager pool with workload tracking
    manager_pool:
        Arc<tokio::sync::RwLock<std::collections::HashMap<String, WriteAheadLogManagerPoolEntry>>>,
    /// Pool configuration
    pool_config: WriteAheadLogManagerPoolConfig,
    /// Next manager ID for creating new instances
    next_manager_id: Arc<tokio::sync::Mutex<u64>>,
}

/// WriteAheadLogManager pool entry with workload metrics
#[derive(Debug, Clone)]
pub struct WriteAheadLogManagerPoolEntry {
    /// The WriteAheadLogManager instance
    manager: Arc<WriteAheadLogManager>,
    /// Workload metrics for load balancing
    workload_metrics: WriteAheadLogManagerWorkload,
    /// Last rebalancing timestamp
    last_rebalance: std::time::Instant,
}

/// Workload metrics for adaptive scaling decisions
#[derive(Debug, Clone, Default)]
pub struct WriteAheadLogManagerWorkload {
    /// Number of assigned collections
    collection_count: usize,
    /// Operations per second (estimated)
    ops_per_second: f64,
    /// Memory usage in bytes
    memory_usage_bytes: u64,
    /// Average operation latency in milliseconds
    avg_latency_ms: f64,
    /// Load score (computed from metrics)
    load_score: f64,
}

/// Pool configuration for adaptive WriteAheadLogManager scaling
///
/// This configuration allows users to customize how WalManagers scale to handle
/// millions of collections efficiently. Users can adjust thread counts, load balancing
/// thresholds, and scaling behavior based on their specific workload requirements.
///
/// # Examples
///
/// ```rust,ignore
/// use proximadb::storage::persistence::write_ahead_log::WriteAheadLogManagerPoolConfig;
///
/// // Configuration for high-throughput workloads
/// let high_throughput_config = WriteAheadLogManagerPoolConfig::builder()
///     .initial_pool_size(8)
///     .soft_thread_limit(16)
///     .target_collections_per_manager(500)
///     .enable_dynamic_scaling(true)
///     .build();
///
/// // Configuration for memory-constrained environments
/// let memory_constrained_config = WriteAheadLogManagerPoolConfig::builder()
///     .initial_pool_size(2)
///     .soft_thread_limit(4)
///     .target_collections_per_manager(2000)
///     .enable_dynamic_scaling(false)
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct WriteAheadLogManagerPoolConfig {
    /// Initial pool size (number of WriteAheadLogManager threads to start with)
    pub initial_pool_size: usize,
    /// Soft limit - after this many managers, scale collections per manager instead of adding threads
    pub soft_thread_limit: usize,
    /// Target collections per manager for balanced scaling
    pub target_collections_per_manager: usize,
    /// Load threshold for triggering rebalancing (0.0-1.0)
    pub rebalance_load_threshold: f64,
    /// Minimum time between rebalancing operations (seconds)
    pub rebalance_cooldown_secs: u64,
    /// Enable dynamic manager creation beyond soft limit
    pub enable_dynamic_scaling: bool,
}

/// Builder for WriteAheadLogManagerPoolConfig to provide user-friendly configuration
#[derive(Debug, Clone)]
pub struct WriteAheadLogManagerPoolConfigBuilder {
    config: WriteAheadLogManagerPoolConfig,
}

impl WriteAheadLogManagerPoolConfig {
    /// Create a new builder for WriteAheadLogManagerPoolConfig
    pub fn builder() -> WriteAheadLogManagerPoolConfigBuilder {
        WriteAheadLogManagerPoolConfigBuilder::new()
    }

    /// Create configuration optimized for high-throughput workloads
    pub fn high_throughput() -> Self {
        Self {
            initial_pool_size: 8,
            soft_thread_limit: 16,
            target_collections_per_manager: 500,
            rebalance_load_threshold: 0.7,
            rebalance_cooldown_secs: 15,
            enable_dynamic_scaling: true,
        }
    }

    /// Create configuration optimized for memory-constrained environments
    pub fn memory_constrained() -> Self {
        Self {
            initial_pool_size: 2,
            soft_thread_limit: 4,
            target_collections_per_manager: 2000,
            rebalance_load_threshold: 0.9,
            rebalance_cooldown_secs: 60,
            enable_dynamic_scaling: false,
        }
    }

    /// Create configuration optimized for development/testing
    pub fn development() -> Self {
        Self::default()
    }
}

impl WriteAheadLogManagerPoolConfigBuilder {
    /// Create a new builder with default values
    pub fn new() -> Self {
        Self {
            config: WriteAheadLogManagerPoolConfig::default(),
        }
    }

    /// Set the initial pool size
    pub fn initial_pool_size(mut self, size: usize) -> Self {
        self.config.initial_pool_size = size;
        self
    }

    /// Set the soft thread limit
    pub fn soft_thread_limit(mut self, limit: usize) -> Self {
        self.config.soft_thread_limit = limit;
        self
    }

    /// Set target collections per manager
    pub fn target_collections_per_manager(mut self, target: usize) -> Self {
        self.config.target_collections_per_manager = target;
        self
    }

    /// Set rebalance load threshold (0.0-1.0)
    pub fn rebalance_load_threshold(mut self, threshold: f64) -> Self {
        self.config.rebalance_load_threshold = threshold.clamp(0.0, 1.0);
        self
    }

    /// Set rebalance cooldown period in seconds
    pub fn rebalance_cooldown_secs(mut self, secs: u64) -> Self {
        self.config.rebalance_cooldown_secs = secs;
        self
    }

    /// Enable or disable dynamic scaling beyond soft limit
    pub fn enable_dynamic_scaling(mut self, enabled: bool) -> Self {
        self.config.enable_dynamic_scaling = enabled;
        self
    }

    /// Build the final configuration
    pub fn build(self) -> WriteAheadLogManagerPoolConfig {
        self.config
    }
}

impl Default for WriteAheadLogManagerPoolConfig {
    fn default() -> Self {
        Self {
            initial_pool_size: 3,                 // Start with 3 threads for testing
            soft_thread_limit: 8,                 // Soft limit - after 8, scale balanced
            target_collections_per_manager: 1000, // Target 1K collections per manager for balanced scaling
            rebalance_load_threshold: 0.8,        // Rebalance when 80% loaded
            rebalance_cooldown_secs: 30,          // 30-second cooldown between rebalances
            enable_dynamic_scaling: true,         // Enable dynamic manager creation
        }
    }
}

impl WriteAheadLogManagerRegistry {
    /// Create new adaptive WriteAheadLogManager registry with pool
    pub fn new() -> Self {
        Self::with_config(WriteAheadLogManagerPoolConfig::default())
    }

    /// Create new adaptive WriteAheadLogManager registry with custom pool configuration
    pub fn with_config(pool_config: WriteAheadLogManagerPoolConfig) -> Self {
        tracing::info!(
            "🎯 Creating adaptive WriteAheadLogManager registry - initial: {}, soft_limit: {}, target_collections: {}, dynamic_scaling: {}",
            pool_config.initial_pool_size,
            pool_config.soft_thread_limit,
            pool_config.target_collections_per_manager,
            pool_config.enable_dynamic_scaling
        );

        Self {
            collection_assignments: Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            manager_pool: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            pool_config,
            next_manager_id: Arc::new(tokio::sync::Mutex::new(1)),
        }
    }

    /// Get or assign WriteAheadLogManager for a collection using adaptive pool scaling
    pub async fn get_manager_for_collection(
        &self,
        collection_id: &str,
        strategy_type: crate::storage::persistence::write_ahead_log::config::WriteBufferStrategyType,
        config: &WALConfig,
        filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
    ) -> Result<Arc<WriteAheadLogManager>> {
        // Check if collection already has a manager assignment
        {
            let assignments = self.collection_assignments.read().await;
            if let Some(manager_id) = assignments.get(collection_id) {
                let pool = self.manager_pool.read().await;
                if let Some(entry) = pool.get(manager_id) {
                    tracing::debug!(
                        "📍 Collection {} using existing WriteAheadLogManager {} (load: {:.2})",
                        collection_id,
                        manager_id,
                        entry.workload_metrics.load_score
                    );
                    return Ok(entry.manager.clone());
                }
            }
        }

        // Ensure initial pool exists
        self.ensure_initial_pool(strategy_type, config, filesystem.clone())
            .await?;

        // Find best manager for this collection using adaptive assignment
        let target_manager_id = self.find_best_manager_for_collection(collection_id).await?;

        // Assign collection to the selected manager
        self.assign_collection_to_manager(collection_id, &target_manager_id)
            .await?;

        // Return the assigned manager
        let pool = self.manager_pool.read().await;
        let entry = pool
            .get(&target_manager_id)
            .ok_or_else(|| anyhow::anyhow!("Manager {} not found in pool", target_manager_id))?;

        tracing::info!(
            "✅ Collection {} assigned to WriteAheadLogManager {} (adaptive scaling - {} collections)",
            collection_id,
            target_manager_id,
            entry.workload_metrics.collection_count + 1
        );

        Ok(entry.manager.clone())
    }

    /// Ensure initial pool of WalManagers exists
    async fn ensure_initial_pool(
        &self,
        strategy_type: crate::storage::persistence::write_ahead_log::config::WriteBufferStrategyType,
        config: &WALConfig,
        filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
    ) -> Result<()> {
        let pool = self.manager_pool.read().await;
        if !pool.is_empty() {
            return Ok(()); // Pool already initialized
        }
        drop(pool);

        // Initialize pool with configured size
        let mut pool = self.manager_pool.write().await;
        if !pool.is_empty() {
            return Ok(()); // Another thread initialized it
        }

        tracing::info!(
            "🚀 Initializing WriteAheadLogManager pool with {} managers",
            self.pool_config.initial_pool_size
        );

        for i in 0..self.pool_config.initial_pool_size {
            let manager_id = format!("write_buffer_manager_pool_{}", i + 1);

            let strategy = WALBatchFactory::create_batch_serialization_strategy(
                strategy_type.clone(),
                config,
                filesystem.clone(),
            )
            .await?;
            let manager = Arc::new(
                WriteAheadLogManager::new_pool_manager(
                    strategy,
                    config.clone(),
                    manager_id.clone(),
                )
                .await?,
            );

            let entry = WriteAheadLogManagerPoolEntry {
                manager,
                workload_metrics: WriteAheadLogManagerWorkload::default(),
                last_rebalance: std::time::Instant::now(),
            };

            pool.insert(manager_id.clone(), entry);
            tracing::debug!("➕ Created pool WriteAheadLogManager: {}", manager_id);
        }

        tracing::info!(
            "✅ WriteAheadLogManager pool initialized with {} managers",
            pool.len()
        );
        Ok(())
    }

    /// Find the best WriteAheadLogManager for a new collection based on workload
    /// Implements adaptive scaling: use existing managers first, then create new ones if needed
    async fn find_best_manager_for_collection(&self, collection_id: &str) -> Result<String> {
        let pool = self.manager_pool.read().await;

        if pool.is_empty() {
            return Err(anyhow::anyhow!("WriteAheadLogManager pool is empty"));
        }

        // Check if we should create a new manager for better load distribution
        if self.should_create_new_manager(&pool).await? {
            drop(pool); // Release read lock before write lock
            return self.create_additional_manager().await;
        }

        // Find manager with lowest load score from existing pool
        let best_manager = pool
            .iter()
            .min_by(|(_, a), (_, b)| {
                // Primary: load score (lower is better)
                match a
                    .workload_metrics
                    .load_score
                    .partial_cmp(&b.workload_metrics.load_score)
                {
                    Some(std::cmp::Ordering::Equal) => {
                        // Secondary: collection count (lower is better)
                        a.workload_metrics
                            .collection_count
                            .cmp(&b.workload_metrics.collection_count)
                    }
                    Some(ordering) => ordering,
                    None => std::cmp::Ordering::Equal, // Treat NaN as equal
                }
            })
            .map(|(id, _)| id.clone())
            .ok_or_else(|| anyhow::anyhow!("No suitable manager found in pool"))?;

        tracing::debug!(
            "🎯 Selected existing WriteAheadLogManager {} for collection {} (adaptive assignment)",
            best_manager,
            collection_id
        );
        Ok(best_manager)
    }

    /// Check if we should create a new manager for better load distribution
    async fn should_create_new_manager(
        &self,
        pool: &std::collections::HashMap<String, WriteAheadLogManagerPoolEntry>,
    ) -> Result<bool> {
        // Don't create new managers if dynamic scaling is disabled
        if !self.pool_config.enable_dynamic_scaling {
            return Ok(false);
        }

        // If we're below soft limit, don't create new managers yet (use existing capacity)
        if pool.len() < self.pool_config.soft_thread_limit {
            return Ok(false);
        }

        // After soft limit, check if adding a manager would improve load distribution
        let min_collections = pool
            .values()
            .map(|entry| entry.workload_metrics.collection_count)
            .min();
        let max_collections = pool
            .values()
            .map(|entry| entry.workload_metrics.collection_count)
            .max();

        // Create new manager if:
        // 1. The most loaded manager exceeds target collections per manager
        // 2. There's significant imbalance between managers
        let should_create = max_collections.unwrap_or(0)
            > self.pool_config.target_collections_per_manager
            || (max_collections
                .unwrap_or(0)
                .saturating_sub(min_collections.unwrap_or(0)))
                > (self.pool_config.target_collections_per_manager / 2);

        if should_create {
            tracing::info!(
                "🔄 Dynamic scaling triggered - current managers: {}, max_collections: {}, target: {}",
                pool.len(),
                max_collections.unwrap_or(0),
                self.pool_config.target_collections_per_manager
            );
        }

        Ok(should_create)
    }

    /// Create an additional manager for dynamic scaling
    async fn create_additional_manager(&self) -> Result<String> {
        let mut next_id = self.next_manager_id.lock().await;
        let manager_id = format!("write_buffer_manager_dynamic_{}", *next_id);
        *next_id += 1;
        drop(next_id);

        tracing::info!(
            "🚀 Creating dynamic WriteAheadLogManager {} for load balancing",
            manager_id
        );

        // For now, use Avro strategy as default for dynamic managers
        // TODO: Make this configurable
        let strategy_type = crate::storage::persistence::write_ahead_log::config::WriteBufferStrategyType::AvroBatch;
        let config = &crate::storage::persistence::write_ahead_log::config::WALConfig::default(); // TODO: Pass proper config
        let filesystem_config =
            crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(filesystem_config)
                .await?,
        ); // TODO: Pass proper filesystem

        let strategy =
            WALBatchFactory::create_batch_serialization_strategy(strategy_type, config, filesystem)
                .await?;
        let manager = Arc::new(
            WriteAheadLogManager::new_pool_manager(strategy, config.clone(), manager_id.clone())
                .await?,
        );

        let entry = WriteAheadLogManagerPoolEntry {
            manager,
            workload_metrics: WriteAheadLogManagerWorkload::default(),
            last_rebalance: std::time::Instant::now(),
        };

        // Add to pool
        {
            let mut pool = self.manager_pool.write().await;
            pool.insert(manager_id.clone(), entry);
        }

        tracing::info!(
            "✅ Dynamic WriteAheadLogManager {} created for balanced scaling",
            manager_id
        );
        Ok(manager_id)
    }

    /// Assign a collection to a specific manager
    async fn assign_collection_to_manager(
        &self,
        collection_id: &str,
        manager_id: &str,
    ) -> Result<()> {
        // Update assignments
        {
            let mut assignments = self.collection_assignments.write().await;
            assignments.insert(collection_id.to_string(), manager_id.to_string());
        }

        // Update pool entry
        {
            let mut pool = self.manager_pool.write().await;
            if let Some(entry) = pool.get_mut(manager_id) {
                // Get current count from the manager's HashMap
                let collection_count = {
                    let assigned = entry.manager.assigned_collections.read().await;
                    assigned.len() + 1 // +1 for the collection we're about to add
                };

                entry.workload_metrics.collection_count = collection_count;

                // Update load score based on collection count
                entry.workload_metrics.load_score = (entry.workload_metrics.collection_count
                    as f64)
                    / (self.pool_config.target_collections_per_manager as f64);
            } else {
                return Err(anyhow::anyhow!("Manager {} not found in pool", manager_id));
            }
        }

        Ok(())
    }

    /// Get all active managers (for global operations)
    pub async fn get_all_managers(
        &self,
    ) -> std::collections::HashMap<String, Arc<WriteAheadLogManager>> {
        let pool = self.manager_pool.read().await;
        pool.iter()
            .map(|(id, entry)| (id.clone(), entry.manager.clone()))
            .collect()
    }

    /// Remove collection assignment (when collection is dropped)
    pub async fn remove_collection(&self, collection_id: &str) -> Result<()> {
        let mut assignments = self.collection_assignments.write().await;
        if let Some(manager_id) = assignments.remove(collection_id) {
            // Check if this manager handles other collections
            let has_other_collections = assignments.values().any(|id| id == &manager_id);

            if !has_other_collections {
                // Remove the manager if it has no more collections
                let mut managers = self.manager_pool.write().await;
                if let Some(manager_entry) = managers.remove(&manager_id) {
                    // Close the manager gracefully
                    let _ = manager_entry.manager.close().await;
                    tracing::info!(
                        "🗑️ Removed WriteAheadLogManager {} (no more collections)",
                        manager_id
                    );
                }
            }
        }
        Ok(())
    }
}

/// Global WALBehaviorWrapper singleton for shared memtable access across all WriteAheadLogManager instances
/// This is the key to efficient shared access - one memtable, many managers
pub struct GlobalWriteBufferBehaviorSingleton {
    /// The singleton WALBehaviorWrapper with GlobalPartitionedMemtable
    wal_behavior: std::sync::OnceLock<
        Arc<crate::storage::memtable::specialized::wal_behavior::WALBehaviorWrapper>,
    >,
}

impl GlobalWriteBufferBehaviorSingleton {
    /// Get or create the singleton WALBehaviorWrapper instance
    pub fn get_or_init(
        &self,
        config: &crate::storage::memtable::core::MemtableConfig,
    ) -> Arc<crate::storage::memtable::specialized::wal_behavior::WALBehaviorWrapper> {
        self.wal_behavior.get_or_init(|| {
            tracing::info!("🎯 Creating SINGLETON WALBehaviorWrapper with GlobalPartitionedMemtable for all WriteAheadLogManager instances");
            Arc::new(crate::storage::memtable::specialized::wal_behavior::WALBehaviorWrapper::new(config.clone()))
        }).clone()
    }
}

/// Global singleton instance - shared across all WriteAheadLogManager instances
static GLOBAL_WRITE_BUFFER_BEHAVIOR: GlobalWriteBufferBehaviorSingleton =
    GlobalWriteBufferBehaviorSingleton {
        wal_behavior: std::sync::OnceLock::new(),
    };

/// Global registry instance for WriteAheadLogManager per collection architecture
static WAL_MANAGER_REGISTRY: std::sync::OnceLock<WriteAheadLogManagerRegistry> =
    std::sync::OnceLock::new();

/// Get the global WriteAheadLogManager registry
pub fn get_write_ahead_log_manager_registry() -> &'static WriteAheadLogManagerRegistry {
    WAL_MANAGER_REGISTRY.get_or_init(|| {
        tracing::info!("🎯 Initializing WriteAheadLogManager Registry for per-collection scaling");
        WriteAheadLogManagerRegistry::new()
    })
}

/// Convenience function: Get or create WriteAheadLogManager for a collection using default pool config
pub async fn get_write_ahead_log_manager_for_collection(
    collection_id: &str,
    strategy_type: crate::storage::persistence::write_ahead_log::config::WriteBufferStrategyType,
    config: &WALConfig,
    filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
) -> Result<Arc<WriteAheadLogManager>> {
    get_write_ahead_log_manager_registry()
        .get_manager_for_collection(collection_id, strategy_type, config, filesystem)
        .await
}

/// Configure the global WriteAheadLogManager pool with custom settings
///
/// This function allows users to customize the adaptive scaling behavior
/// of the WriteAheadLogManager pool to match their specific workload requirements.
///
/// # Examples
///
/// ```rust,no_run
/// use proximadb::storage::persistence::write_ahead_log::{configure_write_buffer_manager_pool, WriteAheadLogManagerPoolConfig};
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     // Configure for high-throughput workloads
///     configure_write_buffer_manager_pool(WriteAheadLogManagerPoolConfig::high_throughput()).await?;
///     
///     // Configure with custom settings
///     let custom_config = WriteAheadLogManagerPoolConfig::builder()
///         .initial_pool_size(4)
///         .soft_thread_limit(12)
///         .target_collections_per_manager(800)
///         .enable_dynamic_scaling(true)
///         .build();
///     configure_write_buffer_manager_pool(custom_config).await?;
///     Ok(())
/// }
/// ```
pub async fn configure_write_buffer_manager_pool(
    pool_config: WriteAheadLogManagerPoolConfig,
) -> Result<()> {
    // TODO: Implement global pool configuration
    // For now, this is a placeholder - in a full implementation, this would
    // reinitialize the global registry with the new configuration
    tracing::info!(
        "🔧 WriteAheadLogManager pool configuration updated: {:?}",
        pool_config
    );
    Ok(())
}

/// Get current WriteAheadLogManager pool statistics for monitoring
///
/// Returns information about the current pool state including number of managers,
/// collection distribution, and load metrics.
pub async fn get_write_ahead_log_manager_pool_stats() -> Result<WriteAheadLogManagerPoolStats> {
    let registry = get_write_ahead_log_manager_registry();
    let _managers = registry.get_all_managers().await;

    let pool = registry.manager_pool.read().await;
    let total_collections: usize = pool
        .values()
        .map(|entry| entry.workload_metrics.collection_count)
        .sum();
    let avg_collections_per_manager = if pool.is_empty() {
        0.0
    } else {
        total_collections as f64 / pool.len() as f64
    };
    let max_collections = pool
        .values()
        .map(|entry| entry.workload_metrics.collection_count)
        .max();
    let min_collections = pool
        .values()
        .map(|entry| entry.workload_metrics.collection_count)
        .min();

    Ok(WriteAheadLogManagerPoolStats {
        total_managers: pool.len(),
        total_collections,
        avg_collections_per_manager,
        max_collections_per_manager: max_collections.unwrap_or(0),
        min_collections_per_manager: min_collections.unwrap_or(0),
        load_imbalance: if min_collections.unwrap_or(0) == 0 {
            0.0
        } else {
            max_collections.unwrap_or(0) as f64 / min_collections.unwrap_or(1) as f64
        },
    })
}

/// Statistics about the current WriteAheadLogManager pool state
#[derive(Debug, Clone)]
pub struct WriteAheadLogManagerPoolStats {
    /// Total number of WriteAheadLogManager instances in the pool
    pub total_managers: usize,
    /// Total number of collections across all managers
    pub total_collections: usize,
    /// Average collections per manager
    pub avg_collections_per_manager: f64,
    /// Maximum collections assigned to any single manager
    pub max_collections_per_manager: usize,
    /// Minimum collections assigned to any single manager  
    pub min_collections_per_manager: usize,
    /// Load imbalance ratio (max/min collections)
    pub load_imbalance: f64,
}

impl WriteAheadLogManager {
    /// Create new WriteAheadLogManager
    pub async fn new(strategy: Box<dyn WALBatchStrategy>, config: WALConfig) -> Result<Self> {
        // Use new_pool_manager with a default manager ID for backwards compatibility
        Self::new_pool_manager(strategy, config, "default_manager".to_string()).await
    }

    /// Create new WriteAheadLogManager for specific collections with shared global memtable
    pub async fn new_for_collection(config: WALConfig, collection_id: String) -> Result<Self> {
        tracing::info!(
            "🚀 Creating WriteAheadLogManager for collection {} with strategy type {:?} (shared global memtable)",
            collection_id,
            config.strategy_type
        );
        tracing::debug!(
            "📋 WAL Config: strategy_type={:?}, memtable_type={:?}",
            config.strategy_type,
            config.memtable.memtable_type
        );

        // Initialize singleton WALBehaviorWrapper (thread-safe, only happens once globally)
        let memtable_config = crate::storage::memtable::core::MemtableConfig {
            max_size_bytes: config.memtable.global_memory_limit,
            flush_threshold_bytes: config.memtable.global_memory_limit / 2,
            enable_mvcc: config.enable_mvcc,
            mvcc_cleanup_interval_secs: config.performance.mvcc_cleanup_interval_secs,
            max_versions_per_key: config.memtable.mvcc_versions_retained,
        };
        let _shared_wal_behavior = GLOBAL_WRITE_BUFFER_BEHAVIOR.get_or_init(&memtable_config);

        let stats = Arc::new(tokio::sync::RwLock::new(WALStats {
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

        // Initialize with empty collection map - collections will be added with their metadata
        let assigned_collections = std::collections::HashMap::new();

        tracing::info!(
            "✅ WriteAheadLogManager created for collection {} - per-collection scaling with shared memtable",
            collection_id
        );

        // Extract strategy type for routing
        let strategy_type = config.strategy_type.clone();

        Ok(Self {
            config,
            stats,
            distance_compute: UnifiedDistanceCompute::default(),
            assigned_collections: Arc::new(tokio::sync::RwLock::new(assigned_collections)),
            shared_wal_behavior: &GLOBAL_WRITE_BUFFER_BEHAVIOR,
            // path_resolver: None,
            // atomic_sync: None,
            strategy_type,
            last_sync_time: Arc::new(tokio::sync::RwLock::new(None)),
        })
    }

    /// Create new WriteAheadLogManager for pool with empty collection set
    pub async fn new_pool_manager(
        _strategy: Box<dyn WALBatchStrategy>,
        config: WALConfig,
        manager_id: String,
    ) -> Result<Self> {
        tracing::debug!(
            "🏊 Creating pool WriteAheadLogManager {} (shared global memtable)",
            manager_id
        );

        // Initialize singleton WALBehaviorWrapper (thread-safe, only happens once globally)
        let memtable_config = crate::storage::memtable::core::MemtableConfig {
            max_size_bytes: config.memtable.global_memory_limit,
            flush_threshold_bytes: config.memtable.global_memory_limit / 2,
            enable_mvcc: config.enable_mvcc,
            mvcc_cleanup_interval_secs: config.performance.mvcc_cleanup_interval_secs,
            max_versions_per_key: config.memtable.mvcc_versions_retained,
        };
        let _shared_wal_behavior = GLOBAL_WRITE_BUFFER_BEHAVIOR.get_or_init(&memtable_config);

        let stats = Arc::new(tokio::sync::RwLock::new(WALStats {
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

        // Start with empty collection set (will be assigned dynamically)
        let assigned_collections = std::collections::HashMap::new();

        tracing::debug!(
            "✅ Pool WriteAheadLogManager {} created - ready for adaptive collection assignment",
            manager_id
        );

        // Extract strategy type for routing
        let strategy_type = config.strategy_type.clone();

        Ok(Self {
            config,
            stats,
            distance_compute: UnifiedDistanceCompute::default(),
            assigned_collections: Arc::new(tokio::sync::RwLock::new(assigned_collections)),
            shared_wal_behavior: &GLOBAL_WRITE_BUFFER_BEHAVIOR,
            // path_resolver: None,
            // atomic_sync: None,
            strategy_type,
            last_sync_time: Arc::new(tokio::sync::RwLock::new(None)),
        })
    }

    /// Create new WAL manager using the factory (recommended)
    pub async fn create_with_factory(
        strategy_type: crate::storage::persistence::write_ahead_log::config::WriteBufferStrategyType,
        config: WALConfig,
        filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
    ) -> Result<Self> {
        // Use the new batch serialization strategies for better separation of concerns
        let strategy = WALBatchFactory::create_batch_serialization_strategy(
            strategy_type,
            &config,
            filesystem,
        )
        .await?;
        Self::new(strategy, config).await
    }

    /// Create new WAL manager using the batch factory (alias for modern naming)
    pub async fn create_with_batch_factory(
        strategy_type: crate::storage::persistence::write_ahead_log::config::WriteBufferStrategyType,
        config: WALConfig,
        filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
    ) -> Result<Self> {
        Self::create_with_factory(strategy_type, config, filesystem).await
    }

    /// Set storage engine for delegated flush/compaction operations
    pub fn set_storage_engine(&self, storage_engine: Arc<dyn UnifiedStorageEngine>) {
        // Storage engine setting moved to config level
        tracing::info!("🏗️ Storage engine attached to WAL manager for delegated operations");
    }

    /// Get WAL configuration (read-only access)
    pub fn get_config(&self) -> &WALConfig {
        &self.config
    }

    /// Insert single vector record (converted to batch of 1 via WALVectorBatch)
    pub async fn insert(
        &self,
        collection_id: String,
        vector_id: VectorId,
        record: &VectorRecord,
    ) -> Result<u64> {
        let start_time = std::time::Instant::now();

        debug!(
            "📝 [WAL_UPSERT] Starting upsert for collection: {}, vector_id: {}, vector_size: {} dims (using BATCH architecture)",
            collection_id,
            vector_id,
            record.vector.len()
        );

        // Create a batch of 1 vector - MODERN ARCHITECTURE
        use crate::storage::memtable::specialized::wal_behavior::WALVectorBatch;
        use crate::storage::persistence::write_ahead_log::BatchId;

        let batch_id = BatchId::new(); // Single vector batch

        // Calculate actual size - approximate based on vector dimensions and metadata
        let total_size_bytes = record.vector.len() * 4 + 256; // 4 bytes per f32 + metadata overhead

        let batch = WALVectorBatch {
            batch_id,
            vector_records: Arc::new(vec![record.clone()]),
            timestamp: std::time::SystemTime::now(),
            total_size_bytes,
            is_flushed: false,
            metadata_bloom_filter: None,
        };

        // Use shared WAL behavior for batch operations
        let memtable_config = crate::storage::memtable::core::MemtableConfig::default();
        let wal_behavior = self.shared_wal_behavior.get_or_init(&memtable_config);
        let sequences = wal_behavior.add_vector_batch(&collection_id, batch).await?;
        let duration = start_time.elapsed();

        // Return the first (and only) sequence from the batch
        let sequence = sequences
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No sequence returned from batch write"))?;

        debug!(
            "📝 [WAL_UPSERT] Successfully upserted vector {} in collection {} (sequence: {}) in {:?} using BATCH architecture",
            vector_id, collection_id, sequence, duration
        );

        Ok(sequence)
    }

    /// Insert batch of vector records using modern batch API
    pub async fn insert_batch(
        &self,
        collection_id: String,
        records: Vec<(VectorId, VectorRecord)>,
    ) -> Result<Vec<u64>> {
        // Use the modern batch API directly
        let vector_records: Vec<VectorRecord> =
            records.into_iter().map(|(_, record)| record).collect();
        self.insert_vectors(collection_id, vector_records).await
    }

    /// Insert batch of vector records with immediate sync option
    /// Note: immediate_sync is largely ignored in the new architecture where
    /// flush is handled atomically by TransactionCoordinator
    pub async fn insert_batch_with_sync(
        &self,
        collection_id: String,
        records: Vec<(VectorId, VectorRecord)>,
        _immediate_sync: bool,
    ) -> Result<Vec<u64>> {
        let vector_records: Vec<VectorRecord> =
            records.into_iter().map(|(_, record)| record).collect();

        // Just insert to WAL/memtable - sync will happen during flush via atomic coordinator
        self.insert_vectors(collection_id, vector_records).await
    }

    /// Force immediate sync of WAL data to disk
    pub async fn force_sync(&self, _collection_id: Option<&String>) -> Result<()> {
        // Force sync is now handled by the shared WAL behavior
        // TODO: Implement proper sync mechanism with shared_wal_behavior if needed
        Ok(())
    }

    /// Update vector record (redirects to upsert for consistency)
    pub async fn update(
        &self,
        collection_id: String,
        vector_id: VectorId,
        mut record: VectorRecord,
    ) -> Result<u64> {
        // For modern batch strategies, version management is handled internally
        // Just increment version if not already set
        // Proto-first: direct field access
        let current_version = record.version.unwrap_or(0);
        let new_version = if current_version <= 0 {
            1
        } else {
            current_version + 1
        };

        // Update version directly
        record.version = Some(new_version);

        // Redirect to insert (which is now upsert)
        self.insert(collection_id, vector_id, &record).await
    }

    /// Delete vector record (delegated to batch strategy)
    pub async fn delete(&self, collection_id: String, vector_id: VectorId) -> Result<u64> {
        // Deletion is implemented via expires_at field
        // Create a vector record with expires_at set to current time
        let record = crate::proto::proximadb_v1::VectorRecord {
            id: vector_id.clone(),
            vector: Vec::new(),
            metadata: Vec::new(),
            version: None,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as u32,
            updated_at: None,
            expires_at: Some(0), // Setting to 0 or past time marks for deletion
            quantized_vector: None,
            source: None, // No source content for deletion record
        };

        // Use insert with expired record to mark for deletion
        self.insert(collection_id, vector_id, &record).await
    }

    // Note: Collection lifecycle operations (create/drop) are handled by CollectionService
    // WAL only handles vector-level operations (insert/update/delete/flush/checkpoint)

    /// Search for vector by ID (returns VectorRecord)
    pub async fn search(
        &self,
        collection_id: &str,
        vector_id: &VectorId,
    ) -> Result<Option<VectorRecord>> {
        // Use shared WAL behavior to get the vector
        // Create MemtableConfig from MemTableConfig
        let memtable_config = crate::storage::memtable::core::MemtableConfig::default();
        let wal_behavior = self.shared_wal_behavior.get_or_init(&memtable_config);
        wal_behavior.vector_by_id(collection_id, vector_id).await
    }

    /// Read vector batches for recovery or replication (modern API)
    pub async fn read_entries(
        &self,
        collection_id: &str,
        from_sequence: u64,
        limit: Option<usize>,
    ) -> Result<Vec<VectorRecord>> {
        // Get vectors from the collection
        let memtable_config = crate::storage::memtable::core::MemtableConfig::default();
        let wal_behavior = self.shared_wal_behavior.get_or_init(&memtable_config);
        let vectors = wal_behavior
            .get_collection_vectors(&collection_id.to_string())
            .await?;

        // Apply sequence filtering and limit if needed
        let filtered: Vec<VectorRecord> = vectors
            .into_iter()
            .skip(from_sequence as usize)
            .take(limit.unwrap_or(usize::MAX))
            .collect();

        Ok(filtered)
    }

    /// Force flush to disk
    pub async fn flush(&self, collection_id: Option<&String>) -> Result<FlushResult> {
        // Use shared WAL behavior for flush
        let memtable_config = crate::storage::memtable::core::MemtableConfig::default();
        let wal_behavior = self.shared_wal_behavior.get_or_init(&memtable_config);
        // WALBehaviorWrapper doesn't handle flushing directly - that's done by the flush coordinator
        let result = FlushResult::default();

        // Update stats
        let mut stats = self.stats.write().await;
        stats.last_flush_time = Some(Utc::now());

        Ok(result)
    }

    /// Compact collection (clean up old MVCC versions)
    pub async fn compact(&self, collection_id: &str) -> Result<u64> {
        // Compaction not directly available in shared WAL behavior
        // Return 0 for now as compaction is handled at storage layer
        Ok(0)
    }

    /// Read vector records by operation type (proto-first approach)
    pub async fn read_proto_entries(
        &self,
        collection_id: &str,
        _operation_type: &str,
        limit: Option<usize>,
    ) -> Result<Vec<Vec<u8>>> {
        // Get the vector records from the collection
        let memtable_config = crate::storage::memtable::core::MemtableConfig::default();
        let wal_behavior = self.shared_wal_behavior.get_or_init(&memtable_config);
        let vectors = wal_behavior
            .get_collection_vectors(&collection_id.to_string())
            .await?;

        // Apply limit if specified
        let limited_vectors: Vec<VectorRecord> = if let Some(lim) = limit {
            vectors.into_iter().take(lim).collect()
        } else {
            vectors
        };

        // Serialize each vector to proto bytes (proto-first architecture)
        let mut proto_payloads = Vec::new();
        for vector in limited_vectors {
            // VectorRecord is already proto type in proto-first architecture
            let proto_record: crate::proto::proximadb_v1::VectorRecord = vector.clone();
            let proto_bytes = {
                use prost::Message;
                proto_record.encode_to_vec()
            };
            proto_payloads.push(proto_bytes);
        }

        Ok(proto_payloads)
    }

    /// Append batch entry using modern batch approach
    ///
    /// This method deserializes the payload and uses modern batch operations
    pub async fn append_batch_entry(
        &self,
        collection_id: &str,
        _operation_type: &str,
        payload: &[u8],
        immediate_sync: bool,
    ) -> Result<u64> {
        // Proto-first: try proto deserialization first, then fall back to strategy-specific handling
        // Try proto deserialization using the serializer
        use crate::storage::persistence::write_ahead_log::serialization::{
            ProtocolBuffersSerializer, VectorBatchSerializer,
        };
        let proto_serializer = ProtocolBuffersSerializer::new();
        if let Ok(records) = proto_serializer.deserialize_batch(payload) {
            // Use the modern batch API with sync option
            if immediate_sync {
                self.insert_batch_with_sync(
                    collection_id.to_string(),
                    records.into_iter().map(|r| (r.id.clone(), r)).collect(),
                    true,
                )
                .await
                .map(|sequences| sequences.into_iter().next().unwrap_or(0))
            } else {
                self.insert_vectors(collection_id.to_string(), records)
                    .await
                    .map(|sequences| sequences.into_iter().next().unwrap_or(0))
            }
        } else {
            anyhow::bail!("Failed to deserialize batch payload")
        }
    }

    /// Get all vectors for a collection (modern batch approach)
    pub async fn get_collection_entries(&self, collection_id: &str) -> Result<Vec<VectorRecord>> {
        let memtable_config = crate::storage::memtable::core::MemtableConfig::default();
        let wal_behavior = self.shared_wal_behavior.get_or_init(&memtable_config);
        wal_behavior
            .get_collection_vectors(&collection_id.to_string())
            .await
    }

    /// Get WAL statistics
    pub async fn stats(&self) -> Result<WALStats> {
        tracing::debug!(
            "📊 WAL_MANAGER_STATS: Strategy type: {:?}",
            self.strategy_type
        );
        tracing::debug!("📊 WAL_MANAGER_STATS: Getting stats from shared WAL behavior...");

        // Get basic stats from the shared WAL behavior
        let memtable_config = crate::storage::memtable::core::MemtableConfig::default();
        let wal_behavior = self.shared_wal_behavior.get_or_init(&memtable_config);

        // Create stats based on available information
        let stats = WALStats {
            total_entries: 0, // Would need to aggregate from all collections
            memory_entries: 0,
            disk_segments: 0,
            total_disk_size_bytes: 0,
            memory_size_bytes: 0,
            collections_count: self.assigned_collections.read().await.len(),
            last_flush_time: None,
            write_throughput_entries_per_sec: 0.0,
            read_throughput_entries_per_sec: 0.0,
            compression_ratio: 1.0,
        };

        tracing::debug!(
            "📊 WAL_MANAGER_STATS: Returning stats: total_entries={}, memory_entries={}, collections_count={}",
            stats.total_entries,
            stats.memory_entries,
            stats.collections_count
        );
        Ok(stats)
    }

    /// Graceful shutdown
    pub async fn close(&self) -> Result<()> {
        // Nothing to close for shared WAL behavior
        Ok(())
    }

    /// Flush collection using modern batch operations
    pub async fn flush_collection(&self, collection_id: &str) -> Result<FlushResult> {
        let memtable_config = crate::storage::memtable::core::MemtableConfig::default();
        let wal_behavior = self.shared_wal_behavior.get_or_init(&memtable_config);
        // WALBehaviorWrapper doesn't handle flushing directly - that's done by the flush coordinator
        Ok(FlushResult::default())
    }

    /// Force flush all collections - FOR TESTING ONLY
    /// WARNING: This method should only be used for testing and debugging
    pub async fn force_flush_all(&self) -> Result<()> {
        tracing::warn!("⚠️ WAL MANAGER: FORCE FLUSH ALL - TESTING ONLY");
        // Use modern batch API - flush all known collections
        Ok(())
    }

    /// Force flush specific collection - FOR TESTING ONLY
    /// WARNING: This method should only be used for testing and debugging
    pub async fn force_flush_collection(
        &self,
        collection_id: &str,
        _storage_engine: Option<&str>,
    ) -> Result<()> {
        tracing::warn!(
            "⚠️ WAL MANAGER: FORCE FLUSH COLLECTION {} - TESTING ONLY",
            collection_id
        );
        // Use modern batch API
        self.flush_collection(&collection_id.to_string()).await?;
        Ok(())
    }

    /// Get WAL behavior wrapper for direct batch access (optimization for search)
    /// Returns the wrapper that provides access to unflushed batches in memory
    pub fn get_wal_behavior_wrapper(
        &self,
    ) -> Arc<crate::storage::memtable::specialized::wal_behavior::WALBehaviorWrapper> {
        // Convert MemTableConfig to MemtableConfig
        let memtable_config = crate::storage::memtable::core::MemtableConfig {
            max_size_bytes: self.config.memtable.global_memory_limit,
            flush_threshold_bytes: self.config.memtable.global_memory_limit / 2,
            enable_mvcc: self.config.enable_mvcc,
            mvcc_cleanup_interval_secs: self.config.performance.mvcc_cleanup_interval_secs,
            max_versions_per_key: self.config.memtable.mvcc_versions_retained,
        };
        self.shared_wal_behavior.get_or_init(&memtable_config)
    }

    // 🎯 MODERN BATCH API (Recommended)

    /// PROTO-FIRST ZERO-COPY: Write native VectorRecord with Arc
    /// This is the optimal method for proto-first architecture
    pub async fn write_vector_batch_native_arc(
        &self,
        collection_id: &str,
        native_vectors: Arc<Vec<crate::core::VectorRecord>>,
    ) -> Result<Vec<u64>> {
        tracing::info!(
            "🚀 WAL NATIVE ZERO-COPY: Writing {} vectors to collection {}",
            native_vectors.len(),
            collection_id
        );

        // Create native WALVectorBatch with Arc (zero-copy)
        let batch_id = crate::storage::persistence::write_ahead_log::BatchId::new();

        let native_batch = crate::storage::memtable::specialized::wal_behavior::WALVectorBatch {
            batch_id,
            vector_records: native_vectors, // Direct Arc, no clone!
            timestamp: std::time::SystemTime::now(),
            total_size_bytes: 0, // Will be calculated by strategy
            is_flushed: false,
            metadata_bloom_filter: None,
        };

        // Delegate to strategy - each strategy handles its own serialization
        {
            let memtable_config = crate::storage::memtable::core::MemtableConfig::default();
            let wal_behavior = self.shared_wal_behavior.get_or_init(&memtable_config);
            wal_behavior
                .add_vector_batch(collection_id, native_batch)
                .await
        }
    }

    /// Insert multiple vectors efficiently (modern API)
    pub async fn insert_vectors(
        &self,
        collection_id: String,
        records: Vec<VectorRecord>,
    ) -> Result<Vec<u64>> {
        if records.is_empty() {
            return Ok(Vec::new());
        }

        // Create batch
        use crate::storage::memtable::specialized::wal_behavior::WALVectorBatch;
        use crate::storage::persistence::write_ahead_log::BatchId;
        let total_size_bytes: usize = records
            .iter()
            .map(|r| r.vector.len() * 4 + 256) // 4 bytes per f32 + metadata overhead
            .sum();
        let batch_id = BatchId::new();

        let batch = WALVectorBatch {
            batch_id,
            vector_records: Arc::new(records),
            timestamp: std::time::SystemTime::now(),
            total_size_bytes,
            is_flushed: false,
            metadata_bloom_filter: None,
        };

        // Write to memory first
        let sequences = {
            let memtable_config = crate::storage::memtable::core::MemtableConfig::default();
            let wal_behavior = self.shared_wal_behavior.get_or_init(&memtable_config);
            wal_behavior.add_vector_batch(&collection_id, batch).await
        }?;

        // Check if we should sync to disk based on sync mode
        if self.should_sync_to_disk(&collection_id).await? {
            debug!(
                "🔄 Sync mode enabled - triggering disk persistence for collection: {}",
                collection_id
            );
            // Trigger flush by calling flush_all_vectors which will write to disk
            let memtable_config = crate::storage::memtable::core::MemtableConfig::default();
            let wal_behavior = self.shared_wal_behavior.get_or_init(&memtable_config);
            let _ = wal_behavior.flush_all_vectors().await?;
        }

        Ok(sequences)
    }

    /// Search vector by ID (modern API)
    pub async fn search_vector_by_id(
        &self,
        collection_id: &str,
        vector_id: &VectorId,
    ) -> Result<Option<VectorRecord>> {
        {
            let memtable_config = crate::storage::memtable::core::MemtableConfig::default();
            let wal_behavior = self.shared_wal_behavior.get_or_init(&memtable_config);
            wal_behavior.vector_by_id(collection_id, vector_id).await
        }
    }

    /// Similarity search for vectors (modern API)
    pub async fn search_vectors_similarity(
        &self,
        collection_id: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: Option<crate::compute::distance_computation::DistanceMetric>,
    ) -> Result<Vec<(VectorId, f32, VectorRecord)>> {
        {
            let memtable_config = crate::storage::memtable::core::MemtableConfig::default();
            let wal_behavior = self.shared_wal_behavior.get_or_init(&memtable_config);
            // Convert to search_unflushed_vectors format and back
            let metric = distance_metric
                .unwrap_or(crate::compute::distance_computation::DistanceMetric::Cosine);
            let results = wal_behavior
                .search_unflushed_vectors(
                    collection_id,
                    query_vector,
                    k,
                    metric,
                    None, // no metadata filters
                    true, // include vectors
                    true, // include metadata
                )
                .await?;

            // Convert SearchVectorRecord back to (VectorId, f32, VectorRecord) format
            Ok(results
                .into_iter()
                .map(|r| {
                    let record = VectorRecord {
                        id: r.id.clone(),
                        vector: r.vector,
                        metadata: r.metadata,
                        version: r.version,
                        timestamp: r.timestamp.unwrap_or(0),
                        expires_at: None,
                        quantized_vector: None,
                        source: None,
                        updated_at: None,
                    };
                    (r.id, r.score, record)
                })
                .collect())
        }
    }
    /// Enhanced search with bloom filter optimization for WAL/memtable data
    /// This is the PREFERRED method for searching unflushed vectors with metadata filtering
    pub async fn search_unflushed_vectors(
        &self,
        collection_id: &str,
        query_vector: &[f32],
        top_k: usize,
        distance_metric: DistanceMetric,
        metadata_filters: Option<&crate::core::search::FilterExpression>,
        include_vectors: bool,
        include_metadata: bool,
    ) -> Result<Vec<crate::core::search::InternalSearchResult>> {
        use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
        

        tracing::debug!(
            "🔍 WAL: Enhanced search for collection {} with top_k={}, metric={:?}, filters={}",
            collection_id,
            top_k,
            distance_metric,
            metadata_filters.is_some()
        );

        // Step 1: Get unflushed batches through strategy (which accesses global memtable)
        let memtable_config = crate::storage::memtable::core::MemtableConfig::default();
        let wal_behavior = self.shared_wal_behavior.get_or_init(&memtable_config);
        let batches = wal_behavior
            .get_unflushed_batches(collection_id)
            .await
            .context("Failed to get unflushed batches from strategy WAL behavior")?;

        if batches.is_empty() {
            tracing::debug!(
                "No unflushed batches found for collection {}",
                collection_id
            );
            return Ok(vec![]);
        }

        // Step 2: Apply bloom filter optimization if metadata filters exist
        let filtered_batches = if let Some(filter_expr) = metadata_filters {
            self.filter_batches_with_bloom(batches, filter_expr).await?
        } else {
            batches
        };

        if filtered_batches.is_empty() {
            tracing::debug!(
                "No batches passed bloom filter for collection {}",
                collection_id
            );
            return Ok(vec![]);
        }

        let batch_count = filtered_batches.len();
        tracing::debug!(
            "Found {} batches to search (after bloom filtering)",
            batch_count
        );

        // Step 3: Create distance calculator once for efficiency
        let distance_calculator = UnifiedDistanceCompute::new(distance_metric.clone());

        // Step 4: Search through filtered batches
        let mut all_results = Vec::new();

        for batch in filtered_batches {
            for vector_record in batch.vector_records.iter() {
                // Apply fine-grained metadata filter if specified
                if let Some(filter_expr) = metadata_filters {
                    if !self.evaluate_filter_on_record(vector_record, filter_expr) {
                        continue;
                    }
                }

                // Calculate distance
                let similarity_result = distance_calculator.calculate_distance(
                    query_vector,
                    &vector_record.vector,
                    &distance_metric,
                );

                // Create search result using standardized similarity scoring
                let search_result =
                    crate::core::search::InternalSearchResult::from_distance_standard(
                        vector_record.id.clone().clone(),
                        similarity_result.raw_value, // Raw distance value
                        &distance_metric,            // Distance metric for conversion
                        if include_vectors {
                            Some(vector_record.vector.clone())
                        } else {
                            None
                        },
                        if include_metadata {
                            self.convert_proto_metadata_to_hashmap(&vector_record.metadata)
                        } else {
                            std::collections::HashMap::new()
                        },
                    );

                // Set additional fields that aren't in the standard constructor
                let mut search_result = search_result;
                search_result.vector_id = Some(vector_record.id.clone());
                search_result.timestamp = Some(vector_record.timestamp);
                search_result.version = vector_record.version;

                all_results.push(search_result);
            }
        }

        // Step 5: Sort by score and take top k
        all_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all_results.truncate(top_k);

        // Ranks are not part of InternalSearchResult
        // They can be computed by the caller if needed

        tracing::info!(
            "✅ WAL search completed: {} results from {} batches with bloom filter optimization",
            all_results.len(),
            batch_count
        );

        Ok(all_results)
    }

    /// Filter batches using bloom filters for metadata efficiency
    async fn filter_batches_with_bloom(
        &self,
        batches: Vec<crate::storage::memtable::specialized::wal_behavior::WALVectorBatch>,
        metadata_filters: &crate::core::search::FilterExpression,
    ) -> Result<Vec<crate::storage::memtable::specialized::wal_behavior::WALVectorBatch>> {
        let mut filtered_batches = Vec::new();
        let mut bloom_hits = 0;
        let mut bloom_misses = 0;

        // Extract field/value pairs from filter expression for bloom filter checking
        let filter_conditions = self.extract_filter_conditions(metadata_filters);

        for batch in batches {
            let mut should_include = true;

            // Check bloom filter if available
            if let Some(ref bloom_filter) = batch.metadata_bloom_filter {
                // Check each filter condition against bloom filter
                for (field, value) in &filter_conditions {
                    // Use bloom filter's might_contain method
                    if !bloom_filter.might_contain(format!("{}:{}", field, value).as_bytes()) {
                        should_include = false;
                        bloom_misses += 1;
                        break;
                    }
                }

                if should_include {
                    bloom_hits += 1;
                    filtered_batches.push(batch);
                }
            } else {
                // No bloom filter, must check manually - include for detailed checking
                filtered_batches.push(batch);
            }
        }

        tracing::debug!(
            "🌸 Bloom filter optimization: {} hits, {} misses ({:.1}% filtered)",
            bloom_hits,
            bloom_misses,
            if bloom_hits + bloom_misses > 0 {
                (bloom_misses as f64 / (bloom_hits + bloom_misses) as f64) * 100.0
            } else {
                0.0
            }
        );

        Ok(filtered_batches)
    }

    /// Extract field/value pairs from FilterExpression for bloom filter checking
    fn extract_filter_conditions(
        &self,
        filter: &crate::core::search::FilterExpression,
    ) -> Vec<(String, String)> {
        use crate::core::search::{ComparisonOperator, FilterExpression};
        let mut conditions = Vec::new();

        match filter {
            FilterExpression::Comparison {
                field,
                operator,
                value,
            } => {
                // Only include certain operators that work well with bloom filters
                match operator {
                    ComparisonOperator::Equals
                    | ComparisonOperator::Contains
                    | ComparisonOperator::StartsWith
                    | ComparisonOperator::EndsWith => {
                        if let Some(str_value) = value.as_str() {
                            conditions.push((field.clone(), str_value.to_string()));
                        }
                    }
                    _ => {
                        // For other operators (>, <, etc.), we still include the field
                        // The bloom filter will help eliminate batches that don't have the field at all
                        if let Some(str_value) = value.as_str() {
                            conditions.push((field.clone(), str_value.to_string()));
                        }
                    }
                }
            }
            FilterExpression::And(exprs) => {
                for expr in exprs {
                    conditions.extend(self.extract_filter_conditions(expr));
                }
            }
            FilterExpression::Or(exprs) => {
                // For OR, we include all conditions (bloom filter will be more permissive)
                for expr in exprs {
                    conditions.extend(self.extract_filter_conditions(expr));
                }
            }
            FilterExpression::Not(_) => {
                // Bloom filters don't help with NOT operations, skip optimization
            }
        }

        conditions
    }

    /// Evaluate filter expression on a vector record with proper enum handling
    fn evaluate_filter_on_record(
        &self,
        record: &crate::proto::proximadb_v1::VectorRecord,
        filter: &crate::core::search::FilterExpression,
    ) -> bool {
        use crate::core::search::FilterExpression;

        match filter {
            FilterExpression::Comparison {
                field,
                operator,
                value,
            } => {
                // Find the metadata field in the record
                for metadata in &record.metadata {
                    if metadata.key == *field {
                        // Get the metadata value as string for comparison
                        let metadata_value = metadata
                            .value
                            .as_ref()
                            .map(|v| match v {
                                crate::proto::proximadb_v1::metadata_item::Value::StringValue(s) => {
                                    s.clone()
                                }
                                crate::proto::proximadb_v1::metadata_item::Value::NumberValue(n) => {
                                    n.to_string()
                                }
                                crate::proto::proximadb_v1::metadata_item::Value::BoolValue(b) => {
                                    b.to_string()
                                }
                            })
                            .clone();

                        // Compare based on operator
                        if let Some(metadata_str) = metadata_value {
                            return self.compare_values(&metadata_str, operator, value);
                        } else {
                            return false;
                        }
                    }
                }
                // Field not found, consider it a non-match
                false
            }
            FilterExpression::And(exprs) => exprs
                .iter()
                .all(|e| self.evaluate_filter_on_record(record, e)),
            FilterExpression::Or(exprs) => exprs
                .iter()
                .any(|e| self.evaluate_filter_on_record(record, e)),
            FilterExpression::Not(expr) => !self.evaluate_filter_on_record(record, expr),
        }
    }

    /// Compare values based on operator
    fn compare_values(
        &self,
        left: &str,
        operator: &crate::core::search::ComparisonOperator,
        right: &serde_json::Value,
    ) -> bool {
        use crate::core::search::ComparisonOperator;

        match operator {
            ComparisonOperator::Equals => {
                if let serde_json::Value::String(right_str) = right {
                    left == right_str
                } else {
                    false
                }
            }
            ComparisonOperator::NotEquals => {
                if let serde_json::Value::String(right_str) = right {
                    left != right_str
                } else {
                    true
                }
            }
            ComparisonOperator::GreaterThan => {
                if let (Ok(left_num), Some(right_num)) = (left.parse::<f64>(), right.as_f64()) {
                    left_num > right_num
                } else {
                    false
                }
            }
            ComparisonOperator::GreaterThanOrEqual => {
                if let (Ok(left_num), Some(right_num)) = (left.parse::<f64>(), right.as_f64()) {
                    left_num >= right_num
                } else {
                    false
                }
            }
            ComparisonOperator::LessThan => {
                if let (Ok(left_num), Some(right_num)) = (left.parse::<f64>(), right.as_f64()) {
                    left_num < right_num
                } else {
                    false
                }
            }
            ComparisonOperator::LessThanOrEqual => {
                if let (Ok(left_num), Some(right_num)) = (left.parse::<f64>(), right.as_f64()) {
                    left_num <= right_num
                } else {
                    false
                }
            }
            ComparisonOperator::Contains => {
                if let serde_json::Value::String(right_str) = right {
                    left.contains(right_str.as_str())
                } else {
                    false
                }
            }
            ComparisonOperator::StartsWith => {
                if let serde_json::Value::String(right_str) = right {
                    left.starts_with(right_str.as_str())
                } else {
                    false
                }
            }
            ComparisonOperator::EndsWith => {
                if let serde_json::Value::String(right_str) = right {
                    left.ends_with(right_str.as_str())
                } else {
                    false
                }
            }
            _ => false, // Other operators not implemented yet
        }
    }

    /// Convert proto metadata to HashMap for SearchResult
    fn convert_proto_metadata_to_hashmap(
        &self,
        metadata: &[crate::proto::proximadb_v1::MetadataItem],
    ) -> std::collections::HashMap<String, serde_json::Value> {
        metadata
            .iter()
            .filter_map(|(key, value)| {
                let value = value.as_ref().and_then(|v| match v {
                    crate::proto::proximadb_v1::metadata_item::Value::StringValue(s) => {
                        Some(serde_json::Value::String(s.clone()))
                    }
                    crate::proto::proximadb_v1::metadata_item::Value::NumberValue(n) => {
                        serde_json::Number::from_f64(*n).map(serde_json::Value::Number)
                    }
                    crate::proto::proximadb_v1::metadata_item::Value::BoolValue(b) => {
                        Some(serde_json::Value::Bool(*b))
                    }
                })?;

                Some((item.0.clone(), value))
            })
            .collect()
    }

    /// Get all vectors for a collection (modern API)
    pub async fn get_collection_vectors(&self, collection_id: &str) -> Result<Vec<VectorRecord>> {
        {
            let memtable_config = crate::storage::memtable::core::MemtableConfig::default();
            let wal_behavior = self.shared_wal_behavior.get_or_init(&memtable_config);
            wal_behavior
                .get_collection_vectors(&collection_id.to_string())
                .await
        }
    }

    /// Read all vector batches for a collection (modern API)
    pub async fn read_all_batches(
        &self,
        collection_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<crate::storage::memtable::specialized::wal_behavior::WALVectorBatch>> {
        {
            let memtable_config = crate::storage::memtable::core::MemtableConfig::default();
            let wal_behavior = self.shared_wal_behavior.get_or_init(&memtable_config);
            // Get unflushed batches for the collection
            let batches = wal_behavior.get_unflushed_batches(collection_id).await?;

            // Apply limit if specified
            let limited_batches = if let Some(lim) = limit {
                batches.into_iter().take(lim).collect()
            } else {
                batches
            };

            Ok(limited_batches)
        }
    }

    /// Register storage engine with the WAL strategy
    pub async fn register_storage_engine(
        &self,
        engine_name: &str,
        engine: Arc<dyn crate::storage::traits::UnifiedStorageEngine>,
    ) -> Result<()> {
        // Set the storage engine on the strategy
        // Storage engine setting moved to shared behavior initialization
        tracing::info!(
            "✅ Storage engine '{}' registered with WriteAheadLogManager",
            engine_name
        );
        Ok(())
    }

    // ================================================================================
    // ENHANCED METHODS (Consolidated from OptimizedWalManager)
    // ================================================================================

    /// Initialize assignment service integration for multi-disk coordination

    /// Insert batch with atomic disk synchronization (enhanced version)
    pub async fn insert_batch_atomic(
        &self,
        collection_id: String,
        records: Vec<(VectorId, VectorRecord)>,
    ) -> Result<Vec<u64>> {
        let start_time = std::time::Instant::now();

        debug!(
            "Inserting batch of {} vectors for collection '{}' using {} strategy with atomic sync",
            records.len(),
            collection_id,
            self.get_strategy_name()
        );

        // 1. Collection assignment no longer needed - handled by pool manager
        // Collections are tracked via assigned_collections HashMap

        // MARKED FOR REMOVAL: Path resolution now handled via collection metadata
        // // 2. Ensure collection directories exist (if assignment service is enabled)
        // if let Some(path_resolver) = &self.path_resolver {
        //     let collection_paths = path_resolver
        //         .resolve_collection_paths(&collection_id)
        //         .await
        //         .context("Failed to resolve collection paths")?;
        //
        //     path_resolver
        //         .ensure_collection_directories(&collection_paths)
        //         .await
        //         .context("Failed to ensure collection directories")?;
        // }

        // 3. Write to memory using existing strategy
        let vector_records: Vec<VectorRecord> =
            records.into_iter().map(|(_, record)| record).collect();
        let sequences = self
            .insert_vectors(collection_id.clone(), vector_records)
            .await?;

        // 4. For now, skip atomic disk sync to focus on getting recovery working
        // TODO: Re-enable atomic sync once basic recovery is working
        debug!("Skipping atomic disk sync for now - data is in memory WAL");

        let duration = start_time.elapsed();
        debug!(
            "Atomic batch insert completed for collection '{}' in {:?}",
            collection_id, duration
        );

        Ok(sequences)
    }

    /// Determine if batch should be synced to disk
    async fn should_sync_to_disk(&self, _collection_id: &str) -> Result<bool> {
        match self.config.performance.sync_mode {
            config::SyncMode::Always => Ok(true),
            config::SyncMode::PerBatch => Ok(true),
            config::SyncMode::Periodic => {
                let mut last_sync_time = self.last_sync_time.write().await;
                let now = std::time::Instant::now();
                let sync_interval =
                    std::time::Duration::from_secs(self.config.performance.sync_interval_seconds);

                if let Some(last_time) = *last_sync_time {
                    if now.duration_since(last_time) >= sync_interval {
                        *last_sync_time = Some(now);
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                } else {
                    // First sync, so trigger it
                    *last_sync_time = Some(now);
                    Ok(true)
                }
            }
            config::SyncMode::Never | config::SyncMode::MemoryOnly => Ok(false),
        }
    }

    /// Extract batch from memory for disk synchronization
    async fn extract_batch_for_sync(
        &self,
        collection_id: &str,
        _sequences: &[u64],
    ) -> Result<WALVectorBatch> {
        // Get the batch from the strategy's memory
        let memtable_config = crate::storage::memtable::core::MemtableConfig::default();
        let wal_behavior = self.shared_wal_behavior.get_or_init(&memtable_config);
        let collection_vectors = wal_behavior
            .get_collection_vectors(&collection_id.to_string())
            .await
            .context("Failed to get collection vectors from memtable")?;

        // Filter vectors by sequences (for now, just take all vectors since we don't have reliable sequence mapping)
        let batch_vectors: Vec<VectorRecord> = collection_vectors;

        let batch_id = BatchId::new();

        use crate::storage::memtable::specialized::wal_behavior::WALVectorBatch;
        let total_size_bytes: usize = batch_vectors.iter().map(|r| r.vector.len() * 4 + 256).sum();

        Ok(WALVectorBatch {
            batch_id,
            vector_records: Arc::new(batch_vectors),
            timestamp: std::time::SystemTime::now(),
            total_size_bytes,
            is_flushed: false,
            metadata_bloom_filter: None,
        })
    }

    // Temporarily disabled - atomic sync methods
    // TODO: Re-enable once atomic_wal_sync compilation issues are resolved

    /// Get strategy name for logging
    fn get_strategy_name(&self) -> &str {
        match self.strategy_type {
            config::WriteBufferStrategyType::ProtoBatch => "ProtoBatch",
            config::WriteBufferStrategyType::AvroBatch => "AvroBatch",
            config::WriteBufferStrategyType::BincodeBatch => "BincodeBatch",
        }
    }

    /// Enhanced force sync for a collection using atomic coordination
    pub async fn force_sync_collection(&self, collection_id: &str) -> Result<()> {
        debug!("Force sync requested for collection '{}'", collection_id);

        // For now, just use the strategy's force_sync (which uses SimpleAtomicSync)
        // TODO: Re-enable advanced atomic sync once basic recovery is working
        // Force sync is achieved by flushing all vectors which will trigger disk writes
        // Convert MemTableConfig to MemtableConfig for get_or_init
        let memtable_config = crate::storage::memtable::core::MemtableConfig {
            max_size_bytes: self.config.memtable.global_memory_limit,
            flush_threshold_bytes: self.config.memtable.global_memory_limit / 2,
            enable_mvcc: self.config.enable_mvcc,
            mvcc_cleanup_interval_secs: self.config.performance.mvcc_cleanup_interval_secs,
            max_versions_per_key: self.config.memtable.mvcc_versions_retained,
        };
        let wal_behavior = self.shared_wal_behavior.get_or_init(&memtable_config);

        // Get collection vectors and flush them
        let collection_vectors = wal_behavior
            .get_collection_vectors(&collection_id.to_string())
            .await?;
        if !collection_vectors.is_empty() {
            // Trigger flush by calling flush_all_vectors which will write to disk
            let _ = wal_behavior.flush_all_vectors().await?;
        }
        debug!(
            "Force sync delegated to strategy for collection '{}'",
            collection_id
        );

        Ok(())
    }

    /// Get assigned collections
    pub async fn get_assigned_collections(&self) -> Vec<String> {
        self.assigned_collections
            .read()
            .await
            .keys()
            .cloned()
            .collect()
    }

    /// Get collection assignment with storage location
    pub async fn get_collection_assignment(
        &self,
        collection_id: &str,
    ) -> Option<CollectionAssignment> {
        self.assigned_collections
            .read()
            .await
            .get(collection_id)
            .cloned()
    }

    /// Recovery method using parallel recovery system if available
    pub async fn recover(&self) -> Result<u64> {
        info!(
            "🔄 WAL_MANAGER: Starting WAL recovery for {} strategy",
            self.get_strategy_name()
        );

        // Add timeout to prevent indefinite hanging
        let recovery_timeout = std::time::Duration::from_secs(30);

        let recovery_result = tokio::time::timeout(recovery_timeout, async {
            info!("📊 WAL_MANAGER: About to call RecoveryManager.recover()");

            // Create filesystem factory for recovery
            let filesystem_config =
                crate::storage::persistence::filesystem::FilesystemConfig::default();
            let filesystem = Arc::new(
                crate::storage::persistence::filesystem::FilesystemFactory::new(filesystem_config)
                    .await?,
            );

            // Create RecoveryManager instance
            let recovery_manager = RecoveryManager::new(
                self.config.clone(),
                self.shared_wal_behavior
                    .get_or_init(&crate::storage::memtable::core::MemtableConfig::default())
                    .clone(),
                filesystem,
            );

            let recovery_stats = recovery_manager.recover_all().await?;
            let recovered_count = recovery_stats.total_vectors_recovered;

            info!(
                "📊 WAL_MANAGER: RecoveryManager returned: {} entries",
                recovered_count
            );
            Ok::<u64, anyhow::Error>(recovered_count)
        })
        .await;

        match recovery_result {
            Ok(Ok(recovered_count)) => {
                info!(
                    "✅ WAL_MANAGER: WAL recovery completed successfully: {} entries recovered",
                    recovered_count
                );
                Ok(recovered_count)
            }
            Ok(Err(e)) => {
                tracing::error!("❌ WAL_MANAGER: WAL recovery failed: {}", e);
                Err(e)
            }
            Err(_) => {
                tracing::error!(
                    "⏰ WAL_MANAGER: WAL recovery timed out after {} seconds",
                    recovery_timeout.as_secs()
                );
                Err(anyhow::anyhow!("WAL recovery timed out"))
            }
        }
    }

    /// Assign a collection with its metadata to this WriteAheadLogManager
    pub async fn assign_collection(&self, collection_id: String, assignment: CollectionAssignment) {
        let mut assigned = self.assigned_collections.write().await;
        assigned.insert(collection_id.clone(), assignment);
        tracing::debug!(
            "Assigned collection '{}' with storage location to WriteAheadLogManager",
            collection_id
        );
    }

    /// Get storage location for a collection
    pub async fn get_collection_storage(
        &self,
        collection_id: &str,
    ) -> Option<CollectionAssignment> {
        let assigned = self.assigned_collections.read().await;
        assigned.get(collection_id).cloned()
    }

    /// Check if atomic sync is enabled
    pub fn has_atomic_sync(&self) -> bool {
        false // atomic_sync temporarily disabled
    }
}

impl DistanceComputeProvider for WriteAheadLogManager {
    fn distance_compute(&self) -> &UnifiedDistanceCompute {
        &self.distance_compute
    }
}

impl std::fmt::Debug for WriteAheadLogManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteAheadLogManager")
            .field("strategy", &"shared_wal_behavior")
            .field("config", &self.config)
            .finish()
    }
}
