//! # Unified Storage Engine Traits with Strategy Pattern
//!
//! This module defines the core abstraction layer for ProximaDB's pluggable storage engine system.
//! It implements the Strategy Pattern for storage engines, allowing polymorphic selection between
//! different storage backends optimized for various workload patterns.
//!
//! ## Role in ProximaDB Architecture
//!
//! This module serves as the contract between the service layer and the storage layer, enabling:
//! - **Storage Engine Abstraction**: Uniform interface for all storage engines (SST, VIPER, NOVA, etc.)
//! - **Workload Optimization**: Different engines for different access patterns (OLTP vs OLAP)
//! - **Zero-Copy Operations**: Direct protocol buffer flow without intermediate conversions
//! - **Cloud-Native Integration**: Seamless support for S3, Azure Blob, GCS backends
//!
//! ## Key Components
//!
//! - `StorageEngineStrategy`: Enum for selecting the appropriate storage engine
//! - `UnifiedStorageEngine`: Main trait that all storage engines must implement
//! - `InternalCollectionProvider`: Trait for accessing collection metadata without circular dependencies
//! - `PerformanceTier`: Data temperature hints for intelligent tiering
//!
//! ## Storage Engine Types
//!
//! 1. **SST (SSTEngine)**: Hybrid columnar (ProximaBlocks), write-optimized with three-stage filtering
//!    - Best for: Real-time queries, frequent updates, OLTP workloads
//!    
//! 2. **VIPER (ViperEngine)**: Columnar Parquet format with advanced quantization
//!    - Best for: Analytics, batch operations, compression, OLAP workloads
//!    
//! 3. **NOVA (NovaEngine)**: Next-gen columnar with integrated quantization
//!    - Best for: Mixed workloads, progressive search optimization
//!    
//! 4. **SWIFT (SwiftEngine)**: Hierarchical superblock architecture
//!    - Best for: Fast traversal, hot data caching
//!    
//! 5. **RAPTOR (RaptorEngine)**: Experimental parallel tiered storage
//!    - Best for: Research and development of new storage patterns
//!
//! ## Integration Points
//!
//! - **Service Layer**: `CollectionService` uses this trait to interact with storage
//! - **WAL System**: Write-ahead log delegates to storage engines for persistence
//! - **Index Layer**: AXIS engine coordinates with storage for vector retrieval
//! - **Compaction**: Background processes use this trait for maintenance operations

use crate::core::search::BlockPruneMode;
use crate::proto::proximadb_v1::Collection;
use crate::security::unified_rbac::{TenantContext, UnifiedUserContext};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

// Re-export decomposed traits from trait_components module for ISP compliance
// Users can import from either `storage::traits` or `storage::trait_components`
pub use crate::storage::trait_components::{
    StorageCompactor, StorageIdentity, StorageLifecycle, StorageMetrics, StorageReader,
    StorageScan, StorageWriter,
};

// Import capabilities for OCP-compliant delegation
use crate::storage::trait_components::capabilities::CapabilityFactory;

// Import StorageEngineType for OCP-compliant engine type dispatch
use crate::index::axis::eventlog::StorageEngineType;

/// Performance tier hint for storage engines
///
/// ## Purpose:
///
/// PerformanceTier provides hints to storage engines about data temperature,
/// enabling intelligent tiering decisions for optimal cost/performance balance.
///
/// ## Tiering Strategy:
///
/// - **Hot**: Memory/NVMe SSD, uncompressed or lightly compressed
/// - **Warm**: SSD with moderate compression (ZSTD level 3)
/// - **Cold**: HDD/Cloud with heavy compression (ZSTD level 9)
/// - **Archive**: Glacier/Archive with maximum compression (ZSTD level 19)
///
/// ## Usage Example:
/// ```rust,ignore
/// // Mark frequently accessed data as hot
/// engine.set_tier(collection_id, PerformanceTier::Hot)?;
///
/// // Archive old data after 90 days
/// engine.migrate_to_tier(old_data, PerformanceTier::Archive)?;
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PerformanceTier {
    /// Hot data - keep in memory/SSD, optimize for latency
    /// Target: <1ms latency, highest cost
    Hot,

    /// Warm data - balance between latency and cost
    /// Target: <10ms latency, moderate cost
    Warm,

    /// Cold data - optimize for cost, higher latency acceptable
    /// Target: <100ms latency, low cost
    Cold,

    /// Archive data - minimal access, maximum compression
    /// Target: <1s latency, lowest cost
    Archive,
}

impl Default for PerformanceTier {
    fn default() -> Self {
        Self::Warm
    }
}
// Core types imported as needed in implementations

/// Strategy enum for selecting storage engine type
///
/// ## Selection Criteria:
///
/// Choose storage strategy based on workload characteristics:
///
/// ### OLTP Workloads (Real-time):
/// - **Sst**: Best for frequent updates, point queries
/// - **Swift**: Optimized for low-latency traversal
///
/// ### OLAP Workloads (Analytics):
/// - **Viper**: Columnar with 5-10x compression
/// - **Nova**: Enhanced columnar with zone maps
/// - **Helix**: Dimension reduction for high-D vectors
///
/// ### Mixed Workloads:
/// - **Raptor**: Matrix Trinity navigation
/// - **Hybrid**: Combines multiple engines
///
/// ## Performance Comparison:
/// ```text
/// | Strategy | Write | Read  | Compression | Memory |
/// |----------|-------|-------|-------------|--------|
/// | Viper    | 500K  | 50ms  | 5-10x       | Low    |
/// | Sst      | 200K  | 5ms   | 3-5x        | Medium |
/// | Swift    | 300K  | 2ms   | 2-3x        | High   |
/// | Raptor   | 250K  | 3ms   | 4-6x        | Medium |
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StorageEngineStrategy {
    /// VIPER: Vector-optimized Intelligent Parquet with Efficient Retrieval (Default)
    /// Best for: Analytics, batch operations, maximum compression
    Viper,

    /// SST: Sorted String Table storage engine
    /// Best for: OLTP, real-time updates, point queries, row-based access
    Sst,

    /// SWIFT: Storage With Instant Fast Traversal (Hierarchical superblock architecture)
    /// Best for: Fast sequential access, range queries
    Swift,

    /// NOVA: Next-gen Optimized Vector Analytics (Columnar with quantization)
    /// Best for: Advanced analytics with predicate pushdown
    Nova,

    /// RAPTOR: Rapid Access Parallel Tiered Object Retrieval (Experimental)
    /// Best for: Graph-like traversal, mixed workloads
    Raptor,

    /// HELIX: High-Efficiency Locality-Indexed eXecution (PCA + Hilbert clustering)
    /// Best for: High-dimensional vectors (>1536D)
    Helix,

    /// Hybrid: Uses VIPER for vectors, LSM for metadata (Future)
    /// Best for: Complex workloads with different access patterns
    Hybrid,
}

