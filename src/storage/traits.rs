//! Unified Storage Engine Traits with Strategy Pattern
//!
//! This module implements the Strategy Pattern for storage engines, allowing polymorphic
//! selection between different storage backends (VIPER default, LSM alternative).
//! Common operations are implemented in the base trait with default implementations,
//! while specialized engines override only what's unique to their approach.

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use crate::proto::proximadb::Collection;

/// Performance tier hint for storage engines
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PerformanceTier {
    /// Hot data - keep in memory/SSD, optimize for latency
    Hot,
    /// Warm data - balance between latency and cost
    Warm,
    /// Cold data - optimize for cost, higher latency acceptable
    Cold,
    /// Archive data - minimal access, maximum compression
    Archive,
}

impl Default for PerformanceTier {
    fn default() -> Self {
        Self::Warm
    }
}
// Core types imported as needed in implementations

/// Strategy enum for selecting storage engine type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageEngineStrategy {
    /// VIPER: Vector-optimized Intelligent Parquet with Efficient Retrieval (Default)
    Viper,
    /// LSM: Log-Structured Merge Tree (Alternative for comparison)
    Lsm,
    /// PRISM: Progressive Retrieval through Indexed Storage Management (Memory-optimized)
    Prism,
    /// SWIFT: Storage With Instant Fast Traversal (Hierarchical superblock architecture)
    Swift,
    /// NOVA: Next-gen Optimized Vector Analytics (Columnar with quantization)
    Nova,
    /// RAPTOR: Rapid Access Parallel Tiered Object Retrieval (Experimental)
    Raptor,
    /// Hybrid: Uses VIPER for vectors, LSM for metadata (Future)
    Hybrid,
}

impl Default for StorageEngineStrategy {
    fn default() -> Self {
        Self::Viper // VIPER is the default strategy
    }
}

/// Trait for providing collection metadata to storage engines
/// This breaks the circular dependency between StorageEngine and CollectionService
#[async_trait]
pub trait CollectionMetadataProvider: Send + Sync {
    /// Get collection UUID by name or ID
    async fn get_uuid(&self, collection_id: &str) -> Result<Option<String>>;
    
    /// Get full collection metadata
    async fn get_collection_metadata(&self, collection_id: &str) -> Result<Option<Collection>>;
    
    /// Get collection as unified type
    async fn get_collection(&self, collection_id: &str) -> Result<Option<Collection>>;
    
    /// List all collections
    async fn list_collections(&self) -> Result<Vec<Collection>>;
    
    /// Check if collection exists
    async fn collection_exists(&self, collection_id: &str) -> Result<bool> {
        Ok(self.get_uuid(collection_id).await?.is_some())
    }
    
    /// Fast check if collection ID exists (for collision detection)
    /// This should be optimized for speed, returning just bool
    async fn collection_id_exists(&self, collection_id: &str) -> Result<bool> {
        // Default implementation delegates to collection_exists
        // Backends can override with more efficient implementation
        self.collection_exists(collection_id).await
    }
}

/// Unified storage engine trait implementing Strategy Pattern
///
/// Common operations have default implementations that can be overridden.
/// Specialized engines only need to implement core abstract methods.
#[async_trait]
pub trait UnifiedStorageEngine: Send + Sync {
    // =============================================================================
    // ABSTRACT METHODS - Must be implemented by each engine
    // =============================================================================

    /// Engine identification (required)
    fn engine_name(&self) -> &'static str;
    fn engine_version(&self) -> &'static str;
    fn strategy(&self) -> StorageEngineStrategy;

    /// Core flush operation - engine-specific implementation (required)
    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult>;

    /// Core compaction operation - engine-specific implementation (required)
    async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult>;

    /// Engine-specific statistics collection (required)
    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>>;

    /// Retrieve a specific vector by ID from storage (required)
    /// This method should search across all storage layers (memtable, SSTables, Parquet files)
    async fn get_vector_by_id(&self, collection_id: &str, vector_id: &str) -> Result<Option<crate::core::VectorRecord>>;

    /// Engine-specific unified search with optimization capabilities (required)
    /// Each engine implements its own optimizations:
    /// - VIPER: Columnar predicate pushdown, Parquet filtering, ML clustering
    /// - LSM: Bloom filter hints, range scans, SSTable optimizations
    /// - SST: Hierarchical bloom filters, progressive quantization
    /// - NOVA: Extended Parquet statistics, aggressive pruning
    /// 
    /// Uses SearchContext which provides zero-copy access via Arc references
    async fn search_vectors_unified(
        &self,
        ctx: &SearchContext,
    ) -> Result<Vec<crate::core::search::SearchResult>>;
    
    /// Compact a specific collection's data
    /// Returns standard CompactionResult - engines can add vector tracking in engine_metrics
    async fn compact_collection(
        &self,
        collection_id: &str,
        collection_config: Option<&Collection>,
    ) -> Result<CompactionResult> {
        // Default implementation delegates to do_compact with proper parameters
        let params = CompactionParameters {
            collection_id: Some(collection_id.to_string()),
            collection_config: collection_config.cloned(),
            force: false,
            synchronous: true,
            ..Default::default()
        };
        
        self.do_compact(&params).await
    }

    // =============================================================================
    // ENGINE CAPABILITIES - Can be overridden, sensible defaults provided
    // =============================================================================

    // Compression support methods removed - use storage::engine_capabilities::EngineCapabilities instead
    // The centralized EngineCapabilities module provides static methods for checking
    // what compression algorithms and features are supported by each engine type
    // This avoids duplication and provides a single source of truth for capabilities

