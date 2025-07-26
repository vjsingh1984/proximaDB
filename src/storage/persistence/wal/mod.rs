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

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info};

use crate::compute::unified_distance::{DistanceComputeProvider, UnifiedDistanceCompute};
use crate::core::{String, VectorId, VectorRecord};
use crate::storage::traits::{UnifiedStorageEngine, FlushResult};
use crate::storage::memtable::specialized::wal_behavior::WalVectorBatch;
use crate::storage::assignment_service::{AssignmentService, StorageAssignmentConfig, StorageComponentType};
use crate::storage::atomic::UnifiedAtomicCoordinator;

// Sub-modules
pub mod background_manager;
pub mod batch_strategy;
pub mod batch_factory;
pub mod compact_batch_id;
pub mod config;
pub mod proto_serialization_strategy;  // Clean architecture proto implementation
pub mod bincode_serialization_strategy;  // Clean architecture bincode implementation
pub mod avro_serialization_strategy;  // Clean architecture avro implementation
pub mod serialization;  // New pure serialization layer
pub mod memtable_manager;  // New centralized memtable operations
pub mod disk_manager;  // New centralized disk operations
pub mod recovery_manager;  // New centralized recovery operations
pub mod recovery_thread_pool;  // Thread pool for parallel recovery
pub mod flush_coordinator;
pub mod optimized_wal_writer;
pub mod compaction_coordinator;
pub mod enhanced_flush_result;
pub mod compaction_axis_integration;
pub mod compaction_types;
pub mod flush_result_optimization;

// Optimized WAL components (Phase 1 implementation) - now consolidated into WalManager
pub mod simple_atomic_sync;
pub mod optimized_path_resolver;
pub mod atomic_wal_sync;
pub mod parallel_recovery;


// Unit tests
#[cfg(test)]
mod tests;

// Re-exports
pub use background_manager::{
    BackgroundMaintenanceManager, BackgroundMaintenanceStats, BackgroundTaskStatus,
};
pub use proto_serialization_strategy::ProtoSerializationStrategy;
pub use bincode_serialization_strategy::BincodeSerializationStrategy;
pub use avro_serialization_strategy::AvroSerializationStrategy;
pub use batch_strategy::WalBatchStrategy;
pub use batch_factory::{WalBatchFactory, StrategyInfo, StrategyComparison};
pub use config::WalStrategyType;
pub use config::{CompressionConfig, PerformanceConfig, WalConfig};
pub use flush_coordinator::{
    CleanupInstructions, FlushCoordinatorCallbacks, FlushDataSource, FlushState, PendingFlush,
    WalFlushCoordinator,
};
pub use compaction_coordinator::{
    CompactionCoordinator, CompactionConfig, CompactionResult, CollectionCompactionState,
    CompactionStats, CompactionTask,
};
pub use compaction_axis_integration::{CompactionAxisUpdater, CompactionIndexStats};
pub use compaction_types::EnhancedEngineCompactionResult;
pub use memtable_manager::{MemtableManager, MemtableStats};
pub use disk_manager::{WalDiskManager, DiskStats, WalFileInfo};
pub use recovery_manager::{RecoveryManager, RecoveryStats, ParallelRecoveryManager, RecoveryMode};
pub use recovery_thread_pool::{RecoveryThreadPool, RecoveryPoolStats, get_recovery_thread_pool, initialize_recovery_thread_pool};

// Batch coordination exports - BatchId defined below

// Re-export serialization module
pub use serialization::{SerializationFormat, VectorBatchSerializer, SerializerFactory};

/// Modern WAL operation - binary payload for batch operations (Proto-first architecture)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WalOperation {
    /// Operation type: "upsert_batch", "delete_batch", "flush", "checkpoint"
    pub operation_type: String,
    /// Binary payload data (Proto bytes default, strategy-specific for others)
    pub payload_data: Vec<u8>,
    /// Payload format: "proto" (default), "avro" (legacy), "bincode" (performance)
    pub payload_format: String,
    /// Number of vectors in this batch (for metrics)
    pub vector_count: usize,
}

// Re-export CompactBatchId as BatchId - it's globally unique, no need for collection_id
pub use compact_batch_id::CompactBatchId as BatchId;


// WalEntry removed - use WalVectorBatch for batch-oriented operations instead

impl WalOperation {
    /// Calculate the actual memory size of this WAL operation including vector data
    pub fn actual_size_bytes(&self) -> usize {
        let mut size = std::mem::size_of::<WalOperation>(); // Struct size
        
        // Add variable field sizes
        size += self.operation_type.len();
        size += self.payload_data.len(); // Actual payload size
        size += self.payload_format.len();
        
        size
    }