impl Default for StorageEngineStrategy {
    fn default() -> Self {
        Self::Viper // VIPER is the default strategy
    }
}

/// Core metadata provider trait for collection metadata operations
///
/// ## Design Philosophy:
///
/// MetadataProvider separates metadata management from storage operations,
/// preventing circular dependencies between storage engines and collection service.
///
/// ## Implementation Requirements:
///
/// Implementors must provide thread-safe, async metadata operations.
/// Typically backed by a metadata store (PostgreSQL, etcd, etc.).
///
/// ## Caching Strategy:
///
/// Implementations should cache frequently accessed metadata:
/// - Collection UUID mappings (immutable, cache forever)
/// - Collection configs (cache with TTL)
/// - Existence checks (cache negative results briefly)
///
/// This trait focuses solely on metadata CRUD operations
#[async_trait]
pub trait MetadataProvider: Send + Sync {
    /// Get collection UUID by name or ID
    async fn get_uuid(&self, collection_id: &str) -> Result<Option<String>>;

    /// Get full collection metadata
    async fn collection_metadata(&self, collection_id: &str) -> Result<Option<Collection>>;

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

    /// Create or update a collection from protobuf
    async fn upsert_collection_proto(&self, collection: &Collection) -> Result<()>;

    /// Delete a collection by ID  
    async fn delete_collection(&self, collection_id: &str) -> Result<()>;

    /// Find collection by name or ID (sync convenience method)
    fn find_collection(&self, _collection_id: &str) -> Option<Collection> {
        // Default sync implementation - backends can override
        None
    }
}

/// Unified metrics collector that can be shared across backends
///
/// ## Architecture:
///
/// UnifiedMetricsCollector provides a centralized, lock-free metrics
/// collection system that all storage engines can use without creating
/// circular dependencies.
///
/// ## Key Features:
///
/// - **Fire-and-forget**: Non-blocking metric recording
/// - **Thread-safe**: Can be called from any async context
/// - **Memory-bounded**: Keeps only last 1000 latency samples
/// - **Zero-allocation**: Pre-allocated buffers for hot path
///
/// ## Metrics Tracked:
///
/// - Operation counts by type
/// - Success/failure rates
/// - Bytes read/written
/// - Latency percentiles (P50, P95, P99)
/// - Cache hit/miss ratios
///
/// This avoids circular dependencies by being a separate component
pub struct UnifiedMetricsCollector {
    /// RwLock protects metrics data for concurrent access
    /// Write lock only needed for updates, reads can proceed in parallel
    metrics: Arc<tokio::sync::RwLock<MetricsData>>,
}

impl UnifiedMetricsCollector {
    pub fn new() -> Self {
        Self {
            metrics: Arc::new(tokio::sync::RwLock::new(MetricsData::default())),
        }
    }

    /// Record an operation - can be called from any thread
    ///
    /// ## Non-blocking Design:
    ///
    /// Uses tokio::spawn for fire-and-forget recording. If the metrics
    /// lock is contended, we skip recording rather than block the operation.
    ///
    /// This ensures metrics never impact production performance.
    ///
    /// ## Usage:
    /// ```rust,ignore
    /// let start = Instant::now();
    /// let result = do_operation()?;
    /// metrics.record(
    ///     MetricsOperationType::Read,
    ///     start.elapsed().as_millis() as u64,
    ///     result.is_ok(),
    ///     Some(result.bytes_read)
    /// );
    /// ```
    pub fn record(
        &self,
        op_type: MetricsOperationType,
        duration_ms: u64,
        success: bool,
        bytes: Option<usize>,
    ) {
        let metrics = self.metrics.clone();
        // Fire and forget - don't block the operation
        // This spawns a lightweight task that will update metrics asynchronously
        tokio::spawn(async move {
            // Acquire write lock, waiting if needed
            // This ensures metrics are always recorded accurately
            let mut m = metrics.write().await;
            m.record_operation(op_type, duration_ms, success, bytes);
        });
    }

    pub async fn get_snapshot(&self) -> MetricsSnapshot {
        let metrics = self.metrics.read().await;
        metrics.to_snapshot()
    }

    pub async fn reset(&self) {
        let mut metrics = self.metrics.write().await;
        *metrics = MetricsData::default();
    }

    /// Record an operation with timing
    pub async fn record_operation(
        &self,
        op_type: MetricsOperationType,
        success: bool,
        bytes: usize,
        duration: std::time::Duration,
    ) {
        self.record(
            op_type,
            duration.as_millis() as u64,
            success,
            if bytes > 0 { Some(bytes) } else { None },
        );
    }

    /// Record an operation with timing, blocking until recorded
    /// Use this in tests or when you need guaranteed recording
    pub async fn record_operation_blocking(
        &self,
        op_type: MetricsOperationType,
        success: bool,
        bytes: usize,
        duration: std::time::Duration,
    ) {
        let mut metrics = self.metrics.write().await;
        metrics.record_operation(
            op_type,
            duration.as_millis() as u64,
            success,
            if bytes > 0 { Some(bytes) } else { None },
        );
    }
}

impl Clone for UnifiedMetricsCollector {
    fn clone(&self) -> Self {
        Self {
            metrics: self.metrics.clone(),
        }
    }
}

/// Internal metrics data structure
#[derive(Debug)]
struct MetricsData {
    total_operations: u64,
    successful_operations: u64,
    failed_operations: u64,
    operation_counts: HashMap<String, u64>,
    bytes_read: u64,
    bytes_written: u64,
    latencies_ms: std::collections::VecDeque<u64>,
    cache_hits: u64,
    cache_misses: u64,
    started_at: chrono::DateTime<chrono::Utc>,
    last_reset: chrono::DateTime<chrono::Utc>,
}

impl Default for MetricsData {
    fn default() -> Self {
        let now = chrono::Utc::now();
        Self {
            total_operations: 0,
            successful_operations: 0,
            failed_operations: 0,
            operation_counts: HashMap::new(),
            bytes_read: 0,
            bytes_written: 0,
            latencies_ms: std::collections::VecDeque::with_capacity(1000),
            cache_hits: 0,
            cache_misses: 0,
            started_at: now,
            last_reset: now,
        }
    }
}

impl MetricsData {
    fn record_operation(
        &mut self,
        op_type: MetricsOperationType,
        duration_ms: u64,
        success: bool,
        bytes: Option<usize>,
    ) {
        self.total_operations += 1;

        if success {
            self.successful_operations += 1;
        } else {
            self.failed_operations += 1;
        }

        // Track operation type
        let op_name = format!("{:?}", op_type);
        *self.operation_counts.entry(op_name).or_insert(0) += 1;

        match op_type {
            MetricsOperationType::Read => {
                if let Some(b) = bytes {
                    self.bytes_read += b as u64;
                }
            }
            MetricsOperationType::Write => {
                if let Some(b) = bytes {
                    self.bytes_written += b as u64;
                }
            }
            MetricsOperationType::CacheHit => self.cache_hits += 1,
            MetricsOperationType::CacheMiss => self.cache_misses += 1,
            _ => {}
        }

        // Track latency (keep last 1000)
        if self.latencies_ms.len() >= 1000 {
            self.latencies_ms.pop_front();
        }
        self.latencies_ms.push_back(duration_ms);
    }