    /// Engine capabilities with defaults based on strategy
    fn supports_collection_level_operations(&self) -> bool {
        match self.strategy() {
            StorageEngineStrategy::Viper => true, // VIPER supports collection-level ops
            StorageEngineStrategy::Lsm => false,  // LSM operates on entire tree
            StorageEngineStrategy::Hybrid => true, // Hybrid supports collection-level ops
            StorageEngineStrategy::Prism => true, // Prism supports collection-level ops
            StorageEngineStrategy::Swift => true, // SWIFT supports collection-level ops
            StorageEngineStrategy::Nova => true, // NOVA supports collection-level ops
            StorageEngineStrategy::Raptor => true, // RAPTOR supports collection-level ops
        }
    }

    fn supports_atomic_operations(&self) -> bool {
        match self.strategy() {
            StorageEngineStrategy::Viper => true, // VIPER has atomic staging operations
            StorageEngineStrategy::Lsm => false,  // LSM has eventual consistency
            StorageEngineStrategy::Hybrid => true, // Hybrid provides atomic guarantees
            StorageEngineStrategy::Prism => true, // Prism provides atomic guarantees
            StorageEngineStrategy::Swift => true, // SWIFT provides atomic guarantees
            StorageEngineStrategy::Nova => true, // NOVA provides atomic guarantees
            StorageEngineStrategy::Raptor => false, // RAPTOR uses eventual consistency
        }
    }

    fn supports_background_operations(&self) -> bool {
        true // All engines support background operations by default
    }

    // =============================================================================
    // STORAGE ASSIGNMENT - Common logic for all engines using singleton pattern
    // =============================================================================

    /// Get storage URL for a collection using assignment service
    /// All storage engines can use this common implementation
    async fn get_collection_storage_url(&self, collection_id: &str) -> Result<String> {
        // Storage location should be passed through FlushParameters/CompactionParameters
        // or retrieved from collection metadata when actually needed
        tracing::error!("❌ get_collection_storage_url called without implementation for collection '{}'. Storage URL must be provided through parameters or collection metadata.", collection_id);
        Err(anyhow::anyhow!(
            "Collection '{}' storage location not found. Please ensure collection exists and has a storage assignment.", 
            collection_id
        ))
    }

    /// Get base storage URL for a collection (without collection subdirectory)
    /// Useful for creating collection directories
    async fn get_base_storage_url(&self, collection_id: &str) -> Result<String> {
        // Base storage should come from collection metadata
        // Engines must override this or provide collection service
        tracing::error!("❌ get_base_storage_url called without implementation for collection '{}'. Storage engines must provide storage URL.", collection_id);
        Err(anyhow::anyhow!("Storage engine must implement get_base_storage_url or provide collection service"))
    }

    /// Check if collection has storage assignment
    async fn has_storage_assignment(&self, _collection_id: &str) -> bool {
        // Collections always have storage now, it's part of their metadata
        true
    }

    // =============================================================================
    // STAGING OPERATIONS - Common staging pattern for flush and compaction
    // =============================================================================

    /// Get filesystem factory for this engine - to be implemented by each engine
    fn get_filesystem_factory(&self)
        -> &crate::storage::persistence::filesystem::FilesystemFactory;

    /// Get collection service for IndexConfig retrieval - to be implemented by each engine
    /// IndexConfig should be handled by AXIS indexing service
    fn get_collection_service(&self) -> Option<&crate::services::collection_service::CollectionService>;

    /// Get collection's IndexConfig from collection service
    async fn get_native_index_config(&self, collection_id: &str) -> Result<crate::index::config::IndexConfig> {
        if let Some(collection_service) = self.get_collection_service() {
            match collection_service.get_native_index_config(collection_id).await {
                Ok(Some(config)) => {
                    tracing::debug!("📋 Retrieved IndexConfig for collection: {}", collection_id);
                    Ok(config)
                }
                Ok(None) => {
                    tracing::warn!("⚠️ Collection not found for IndexConfig: {}", collection_id);
                    // Return default IndexConfig as fallback
                    Ok(crate::index::config::IndexConfig::default())
                }
                Err(e) => {
                    tracing::error!("❌ Failed to retrieve IndexConfig for collection {}: {}", collection_id, e);
                    // Return default IndexConfig as fallback
                    Ok(crate::index::config::IndexConfig::default())
                }
            }
        } else {
            tracing::warn!("⚠️ Collection service not available, using default IndexConfig");
            // Default implementation: return default IndexConfig
            Ok(crate::index::config::IndexConfig::default())
        }
    }

    /// Ensure staging directory exists for the given operation type
    /// operation_type: "__flush" for flush operations, "__compact" for compaction operations
    async fn ensure_staging_directory(
        &self,
        collection_id: &str,
        operation_type: &str,
    ) -> Result<String> {
        let collection_storage_url = self.get_collection_storage_url(collection_id).await?;
        let staging_dir = format!("{}/{}", collection_storage_url, operation_type);

        // Get filesystem factory from engine
        let filesystem_factory = self.get_filesystem_factory();

        match filesystem_factory.create_dir_all(&staging_dir).await {
            Ok(_) => {
                tracing::debug!("📁 Created staging directory: {}", staging_dir);
                Ok(staging_dir)
            }
            Err(e) => {
                // Directory might already exist, which is fine
                tracing::debug!(
                    "📁 Staging directory {} already exists or creation not needed: {}",
                    staging_dir,
                    e
                );
                Ok(staging_dir)
            }
        }
    }

    /// Write data to staging area with proper naming for atomic operations
    async fn write_to_staging(
        &self,
        staging_dir: &str,
        filename: &str,
        data: &[u8],
    ) -> Result<String> {
        let staging_file_path = format!("{}/{}", staging_dir, filename);

        // Get filesystem factory from engine
        let filesystem_factory = self.get_filesystem_factory();

        filesystem_factory
            .write(&staging_file_path, data, None)
            .await
            .with_context(|| {
                format!(
                    "Failed to write data to staging file: {}",
                    staging_file_path
                )
            })?;

        tracing::debug!(
            "💾 Wrote {} bytes to staging: {}",
            data.len(),
            staging_file_path
        );
        Ok(staging_file_path)
    }

