//! Unified Storage Engine Traits with Strategy Pattern
//!
//! This module implements the Strategy Pattern for storage engines, allowing polymorphic
//! selection between different storage backends (VIPER default, LSM alternative).
//! Common operations are implemented in the base trait with default implementations,
//! while specialized engines override only what's unique to their approach.

use anyhow::{Result, Context};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
// Core types imported as needed in implementations

/// Strategy enum for selecting storage engine type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageEngineStrategy {
    /// VIPER: Vector-optimized Intelligent Parquet with Efficient Retrieval (Default)
    Viper,
    /// LSM: Log-Structured Merge Tree (Alternative for comparison)
    Lsm,
    /// Hybrid: Uses VIPER for vectors, LSM for metadata (Future)
    Hybrid,
}

impl Default for StorageEngineStrategy {
    fn default() -> Self {
        Self::Viper // VIPER is the default strategy
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
    
    // =============================================================================
    // ENGINE CAPABILITIES - Can be overridden, sensible defaults provided
    // =============================================================================
    
    /// Engine capabilities with defaults based on strategy
    fn supports_collection_level_operations(&self) -> bool {
        match self.strategy() {
            StorageEngineStrategy::Viper => true,   // VIPER supports collection-level ops
            StorageEngineStrategy::Lsm => false,    // LSM operates on entire tree
            StorageEngineStrategy::Hybrid => true,  // Hybrid supports collection-level ops
        }
    }
    
    fn supports_atomic_operations(&self) -> bool {
        match self.strategy() {
            StorageEngineStrategy::Viper => true,   // VIPER has atomic staging operations
            StorageEngineStrategy::Lsm => false,    // LSM has eventual consistency
            StorageEngineStrategy::Hybrid => true,  // Hybrid provides atomic guarantees
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
        let assignment_service = crate::storage::assignment_service::get_assignment_service();
        let component_type = match self.strategy() {
            StorageEngineStrategy::Viper => crate::storage::assignment_service::StorageComponentType::Storage,
            StorageEngineStrategy::Lsm => crate::storage::assignment_service::StorageComponentType::Storage, // LSM uses same storage assignment
            StorageEngineStrategy::Hybrid => crate::storage::assignment_service::StorageComponentType::Storage, // Use VIPER for hybrid
        };
        
        match assignment_service.get_assignment(
            &crate::core::CollectionId::from(collection_id.to_string()), 
            component_type
        ).await {
            Some(assignment) => {
                // Return the assigned storage URL with collection subdirectory
                Ok(format!("{}/{}", assignment.storage_url, collection_id))
            },
            None => {
                Err(anyhow::anyhow!(
                    "No storage assignment found for collection {} in {} component. Ensure collection was created properly.",
                    collection_id, self.engine_name()
                ))
            }
        }
    }
    
    /// Get base storage URL for a collection (without collection subdirectory)
    /// Useful for creating collection directories
    async fn get_base_storage_url(&self, collection_id: &str) -> Result<String> {
        let assignment_service = crate::storage::assignment_service::get_assignment_service();
        let component_type = match self.strategy() {
            StorageEngineStrategy::Viper => crate::storage::assignment_service::StorageComponentType::Storage,
            StorageEngineStrategy::Lsm => crate::storage::assignment_service::StorageComponentType::Storage, // LSM uses same storage assignment
            StorageEngineStrategy::Hybrid => crate::storage::assignment_service::StorageComponentType::Storage,
        };
        
        match assignment_service.get_assignment(
            &crate::core::CollectionId::from(collection_id.to_string()), 
            component_type
        ).await {
            Some(assignment) => Ok(assignment.storage_url),
            None => {
                Err(anyhow::anyhow!(
                    "No storage assignment found for collection {} in {} component",
                    collection_id, self.engine_name()
                ))
            }
        }
    }
    
    /// Check if collection has storage assignment
    async fn has_storage_assignment(&self, collection_id: &str) -> bool {
        let assignment_service = crate::storage::assignment_service::get_assignment_service();
        let component_type = match self.strategy() {
            StorageEngineStrategy::Viper => crate::storage::assignment_service::StorageComponentType::Storage,
            StorageEngineStrategy::Lsm => crate::storage::assignment_service::StorageComponentType::Storage, // LSM uses same storage assignment
            StorageEngineStrategy::Hybrid => crate::storage::assignment_service::StorageComponentType::Storage,
        };
        
        assignment_service.get_assignment(
            &crate::core::CollectionId::from(collection_id.to_string()), 
            component_type
        ).await.is_some()
    }

    // =============================================================================
    // STAGING OPERATIONS - Common staging pattern for flush and compaction
    // =============================================================================
    
    /// Get filesystem factory for this engine - to be implemented by each engine
    fn get_filesystem_factory(&self) -> &crate::storage::persistence::filesystem::FilesystemFactory;
    
    /// Ensure staging directory exists for the given operation type
    /// operation_type: "__flush" for flush operations, "__compact" for compaction operations
    async fn ensure_staging_directory(&self, collection_id: &str, operation_type: &str) -> Result<String> {
        let collection_storage_url = self.get_collection_storage_url(collection_id).await?;
        let staging_dir = format!("{}/{}", collection_storage_url, operation_type);
        
        // Get filesystem factory from engine
        let filesystem_factory = self.get_filesystem_factory();
        
        match filesystem_factory.create_dir_all(&staging_dir).await {
            Ok(_) => {
                tracing::debug!("📁 Created staging directory: {}", staging_dir);
                Ok(staging_dir)
            },
            Err(e) => {
                // Directory might already exist, which is fine
                tracing::debug!("📁 Staging directory {} already exists or creation not needed: {}", staging_dir, e);
                Ok(staging_dir)
            }
        }
    }
    
    /// Write data to staging area with proper naming for atomic operations
    async fn write_to_staging(&self, staging_dir: &str, filename: &str, data: &[u8]) -> Result<String> {
        let staging_file_path = format!("{}/{}", staging_dir, filename);
        
        // Get filesystem factory from engine
        let filesystem_factory = self.get_filesystem_factory();
        
        filesystem_factory.write(&staging_file_path, data, None).await
            .with_context(|| format!("Failed to write data to staging file: {}", staging_file_path))?;
        
        tracing::debug!("💾 Wrote {} bytes to staging: {}", data.len(), staging_file_path);
        Ok(staging_file_path)
    }
    
    /// Atomically move file from staging to final storage location
    async fn atomic_move_from_staging(&self, staging_file_path: &str, final_storage_path: &str) -> Result<()> {
        // Get filesystem factory from engine
        let filesystem_factory = self.get_filesystem_factory();
        
        // Ensure the target directory exists
        if let Some(parent_dir) = final_storage_path.rfind('/') {
            let target_dir = &final_storage_path[..parent_dir];
            filesystem_factory.create_dir_all(target_dir).await
                .with_context(|| format!("Failed to create target directory: {}", target_dir))?;
        }
        
        // Perform atomic move
        filesystem_factory.move_atomic(staging_file_path, final_storage_path).await
            .with_context(|| format!("Failed to move {} to {}", staging_file_path, final_storage_path))?;
        
        tracing::info!("⚡ Atomic move completed: {} → {}", staging_file_path, final_storage_path);
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
            },
            Err(e) => {
                // Log but don't fail - staging cleanup is not critical
                tracing::warn!("⚠️ Failed to cleanup staging directory {}: {}", staging_dir, e);
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
                synchronous: false, // Background compaction
                priority: OperationPriority::Low,
                ..Default::default()
            };
            
            match self.compact(compact_params).await {
                Ok(_) => result.compaction_triggered = true,
                Err(e) => tracing::warn!("⚠️ Post-flush compaction failed: {}", e),
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
            },
            StorageEngineStrategy::Lsm => {
                // LSM default: flush when memtable size exceeds threshold
                let stats = self.get_engine_stats().await?;
                Ok(stats.memory_usage_bytes > 64 * 1024 * 1024) // 64MB default
            },
            StorageEngineStrategy::Hybrid => {
                // Hybrid: use VIPER heuristics
                let stats = self.get_engine_stats().await?;
                Ok(stats.memory_usage_bytes > 100 * 1024 * 1024)
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
                Ok(stats.engine_specific.get("small_files_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) > 10)
            },
            StorageEngineStrategy::Lsm => {
                // LSM default: compact when level ratios are unbalanced
                let stats = self.get_engine_stats().await?;
                Ok(stats.engine_specific.get("needs_compaction")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false))
            },
            StorageEngineStrategy::Hybrid => {
                // Hybrid: check both strategies
                self.should_flush(collection_id).await
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
            total_storage_bytes: engine_metrics.get("total_storage_bytes")
                .and_then(|v| v.as_u64()).unwrap_or(0),
            memory_usage_bytes: engine_metrics.get("memory_usage_bytes")
                .and_then(|v| v.as_u64()).unwrap_or(0),
            collection_count: engine_metrics.get("collection_count")
                .and_then(|v| v.as_u64()).unwrap_or(0) as usize,
            last_flush: engine_metrics.get("last_flush_timestamp")
                .and_then(|v| v.as_i64())
                .and_then(|ts| DateTime::from_timestamp_millis(ts)),
            last_compaction: engine_metrics.get("last_compaction_timestamp")
                .and_then(|v| v.as_i64())
                .and_then(|ts| DateTime::from_timestamp_millis(ts)),
            pending_flushes: engine_metrics.get("pending_flushes")
                .and_then(|v| v.as_u64()).unwrap_or(0),
            pending_compactions: engine_metrics.get("pending_compactions")
                .and_then(|v| v.as_u64()).unwrap_or(0),
            engine_specific: engine_metrics,
        })
    }
    
    /// Health check with common validation
    async fn health_check(&self) -> Result<EngineHealth> {
        let start_time = std::time::Instant::now();
        
        let stats = self.get_engine_stats().await?;
        let response_time = start_time.elapsed().as_secs_f64() * 1000.0;
        
        let healthy = stats.engine_specific.get("healthy")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
            
        let error_count = stats.engine_specific.get("error_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;
            
        let warnings = stats.engine_specific.get("warnings")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.to_string())
                .collect())
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
                "⚠️ {} engine doesn't support collection-level compaction, performing global compaction",
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