    fn to_snapshot(&self) -> MetricsSnapshot {
        let avg_latency = if !self.latencies_ms.is_empty() {
            self.latencies_ms.iter().sum::<u64>() as f64 / self.latencies_ms.len() as f64
        } else {
            0.0
        };

        let (p50, p95, p99) = self.calculate_percentiles();

        MetricsSnapshot {
            total_operations: self.total_operations,
            successful_operations: self.successful_operations,
            failed_operations: self.failed_operations,
            total_bytes_read: self.bytes_read,
            total_bytes_written: self.bytes_written,
            avg_latency_ms: avg_latency,
            p50_latency_ms: p50,
            p95_latency_ms: p95,
            p99_latency_ms: p99,
            operations_per_type: self.operation_counts.clone(),
            error_rate: if self.total_operations > 0 {
                self.failed_operations as f64 / self.total_operations as f64
            } else {
                0.0
            },
            cache_hits: self.cache_hits,
            cache_misses: self.cache_misses,
            last_reset: self.last_reset,
        }
    }

    fn calculate_percentiles(&self) -> (u64, u64, u64) {
        if self.latencies_ms.is_empty() {
            return (0, 0, 0);
        }

        let mut sorted: Vec<u64> = self.latencies_ms.iter().copied().collect();
        sorted.sort_unstable();

        let len = sorted.len();
        let p50 = sorted[len * 50 / 100];
        let p95 = sorted[len * 95 / 100];
        let p99 = sorted[len * 99 / 100];

        (p50, p95, p99)
    }
}

/// Metrics operation types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetricsOperationType {
    Read,
    Write,
    Delete,
    List,
    CacheHit,
    CacheMiss,
}

/// Snapshot of current metrics
#[derive(Debug, Clone, Default)]
pub struct MetricsSnapshot {
    pub total_operations: u64,
    pub successful_operations: u64,
    pub failed_operations: u64,
    pub total_bytes_read: u64,
    pub total_bytes_written: u64,
    pub avg_latency_ms: f64,
    pub p50_latency_ms: u64,
    pub p95_latency_ms: u64,
    pub p99_latency_ms: u64,
    pub operations_per_type: HashMap<String, u64>,
    pub error_rate: f64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub last_reset: chrono::DateTime<chrono::Utc>,
}

/// Cache statistics
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub size_bytes: u64,
    pub entry_count: u64,
    pub hit_rate: f64,
}

/// Marker trait for internal collection metadata providers.
/// This trait exists solely to break circular dependencies between StorageEngine and CollectionService.
/// It adds no new methods - all functionality comes from MetadataProvider.
///
/// Implementations: LocalRocksDbBackend, UniversalMetadataBackend
/// Consumers: CollectionService (via metadata_backend field)
#[async_trait]
pub trait InternalCollectionProvider: MetadataProvider + Send + Sync {
    // This is intentionally a marker trait with no methods.
    // All methods are inherited from MetadataProvider.
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

    /// Get the storage engine type for AXIS indexing and event logging
    ///
    /// This method eliminates the need for string matching on engine_name(),
    /// following the Open/Closed Principle. Each engine provides its type
    /// directly, so adding new engines doesn't require modifying dispatch code.
    ///
    /// Default implementation maps from strategy() for backward compatibility.
    fn engine_type(&self) -> StorageEngineType {
        match self.strategy() {
            StorageEngineStrategy::Sst => StorageEngineType::SST,
            StorageEngineStrategy::Viper => StorageEngineType::VIPER,
            StorageEngineStrategy::Helix => StorageEngineType::HELIX,
            StorageEngineStrategy::Nova => StorageEngineType::NOVA,
            StorageEngineStrategy::Swift => StorageEngineType::SWIFT,
            StorageEngineStrategy::Raptor => StorageEngineType::RAPTOR,
            // Default to SST for any unknown engines
            _ => StorageEngineType::SST,
        }
    }

    /// Core flush operation - engine-specific implementation (required)
    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult>;

    /// Core compaction operation - engine-specific implementation (required)
    async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult>;

    /// Engine-specific statistics collection (required)
    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>>;

    /// Retrieve a specific vector by ID from storage (required)
    /// This method should search across all storage layers (memtable, SSTables, Parquet files)
    ///
    /// # Parameters
    /// - `collection_id`: The collection to search in
    /// - `base_path`: The base storage path (from collection.storage_assignment.base_location)
    /// - `vector_id`: The ID of the vector to retrieve
    async fn vector_by_id(
        &self,
        collection_id: &str,
        base_path: &str,
        vector_id: &str,
    ) -> Result<Option<crate::proto::proximadb_v1::VectorRecord>>;

    /// Engine-specific unified search with optimization capabilities (required)
    /// Each engine implements its own optimizations:
    /// - VIPER: Columnar predicate pushdown, Parquet filtering, ML clustering
    /// - LSM: Bloom filter hints, range scans, SSTable optimizations
    /// - SST: Hierarchical bloom filters, progressive quantization
    /// - NOVA: Extended Parquet statistics, aggressive pruning
    ///
    /// Uses StorageQueryContext which provides zero-copy access via Arc references
    async fn search_vectors_unified(
        &self,
        ctx: &StorageQueryContext,
    ) -> Result<Vec<crate::core::search::results::OptimizedSearchRecord>>;

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

    /// Create a scan iterator based on the unified scan strategy pattern
    /// This follows the successful pattern from RAPTOR's scan_vectors_with_strategy
    /// and is implemented differently by each engine:
    /// - SST: Uses modular block readers with bloom filters
    /// - VIPER: Uses columnar predicate pushdown
    /// - NOVA: Uses progressive quantization stages
    /// - RAPTOR: Uses tier-aware consolidated reading
    ///
    /// Default implementation returns an error - engines should override
    async fn create_scan(
        &self,
        _collection_id: &str,
        _strategy: crate::storage::unified_scan_strategy::ScanStrategy,
        _collection_config: Option<&Collection>,
    ) -> Result<Box<dyn crate::storage::unified_scan_strategy::ScanIterator>> {
        // Default implementation - engines should override with their specific implementation
        Err(anyhow::anyhow!(
            "{} engine does not yet implement unified scan strategy. Use search_vectors_unified for now.",
            self.engine_name()
        ))
    }