    /// Atomically move file from staging to final storage location
    async fn atomic_move_from_staging(
        &self,
        staging_file_path: &str,
        final_storage_path: &str,
    ) -> Result<()> {
        // Get filesystem factory from engine
        let filesystem_factory = self.get_filesystem_factory();

        // Ensure the target directory exists
        if let Some(parent_dir) = final_storage_path.rfind('/') {
            let target_dir = &final_storage_path[..parent_dir];
            filesystem_factory
                .create_dir_all(target_dir)
                .await
                .with_context(|| format!("Failed to create target directory: {}", target_dir))?;
        }

        // Perform atomic move
        filesystem_factory
            .move_atomic(staging_file_path, final_storage_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to move {} to {}",
                    staging_file_path, final_storage_path
                )
            })?;

        tracing::info!(
            "⚡ Atomic move completed: {} → {}",
            staging_file_path,
            final_storage_path
        );
        Ok(())
    }

    /// Complete staging cleanup after successful operation
    async fn cleanup_staging_directory(&self, staging_dir: &str) -> Result<()> {
        let filesystem_factory = self.get_filesystem_factory();

        // Try to delete the staging directory (best effort)
        match filesystem_factory.delete(staging_dir).await {
            Ok(_) => {
                tracing::debug!("🧹 Cleaned up staging directory: {}", staging_dir);
                Ok(())
            }
            Err(e) => {
                // Log but don't fail - staging cleanup is not critical
                tracing::warn!(
                    "⚠️ Failed to cleanup staging directory {}: {}",
                    staging_dir,
                    e
                );
                Ok(())
            }
        }
    }

    // =============================================================================
    // COMMON OPERATIONS - Default implementations with delegation to engine-specific
    // =============================================================================

    /// High-level flush operation with common pre/post processing
    async fn flush(&self, params: FlushParameters) -> Result<FlushResult> {
        let start_time = std::time::Instant::now();

        // Common pre-flush validation
        self.validate_flush_parameters(&params).await?;

        // Log operation start
        tracing::info!(
            "🔄 Starting {} flush for collection: {:?} (force: {}, sync: {})",
            self.engine_name(),
            params.collection_id,
            params.force,
            params.synchronous
        );

        // Delegate to engine-specific implementation
        let mut result = self.do_flush(&params).await?;

        // Common post-flush processing
        result.duration_ms = start_time.elapsed().as_millis() as u64;
        result.completed_at = Utc::now();

        // Log operation completion
        tracing::info!(
            "✅ {} flush completed: {} entries, {} bytes in {}ms",
            self.engine_name(),
            result.entries_flushed,
            result.bytes_written,
            result.duration_ms
        );

        // Trigger compaction if requested and supported
        if params.trigger_compaction && result.success {
            let compact_params = CompactionParameters {
                collection_id: params.collection_id.clone(),
                force: false,
                synchronous: true, // 🎯 SEQUENTIAL: Must be synchronous for atomic file replacement
                priority: OperationPriority::Low,
                ..Default::default()
            };

            match self.compact(compact_params).await {
                Ok(_) => result.compaction_triggered = true,
                Err(e) => tracing::warn!("⚠️ Post-flush compaction failed: {}", e),
            }
        }

        // 🚀 INDEX UPDATES: Delegate to AXIS indexing service for proper configuration handling
        if result.success {
            if let Some(collection_id) = &params.collection_id {
                tracing::debug!("🔄 Flush successful for collection: {} - AXIS will handle index updates", collection_id);
                // NOTE: Index updates are now handled by AXIS indexing service based on collection IndexConfig
                // The flush coordinator will notify AXIS about new vectors to index
            }
        }

        Ok(result)
    }

    /// High-level compaction operation with common pre/post processing
    async fn compact(&self, params: CompactionParameters) -> Result<CompactionResult> {
        let start_time = std::time::Instant::now();

        // Common pre-compaction validation
        self.validate_compaction_parameters(&params).await?;

        // Log operation start
        tracing::info!(
            "🗜️ Starting {} compaction for collection: {:?} (force: {}, priority: {:?})",
            self.engine_name(),
            params.collection_id,
            params.force,
            params.priority
        );

        // Delegate to engine-specific implementation
        let mut result = self.do_compact(&params).await?;

        // Common post-compaction processing
        result.duration_ms = start_time.elapsed().as_millis() as u64;
        result.completed_at = Utc::now();

        // Log operation completion
        tracing::info!(
            "✅ {} compaction completed: {} entries processed, {} removed in {}ms",
            self.engine_name(),
            result.entries_processed,
            result.entries_removed,
            result.duration_ms
        );

        Ok(result)
    }

    // =============================================================================
    // HEURISTIC METHODS - Override for engine-specific thresholds
    // =============================================================================

    /// Check if flush is needed with engine-specific heuristics
    async fn should_flush(&self, _collection_id: Option<&str>) -> Result<bool> {
        match self.strategy() {
            StorageEngineStrategy::Viper => {
                // VIPER default: flush when memory usage exceeds threshold
                let stats = self.get_engine_stats().await?;
                Ok(stats.memory_usage_bytes > 100 * 1024 * 1024) // 100MB default
            }
            StorageEngineStrategy::Lsm => {
                // LSM default: flush when memtable size exceeds threshold
                let stats = self.get_engine_stats().await?;
                Ok(stats.memory_usage_bytes > 64 * 1024 * 1024) // 64MB default
            }
            StorageEngineStrategy::Hybrid => {
                // Hybrid: use VIPER heuristics
                let stats = self.get_engine_stats().await?;
                Ok(stats.memory_usage_bytes > 100 * 1024 * 1024)
            }
            StorageEngineStrategy::Prism => {
                // Prism: use LSM heuristics
                let stats = self.get_engine_stats().await?;
                Ok(stats.memory_usage_bytes > 64 * 1024 * 1024)
            }
            StorageEngineStrategy::Swift => {
                // SWIFT: use SST-like heuristics
                let stats = self.get_engine_stats().await?;
                Ok(stats.memory_usage_bytes > 64 * 1024 * 1024) // 64MB default
            }
            StorageEngineStrategy::Nova => {
                // NOVA: use columnar heuristics
                let stats = self.get_engine_stats().await?;
                Ok(stats.memory_usage_bytes > 128 * 1024 * 1024) // 128MB default
            }
            StorageEngineStrategy::Raptor => {
                // RAPTOR: aggressive flushing
                let stats = self.get_engine_stats().await?;
                Ok(stats.memory_usage_bytes > 32 * 1024 * 1024) // 32MB default
            }
        }
    }

    /// Check if compaction is needed with engine-specific heuristics
    async fn should_compact(&self, collection_id: Option<&str>) -> Result<bool> {
        match self.strategy() {
            StorageEngineStrategy::Viper => {
                // VIPER default: compact when too many small files
                let stats = self.get_engine_stats().await?;
                // Engine-specific logic would go in metrics
                Ok(stats
                    .engine_specific
                    .get("vector_count")
                    .and_then(|v| v.as_u64())
                    
                    > 10)
            }
            StorageEngineStrategy::Lsm => {
                // LSM default: compact when level ratios are unbalanced
                let stats = self.get_engine_stats().await?;
                Ok(stats
                    .engine_specific
                    .get("index_count")
                    .and_then(|v| v.as_bool())
                    )
            }
            StorageEngineStrategy::Hybrid => {
                // Hybrid: check both strategies
                self.should_flush(collection_id).await
            }
            StorageEngineStrategy::Prism => {
                // Prism: use LSM compaction strategy
                let stats = self.get_engine_stats().await?;
                Ok(stats
                    .engine_specific
                    .get("index_count")
                    .and_then(|v| v.as_bool())
                    )
            }
            StorageEngineStrategy::Swift => {
                // SWIFT: compact based on file count
                let stats = self.get_engine_stats().await?;
                Ok(stats
                    .engine_specific
                    .get("file_count")
                    .and_then(|v| v.as_u64())
                    
                    > 5)
            }
            StorageEngineStrategy::Nova => {
                // NOVA: compact when row groups exceed threshold
                let stats = self.get_engine_stats().await?;
                Ok(stats
                    .engine_specific
                    .get("row_group_count")
                    .and_then(|v| v.as_u64())
                    
                    > 20)
            }
            StorageEngineStrategy::Raptor => {
                // RAPTOR: adaptive compaction
                let stats = self.get_engine_stats().await?;
                Ok(stats
                    .engine_specific
                    .get("needs_compaction")
                    .and_then(|v| v.as_bool())
                    )
            }
        }
    }

    // =============================================================================
    // COMMON UTILITY METHODS - Shared across all engines
    // =============================================================================

    /// Get comprehensive engine statistics with common fields
    async fn get_engine_stats(&self) -> Result<EngineStatistics> {
        let engine_metrics = self.collect_engine_metrics().await?;

        Ok(EngineStatistics {
            engine_name: self.engine_name().to_string(),
            engine_version: self.engine_version().to_string(),
            total_storage_bytes: engine_metrics
                .get("collection_id")
                .and_then(|v| v.as_u64())
                ,
            memory_usage_bytes: engine_metrics
                .get("dimension")
                .and_then(|v| v.as_u64())
                ,
            collection_count: engine_metrics
                .get("engine_type")
                .and_then(|v| v.as_u64())
                 as usize,
            last_flush: engine_metrics
                .get("created_at")
                .and_then(|v| v.as_i64())
                .and_then(|ts| DateTime::from_timestamp_millis(ts)),
            last_compaction: engine_metrics
                .get("updated_at")
                .and_then(|v| v.as_i64())
                .and_then(|ts| DateTime::from_timestamp_millis(ts)),
            pending_flushes: engine_metrics
                .get("is_active")
                .and_then(|v| v.as_u64())
                ,
            pending_compactions: engine_metrics
                .get("metadata")
                .and_then(|v| v.as_u64())
                ,
            engine_specific: engine_metrics,
        })
    }

    /// Health check with common validation
    async fn health_check(&self) -> Result<EngineHealth> {
        let start_time = std::time::Instant::now();

        let stats = self.get_engine_stats().await?;
        let response_time = start_time.elapsed().as_secs_f64() * 1000.0;

        let healthy = stats
            .engine_specific
            .get("is_healthy")
            .and_then(|v| v.as_bool())
            ;

        let error_count = stats
            .engine_specific
            .get("error_count")
            .and_then(|v| v.as_u64())
             as usize;

        let warnings = stats
            .engine_specific
            .get("warnings")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        Ok(EngineHealth {
            healthy,
            status: if healthy {
                format!("{} engine healthy", self.engine_name())
            } else {
                format!("{} engine unhealthy", self.engine_name())
            },
            last_check: Utc::now(),
            response_time_ms: response_time,
            error_count,
            warnings,
            metrics: stats.engine_specific,
        })
    }

    // =============================================================================
    // VALIDATION HELPERS - Common validation logic
    // =============================================================================

    /// Validate flush parameters with common checks
    async fn validate_flush_parameters(&self, params: &FlushParameters) -> Result<()> {
        // Check collection-level operations support
        if params.collection_id.is_some() && !self.supports_collection_level_operations() {
            tracing::warn!(
                "⚠️ {} engine doesn't support collection-level flush, performing global flush",
                self.engine_name()
            );
        }

        // Validate timeout
        if let Some(timeout) = params.timeout_ms {
            if timeout == 0 {
                return Err(anyhow::anyhow!("Flush timeout cannot be zero"));
            }
        }

        Ok(())
    }

    /// Validate compaction parameters with common checks
    async fn validate_compaction_parameters(&self, params: &CompactionParameters) -> Result<()> {
        // Check collection-level operations support
        if params.collection_id.is_some() && !self.supports_collection_level_operations() {
            tracing::warn!(
                "⚠️ {} engine doesn't support collection-level compaction, performing global compaction_info",
                self.engine_name()
            );
        }

        // Validate timeout
        if let Some(timeout) = params.timeout_ms {
            if timeout == 0 {
                return Err(anyhow::anyhow!("Compaction timeout cannot be zero"));
            }
        }

        Ok(())
    }
    
    // =============================================================================
    // ADDITIONAL ENGINE OPERATIONS - Default implementations provided
    // =============================================================================
    
    /// Optimize engine performance for a specific collection
    async fn optimize(&self, _collection_id: &str) -> Result<()> {
        // Default implementation: no-op
        tracing::debug!("Engine {} optimize operation (no-op)", self.engine_name());
        Ok(())
    }
    
    /// Get detailed engine statistics
    async fn get_statistics(&self) -> Result<EngineStatistics> {
        // Default implementation: return basic statistics
        Ok(EngineStatistics {
            engine_name: self.engine_name().to_string(),
            engine_version: self.engine_version().to_string(),
            // strategy removed -  self.strategy(),
            collections_count: 0,
            total_vectors: 0,
            total_storage_bytes: 0,
            memory_usage_bytes: 0,
            last_flush: None,
            last_compaction: None,
            background_tasks_active: 0,
        })
    }
    
    /// Check if engine supports a specific feature
    fn supports_feature(&self, feature: &str) -> bool {
        // Default implementation: check common features
        match feature {
            "collection_level_operations" => self.supports_collection_level_operations(),
            "atomic_operations" => self.supports_atomic_operations(),
            "background_operations" => self.supports_background_operations(),
            _ => false,
        }
    }
}