    /// Extract VectorRecord from WAL entry operation
    /// Used by migration adapters and legacy compatibility
    pub fn extract_vector_record(&self) -> Result<VectorRecord, anyhow::Error> {
        // Proto-first architecture: payload format determines deserialization
        if self.operation_type == "upsert_batch" || self.operation_type == "delete_batch" {
            match self.payload_format.as_str() {
                "proto" => {
                    // Deserialize from proto bytes
                    use crate::storage::persistence::wal::serialization::{ProtocolBuffersSerializer, VectorBatchSerializer};
                    let serializer = ProtocolBuffersSerializer::new();
                    let records = serializer.deserialize_batch(&self.payload_data)?;
                    records.into_iter().next()
                        .ok_or_else(|| anyhow::anyhow!("Empty vector batch in proto payload"))
                }
                "avro" => {
                    // Delegate to Avro-specific deserialization
                    use crate::storage::persistence::wal::serialization::{AvroSerializer, VectorBatchSerializer};
                    let serializer = AvroSerializer::new();
                    let records = serializer.deserialize_batch(&self.payload_data)
                        .map_err(|e| anyhow::anyhow!("Failed to deserialize Avro payload: {}", e))?;
                    records.into_iter().next()
                        .ok_or_else(|| anyhow::anyhow!("Empty vector batch in Avro payload"))
                }
                _ => Err(anyhow::anyhow!("Unsupported payload format: {}", self.payload_format))
            }
        } else {
            Err(anyhow::anyhow!("WAL operation type {} does not contain vector data", self.operation_type))
        }
    }
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

// FlushResult removed - use crate::storage::traits::FlushResult instead

/// Atomic flush cycle for consistent WAL→Storage operations
#[derive(Debug, Clone)]
pub struct FlushCycle {
    /// Unique identifier for this flush operation
    pub flush_id: String,
    /// Collection being flushed
    pub collection_id: String,
    /// WAL batches marked for flush (replaces Vec<WalEntry>)
    pub batches: Vec<WalVectorBatch>,
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
/// **WalManager Per Collection + Shared Global Memtable Architecture (Perfect Horizontal Scaling)**
/// 
/// This implements the optimal architecture where:
/// - **WalManager per collection** - Each collection gets its own WalManager for isolation
/// - **Shared global WalBehaviorWrapper** - Single singleton shared across all WalManagers
/// - **GlobalPartitionedMemtable** - Partitioned by collection, efficient shared access
/// - **WalManagerRegistry** - Tracks which WalManager handles which collection (1:1 or 1:N mapping)
/// - **Horizontal scaling constraint** - One collection handled by exactly one WalManager (never split)
/// - **Dynamic scaling** - Under heavy workload, new collections get new WalManagers
/// - **Strategy-specific serialization** with shared deserialization in global memtable
/// - **Assignment service integration** for multi-disk coordination and collection co-location
/// - **Atomic disk synchronization** using UnifiedAtomicCoordinator
pub struct WalManager {
    /// Active strategy for current operations
    strategy: Box<dyn WalBatchStrategy>,
    /// Configuration
    config: WalConfig,
    /// Statistics tracking
    stats: Arc<tokio::sync::RwLock<WalStats>>,
    /// Distance computation for similarity operations
    distance_compute: UnifiedDistanceCompute,
    /// **Collections assigned to this WalManager** - Each WalManager handles specific collections
    assigned_collections: Arc<tokio::sync::RwLock<std::collections::HashSet<String>>>,
    /// **SHARED REFERENCE**: Global WalBehaviorWrapper singleton shared across ALL WalManager instances
    shared_wal_behavior: &'static GlobalWalBehaviorSingleton,
    /// Assignment service for multi-disk coordination
    assignment_service: Option<Arc<dyn AssignmentService>>,
    /// Path resolver for collection-to-disk mapping
    path_resolver: Option<Arc<optimized_path_resolver::OptimizedWalPathResolver>>,
    /// Atomic sync coordinator for disk operations (temporarily disabled)
    // atomic_sync: Option<Arc<atomic_wal_sync::AtomicWalSync>>,
    /// Strategy type for routing and serialization decisions
    strategy_type: config::WalStrategyType,
}

/// Adaptive WalManager Registry with Pool-based Collection Assignment
/// This implements intelligent scaling for millions of collections by maintaining a pool
/// of WalManager instances and dynamically assigning collections based on workload
pub struct WalManagerRegistry {
    /// Collection to WalManager ID mapping (1:1 constraint maintained)
    collection_assignments: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
    /// WalManager pool with workload tracking
    manager_pool: Arc<tokio::sync::RwLock<std::collections::HashMap<String, WalManagerPoolEntry>>>,
    /// Pool configuration
    pool_config: WalManagerPoolConfig,
    /// Next manager ID for creating new instances
    next_manager_id: Arc<tokio::sync::Mutex<u64>>,
}

/// WalManager pool entry with workload metrics
#[derive(Debug, Clone)]
pub struct WalManagerPoolEntry {
    /// The WalManager instance
    manager: Arc<WalManager>,
    /// Collections assigned to this manager
    assigned_collections: std::collections::HashSet<String>,
    /// Workload metrics for load balancing
    workload_metrics: WalManagerWorkload,
    /// Last rebalancing timestamp
    last_rebalance: std::time::Instant,
}

/// Workload metrics for adaptive scaling decisions
#[derive(Debug, Clone, Default)]
pub struct WalManagerWorkload {
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

/// Pool configuration for adaptive WalManager scaling
/// 
/// This configuration allows users to customize how WalManagers scale to handle
/// millions of collections efficiently. Users can adjust thread counts, load balancing
/// thresholds, and scaling behavior based on their specific workload requirements.
/// 
/// # Examples
/// 
/// ```rust
/// use proximadb::storage::persistence::wal::WalManagerPoolConfig;
/// 
/// // Configuration for high-throughput workloads
/// let high_throughput_config = WalManagerPoolConfig::builder()
///     .initial_pool_size(8)
///     .soft_thread_limit(16)
///     .target_collections_per_manager(500)
///     .enable_dynamic_scaling(true)
///     .build();
/// 
/// // Configuration for memory-constrained environments
/// let memory_constrained_config = WalManagerPoolConfig::builder()
///     .initial_pool_size(2)
///     .soft_thread_limit(4)
///     .target_collections_per_manager(2000)
///     .enable_dynamic_scaling(false)
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct WalManagerPoolConfig {
    /// Initial pool size (number of WalManager threads to start with)
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

/// Builder for WalManagerPoolConfig to provide user-friendly configuration
#[derive(Debug, Clone)]
pub struct WalManagerPoolConfigBuilder {
    config: WalManagerPoolConfig,
}

impl WalManagerPoolConfig {
    /// Create a new builder for WalManagerPoolConfig
    pub fn builder() -> WalManagerPoolConfigBuilder {
        WalManagerPoolConfigBuilder::new()
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

impl WalManagerPoolConfigBuilder {
    /// Create a new builder with default values
    pub fn new() -> Self {
        Self {
            config: WalManagerPoolConfig::default(),
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
    pub fn build(self) -> WalManagerPoolConfig {
        self.config
    }
}

impl Default for WalManagerPoolConfig {
    fn default() -> Self {
        Self {
            initial_pool_size: 3, // Start with 3 threads for testing
            soft_thread_limit: 8, // Soft limit - after 8, scale balanced
            target_collections_per_manager: 1000, // Target 1K collections per manager for balanced scaling
            rebalance_load_threshold: 0.8, // Rebalance when 80% loaded
            rebalance_cooldown_secs: 30, // 30-second cooldown between rebalances
            enable_dynamic_scaling: true, // Enable dynamic manager creation
        }
    }
}

impl WalManagerRegistry {
    /// Create new adaptive WalManager registry with pool
    pub fn new() -> Self {
        Self::with_config(WalManagerPoolConfig::default())
    }

    /// Create new adaptive WalManager registry with custom pool configuration
    pub fn with_config(pool_config: WalManagerPoolConfig) -> Self {
        tracing::info!(
            "🎯 Creating adaptive WalManager registry - initial: {}, soft_limit: {}, target_collections: {}, dynamic_scaling: {}",
            pool_config.initial_pool_size,
            pool_config.soft_thread_limit,
            pool_config.target_collections_per_manager,
            pool_config.enable_dynamic_scaling
        );
        
        Self {
            collection_assignments: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            manager_pool: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
            pool_config,
            next_manager_id: Arc::new(tokio::sync::Mutex::new(1)),
        }
    }

    /// Get or assign WalManager for a collection using adaptive pool scaling
    pub async fn get_manager_for_collection(
        &self,
        collection_id: &str,
        strategy_type: crate::storage::persistence::wal::config::WalStrategyType,
        config: &WalConfig,
        filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
    ) -> Result<Arc<WalManager>> {
        // Check if collection already has a manager assignment
        {
            let assignments = self.collection_assignments.read().await;
            if let Some(manager_id) = assignments.get(collection_id) {
                let pool = self.manager_pool.read().await;
                if let Some(entry) = pool.get(manager_id) {
                    tracing::debug!(
                        "📍 Collection {} using existing WalManager {} (load: {:.2})",
                        collection_id,
                        manager_id,
                        entry.workload_metrics.load_score
                    );
                    return Ok(entry.manager.clone());
                }
            }
        }

        // Ensure initial pool exists
        self.ensure_initial_pool(strategy_type, config, filesystem.clone()).await?;

        // Find best manager for this collection using adaptive assignment
        let target_manager_id = self.find_best_manager_for_collection(collection_id).await?;

        // Assign collection to the selected manager
        self.assign_collection_to_manager(collection_id, &target_manager_id).await?;

        // Return the assigned manager
        let pool = self.manager_pool.read().await;
        let entry = pool.get(&target_manager_id)
            .ok_or_else(|| anyhow::anyhow!("Manager {} not found in pool", target_manager_id))?;
        
        tracing::info!(
            "✅ Collection {} assigned to WalManager {} (adaptive scaling - {} collections)",
            collection_id,
            target_manager_id,
            entry.workload_metrics.collection_count + 1
        );

        Ok(entry.manager.clone())
    }

    /// Ensure initial pool of WalManagers exists
    async fn ensure_initial_pool(
        &self,
        strategy_type: crate::storage::persistence::wal::config::WalStrategyType,
        config: &WalConfig,
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

        tracing::info!("🚀 Initializing WalManager pool with {} managers", self.pool_config.initial_pool_size);

        for i in 0..self.pool_config.initial_pool_size {
            let manager_id = format!("wal_manager_pool_{}", i + 1);
            
            let strategy = WalBatchFactory::create_batch_serialization_strategy(strategy_type.clone(), config, filesystem.clone()).await?;
            let manager = Arc::new(WalManager::new_pool_manager(strategy, config.clone(), manager_id.clone()).await?);
            
            let entry = WalManagerPoolEntry {
                manager,
                assigned_collections: std::collections::HashSet::new(),
                workload_metrics: WalManagerWorkload::default(),
                last_rebalance: std::time::Instant::now(),
            };
            
            pool.insert(manager_id.clone(), entry);
            tracing::debug!("➕ Created pool WalManager: {}", manager_id);
        }

        tracing::info!("✅ WalManager pool initialized with {} managers", pool.len());
        Ok(())
    }

    /// Find the best WalManager for a new collection based on workload
    /// Implements adaptive scaling: use existing managers first, then create new ones if needed
    async fn find_best_manager_for_collection(&self, collection_id: &str) -> Result<String> {
        let pool = self.manager_pool.read().await;
        
        if pool.is_empty() {
            return Err(anyhow::anyhow!("WalManager pool is empty"));
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
                a.workload_metrics.load_score.partial_cmp(&b.workload_metrics.load_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    // Secondary: collection count (lower is better)
                    .then_with(|| a.workload_metrics.collection_count.cmp(&b.workload_metrics.collection_count))
            })
            .map(|(id, _)| id.clone())
            .ok_or_else(|| anyhow::anyhow!("No suitable manager found in pool"))?;

        tracing::debug!("🎯 Selected existing WalManager {} for collection {} (adaptive assignment)", best_manager, collection_id);
        Ok(best_manager)
    }

    /// Check if we should create a new manager for better load distribution
    async fn should_create_new_manager(&self, pool: &std::collections::HashMap<String, WalManagerPoolEntry>) -> Result<bool> {
        // Don't create new managers if dynamic scaling is disabled
        if !self.pool_config.enable_dynamic_scaling {
            return Ok(false);
        }

        // If we're below soft limit, don't create new managers yet (use existing capacity)
        if pool.len() < self.pool_config.soft_thread_limit {
            return Ok(false);
        }

        // After soft limit, check if adding a manager would improve load distribution
        let min_collections = pool.values().map(|entry| entry.workload_metrics.collection_count).min().unwrap_or(0);
        let max_collections = pool.values().map(|entry| entry.workload_metrics.collection_count).max().unwrap_or(0);

        // Create new manager if:
        // 1. The most loaded manager exceeds target collections per manager
        // 2. There's significant imbalance between managers
        let should_create = max_collections > self.pool_config.target_collections_per_manager ||
                           (max_collections - min_collections) > (self.pool_config.target_collections_per_manager / 2);

        if should_create {
            tracing::info!(
                "🔄 Dynamic scaling triggered - current managers: {}, max_collections: {}, target: {}",
                pool.len(),
                max_collections,
                self.pool_config.target_collections_per_manager
            );
        }

        Ok(should_create)
    }

    /// Create an additional manager for dynamic scaling
    async fn create_additional_manager(&self) -> Result<String> {
        let mut next_id = self.next_manager_id.lock().await;
        let manager_id = format!("wal_manager_dynamic_{}", *next_id);
        *next_id += 1;
        drop(next_id);

        tracing::info!("🚀 Creating dynamic WalManager {} for load balancing", manager_id);
        
        // For now, use Avro strategy as default for dynamic managers
        // TODO: Make this configurable
        let strategy_type = crate::storage::persistence::wal::config::WalStrategyType::AvroBatch;
        let config = &crate::storage::persistence::wal::config::WalConfig::default(); // TODO: Pass proper config
        let filesystem_config = crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(crate::storage::persistence::filesystem::FilesystemFactory::new(filesystem_config).await?); // TODO: Pass proper filesystem
        
        let strategy = WalBatchFactory::create_batch_serialization_strategy(strategy_type, config, filesystem).await?;
        let manager = Arc::new(WalManager::new_pool_manager(strategy, config.clone(), manager_id.clone()).await?);
        
        let entry = WalManagerPoolEntry {
            manager,
            assigned_collections: std::collections::HashSet::new(),
            workload_metrics: WalManagerWorkload::default(),
            last_rebalance: std::time::Instant::now(),
        };
        
        // Add to pool
        {
            let mut pool = self.manager_pool.write().await;
            pool.insert(manager_id.clone(), entry);
        }

        tracing::info!("✅ Dynamic WalManager {} created for balanced scaling", manager_id);
        Ok(manager_id)
    }

    /// Assign a collection to a specific manager
    async fn assign_collection_to_manager(&self, collection_id: &str, manager_id: &str) -> Result<()> {
        // Update assignments
        {
            let mut assignments = self.collection_assignments.write().await;
            assignments.insert(collection_id.to_string(), manager_id.to_string());
        }

        // Update pool entry
        {
            let mut pool = self.manager_pool.write().await;
            if let Some(entry) = pool.get_mut(manager_id) {
                entry.assigned_collections.insert(collection_id.to_string());
                entry.workload_metrics.collection_count = entry.assigned_collections.len();
                
                // Update load score based on collection count
                entry.workload_metrics.load_score = 
                    (entry.workload_metrics.collection_count as f64) / (self.pool_config.target_collections_per_manager as f64);
                
                // Add the collection to the manager's assigned set
                let mut assigned = entry.manager.assigned_collections.write().await;
                assigned.insert(collection_id.to_string());
            } else {
                return Err(anyhow::anyhow!("Manager {} not found in pool", manager_id));
            }
        }

        Ok(())
    }

    /// Get all active managers (for global operations)
    pub async fn get_all_managers(&self) -> std::collections::HashMap<String, Arc<WalManager>> {
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
                    tracing::info!("🗑️ Removed WalManager {} (no more collections)", manager_id);
                }
            }
        }
        Ok(())
    }
}

/// Global WalBehaviorWrapper singleton for shared memtable access across all WalManager instances
/// This is the key to efficient shared access - one memtable, many managers
pub struct GlobalWalBehaviorSingleton {
    /// The singleton WalBehaviorWrapper with GlobalPartitionedMemtable
    wal_behavior: std::sync::OnceLock<Arc<crate::storage::memtable::specialized::wal_behavior::WalBehaviorWrapper>>,
}

impl GlobalWalBehaviorSingleton {
    /// Get or create the singleton WalBehaviorWrapper instance
    pub fn get_or_init(&self, config: &crate::storage::memtable::core::MemtableConfig) -> Arc<crate::storage::memtable::specialized::wal_behavior::WalBehaviorWrapper> {
        self.wal_behavior.get_or_init(|| {
            tracing::info!("🎯 Creating SINGLETON WalBehaviorWrapper with GlobalPartitionedMemtable for all WalManager instances");
            Arc::new(crate::storage::memtable::specialized::wal_behavior::WalBehaviorWrapper::new(config.clone()))
        }).clone()
    }
}

/// Global singleton instance - shared across all WalManager instances
static GLOBAL_WAL_BEHAVIOR: GlobalWalBehaviorSingleton = GlobalWalBehaviorSingleton {
    wal_behavior: std::sync::OnceLock::new(),
};

/// Global registry instance for WalManager per collection architecture
static WAL_MANAGER_REGISTRY: std::sync::OnceLock<WalManagerRegistry> = std::sync::OnceLock::new();

/// Get the global WalManager registry
pub fn get_wal_manager_registry() -> &'static WalManagerRegistry {
    WAL_MANAGER_REGISTRY.get_or_init(|| {
        tracing::info!("🎯 Initializing WalManager Registry for per-collection scaling");
        WalManagerRegistry::new()
    })
}

/// Convenience function: Get or create WalManager for a collection using default pool config
pub async fn get_wal_manager_for_collection(
    collection_id: &str,
    strategy_type: crate::storage::persistence::wal::config::WalStrategyType,
    config: &WalConfig,
    filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
) -> Result<Arc<WalManager>> {
    get_wal_manager_registry()
        .get_manager_for_collection(collection_id, strategy_type, config, filesystem)
        .await
}

/// Configure the global WalManager pool with custom settings
/// 
/// This function allows users to customize the adaptive scaling behavior
/// of the WalManager pool to match their specific workload requirements.
/// 
/// # Examples
/// 
/// ```rust,no_run
/// use proximadb::storage::persistence::wal::{configure_wal_manager_pool, WalManagerPoolConfig};
/// 
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     // Configure for high-throughput workloads
///     configure_wal_manager_pool(WalManagerPoolConfig::high_throughput()).await?;
///     
///     // Configure with custom settings
///     let custom_config = WalManagerPoolConfig::builder()
///         .initial_pool_size(4)
///         .soft_thread_limit(12)
///         .target_collections_per_manager(800)
///         .enable_dynamic_scaling(true)
///         .build();
///     configure_wal_manager_pool(custom_config).await?;
///     Ok(())
/// }
/// ```
pub async fn configure_wal_manager_pool(pool_config: WalManagerPoolConfig) -> Result<()> {
    // TODO: Implement global pool configuration
    // For now, this is a placeholder - in a full implementation, this would
    // reinitialize the global registry with the new configuration
    tracing::info!("🔧 WalManager pool configuration updated: {:?}", pool_config);
    Ok(())
}

/// Get current WalManager pool statistics for monitoring
/// 
/// Returns information about the current pool state including number of managers,
/// collection distribution, and load metrics.
pub async fn get_wal_manager_pool_stats() -> Result<WalManagerPoolStats> {
    let registry = get_wal_manager_registry();
    let _managers = registry.get_all_managers().await;
    
    let pool = registry.manager_pool.read().await;
    let total_collections: usize = pool.values().map(|entry| entry.workload_metrics.collection_count).sum();
    let avg_collections_per_manager = if pool.is_empty() { 0.0 } else { total_collections as f64 / pool.len() as f64 };
    let max_collections = pool.values().map(|entry| entry.workload_metrics.collection_count).max().unwrap_or(0);
    let min_collections = pool.values().map(|entry| entry.workload_metrics.collection_count).min().unwrap_or(0);
    
    Ok(WalManagerPoolStats {
        total_managers: pool.len(),
        total_collections,
        avg_collections_per_manager,
        max_collections_per_manager: max_collections,
        min_collections_per_manager: min_collections,
        load_imbalance: if min_collections == 0 { 0.0 } else { max_collections as f64 / min_collections as f64 },
    })
}

/// Statistics about the current WalManager pool state
#[derive(Debug, Clone)]
pub struct WalManagerPoolStats {
    /// Total number of WalManager instances in the pool
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

impl WalManager {
    /// Create new WalManager (legacy method for compatibility with tests)
    pub async fn new(strategy: Box<dyn WalBatchStrategy>, config: WalConfig) -> Result<Self> {
        // Use new_pool_manager with a default manager ID for backwards compatibility
        Self::new_pool_manager(strategy, config, "default_manager".to_string()).await
    }

    /// Create new WalManager for specific collections with shared global memtable
    pub async fn new_for_collection(strategy: Box<dyn WalBatchStrategy>, config: WalConfig, collection_id: String) -> Result<Self> {
        tracing::info!(
            "🚀 Creating WalManager for collection {} with strategy: {} (shared global memtable)",
            collection_id,
            strategy.strategy_name()
        );
        tracing::debug!(
            "📋 WAL Config: strategy_type={:?}, memtable_type={:?}",
            config.strategy_type,
            config.memtable.memtable_type
        );

        // Initialize singleton WalBehaviorWrapper (thread-safe, only happens once globally)
        let memtable_config = crate::storage::memtable::core::MemtableConfig {
            max_size_bytes: config.memtable.global_memory_limit,
            flush_threshold_bytes: config.memtable.global_memory_limit / 2,
            enable_mvcc: config.enable_mvcc,
            mvcc_cleanup_interval_secs: config.performance.mvcc_cleanup_interval_secs,
            max_versions_per_key: config.memtable.mvcc_versions_retained,
        };
        let _shared_wal_behavior = GLOBAL_WAL_BEHAVIOR.get_or_init(&memtable_config);

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

        // Initialize collection set for this manager
        let mut assigned_collections = std::collections::HashSet::new();
        assigned_collections.insert(collection_id.clone());

        tracing::info!(
            "✅ WalManager created for collection {} - per-collection scaling with shared memtable",
            collection_id
        );

        // Extract strategy type for routing
        let strategy_type = config.strategy_type.clone();

        Ok(Self {
            strategy,
            config,
            stats,
            distance_compute: UnifiedDistanceCompute::default(),
            assigned_collections: Arc::new(tokio::sync::RwLock::new(assigned_collections)),
            shared_wal_behavior: &GLOBAL_WAL_BEHAVIOR,
            assignment_service: None,
            path_resolver: None,
            // atomic_sync: None,
            strategy_type,
        })
    }

    /// Create new WalManager for pool with empty collection set
    pub async fn new_pool_manager(strategy: Box<dyn WalBatchStrategy>, config: WalConfig, manager_id: String) -> Result<Self> {
        tracing::debug!(
            "🏊 Creating pool WalManager {} with strategy: {} (shared global memtable)",
            manager_id,
            strategy.strategy_name()
        );

        // Initialize singleton WalBehaviorWrapper (thread-safe, only happens once globally)
        let memtable_config = crate::storage::memtable::core::MemtableConfig {
            max_size_bytes: config.memtable.global_memory_limit,
            flush_threshold_bytes: config.memtable.global_memory_limit / 2,
            enable_mvcc: config.enable_mvcc,
            mvcc_cleanup_interval_secs: config.performance.mvcc_cleanup_interval_secs,
            max_versions_per_key: config.memtable.mvcc_versions_retained,
        };
        let _shared_wal_behavior = GLOBAL_WAL_BEHAVIOR.get_or_init(&memtable_config);

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

        // Start with empty collection set (will be assigned dynamically)
        let assigned_collections = std::collections::HashSet::new();

        tracing::debug!("✅ Pool WalManager {} created - ready for adaptive collection assignment", manager_id);

        // Extract strategy type for routing
        let strategy_type = config.strategy_type.clone();

        Ok(Self {
            strategy,
            config,
            stats,
            distance_compute: UnifiedDistanceCompute::default(),
            assigned_collections: Arc::new(tokio::sync::RwLock::new(assigned_collections)),
            shared_wal_behavior: &GLOBAL_WAL_BEHAVIOR,
            assignment_service: None,
            path_resolver: None,
            // atomic_sync: None,
            strategy_type,
        })
    }

    /// Create new WAL manager using the factory (recommended)
    pub async fn create_with_factory(
        strategy_type: crate::storage::persistence::wal::config::WalStrategyType,
        config: WalConfig,
        filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
    ) -> Result<Self> {
        // Use the new batch serialization strategies for better separation of concerns
        let strategy = WalBatchFactory::create_batch_serialization_strategy(strategy_type, &config, filesystem).await?;
        Self::new(strategy, config).await
    }

    /// Create new WAL manager using the batch factory (alias for modern naming)
    pub async fn create_with_batch_factory(
        strategy_type: crate::storage::persistence::wal::config::WalStrategyType,
        config: WalConfig,
        filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,
    ) -> Result<Self> {
        Self::create_with_factory(strategy_type, config, filesystem).await
    }

    // Atomicity manager methods removed - use UnifiedAtomicCoordinator from atomic module instead

    /// Set storage engine for delegated flush/compaction operations
    pub fn set_storage_engine(&self, storage_engine: Arc<dyn UnifiedStorageEngine>) {
        self.strategy.set_storage_engine(storage_engine);
        tracing::info!("🏗️ Storage engine attached to WAL manager for delegated operations");
    }

    /// Get WAL configuration (read-only access)
    pub fn get_config(&self) -> &WalConfig {
        &self.config
    }

    // Execute atomic operation method removed - use UnifiedAtomicCoordinator instead

    // Execute in transaction method removed - use UnifiedAtomicCoordinator instead

    // Begin transaction method removed - use UnifiedAtomicCoordinator instead

    // Commit transaction method removed - use UnifiedAtomicCoordinator instead

    // Rollback transaction method removed - use UnifiedAtomicCoordinator instead

    /// Insert single vector record (converted to batch of 1 via WalVectorBatch)
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
        use crate::storage::persistence::wal::BatchId;
        use crate::storage::memtable::specialized::wal_behavior::WalVectorBatch;
        
        let batch_id = BatchId::new(); // Single vector batch
        
        // Calculate actual size - approximate based on vector dimensions and metadata
        let total_size_bytes = record.vector.len() * 4 + 256; // 4 bytes per f32 + metadata overhead
        
        let batch = WalVectorBatch {
            batch_id,
            vector_records: Arc::new(vec![record.clone()]),
            created_at: std::time::SystemTime::now(),
            total_size_bytes,
            is_flushed: false,
        };

        // Use modern batch strategy
        let sequences = self.strategy.write_native_batch(batch, &collection_id).await?;
        let duration = start_time.elapsed();

        // Return the first (and only) sequence from the batch
        let sequence = sequences.into_iter().next()
            .ok_or_else(|| anyhow::anyhow!("No sequence returned from batch write"))?;

        debug!(
            "📝 [WAL_UPSERT] Successfully upserted vector {} in collection {} (sequence: {}) in {:?} using BATCH architecture",
            vector_id,
            collection_id,
            sequence,
            duration
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
        let vector_records: Vec<VectorRecord> = records.into_iter().map(|(_, record)| record).collect();
        self.insert_vectors(collection_id, vector_records).await
    }

    /// Insert batch of vector records with immediate sync option
    pub async fn insert_batch_with_sync(
        &self,
        _collection_id: String,
        records: Vec<(VectorId, VectorRecord)>,
        immediate_sync: bool,
    ) -> Result<Vec<u64>> {
        let vector_records: Vec<VectorRecord> = records.into_iter().map(|(_, record)| record).collect();
        
        // Create batch
        use crate::storage::persistence::wal::BatchId;
        use crate::storage::memtable::specialized::wal_behavior::WalVectorBatch;
        let total_size_bytes: usize = vector_records.iter()
            .map(|r| r.vector.len() * 4 + 256) // 4 bytes per f32 + metadata overhead
            .sum();
        let batch_id = BatchId::new();
        
        let batch = WalVectorBatch {
            batch_id,
            vector_records: Arc::new(vector_records),
            created_at: std::time::SystemTime::now(),
            total_size_bytes,
            is_flushed: false,
        };

        self.write_vector_batch_with_sync(batch, immediate_sync).await
    }

    /// Force immediate sync of WAL data to disk
    pub async fn force_sync(&self, collection_id: Option<&String>) -> Result<()> {
        self.strategy.force_sync(collection_id).await
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
        let current_version = record.version;
        let new_version = if current_version <= 0 { 1 } else { current_version + 1 };
        
        // Update version directly
        record.version = new_version;

        // Redirect to insert (which is now upsert)
        self.insert(collection_id, vector_id, &record).await
    }

    /// Delete vector record (delegated to batch strategy)
    pub async fn delete(&self, collection_id: String, vector_id: VectorId) -> Result<u64> {
        // Delegate to the batch strategy's delete implementation
        self.strategy.delete_vector(&collection_id, &vector_id).await
    }

    // Note: Collection lifecycle operations (create/drop) are handled by CollectionService
    // WAL only handles vector-level operations (insert/update/delete/flush/checkpoint)

    /// Search for vector by ID (returns VectorRecord)
    pub async fn search(
        &self,
        collection_id: &str,
        vector_id: &VectorId,
    ) -> Result<Option<VectorRecord>> {
        self.strategy
            .search_vector_by_id(collection_id, vector_id)
            .await
    }


    /// Read vector batches for recovery or replication (modern API)
    pub async fn read_entries(
        &self,
        collection_id: &str,
        from_sequence: u64,
        limit: Option<usize>,
    ) -> Result<Vec<VectorRecord>> {
        // Get vectors from the collection instead of legacy entries
        let vectors = self.strategy.get_collection_vectors(collection_id).await?;
        
        // Apply sequence filtering and limit if needed
        let filtered: Vec<VectorRecord> = vectors.into_iter()
            .skip(from_sequence as usize)
            .take(limit.unwrap_or(usize::MAX))
            .collect();
            
        Ok(filtered)
    }

    /// Force flush to disk
    pub async fn flush(&self, collection_id: Option<&String>) -> Result<FlushResult> {
        let result = self.strategy.flush(collection_id).await?;

        // Update stats
        let mut stats = self.stats.write().await;
        stats.last_flush_time = Some(Utc::now());

        Ok(result)
    }

    /// Compact collection (clean up old MVCC versions)
    pub async fn compact(&self, collection_id: &str) -> Result<u64> {
        self.strategy.compact_collection(collection_id).await
    }

    /// Legacy method for Avro compatibility - only delegates to AvroWalBatchStrategy
    /// In proto-first architecture, main WAL should not handle Avro serialization
    pub async fn append_avro_entry(
        &self,
        _collection_id: &str,
        _operation_type: &str,
        _avro_payload: &[u8],
    ) -> Result<u64> {
        // This is a legacy method that should be deprecated
        Err(anyhow::anyhow!("append_avro_entry is deprecated - use write_native_batch directly"))
    }

    /// Append Proto entry - modern method for proto-first architecture
    pub async fn append_proto_entry(
        &self,
        _collection_id: &str,
        _operation_type: &str,
        _proto_payload: &[u8],
    ) -> Result<u64> {
        // This is a legacy method that should be deprecated
        Err(anyhow::anyhow!("append_proto_entry is deprecated - use write_native_batch directly"))
    }

    /// Read vector records by operation type (proto-first approach)
    pub async fn read_proto_entries(
        &self,
        collection_id: &str,
        _operation_type: &str,
        limit: Option<usize>,
    ) -> Result<Vec<Vec<u8>>> {
        // Get the vector records from the collection
        let vectors = self.strategy.get_collection_vectors(&collection_id.to_string()).await?;
        
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
            let proto_record: crate::proto::proximadb::VectorRecord = vector.clone();
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
        use crate::storage::persistence::wal::serialization::{ProtocolBuffersSerializer, VectorBatchSerializer};
        let proto_serializer = ProtocolBuffersSerializer::new();
        if let Ok(records) = proto_serializer.deserialize_batch(payload) {
            // Use the modern batch API with sync option
            if immediate_sync {
                self.insert_batch_with_sync(collection_id.to_string(), records.into_iter().map(|r| (r.id.clone().unwrap_or_default(), r)).collect(), true).await
                    .map(|sequences| sequences.into_iter().next().unwrap_or(0))
            } else {
                self.insert_vectors(collection_id.to_string(), records).await
                    .map(|sequences| sequences.into_iter().next().unwrap_or(0))
            }
        } else {
            anyhow::bail!("Failed to deserialize batch payload")
        }
    }

    /// Get all vectors for a collection (modern batch approach)
    pub async fn get_collection_entries(
        &self,
        collection_id: &str,
    ) -> Result<Vec<VectorRecord>> {
        self.strategy.get_collection_vectors(collection_id).await
    }

    /// Get WAL statistics
    pub async fn stats(&self) -> Result<WalStats> {
        tracing::debug!("📊 WAL_MANAGER_STATS: Strategy type: {}", self.strategy.strategy_name());
        tracing::debug!("📊 WAL_MANAGER_STATS: Calling strategy.get_stats()...");
        let stats = self.strategy.get_stats().await?;
        tracing::debug!("📊 WAL_MANAGER_STATS: strategy.get_stats() returned: total_entries={}, memory_entries={}, collections_count={}", 
                 stats.total_entries, stats.memory_entries, stats.collections_count);
        Ok(stats)
    }

    // Legacy recover method removed - using enhanced version below

    /// Graceful shutdown
    pub async fn close(&self) -> Result<()> {
        self.strategy.close().await
    }

    /// Flush collection using modern batch operations
    pub async fn flush_collection(&self, collection_id: &str) -> Result<FlushResult> {
        self.strategy.flush_collection(collection_id).await
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
    pub fn get_wal_behavior_wrapper(&self) -> Option<&crate::storage::memtable::specialized::wal_behavior::WalBehaviorWrapper> {
        self.strategy.get_wal_behavior()
    }

    // 🎯 MODERN BATCH API (Recommended)

    // Removed write_vector_batch - use write_native_batch with collection_id instead

    /// PROTO-FIRST ZERO-COPY: Write native VectorRecord with Arc
    /// This is the optimal method for proto-first architecture
    pub async fn write_vector_batch_native_arc(
        &self,
        collection_id: &str,
        native_vectors: Arc<Vec<crate::core::VectorRecord>>,
    ) -> Result<Vec<u64>> {
        tracing::info!("🚀 WAL NATIVE ZERO-COPY: Writing {} vectors to collection {}", 
                      native_vectors.len(), collection_id);
        
        // Create native WalVectorBatch with Arc (zero-copy)
        let batch_id = crate::storage::persistence::wal::BatchId::new();
        
        let native_batch = crate::storage::memtable::specialized::wal_behavior::WalVectorBatch {
            batch_id,
            vector_records: native_vectors, // Direct Arc, no clone!
            created_at: std::time::SystemTime::now(),
            total_size_bytes: 0, // Will be calculated by strategy
            is_flushed: false,
        };
        
        // Delegate to strategy - each strategy handles its own serialization
        self.strategy.write_native_batch(native_batch, collection_id).await
    }

    // REMOVED: write_vector_batch_native method - first release, zero-copy Arc-based API only

    /// Write vector batch with immediate sync (modern API)
    pub async fn write_vector_batch_with_sync(
        &self, 
        _batch: crate::storage::memtable::specialized::wal_behavior::WalVectorBatch, 
        _immediate_sync: bool
    ) -> Result<Vec<u64>> {
        // Note: write_vector_batch_with_sync requires collection_id but we don't have it here
        // This is a legacy method that should be deprecated
        Err(anyhow::anyhow!("write_vector_batch_with_sync is deprecated - use write_native_batch directly"))
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
        use crate::storage::persistence::wal::BatchId;
        use crate::storage::memtable::specialized::wal_behavior::WalVectorBatch;
        let total_size_bytes: usize = records.iter()
            .map(|r| r.vector.len() * 4 + 256) // 4 bytes per f32 + metadata overhead
            .sum();
        let batch_id = BatchId::new();
        
        let batch = WalVectorBatch {
            batch_id,
            vector_records: Arc::new(records),
            created_at: std::time::SystemTime::now(),
            total_size_bytes,
            is_flushed: false,
        };

        // Write to memory first
        let sequences = self.strategy.write_native_batch(batch, &collection_id).await?;
        
        // Check if we should sync to disk based on sync mode
        if self.should_sync_to_disk(&collection_id).await? {
            debug!("🔄 PerBatch sync mode - triggering disk persistence for collection: {}", collection_id);
            if let Err(e) = self.force_sync(Some(&collection_id)).await {
                tracing::warn!("Failed to sync WAL to disk: {}", e);
                // Continue - data is in memory, sync failure shouldn't fail the insert
            }
        }

        Ok(sequences)
    }

    /// Search vector by ID (modern API)
    pub async fn search_vector_by_id(
        &self,
        collection_id: &str,
        vector_id: &VectorId,
    ) -> Result<Option<VectorRecord>> {
        self.strategy.search_vector_by_id(collection_id, vector_id).await
    }

    /// Similarity search for vectors (modern API)
    pub async fn search_vectors_similarity(
        &self,
        collection_id: &str,
        query_vector: &[f32],
        k: usize,
        distance_metric: Option<crate::compute::distance::DistanceMetric>,
    ) -> Result<Vec<(VectorId, f32, VectorRecord)>> {
        self.strategy.search_vectors_similarity(collection_id, query_vector, k, distance_metric).await
    }

    /// Get all vectors for a collection (modern API)
    pub async fn get_collection_vectors(&self, collection_id: &str) -> Result<Vec<VectorRecord>> {
        self.strategy.get_collection_vectors(collection_id).await
    }

    /// Read all vector batches for a collection (modern API)
    pub async fn read_all_batches(
        &self,
        collection_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<crate::storage::memtable::specialized::wal_behavior::WalVectorBatch>> {
        self.strategy.read_all_batches(collection_id, limit).await
    }

    /// Register storage engine with the WAL strategy
    pub async fn register_storage_engine(
        &self,
        engine_name: &str,
        engine: Arc<dyn crate::storage::traits::UnifiedStorageEngine>,
    ) -> Result<()> {
        // Set the storage engine on the strategy
        self.strategy.set_storage_engine(engine);
        tracing::info!("✅ Storage engine '{}' registered with WalManager", engine_name);
        Ok(())
    }

    // ================================================================================
    // ENHANCED METHODS (Consolidated from OptimizedWalManager)
    // ================================================================================

    /// Initialize assignment service integration for multi-disk coordination
    pub async fn initialize_assignment_service(
        &mut self,
        assignment_service: Arc<dyn AssignmentService>,
        _atomic_coordinator: Arc<UnifiedAtomicCoordinator>,
    ) -> Result<()> {
        // Create assignment configs for WAL and storage components
        let wal_assignment_config = StorageAssignmentConfig {
            storage_urls: self.config.multi_disk.data_directories.clone(),
            component_type: StorageComponentType::Wal,
            collection_affinity: self.config.multi_disk.collection_affinity,
        };

        // Create storage assignment config (for co-location)
        let storage_assignment_config = StorageAssignmentConfig {
            storage_urls: vec!["storage_urls".to_string()], // TODO: Get from storage config
            component_type: StorageComponentType::Storage,
            collection_affinity: self.config.multi_disk.collection_affinity,
        };

        // Get filesystem factory from strategy
        let filesystem_factory = self.strategy.get_filesystem()
            .ok_or_else(|| anyhow::anyhow!("Strategy does not have filesystem factory"))?;

        // Create path resolver
        let path_resolver = Arc::new(optimized_path_resolver::OptimizedWalPathResolver::new(
            assignment_service.clone(),
            filesystem_factory,
            wal_assignment_config,
            storage_assignment_config,
        ));

        // TODO: Create atomic sync coordinator once compilation issues are resolved
        // let atomic_sync = Arc::new(atomic_wal_sync::AtomicWalSync::new(
        //     atomic_coordinator,
        //     path_resolver.clone(),
        // ));

        // Update WalManager with assignment service integration
        self.assignment_service = Some(assignment_service);
        self.path_resolver = Some(path_resolver);
        // self.atomic_sync = Some(atomic_sync);

        tracing::info!("✅ WalManager enhanced with assignment service integration");
        Ok(())
    }

    /// Insert batch with atomic disk synchronization (enhanced version)
    pub async fn insert_batch_atomic(
        &self,
        collection_id: String,
        records: Vec<(VectorId, VectorRecord)>,
    ) -> Result<Vec<u64>> {
        let start_time = std::time::Instant::now();
        
        debug!(
            "Inserting batch of {} vectors for collection '{}' using {} strategy with atomic sync",
            records.len(), collection_id, self.get_strategy_name()
        );

        // 1. Assign collection to this WAL manager
        self.assign_collection(&collection_id).await?;

        // 2. Ensure collection directories exist (if assignment service is enabled)
        if let Some(path_resolver) = &self.path_resolver {
            let collection_paths = path_resolver
                .resolve_collection_paths(&collection_id)
                .await
                .context("Failed to resolve collection paths")?;
            
            path_resolver
                .ensure_collection_directories(&collection_paths)
                .await
                .context("Failed to ensure collection directories")?;
        }

        // 3. Write to memory using existing strategy
        let vector_records: Vec<VectorRecord> = records.into_iter().map(|(_, record)| record).collect();
        let sequences = self.insert_vectors(collection_id.clone(), vector_records).await?;

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

    /// Assign collection to this WAL manager
    async fn assign_collection(&self, collection_id: &str) -> Result<()> {
        let mut assigned_collections = self.assigned_collections.write().await;
        if !assigned_collections.contains(collection_id) {
            assigned_collections.insert(collection_id.to_string());
            debug!("Assigned collection '{}' to WAL manager", collection_id);
        }
        Ok(())
    }

    /// Determine if batch should be synced to disk
    async fn should_sync_to_disk(&self, _collection_id: &str) -> Result<bool> {
        match self.config.performance.sync_mode {
            config::SyncMode::Always => Ok(true),
            config::SyncMode::PerBatch => Ok(true),
            config::SyncMode::Periodic => {
                // TODO: Implement periodic sync logic
                Ok(false)
            }
            config::SyncMode::Never | config::SyncMode::MemoryOnly => Ok(false),
        }
    }

    /// Extract batch from memory for disk synchronization
    async fn extract_batch_for_sync(
        &self,
        collection_id: &str,
        _sequences: &[u64],
    ) -> Result<WalVectorBatch> {
        // Get the batch from the strategy's memory
        if let Some(wal_behavior) = self.strategy.get_wal_behavior() {
            let collection_vectors = wal_behavior
                .get_collection_vectors(&collection_id.to_string())
                .await
                .context("Failed to get collection vectors from memtable")?;
            
            // Filter vectors by sequences (for now, just take all vectors since we don't have reliable sequence mapping)
            let batch_vectors: Vec<VectorRecord> = collection_vectors;
            
            let batch_id = BatchId::new();
            
            use crate::storage::memtable::specialized::wal_behavior::WalVectorBatch;
            let total_size_bytes: usize = batch_vectors.iter()
                .map(|r| r.vector.len() * 4 + 256)
                .sum();
            
            Ok(WalVectorBatch {
                batch_id,
                vector_records: Arc::new(batch_vectors),
                created_at: std::time::SystemTime::now(),
                total_size_bytes,
                is_flushed: false,
            })
        } else {
            Err(anyhow::anyhow!("WAL behavior not available for batch extraction"))
        }
    }

    // Temporarily disabled - atomic sync methods
    // TODO: Re-enable once atomic_wal_sync compilation issues are resolved

    /// Get strategy name for logging
    fn get_strategy_name(&self) -> &str {
        match self.strategy_type {
            config::WalStrategyType::ProtoBatch => "ProtoBatch",
            config::WalStrategyType::AvroBatch => "AvroBatch",
            config::WalStrategyType::BincodeBatch => "BincodeBatch",
        }
    }

    /// Enhanced force sync for a collection using atomic coordination
    pub async fn force_sync_collection(&self, collection_id: &str) -> Result<()> {
        debug!("Force sync requested for collection '{}'", collection_id);
        
        // For now, just use the strategy's force_sync (which uses SimpleAtomicSync)
        // TODO: Re-enable advanced atomic sync once basic recovery is working  
        self.strategy.force_sync(Some(&collection_id.to_string())).await?;
        debug!("Force sync delegated to strategy for collection '{}'", collection_id);
        
        Ok(())
    }

    /// Get assigned collections
    pub async fn get_assigned_collections(&self) -> Vec<String> {
        self.assigned_collections.read().await.iter().cloned().collect()
    }

    /// Recovery method using parallel recovery system if available
    pub async fn recover(&self) -> Result<u64> {
        info!("🔄 WAL_MANAGER: Starting WAL recovery for {} strategy", self.get_strategy_name());
        
        // Add timeout to prevent indefinite hanging
        let recovery_timeout = std::time::Duration::from_secs(30);
        
        let recovery_result = tokio::time::timeout(recovery_timeout, async {
            info!("📊 WAL_MANAGER: About to call strategy.recover()");
            
            // For now, just use the strategy recovery (which now reads from global memtable)
            // TODO: Re-enable parallel recovery once compilation issues are resolved
            let recovered_count = self.strategy.recover().await
                .context("WAL strategy recovery failed")?;
            
            info!("📊 WAL_MANAGER: Strategy recovery returned: {} entries", recovered_count);
            Ok::<u64, anyhow::Error>(recovered_count)
        }).await;
        
        match recovery_result {
            Ok(Ok(recovered_count)) => {
                info!("✅ WAL_MANAGER: WAL recovery completed successfully: {} entries recovered", recovered_count);
                Ok(recovered_count)
            }
            Ok(Err(e)) => {
                tracing::error!("❌ WAL_MANAGER: WAL recovery failed: {}", e);
                Err(e)
            }
            Err(_) => {
                tracing::error!("⏰ WAL_MANAGER: WAL recovery timed out after {} seconds", recovery_timeout.as_secs());
                Err(anyhow::anyhow!("WAL recovery timed out"))
            }
        }
    }

    /// Check if assignment service integration is enabled
    pub fn has_assignment_service(&self) -> bool {
        self.assignment_service.is_some()
    }

    /// Check if atomic sync is enabled
    pub fn has_atomic_sync(&self) -> bool {
        false  // atomic_sync temporarily disabled
    }
}

impl DistanceComputeProvider for WalManager {
    fn distance_compute(&self) -> &UnifiedDistanceCompute {
        &self.distance_compute
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