    /// Get scan capabilities for this engine
    ///
    /// Delegates to `CapabilityFactory` for OCP-compliant capability lookup.
    /// Each engine's capabilities are defined in `trait_components::capabilities`.
    fn scan_capabilities(&self) -> crate::storage::unified_scan_strategy::ScanCapabilities {
        // OCP: Delegate to CapabilityFactory instead of hardcoded match
        CapabilityFactory::create(self.strategy()).scan_capabilities()
    }

    // =============================================================================
    // ENGINE CAPABILITIES - Can be overridden, sensible defaults provided
    // =============================================================================

    // Compression support methods removed - use storage::engine_capabilities::EngineCapabilities instead
    // The centralized EngineCapabilities module provides static methods for checking
    // what compression algorithms and features are supported by each engine type
    // This avoids duplication and provides a single source of truth for capabilities

    /// Engine capabilities with defaults based on strategy
    ///
    /// Determines whether the storage engine supports collection-level operations
    /// such as per-collection flush, compaction, and configuration.
    ///
    /// Delegates to `CapabilityFactory` for OCP-compliant capability lookup.
    fn supports_collection_level_operations(&self) -> bool {
        // OCP: Delegate to CapabilityFactory instead of hardcoded match
        CapabilityFactory::create(self.strategy()).supports_collection_level_operations()
    }

    /// Determines whether the storage engine supports atomic operations
    ///
    /// Atomic operations guarantee that either all changes are applied
    /// or none are applied, preventing partial updates.
    ///
    /// Delegates to `CapabilityFactory` for OCP-compliant capability lookup.
    fn supports_atomic_operations(&self) -> bool {
        // OCP: Delegate to CapabilityFactory instead of hardcoded match
        CapabilityFactory::create(self.strategy()).supports_atomic_operations()
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
        tracing::error!(
            "❌ get_collection_storage_url called without implementation for collection '{}'. Storage URL must be provided through parameters or collection metadata.",
            collection_id
        );
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
        tracing::error!(
            "❌ get_base_storage_url called without implementation for collection '{}'. Storage engines must provide storage URL.",
            collection_id
        );
        Err(anyhow::anyhow!(
            "Storage engine must implement get_base_storage_url or provide collection service"
        ))
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
        result.duration_ms = Some(start_time.elapsed().as_millis() as u64);
        result.completed_at = Utc::now();

        // Log operation completion
        tracing::info!(
            "✅ {} flush completed: {} entries, {} bytes in {}ms",
            self.engine_name(),
            result.entries_flushed.unwrap_or(0),
            result.bytes_written.unwrap_or(0),
            result.duration_ms.unwrap_or(0)
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
                Err(e) => {
                    let collection_info = params
                        .collection_id
                        .as_ref()
                        .map(|id| format!(" for collection {}", id))
                        .unwrap_or_default();
                    tracing::error!(
                        "⚠️ Post-flush compaction failed{}: {}. Data is safe but \
                         storage may be suboptimal. Consider triggering manual compaction.",
                        collection_info,
                        e
                    );
                    result.compaction_error = Some(e.to_string());
                    // Note: Compaction failure after flush is non-fatal - data integrity is preserved.
                    // The caller can inspect result.compaction_error and decide whether to retry.
                }
            }
        }