/// Flexible flush parameters that work for both engine types
#[derive(Debug, Clone, Default)]
pub struct FlushParameters {
    /// Target collection (None means global flush for engines that support it)
    pub collection_id: Option<String>,

    /// Force immediate flush regardless of thresholds
    pub force: bool,

    /// Wait for completion before returning
    pub synchronous: bool,

    /// Engine-specific hints
    pub hints: HashMap<String, serde_json::Value>,

    /// Maximum time to wait for operation
    pub timeout_ms: Option<u64>,

    /// Vector records to flush (provided by FlushCoordinator from WAL)
    pub vector_records: Vec<crate::core::VectorRecord>,

    /// Whether to trigger compaction after flush
    pub trigger_compaction: bool,

    /// Batch IDs involved in this flush operation (for coordination)
    pub batch_ids: Vec<crate::storage::persistence::write_ahead_log::BatchId>,
    
    /// Collection configuration to avoid redundant lookups
    pub collection_config: Option<Collection>,
    
    /// Estimated size in bytes for metrics tracking
    pub estimated_size: usize,
}

/// Flexible compaction parameters that work for both engine types
#[derive(Debug, Clone, Default)]
pub struct CompactionParameters {
    /// Target collection (None means global compaction for engines that support it)
    pub collection_id: Option<String>,

    /// Force compaction regardless of thresholds
    pub force: bool,

    /// Wait for completion before returning
    pub synchronous: bool,

    /// Engine-specific hints (e.g., target level for LSM, cluster hints for VIPER)
    pub hints: HashMap<String, serde_json::Value>,

    /// Maximum time to wait for operation
    pub timeout_ms: Option<u64>,

    /// Priority level for the operation
    pub priority: OperationPriority,
    
    /// Collection configuration to avoid redundant lookups
    pub collection_config: Option<Collection>,
    
    /// Estimated input size in bytes for metrics tracking
    pub estimated_input_size: usize,
}

/// Operation priority levels
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum OperationPriority {
    Low = 0,
    #[default]
    Medium = 1,
    High = 2,
    Critical = 3,
}

/// Search context that bundles immutable references to search parameters
/// and collection configuration for zero-copy access during search operations.
/// 
/// Design principles:
/// - Immutable: All references are read-only during search
/// - Zero-copy: Uses Arc for shared ownership without cloning
/// - Cache-friendly: Collection comes directly from cache as Arc
/// - Extensible: Additional context can be added as needed
#[derive(Debug, Clone)]
pub struct SearchContext {
    /// Original search parameters (immutable reference)
    pub search_params: Arc<crate::core::search::SearchParams>,
    
    /// Collection configuration from cache (immutable reference)
    /// Contains storage_assignment with storage URL
    pub collection: Arc<Collection>,
    
    /// Additional context that might be needed during search
    /// (can be extended without breaking existing code)
    pub metadata: SearchContextMetadata,
}

/// Parsed quantization configuration for efficient progressive search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedQuantizationConfig {
    /// Strategy being used (SmartDefaults, CustomLevels, etc.)
    pub strategy: crate::proto::proximadb::quantization_config::Strategy,
    
    /// Whether progressive search is enabled
    pub progressive_search_enabled: bool,
    
    /// Ordered quantization levels for progressive refinement  
    pub progressive_levels: Vec<QuantizationLevel>,
    
    /// Search stage selectivity thresholds
    pub binary_filter_selectivity: f32,
    pub int8_ranking_selectivity: f32,
    pub pq_ranking_selectivity: f32,
    
    /// Quality and performance settings
    pub quality_threshold: f32,
    pub training_sample_size: i32,
    pub enable_simd_acceleration: bool,
    pub optimize_for_storage: bool,
    pub optimize_for_memory: bool,
}