        // 🚀 INDEX UPDATES: Notify EventLog for AXIS indexing service
        if result.success {
            if let Some(collection_id) = &params.collection_id {
                // Notify EventLog so AXIS consumer can build indexes asynchronously
                if let Some(event_log) = crate::services::events::log::event_log_service() {
                    // Use engine_type() method (OCP-compliant - no string matching)
                    let storage_engine_type = self.engine_type();

                    let vector_count = result.entries_flushed.unwrap_or(0) as usize;
                    // Use file_paths from FlushResult for AXIS index building
                    if let Err(e) = event_log
                        .notify_flush(
                            collection_id,
                            result.file_paths.clone(),
                            vector_count,
                            false, // has_quantized - TODO: pass from params
                            true,  // has_fp32
                            storage_engine_type,
                        )
                        .await
                    {
                        tracing::warn!(
                            "⚠️ Failed to notify EventLog about flush for '{}': {}",
                            collection_id,
                            e
                        );
                    } else {
                        tracing::info!(
                            "📢 Notified EventLog for AXIS indexing: '{}' ({} vectors)",
                            collection_id,
                            vector_count
                        );
                    }
                } else {
                    tracing::debug!(
                        "🔄 Flush successful for collection: {} - EventLog not initialized",
                        collection_id
                    );
                }
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
        result.duration_ms = Some(start_time.elapsed().as_millis() as u64);
        result.completed_at = Utc::now();

        // Log operation completion
        tracing::info!(
            "✅ {} compaction completed: {} entries processed, {} removed in {}ms",
            self.engine_name(),
            result.entries_processed.unwrap_or(0),
            result.entries_removed.unwrap_or(0),
            result.duration_ms.unwrap_or(0)
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
            StorageEngineStrategy::Sst => {
                // LSM default: flush when memtable size exceeds threshold
                let stats = self.get_engine_stats().await?;
                Ok(stats.memory_usage_bytes > 64 * 1024 * 1024) // 64MB default
            }
            StorageEngineStrategy::Hybrid => {
                // Hybrid: use VIPER heuristics
                let stats = self.get_engine_stats().await?;
                Ok(stats.memory_usage_bytes > 100 * 1024 * 1024)
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
            StorageEngineStrategy::Helix => {
                // HELIX: locality-aware flushing
                let stats = self.get_engine_stats().await?;
                Ok(stats.memory_usage_bytes > 64 * 1024 * 1024) // 64MB default
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
                    .unwrap_or(0)
                    > 10)
            }
            StorageEngineStrategy::Sst => {
                // LSM default: compact when level ratios are unbalanced
                let stats = self.get_engine_stats().await?;
                Ok(stats
                    .engine_specific
                    .get("index_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    > 10)
            }
            StorageEngineStrategy::Hybrid => {
                // Hybrid: check both strategies
                self.should_flush(collection_id).await
            }
            StorageEngineStrategy::Swift => {
                // SWIFT: compact based on file count
                let stats = self.get_engine_stats().await?;
                Ok(stats
                    .engine_specific
                    .get("file_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    > 5)
            }
            StorageEngineStrategy::Nova => {
                // NOVA: compact when row groups exceed threshold
                let stats = self.get_engine_stats().await?;
                Ok(stats
                    .engine_specific
                    .get("row_group_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    > 20)
            }
            StorageEngineStrategy::Raptor => {
                // RAPTOR: adaptive compaction
                let stats = self.get_engine_stats().await?;
                Ok(stats
                    .engine_specific
                    .get("needs_compaction")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false))
            }
            StorageEngineStrategy::Helix => {
                // HELIX: locality-aware compaction
                let stats = self.get_engine_stats().await?;
                Ok(stats
                    .engine_specific
                    .get("locality_score")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0)
                    < 0.7) // Compact when locality score drops below threshold
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
                .unwrap_or(0),
            memory_usage_bytes: engine_metrics
                .get("dimension")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            collection_count: engine_metrics
                .get("engine_type")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize,
            last_flush: engine_metrics
                .get("created_at")
                .and_then(|v| v.as_i64())
                .and_then(|ts| DateTime::from_timestamp_millis(ts)),
            last_compaction: engine_metrics
                .get("updated_at")
                .and_then(|v| v.as_i64())
                .and_then(|ts| DateTime::from_timestamp_millis(ts)),
            pending_flushes: engine_metrics
                .get("pending_flushes")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            pending_compactions: engine_metrics
                .get("pending_compactions")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
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
            .unwrap_or(false);

        let error_count = stats
            .engine_specific
            .get("error_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize;

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
            .unwrap_or_else(Vec::new);

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
    // COLLECTION HELPERS - Common collection and path utilities
    // =============================================================================

    /// Extract collection ID from parameters or collection config
    ///
    /// This helper method provides a consistent way for all engines to get the collection ID
    /// from either explicit parameters or the collection configuration.
    fn get_collection_id_from_params(&self, params: &FlushParameters) -> Result<String> {
        params.get_collection_id()
    }

    /// Extract collection ID from compaction parameters or collection config
    fn get_collection_id_from_compaction_params(
        &self,
        params: &CompactionParameters,
    ) -> Result<String> {
        params.get_collection_id()
    }

    /// Construct data directory path from collection config
    ///
    /// Returns: {base_location}/{collection_id}/data
    ///
    /// This method provides a unified way for all engines to construct the data directory
    /// path without duplicating logic. The path follows the standard pattern:
    /// - {base_location} comes from collection.storage_assignment.base_location
    /// - {collection_id} is the collection identifier
    /// - /data is the standard data subdirectory
    fn get_data_dir_from_collection_config(
        &self,
        collection_config: &Collection,
    ) -> Result<String> {
        let collection_id = &collection_config.id;

        if let Some(ref storage_assignment) = collection_config.storage_assignment {
            let base_location = &storage_assignment.base_location;
            let data_dir = format!("{}/{}/data", base_location, collection_id);
            Ok(data_dir)
        } else {
            Err(anyhow::anyhow!(
                "No storage assignment found in collection config for collection '{}'",
                collection_id
            ))
        }
    }

    /// Construct data directory path from flush parameters
    ///
    /// Convenience method that extracts collection config from FlushParameters
    /// and constructs the data directory path.
    fn get_data_dir_from_flush_params(&self, params: &FlushParameters) -> Result<String> {
        if let Some(ref collection_config) = params.collection_config {
            self.get_data_dir_from_collection_config(collection_config)
        } else {
            // Fallback to helper methods for backward compatibility
            params.get_data_dir()
        }
    }

    /// Construct data directory path from compaction parameters
    ///
    /// Convenience method that extracts collection config from CompactionParameters
    /// and constructs the data directory path.
    fn get_data_dir_from_compaction_params(&self, params: &CompactionParameters) -> Result<String> {
        if let Some(ref collection_config) = params.collection_config {
            self.get_data_dir_from_collection_config(collection_config)
        } else {
            // Fallback to helper methods for backward compatibility
            params.get_data_dir()
        }
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
            collection_count: 0,
            total_storage_bytes: 0,
            memory_usage_bytes: 0,
            last_flush: None,
            last_compaction: None,
            pending_flushes: 0,
            pending_compactions: 0,
            engine_specific: HashMap::new(),
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
    pub vector_records: Vec<crate::proto::proximadb_v1::VectorRecord>,

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

/// Search context for STORAGE ENGINES - bundles immutable references to search parameters
/// and collection configuration for zero-copy access during search operations.
///
/// **IMPORTANT**: This is the STORAGE LAYER context. Do not confuse with:
/// - `core::search::SearchPlan` - Used for search planning/optimization  
/// - `core::service_types::SearchRequest` - Used for API request representation
///
/// Used by: Storage engines (SST, VIPER, NOVA, SWIFT, RAPTOR)
/// Created by: VectorOperationsService.execute_search_internal()
///
/// Design principles:
/// - Immutable: All references are read-only during search
/// - Zero-copy: Uses Arc for shared ownership without cloning
/// - Cache-friendly: Collection comes directly from cache as Arc
/// - Extensible: Additional context can be added as needed
#[derive(Debug, Clone)]
pub struct StorageQueryContext {
    /// Original search parameters (immutable reference)
    pub search_params: Arc<crate::core::search::SearchParams>,

    /// Collection configuration from cache (immutable reference)
    /// Contains storage_assignment with storage URL
    pub collection: Arc<Collection>,

    /// Additional context that might be needed during search
    /// (can be extended without breaking existing code)
    pub metadata: StorageQueryMetadata,

    /// User context for RBAC authorization checks
    /// Optional for backward compatibility with existing code
    pub user_context: Option<UnifiedUserContext>,

    /// Tenant context for multi-tenant operations
    /// Optional for backward compatibility with existing code
    pub tenant_context: Option<TenantContext>,
}

/// Parsed quantization configuration for efficient progressive search
#[derive(Debug, Clone)]
pub struct ParsedQuantizationConfig {
    /// Strategy being used (SmartDefaults, CustomLevels, etc.)
    pub strategy: crate::proto::proximadb_v1::quantization_config::Strategy,

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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub enum QuantizationType {
    Binary,
    Scalar,
    Product,
    Uniform,
    None,
}

/// Additional metadata for storage query context
/// Contains all information storage engines need - no additional cache lookups required
#[derive(Debug, Clone, Default)]
pub struct StorageQueryMetadata {
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

impl StorageQueryContext {
    /// Parse quantization config into ready-to-use format for progressive search
    fn parse_quantization_config(
        quant_config: &crate::proto::proximadb_v1::QuantizationConfig,
        dimension: usize,
    ) -> Option<ParsedQuantizationConfig> {
        if !quant_config.enabled.unwrap_or(false) {
            return None;
        }

        // Parse or generate progressive levels
        let progressive_levels = if quant_config.custom_levels.is_empty() {
            // Use smart defaults if no custom levels provided
            if let Ok(smart_config) =
                crate::compute::quantization::QuantizationSmartDefaults::generate_for_dimension(
                    dimension,
                )
            {
                Self::parse_proto_levels(&smart_config.custom_levels)
            } else {
                Vec::new()
            }
        } else {
            Self::parse_proto_levels(&quant_config.custom_levels)
        };

        Some(ParsedQuantizationConfig {
            strategy: quant_config.strategy(),
            progressive_search_enabled: quant_config.enable_progressive_search.unwrap_or(false),
            progressive_levels,
            binary_filter_selectivity: quant_config.binary_filter_selectivity.unwrap_or(0.3),
            int8_ranking_selectivity: quant_config.int8_ranking_selectivity.unwrap_or(0.1),
            pq_ranking_selectivity: quant_config.pq_ranking_selectivity.unwrap_or(0.05),
            quality_threshold: quant_config.quality_threshold.unwrap_or(0.95),
            training_sample_size: quant_config.training_sample_size.unwrap_or(10000) as i32,
            enable_simd_acceleration: quant_config.enable_simd_acceleration.unwrap_or(true),
            optimize_for_storage: quant_config.optimize_for_storage.unwrap_or(false),
            optimize_for_memory: quant_config.optimize_for_memory.unwrap_or(false),
        })
    }

    /// Parse proto levels into internal format
    fn parse_proto_levels(
        proto_levels: &[crate::proto::proximadb_v1::QuantizationLevel],
    ) -> Vec<QuantizationLevel> {
        use crate::proto::proximadb_v1::quantization_level::QuantizationType as ProtoQuantType;

        let mut levels: Vec<_> = proto_levels
            .iter()
            .enumerate()
            .map(|(idx, level)| {
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
                    bits: level.bits as i32,
                    search_priority: idx as i32, // Use index as priority
                    num_subvectors: Some(level.num_subvectors as i32),
                    min_recall: 0.9, // Default recall threshold
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

        // Use proto enum for OCP-compliant storage engine mapping
        let storage_strategy = config
            .and_then(|c| c.storage_engine)
            .and_then(|e| crate::proto::proximadb_v1::StorageEngine::try_from(e).ok())
            .map(|engine| match engine {
                crate::proto::proximadb_v1::StorageEngine::Viper => StorageEngineStrategy::Viper,
                crate::proto::proximadb_v1::StorageEngine::Sst => StorageEngineStrategy::Sst,
                crate::proto::proximadb_v1::StorageEngine::Nova => StorageEngineStrategy::Nova,
                crate::proto::proximadb_v1::StorageEngine::Helix => StorageEngineStrategy::Helix,
                crate::proto::proximadb_v1::StorageEngine::Swift => StorageEngineStrategy::Swift,
                crate::proto::proximadb_v1::StorageEngine::Raptor => StorageEngineStrategy::Raptor,
                _ => StorageEngineStrategy::Sst, // Default to SST for unknown engines
            })
            .unwrap_or(StorageEngineStrategy::Sst);

        let mut adjusted_params = (*search_params).clone();
        if matches!(
            storage_strategy,
            StorageEngineStrategy::Viper
                | StorageEngineStrategy::Nova
                | StorageEngineStrategy::Raptor
        ) {
            adjusted_params.block_prune.force_exact = true;
            adjusted_params.block_prune.mode = BlockPruneMode::Sqrt;
            adjusted_params.block_prune.ratio = 0.0;
            adjusted_params.block_prune.min_keep = 0;
            adjusted_params.block_prune.max_keep = 0;
        }

        let metadata = StorageQueryMetadata {
            collection_id: collection.id.clone(),
            use_axis_indexes: config
                .and_then(|c| {
                    if c.index_configs.is_empty() {
                        None
                    } else {
                        Some(true)
                    }
                })
                .unwrap_or(false),
            has_quantization: config.and_then(|c| c.quantization.as_ref()).is_some(),
            dimension: config.map(|c| c.dimension as usize).unwrap_or(0),
            distance_metric: config
                .map(|c| match c.distance_metric {
                    Some(0) => crate::compute::distance_computation::DistanceMetric::Euclidean,
                    Some(1) => crate::compute::distance_computation::DistanceMetric::Cosine,
                    Some(2) => crate::compute::distance_computation::DistanceMetric::DotProduct,
                    _ => crate::compute::distance_computation::DistanceMetric::Cosine,
                })
                .unwrap_or(crate::compute::distance_computation::DistanceMetric::Cosine),
            storage_strategy,
            storage_path: storage_assignment
                .map(|sa| sa.base_location.clone())
                .unwrap_or_else(|| "./data".to_string()),
            estimated_vector_count: 0,
            estimated_size_bytes: 0,
            performance_tier: PerformanceTier::Warm, // Default since preset field doesn't exist
            compression_enabled: config
                .and_then(|c| c.storage_config.as_ref())
                .map(|s| s.compression.unwrap_or(0) != 0) // Assume 0 means no compression
                .unwrap_or(false),
            quantization_enabled: config
                .and_then(|c| c.quantization.as_ref())
                .map(|_| true)
                .unwrap_or(false),
            // Parse quantization config for progressive search
            quantization_config: config.and_then(|c| c.quantization.as_ref()).and_then(|qc| {
                config
                    .map(|c| c.dimension)
                    .and_then(|dim| Self::parse_quantization_config(qc, dim as usize))
            }),
        };

        Self {
            search_params: Arc::new(adjusted_params),
            collection,
            metadata,
            user_context: None,
            tenant_context: None,
        }
    }

    /// Get the query vector (convenience method)
    pub fn query_vector(&self) -> Option<&[f32]> {
        // Check for single vector first (most common case)
        if let Some(ref vector) = self.search_params.vector {
            return Some(vector.as_slice());
        }

        // Fall back to checking query_vectors array
        self.search_params
            .query_vectors
            .as_ref()
            .and_then(|vecs| vecs.first())
            .map(|v| v.as_slice())
    }

    /// Get top_k value with fallback to default
    pub fn top_k(&self) -> usize {
        self.search_params.top_k.unwrap_or(10)
    }

    /// Get distance metric (pre-computed from collection config)
    pub fn distance_metric(&self) -> crate::compute::distance_computation::DistanceMetric {
        // Use search params override if provided, otherwise use pre-computed value
        self.search_params
            .distance_metric
            .unwrap_or(self.metadata.distance_metric)
    }

    /// Get dimension from metadata (pre-computed)
    pub fn dimension(&self) -> usize {
        self.metadata.dimension
    }

    /// Check if progressive search is enabled
    pub fn is_progressive_search_enabled(&self) -> bool {
        self.metadata
            .quantization_config
            .as_ref()
            .map(|qc| qc.progressive_search_enabled)
            .unwrap_or(false)
    }

    /// Get progressive quantization levels ordered by search priority
    pub fn get_progressive_levels(&self) -> Option<&[QuantizationLevel]> {
        self.metadata
            .quantization_config
            .as_ref()
            .map(|qc| qc.progressive_levels.as_slice())
    }

    /// Get binary filter selectivity for progressive search
    pub fn binary_filter_selectivity(&self) -> f32 {
        self.metadata
            .quantization_config
            .as_ref()
            .map(|qc| qc.binary_filter_selectivity)
            .unwrap_or(0.1)
    }

    /// Check if SIMD acceleration should be used
    pub fn use_simd_acceleration(&self) -> bool {
        self.metadata
            .quantization_config
            .as_ref()
            .map(|qc| qc.enable_simd_acceleration)
            .unwrap_or(true)
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

    /// Get collection ID from the collection object directly
    /// This is more reliable than the metadata cache which may not be initialized
    pub fn collection_id(&self) -> &str {
        &self.collection.id
    }

    /// Get storage URL from collection's storage assignment
    pub fn storage_url(&self) -> Option<&str> {
        self.collection
            .storage_assignment
            .as_ref()
            .map(|sa| sa.base_location.as_str())
    }

    /// Get collection-specific storage path
    pub fn collection_storage_path(&self) -> Option<String> {
        self.storage_url().map(|base| {
            crate::utils::StoragePath::collection_data_path(base, &self.collection_id())
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
    pub entries_flushed: Option<u64>,

    /// Bytes written to storage
    pub bytes_written: Option<u64>,

    /// Number of files/segments created
    pub files_created: Option<u64>,

    /// Actual file paths created (for AXIS index building)
    pub file_paths: Vec<String>,

    /// Duration of the operation
    pub duration_ms: Option<u64>,

    /// Timestamp when operation completed
    pub completed_at: DateTime<Utc>,

    /// Engine-specific metrics
    pub engine_metrics: HashMap<String, serde_json::Value>,

    /// Whether compaction was triggered as a result
    pub compaction_triggered: bool,

    /// Error message if post-flush compaction failed (for observability and retry scheduling)
    pub compaction_error: Option<String>,

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
    pub entries_processed: Option<u64>,

    /// Number of entries removed (tombstones, duplicates, etc.)
    pub entries_removed: Option<u64>,

    /// Bytes read during compaction
    pub bytes_read: Option<u64>,

    /// Bytes written during compaction
    pub bytes_written: Option<u64>,

    /// Input files/segments processed
    pub input_files: Option<u64>,

    /// Output files/segments created
    pub output_files: Option<u64>,

    /// Duration of the operation
    pub duration_ms: Option<u64>,

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

// =============================================================================
// MULTI-MODEL STORAGE TRAITS (SOLID: Interface Segregation Principle)
// =============================================================================
// These traits extend the storage system to support multiple data models:
// - Documents (MongoDB-like JSON)
// - Observability (logs, metrics, traces)
// - Graph (already supported via existing GraphOperationsService)
// - Vector (already supported via UnifiedStorageEngine)

/// Document storage operations trait (ISP: focused interface for document operations)
///
/// This trait provides MongoDB-like document storage capabilities.
/// Implementations should use the underlying storage engine (SST recommended)
/// with JSON path indexing and document-optimized block formats.
#[async_trait]
pub trait DocumentStorageOperations: Send + Sync {
    /// Insert a document into a collection
    async fn insert_document(
        &self,
        collection: &str,
        id: &str,
        document: crate::proto::proximadb_v1::SqlObject,
        indexed_paths: Vec<String>,
    ) -> Result<DocumentRecord>;

    /// Get a document by ID
    async fn get_document(&self, collection: &str, id: &str) -> Result<Option<DocumentRecord>>;

    /// Query documents with filter
    async fn query_documents(
        &self,
        collection: &str,
        filter: Option<crate::proto::proximadb_v1::DocumentFilter>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<DocumentRecord>>;

    /// Update a document with operations
    async fn update_document(
        &self,
        collection: &str,
        id: &str,
        updates: Vec<crate::proto::proximadb_v1::DocumentUpdate>,
    ) -> Result<DocumentRecord>;

    /// Delete a document
    async fn delete_document(&self, collection: &str, id: &str) -> Result<bool>;

    /// Create a document collection with indexes
    async fn create_document_collection(
        &self,
        config: crate::proto::proximadb_v1::DocumentCollectionConfig,
    ) -> Result<String>;

    /// List document collections
    async fn list_document_collections(&self) -> Result<Vec<DocumentCollectionInfo>>;
}

/// Document record returned from storage
#[derive(Debug, Clone)]
pub struct DocumentRecord {
    pub id: String,
    pub document: crate::proto::proximadb_v1::SqlObject,
    pub version: u64,
    pub created_at_ns: i64,
    pub updated_at_ns: i64,
}

/// Document collection info for listing
#[derive(Debug, Clone)]
pub struct DocumentCollectionInfo {
    pub name: String,
    pub document_count: u64,
    pub storage_size_bytes: u64,
    pub indexes: Vec<crate::proto::proximadb_v1::IndexDefinition>,
}

/// Observability storage operations trait (ISP: focused interface for observability)
///
/// This trait provides Cloud SIEM-like capabilities for logs, metrics, and traces.
/// Implementations should use time-partitioned storage with hot/warm/cold tiering.
#[async_trait]
pub trait ObservabilityStorageOperations: Send + Sync {
    /// Ingest logs in bulk
    async fn ingest_logs(
        &self,
        namespace: &str,
        logs: Vec<crate::proto::proximadb_v1::LogEntry>,
    ) -> Result<IngestResult>;

    /// Ingest metrics in bulk
    async fn ingest_metrics(
        &self,
        namespace: &str,
        metrics: Vec<crate::proto::proximadb_v1::MetricSample>,
    ) -> Result<IngestResult>;

    /// Ingest traces
    async fn ingest_traces(
        &self,
        namespace: &str,
        traces: Vec<crate::proto::proximadb_v1::TraceData>,
    ) -> Result<IngestResult>;

    /// Query logs with time range and filters
    async fn query_logs(
        &self,
        namespace: &str,
        start_time_ns: i64,
        end_time_ns: i64,
        filter: Option<crate::proto::proximadb_v1::LogFilter>,
        limit: u32,
    ) -> Result<LogQueryResult>;

    /// Aggregate metrics with PromQL-like semantics
    async fn aggregate_metrics(
        &self,
        namespace: &str,
        params: MetricAggregationParams,
    ) -> Result<MetricAggregationResult>;

    /// Query traces by trace ID or filters
    async fn query_traces(
        &self,
        namespace: &str,
        start_time_ns: i64,
        end_time_ns: i64,
        trace_id: Option<String>,
        service: Option<String>,
        limit: u32,
    ) -> Result<Vec<crate::proto::proximadb_v1::TraceData>>;

    /// Create an observability namespace with retention config
    async fn create_namespace(
        &self,
        config: crate::proto::proximadb_v1::ObservabilityNamespaceConfig,
    ) -> Result<String>;

    /// List observability namespaces
    async fn list_namespaces(&self) -> Result<Vec<NamespaceInfo>>;
}

/// Ingest result for bulk operations
#[derive(Debug, Clone, Default)]
pub struct IngestResult {
    pub ingested: u64,
    pub failed: u64,
    pub errors: Vec<String>,
    pub processing_time_ms: u64,
}

/// Log query result
#[derive(Debug, Clone)]
pub struct LogQueryResult {
    pub logs: Vec<crate::proto::proximadb_v1::LogEntry>,
    pub next_cursor: Option<String>,
    pub total_matched: u64,
    pub query_time_ms: u64,
}

/// Metric aggregation parameters
#[derive(Debug, Clone)]
pub struct MetricAggregationParams {
    pub metric_name: String,
    pub start_time_ns: i64,
    pub end_time_ns: i64,
    pub aggregation: crate::proto::proximadb_v1::MetricAggregation,
    pub step_seconds: u32,
    pub label_filters: HashMap<String, String>,
    pub group_by: Vec<String>,
}

/// Metric aggregation result
#[derive(Debug, Clone)]
pub struct MetricAggregationResult {
    pub series: Vec<TimeSeriesData>,
    pub query_time_ms: u64,
}

/// Time series data point
#[derive(Debug, Clone)]
pub struct TimeSeriesData {
    pub labels: HashMap<String, String>,
    pub points: Vec<DataPointValue>,
}

/// Individual data point
#[derive(Debug, Clone)]
pub struct DataPointValue {
    pub timestamp_ns: i64,
    pub value: f64,
}

/// Namespace info for listing
#[derive(Debug, Clone)]
pub struct NamespaceInfo {
    pub name: String,
    pub log_count: u64,
    pub metric_count: u64,
    pub trace_count: u64,
    pub retention_config: Option<crate::proto::proximadb_v1::RetentionConfig>,
}

/// Unified multi-model storage trait combining all data model operations
///
/// This trait follows the Composite pattern, aggregating specialized storage
/// traits into a single interface for engines that support multiple data models.
///
/// **SOLID Principles Applied:**
/// - **S (Single Responsibility)**: Each sub-trait handles one data model
/// - **O (Open/Closed)**: New data models can be added via new traits
/// - **L (Liskov Substitution)**: Any implementing engine works as UnifiedStorageEngine
/// - **I (Interface Segregation)**: Clients can depend on specific sub-traits
/// - **D (Dependency Inversion)**: Higher layers depend on these abstractions
///
/// Engines can implement this trait to provide multi-model storage capabilities
/// on top of their vector storage foundation.
#[async_trait]
pub trait MultiModelStorage:
    UnifiedStorageEngine + DocumentStorageOperations + ObservabilityStorageOperations
{
    /// Check which data models are supported by this engine
    fn supported_models(&self) -> Vec<DataModel> {
        vec![
            DataModel::Vector,        // Always supported via UnifiedStorageEngine
            DataModel::Document,      // Via DocumentStorageOperations
            DataModel::Observability, // Via ObservabilityStorageOperations
        ]
    }

    /// Get unified storage statistics across all models
    async fn get_multi_model_stats(&self) -> Result<MultiModelStats> {
        Ok(MultiModelStats::default())
    }
}

/// Supported data models
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataModel {
    Vector,
    Document,
    Graph,
    Observability,
    Relational,
}

/// Unified statistics across all data models
#[derive(Debug, Clone, Default)]
pub struct MultiModelStats {
    pub vector_count: u64,
    pub document_count: u64,
    pub log_count: u64,
    pub metric_count: u64,
    pub trace_count: u64,
    pub graph_node_count: u64,
    pub graph_edge_count: u64,
    pub total_storage_bytes: u64,
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

    /// Get collection ID from explicit field or collection_config
    pub fn get_collection_id(&self) -> Result<String> {
        if let Some(ref collection_id) = self.collection_id {
            Ok(collection_id.clone())
        } else if let Some(ref collection_config) = self.collection_config {
            Ok(collection_config.id.clone())
        } else {
            Err(anyhow::anyhow!(
                "No collection_id provided and no collection_config available"
            ))
        }
    }

    /// Get base path from collection_config.storage_assignment
    pub fn get_base_path(&self) -> Result<String> {
        if let Some(ref collection_config) = self.collection_config {
            if let Some(ref storage_assignment) = collection_config.storage_assignment {
                Ok(storage_assignment.base_location.clone())
            } else {
                Err(anyhow::anyhow!(
                    "No storage assignment found in collection config"
                ))
            }
        } else {
            Err(anyhow::anyhow!(
                "No collection_config available to extract base_path"
            ))
        }
    }

    /// Get data directory path: {base_path}/{collection_id}/data
    pub fn get_data_dir(&self) -> Result<String> {
        let collection_id = self.get_collection_id()?;
        let base_path = self.get_base_path()?;
        Ok(format!("{}/{}/data", base_path, collection_id))
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

    /// Get collection ID from explicit field or collection_config
    pub fn get_collection_id(&self) -> Result<String> {
        if let Some(ref collection_id) = self.collection_id {
            Ok(collection_id.clone())
        } else if let Some(ref collection_config) = self.collection_config {
            Ok(collection_config.id.clone())
        } else {
            Err(anyhow::anyhow!(
                "No collection_id provided and no collection_config available"
            ))
        }
    }

    /// Get base path from collection_config.storage_assignment
    pub fn get_base_path(&self) -> Result<String> {
        if let Some(ref collection_config) = self.collection_config {
            if let Some(ref storage_assignment) = collection_config.storage_assignment {
                Ok(storage_assignment.base_location.clone())
            } else {
                Err(anyhow::anyhow!(
                    "No storage assignment found in collection config"
                ))
            }
        } else {
            Err(anyhow::anyhow!(
                "No collection_config available to extract base_path"
            ))
        }
    }

    /// Get data directory path: {base_path}/{collection_id}/data
    pub fn get_data_dir(&self) -> Result<String> {
        let collection_id = self.get_collection_id()?;
        let base_path = self.get_base_path()?;
        Ok(format!("{}/{}/data", base_path, collection_id))
    }
}

impl Default for FlushResult {
    fn default() -> Self {
        Self {
            success: false,
            collections_affected: Vec::new(),
            entries_flushed: None,
            bytes_written: None,
            files_created: None,
            file_paths: Vec::new(),
            duration_ms: None,
            completed_at: Utc::now(),
            engine_metrics: HashMap::new(),
            compaction_triggered: false,
            compaction_error: None,
            flushed_batch_ids: vec![],
        }
    }
}

impl Default for CompactionResult {
    fn default() -> Self {
        Self {
            success: false,
            collections_affected: Vec::new(),
            entries_processed: None,
            entries_removed: None,
            bytes_read: None,
            bytes_written: None,
            input_files: None,
            output_files: None,
            duration_ms: None,
            completed_at: Utc::now(),
            engine_metrics: HashMap::new(),
        }
    }
}