/// Individual quantization level for progressive search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationLevel {
    /// Level identifier (e.g., "binary", "int8", "pq8")
    pub level_id: String,
    
    /// Quantization type
    pub quantization_type: QuantizationType,
    
    /// Bits per element
    pub bits: i32,
    
    /// Search priority (0 = first filter)
    pub search_priority: i32,
    
    /// PQ-specific settings
    pub num_subvectors: Option<i32>,
    
    /// Minimum recall for this level
    pub min_recall: f32,
}

/// Quantization type enumeration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum QuantizationType {
    Binary,
    Scalar,
    Product,
    Uniform,
    None,
}

/// Additional metadata for search context
/// Contains all information storage engines need - no additional cache lookups required
#[derive(Debug, Clone, Default)]
pub struct SearchContextMetadata {
    /// Collection ID extracted for convenience
    pub collection_id: String,
    
    /// Whether this search should use AXIS indexes
    pub use_axis_indexes: bool,
    
    /// Whether progressive quantization is available
    pub has_quantization: bool,
    
    /// Dimension of vectors in this collection
    pub dimension: usize,
    
    /// Distance metric for the collection
    pub distance_metric: crate::compute::distance_computation::DistanceMetric,
    
    /// Storage engine strategy for this collection
    pub storage_strategy: StorageEngineStrategy,
    
    /// Base storage path for this collection (extracted from storage_assignment)
    pub storage_path: String,
    
    /// Parsed quantization configuration for progressive search
    pub quantization_config: Option<ParsedQuantizationConfig>,
    
    /// Collection size estimates for strategy selection
    pub estimated_vector_count: u64,
    pub estimated_size_bytes: u64,
    
    /// Performance hints for engines
    pub performance_tier: PerformanceTier,
    pub compression_enabled: bool,
    pub quantization_enabled: bool,
}

impl SearchContext {
    /// Parse quantization config into ready-to-use format for progressive search
    fn parse_quantization_config(
        quant_config: &crate::proto::proximadb::QuantizationConfig,
        dimension: u32,
    ) -> Option<ParsedQuantizationConfig> {
        use crate::proto::proximadb::quantization_level::QuantizationType as ProtoQuantType;
        
        if !quant_config.enabled {
            return None;
        }
        
        // Parse or generate progressive levels
        let progressive_levels = if quant_config.custom_levels.is_empty() {
            // Use smart defaults if no custom levels provided
            if let Ok(smart_config) = crate::compute::quantization::QuantizationSmartDefaults::generate_for_dimension(dimension) {
                Self::parse_proto_levels(&smart_config.custom_levels)
            } else {
                Vec::new()
            }
        } else {
            Self::parse_proto_levels(&quant_config.custom_levels)
        };
        
        Some(ParsedQuantizationConfig {
            strategy: quant_config.strategy(),
            progressive_search_enabled: quant_config.enable_progressive_search,
            progressive_levels,
            binary_filter_selectivity: quant_config.binary_filter_selectivity,
            int8_ranking_selectivity: quant_config.int8_ranking_selectivity,
            pq_ranking_selectivity: quant_config.pq_ranking_selectivity,
            quality_threshold: quant_config.quality_threshold,
            training_sample_size: quant_config.training_sample_size,
            enable_simd_acceleration: quant_config.enable_simd_acceleration,
            optimize_for_storage: quant_config.optimize_for_storage,
            optimize_for_memory: quant_config.optimize_for_memory,
        })
    }
    
    /// Parse proto levels into internal format
    fn parse_proto_levels(proto_levels: &[crate::proto::proximadb::QuantizationLevel]) -> Vec<QuantizationLevel> {
        use crate::proto::proximadb::quantization_level::QuantizationType as ProtoQuantType;
        
        let mut levels: Vec<_> = proto_levels
            .iter()
            .map(|level| {
                let quantization_type = match level.r#type() {
                    ProtoQuantType::Binary => QuantizationType::Binary,
                    ProtoQuantType::Scalar => QuantizationType::Scalar,
                    ProtoQuantType::Product => QuantizationType::Product,
                    ProtoQuantType::Uniform => QuantizationType::Uniform,
                    ProtoQuantType::None => QuantizationType::None,
                };
                
                QuantizationLevel {
                    level_id: level.level_id.clone(),
                    quantization_type,
                    bits: level.bits,
                    search_priority: level.search_priority,
                    num_subvectors: level.num_subvectors,
                    min_recall: level.min_recall,
                }
            })
            .collect();
        
        // Sort by search priority for progressive search
        levels.sort_by_key(|l| l.search_priority);
        levels
    }
    
    /// Create a new search context from cached components
    pub fn new(
        search_params: Arc<crate::core::search::SearchParams>,
        collection: Arc<Collection>,
    ) -> Self {
        // Extract metadata once during context creation
        let config = collection.config.as_ref();
        let storage_assignment = collection.storage_assignment.as_ref();
        
        let metadata = SearchContextMetadata {
            collection_id: collection.id.clone().unwrap_or_default(),
            use_axis_indexes: config
                .and_then(|c| c.index_config.as_ref())
                .map(|_| true)
                ,
            has_quantization: config
                .and_then(|c| c.quantization.as_ref())
                .is_some(),
            dimension: config
                .map(|c| c.dimension as usize)
                ,
            distance_metric: config
                .and_then(|c| c.distance_metric)
                .map(|dm| dm.into())
                ,
            storage_strategy: config
                .and_then(|c| c.storage.as_ref())
                .and_then(|s| s.engine.as_ref())
                .map(|e| match e.as_str() {
                    "VIPER" => StorageEngineStrategy::Viper,
                    "SST" => StorageEngineStrategy::Lsm,
                    "PRISM" => StorageEngineStrategy::Prism,
                    _ => StorageEngineStrategy::Viper,
                })
                .unwrap_or_default(),
            storage_path: storage_assignment
                .map(|sa| sa.base_location.clone())
                .unwrap_or_default(),
            estimated_vector_count: config
                .map(|c| c.estimated_vector_count)
                ,
            estimated_size_bytes: config
                .map(|c| c.estimated_size_bytes)
                ,
            performance_tier: config
                .and_then(|c| c.storage.as_ref())
                .and_then(|s| s.performance_tier.as_ref())
                .map(|pt| match pt.as_str() {
                    "hot" => PerformanceTier::Hot,
                    "warm" => PerformanceTier::Warm,
                    "cold" => PerformanceTier::Cold,
                    "archive" => PerformanceTier::Archive,
                    _ => PerformanceTier::Warm,
                })
                .unwrap_or_default(),
            compression_enabled: config
                .and_then(|c| c.storage.as_ref())
                .and_then(|s| s.compression.as_ref())
                .map(|_| true)
                ,
            quantization_enabled: config
                .and_then(|c| c.quantization.as_ref())
                .map(|_| true)
                ,
            // Parse quantization config for progressive search
            quantization_config: config
                .and_then(|c| c.quantization.as_ref())
                .and_then(|qc| Self::parse_quantization_config(qc, config.map(|c| c.dimension as u32))),
        };
        
        Self {
            search_params,
            collection,
            metadata,
        }
    }
    
    /// Get the query vector (convenience method)
    pub fn query_vector(&self) -> Option<&[f32]> {
        self.search_params.query_vectors.as_ref()
            .and_then(|vecs| vecs.first())
            .map(|v| v.as_slice())
    }
    
    /// Get top_k value with fallback to default
    pub fn top_k(&self) -> usize {
        self.search_params.top_k
    }
    
    /// Get distance metric (pre-computed from collection config)
    pub fn distance_metric(&self) -> crate::compute::distance_computation::DistanceMetric {
        // Use search params override if provided, otherwise use pre-computed value
        self.search_params.distance_metric
    }
    
    /// Get dimension from metadata (pre-computed)
    pub fn dimension(&self) -> usize {
        self.metadata.dimension
    }
    
    /// Check if progressive search is enabled
    pub fn is_progressive_search_enabled(&self) -> bool {
        self.metadata.quantization_config
            .as_ref()
            .map(|qc| qc.progressive_search_enabled)
            
    }
    
    /// Get progressive quantization levels ordered by search priority
    pub fn get_progressive_levels(&self) -> Option<&[QuantizationLevel]> {
        self.metadata.quantization_config
            .as_ref()
            .map(|qc| qc.progressive_levels.as_slice())
    }
    
    /// Get binary filter selectivity for progressive search
    pub fn binary_filter_selectivity(&self) -> f32 {
        self.metadata.quantization_config
            .as_ref()
            .map(|qc| qc.binary_filter_selectivity)
            
    }
    
    /// Check if SIMD acceleration should be used
    pub fn use_simd_acceleration(&self) -> bool {
        self.metadata.quantization_config
            .as_ref()
            .map(|qc| qc.enable_simd_acceleration)
            
    }
    
    /// Get the parsed quantization config
    pub fn quantization_config(&self) -> Option<&ParsedQuantizationConfig> {
        self.metadata.quantization_config.as_ref()
    }
    
    /// Check if quantization is enabled (pre-computed)
    pub fn has_quantization(&self) -> bool {
        self.metadata.has_quantization
    }
    
    /// Get storage path (pre-computed from storage assignment)
    pub fn storage_path(&self) -> &str {
        &self.metadata.storage_path
    }
    
    /// Get storage strategy (pre-computed)
    pub fn storage_strategy(&self) -> StorageEngineStrategy {
        self.metadata.storage_strategy.clone()
    }
    
    /// Get performance tier hint (pre-computed)
    pub fn performance_tier(&self) -> PerformanceTier {
        self.metadata.performance_tier.clone()
    }
    
    /// Get collection size estimates (pre-computed)
    pub fn estimated_vector_count(&self) -> u64 {
        self.metadata.estimated_vector_count
    }
    
    /// Get estimated collection size in bytes (pre-computed)
    pub fn estimated_size_bytes(&self) -> u64 {
        self.metadata.estimated_size_bytes
    }
    
    /// Check if compression is enabled (pre-computed)
    pub fn compression_enabled(&self) -> bool {
        self.metadata.compression_enabled
    }
    
    /// Check if quantization is enabled (pre-computed)
    pub fn quantization_enabled(&self) -> bool {
        self.metadata.quantization_enabled
    }
    
    /// Get collection ID (pre-computed)
    pub fn collection_id(&self) -> &str {
        &self.metadata.collection_id
    }
    
    /// Get storage URL from collection's storage assignment
    pub fn storage_url(&self) -> Option<&str> {
        self.collection.storage_assignment.as_ref()
            .and_then(|sa| sa.base_url.as_str())
    }
    
    /// Get collection-specific storage path
    pub fn collection_storage_path(&self) -> Option<String> {
        self.storage_url().map(|base| {
            format!("{}/{}", base, self.collection_id())
        })
    }
}

/// Unified flush result that accommodates different engine types
///
/// Note: Default values use u64::MAX to indicate uninitialized state.
/// This allows distinguishing between:
/// - Uninitialized: u64::MAX (default)
/// - Successful operation with zero results: 0
#[derive(Debug, Clone)]
pub struct FlushResult {
    /// Operation completed successfully
    pub success: bool,

    /// Collections affected by the flush
    pub collections_affected: Vec<String>,

    /// Number of entries flushed
    pub entries_flushed: u64,

    /// Bytes written to storage
    pub bytes_written: u64,

    /// Number of files/segments created
    pub files_created: u64,

    /// Duration of the operation
    pub duration_ms: u64,

    /// Timestamp when operation completed
    pub completed_at: DateTime<Utc>,

    /// Engine-specific metrics
    pub engine_metrics: HashMap<String, serde_json::Value>,

    /// Whether compaction was triggered as a result
    pub compaction_triggered: bool,

    /// Batch IDs that were successfully flushed (for WAL cleanup coordination)
    pub flushed_batch_ids: Vec<crate::storage::persistence::write_ahead_log::BatchId>,
}

/// Unified compaction result that accommodates different engine types
///
/// Note: Default values use u64::MAX to indicate uninitialized state.
/// This allows distinguishing between:
/// - Uninitialized: u64::MAX (default)
/// - Successful operation with zero results: 0
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// Operation completed successfully
    pub success: bool,

    /// Collections affected by the compaction
    pub collections_affected: Vec<String>,

    /// Number of entries processed
    pub entries_processed: u64,

    /// Number of entries removed (tombstones, duplicates, etc.)
    pub entries_removed: u64,

    /// Bytes read during compaction
    pub bytes_read: u64,

    /// Bytes written during compaction
    pub bytes_written: u64,

    /// Input files/segments processed
    pub input_files: u64,

    /// Output files/segments created
    pub output_files: u64,

    /// Duration of the operation
    pub duration_ms: u64,

    /// Timestamp when operation completed
    pub completed_at: DateTime<Utc>,

    /// Engine-specific metrics (e.g., compression ratio, level info)
    pub engine_metrics: HashMap<String, serde_json::Value>,
}

/// Engine statistics
#[derive(Debug, Clone)]
pub struct EngineStatistics {
    /// Engine name and version
    pub engine_name: String,
    pub engine_version: String,

    /// Total storage size
    pub total_storage_bytes: u64,

    /// Memory usage
    pub memory_usage_bytes: u64,

    /// Number of collections
    pub collection_count: usize,

    /// Last flush time
    pub last_flush: Option<DateTime<Utc>>,

    /// Last compaction time
    pub last_compaction: Option<DateTime<Utc>>,

    /// Pending operations
    pub pending_flushes: u64,
    pub pending_compactions: u64,

    /// Engine-specific metrics
    pub engine_specific: HashMap<String, serde_json::Value>,
}

/// Engine health status
#[derive(Debug, Clone)]
pub struct EngineHealth {
    /// Overall health status
    pub healthy: bool,

    /// Health status message
    pub status: String,

    /// Last health check time
    pub last_check: DateTime<Utc>,

    /// Response time for health check
    pub response_time_ms: f64,

    /// Error count in recent period
    pub error_count: usize,

    /// Warning messages
    pub warnings: Vec<String>,

    /// Engine-specific health metrics
    pub metrics: HashMap<String, serde_json::Value>,
}

/// Builder pattern for creating flush parameters
impl FlushParameters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn collection(mut self, collection_id: impl Into<String>) -> Self {
        self.collection_id = Some(collection_id.into());
        self
    }

    pub fn force(mut self) -> Self {
        self.force = true;
        self
    }

    pub fn synchronous(mut self) -> Self {
        self.synchronous = true;
        self
    }

    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    pub fn trigger_compaction(mut self) -> Self {
        self.trigger_compaction = true;
        self
    }

    pub fn hint(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.hints.insert(key.into(), value);
        self
    }
}

/// Builder pattern for creating compaction parameters
impl CompactionParameters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn collection(mut self, collection_id: impl Into<String>) -> Self {
        self.collection_id = Some(collection_id.into());
        self
    }

    pub fn force(mut self) -> Self {
        self.force = true;
        self
    }

    pub fn synchronous(mut self) -> Self {
        self.synchronous = true;
        self
    }

    pub fn priority(mut self, priority: OperationPriority) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = Some(timeout_ms);
        self
    }

    pub fn hint(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.hints.insert(key.into(), value);
        self
    }
}

impl Default for FlushResult {
    fn default() -> Self {
        Self {
            success: false,
            collections_affected: Vec::new(),
            entries_flushed: u64::MAX, // -1 equivalent for u64 (indicates uninitialized)
            bytes_written: u64::MAX,   // -1 equivalent for u64 (indicates uninitialized)
            files_created: u64::MAX,   // -1 equivalent for u64 (indicates uninitialized)
            duration_ms: u64::MAX,     // -1 equivalent for u64 (indicates uninitialized)
            completed_at: Utc::now(),
            engine_metrics: HashMap::new(),
            compaction_triggered: false,
            flushed_batch_ids: vec![],
        }
    }
}

impl Default for CompactionResult {
    fn default() -> Self {
        Self {
            success: false,
            collections_affected: Vec::new(),
            entries_processed: u64::MAX, // -1 equivalent for u64 (indicates uninitialized)
            entries_removed: u64::MAX,   // -1 equivalent for u64 (indicates uninitialized)
            bytes_read: u64::MAX,        // -1 equivalent for u64 (indicates uninitialized)
            bytes_written: u64::MAX,     // -1 equivalent for u64 (indicates uninitialized)
            input_files: u64::MAX,       // -1 equivalent for u64 (indicates uninitialized)
            output_files: u64::MAX,      // -1 equivalent for u64 (indicates uninitialized)
            duration_ms: u64::MAX,       // -1 equivalent for u64 (indicates uninitialized)
            completed_at: Utc::now(),
            engine_metrics: HashMap::new(),
        }
    }
}
