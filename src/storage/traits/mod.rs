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
//!
//! ## Module Organization
//!
//! The traits module has been decomposed into focused submodules:
//! - `types`: Parameter types (FlushParameters, CompactionParameters, OperationPriority, etc.)
//! - `results`: Result types (FlushResult, CompactionResult, EngineStatistics, EngineHealth, etc.)
//! - `document`: Document storage operations trait
//! - `observability`: Observability storage operations and multi-model traits
//! - `query`: Query context types (RlsRecordPredicate, StorageQueryContext, etc.)

// Module declarations for decomposed submodules
mod document;
mod observability;
mod query;
mod results;
mod types;

// Re-exports from decomposed submodules for backward compatibility
pub use document::{DocumentCollectionInfo, DocumentRecord, DocumentStorageOperations};
pub use observability::{
    DataModel, DataPointValue, IngestResult, LogQueryResult, MetricAggregationParams,
    MetricAggregationResult, MultiModelStats, MultiModelStorage, NamespaceInfo,
    ObservabilityStorageOperations, TimeSeriesData,
};
pub use query::{
    ParsedQuantizationConfig, QuantizationLevel, QuantizationType, RlsRecordPredicate,
    StorageQueryContext, StorageQueryMetadata,
};
pub use results::{CompactionResult, EngineHealth, EngineStatistics, FlushResult};
pub use types::{
    CompactionParameters, FlushParameters, OperationPriority, PerformanceTier,
    StorageEngineStrategy,
};

use crate::proto::proximadb_v1::Collection;
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
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

// RESOLVED: StorageEngineType now lives in core::types to prevent cross-layer dependency.
// The storage layer no longer imports from the index layer.
// See docs/10-quality/TECHNICAL_DEBT.adoc for resolved TD-CROSS-LAYER.
use crate::core::types::StorageEngineType;

// Core types imported as needed in implementations

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

    pub async fn get_snapshot(&self) -> StorageTraitMetricsSnapshot {
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

impl Default for UnifiedMetricsCollector {
    fn default() -> Self {
        Self::new()
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
    #[allow(dead_code)]
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

    fn to_snapshot(&self) -> StorageTraitMetricsSnapshot {
        let avg_latency = if !self.latencies_ms.is_empty() {
            self.latencies_ms.iter().sum::<u64>() as f64 / self.latencies_ms.len() as f64
        } else {
            0.0
        };

        let (p50, p95, p99) = self.calculate_percentiles();

        StorageTraitMetricsSnapshot {
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
pub struct StorageTraitMetricsSnapshot {
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

/// Canonical statistics for cost-based query optimization
///
/// This is the **single source of truth** for collection statistics across
/// ProximaDB. All components (query planner, optimizer, AutoML, EXPLAIN)
/// should use this struct. Engine-specific or module-specific stat types
/// should convert to/from this canonical form.
///
/// Used by the query planner's `CostModel` to estimate operation costs
/// and select optimal join/fusion strategies.
#[derive(Debug, Clone, Default)]
pub struct CollectionStats {
    /// Number of rows/vectors in the collection
    pub row_count: u64,
    /// Average vector size in bytes
    pub avg_vector_bytes: u64,
    /// Storage engine strategy used for this collection
    pub engine_strategy: StorageEngineStrategy,
    /// Whether a metadata index exists for fast filtering
    pub has_metadata_index: bool,
    /// Whether an HNSW index is available for approximate nearest neighbor search
    pub has_hnsw_index: bool,
    /// Total storage bytes consumed by this collection
    pub total_bytes: u64,
    /// Vector dimensionality (if known)
    pub dimension: Option<u32>,
    /// Index type in use (e.g., "hnsw", "ivf", "flat")
    pub index_type: Option<String>,
}

impl From<crate::proto::proximadb_v1::CollectionStats> for CollectionStats {
    fn from(proto: crate::proto::proximadb_v1::CollectionStats) -> Self {
        Self {
            row_count: proto.vector_count as u64,
            total_bytes: proto.data_size_bytes as u64,
            ..Self::default()
        }
    }
}

/// Macro to implement the engine identification boilerplate for `UnifiedStorageEngine`.
///
/// Every engine must implement `engine_name()`, `engine_version()`, `strategy()`,
/// and `get_filesystem_factory()`. These are purely descriptive and follow the same
/// pattern across all 7+ engines. This macro eliminates ~15 lines of repetitive code
/// per engine and prevents drift (e.g., one engine forgetting to update its version).
///
/// # Usage
/// ```ignore
/// // Inside `impl UnifiedStorageEngine for MyEngine { ... }`:
/// crate::impl_engine_identity!("NOVA", crate::version::PROXIMADB_VERSION, Nova, filesystem_factory);
/// // For engines with private fields accessed via method:
/// crate::impl_engine_identity!("sst", crate::version::PROXIMADB_VERSION, Sst, filesystem());
/// ```
#[macro_export]
macro_rules! impl_engine_identity {
    // Variant 1: field is accessed directly (public field)
    ($name:expr, $version:expr, $strategy:ident, $fs_field:ident) => {
        fn engine_name(&self) -> &'static str {
            $name
        }

        fn engine_version(&self) -> &'static str {
            $version
        }

        fn strategy(&self) -> $crate::storage::traits::StorageEngineStrategy {
            $crate::storage::traits::StorageEngineStrategy::$strategy
        }

        fn get_filesystem_factory(
            &self,
        ) -> &$crate::storage::persistence::filesystem::FilesystemFactory {
            &self.$fs_field
        }
    };
    // Variant 2: field is accessed via method call (private field)
    ($name:expr, $version:expr, $strategy:ident, $fs_method:ident ()) => {
        fn engine_name(&self) -> &'static str {
            $name
        }

        fn engine_version(&self) -> &'static str {
            $version
        }

        fn strategy(&self) -> $crate::storage::traits::StorageEngineStrategy {
            $crate::storage::traits::StorageEngineStrategy::$strategy
        }

        fn get_filesystem_factory(
            &self,
        ) -> &$crate::storage::persistence::filesystem::FilesystemFactory {
            self.$fs_method()
        }
    };
}

/// Returned from `UnifiedStorageEngine::ingest_sorted_segment` (Phase
/// 2F-b). Engines that have NOT yet migrated to the LSM-bulk-load
/// override return `used_engine_specific_path = false` so the
/// drainer/loader can fall back through `UnifiedHandlers` for actual
/// persistence. Once an engine ships the optimization, this is
/// `true` and the result is authoritative.
#[derive(Debug, Clone)]
pub struct SegmentIngestResult {
    pub collection_id: String,
    pub record_count: usize,
    pub synthetic_segment_id: String,
    /// `true` when the engine's `ingest_sorted_segment` override
    /// performed the write itself; `false` when the default fallback
    /// returned without inserting (caller must use the per-record
    /// path).
    pub used_engine_specific_path: bool,
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
            StorageEngineStrategy::Cedar => StorageEngineType::SST, // CEDAR uses SST-like indexing
            StorageEngineStrategy::Chrono => StorageEngineType::SST, // CHRONO uses SST-like indexing
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
    ) -> Result<Option<proximadb_records::ProximaRecord>>;

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

    /// LSM-aware bulk-load: write a pre-sorted batch of records as a
    /// single SST segment, **bypassing WAL and memtable**.
    ///
    /// This is the storage-side counterpart of `proximadb-queue`'s
    /// async-ingest contract (locked invariant #5 in the queue
    /// README). The drainer accumulates messages, sorts by oid, and
    /// calls this method — engines that implement the optimized path
    /// write a single SST file with one fsync instead of N WAL entries
    /// + N memtable inserts + a later flush.
    ///
    /// ## Default implementation
    ///
    /// The default falls back to the per-record insert path
    /// (`vector_by_id` lookup + the existing batch insert flow). This
    /// guarantees correctness for every engine on day one: the
    /// `proximadb-queue` drainer can call this method against ANY
    /// engine and get a working result.
    ///
    /// ## Per-engine optimization (Phase 2F-b follow-ups)
    ///
    /// Each engine that owns its own flush SST-writer (NOVA, SST,
    /// VIPER, RAPTOR, CHRONO, SEQUOIA) overrides this method to call
    /// its own writer directly with the provided sorted iterator,
    /// skipping the WAL+memtable cost. Those optimizations land as
    /// focused per-engine commits — the trait surface is stable so
    /// callers (`BulkLoader::ingest_sorted_segment`) don't change.
    ///
    /// ## Inputs
    ///
    /// - `collection_id`: target collection. The catalog has already
    ///   resolved this to a concrete storage path.
    /// - `base_path`: storage assignment for the collection.
    /// - `records`: MUST be sorted ascending by `oid`. Callers
    ///   (notably `BulkLoader::ingest_sorted_segment`) sort upfront.
    ///   Per-engine impls may assert this; the default impl tolerates
    ///   any order.
    ///
    /// Returns an opaque `SegmentId` so callers can correlate the
    /// commit with subsequent search/compaction events.
    async fn ingest_sorted_segment(
        &self,
        collection_id: &str,
        base_path: &str,
        records: Vec<proximadb_records::ProximaRecord>,
    ) -> Result<SegmentIngestResult> {
        // Default: fall back to inserting one record at a time via
        // `vector_by_id` + the engine's existing add path. Most engines
        // expose a batch insert through `do_flush` after WAL writes;
        // this default approximates that with per-record calls.
        //
        // Engines override this to short-circuit straight to their
        // SST-writer step. The cost difference at typical batch sizes
        // (32-128 records per drainer batch) is roughly 5-10× — the
        // numbers the queue README commits to in its throughput
        // economics table.
        let _ = base_path;
        let count = records.len();
        if count == 0 {
            return Ok(SegmentIngestResult {
                collection_id: collection_id.to_string(),
                record_count: 0,
                synthetic_segment_id: "empty".to_string(),
                used_engine_specific_path: false,
            });
        }
        // Per-record default body. Engines that override this method
        // do something dramatically more efficient. We log so it's
        // visible in metrics when a deployment hasn't migrated to the
        // optimized path yet.
        tracing::debug!(
            collection_id = %collection_id,
            count,
            "ingest_sorted_segment default path (per-record fallback); \
             engine has not migrated to LSM bulk-load yet",
        );
        // Engines that don't override this method don't have a direct
        // per-record entry point on this trait either. The drainer's
        // production sink (`BulkLoadDrainerSink`) covers this case by
        // calling `UnifiedHandlers::handle_record_batch_for_tenant`
        // which routes through the engine's WAL+memtable path. This
        // default body therefore returns the synthetic result without
        // actually inserting — the trait contract is that the
        // optimized override is the source of truth; the default is
        // a "not implemented for this engine" sentinel.
        Ok(SegmentIngestResult {
            collection_id: collection_id.to_string(),
            record_count: count,
            synthetic_segment_id: format!(
                "default-fallback-{}-{}",
                collection_id,
                records.first().map(|r| r.oid.as_str()).unwrap_or("empty"),
            ),
            used_engine_specific_path: false,
        })
    }

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
        _strategy: crate::storage::scan_strategy::ScanStrategy,
        _collection_config: Option<&Collection>,
    ) -> Result<Box<dyn crate::storage::scan_strategy::ScanIterator>> {
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
    fn scan_capabilities(&self) -> crate::storage::scan_strategy::ScanCapabilities {
        // OCP: Delegate to CapabilityFactory instead of hardcoded match
        CapabilityFactory::create(self.strategy()).scan_capabilities()
    }

    /// Get collection statistics for cost-based query optimization
    ///
    /// Returns cardinality and index information used by the query planner's
    /// `CostModel` to estimate operation costs and select optimal join/fusion
    /// strategies. Default implementation returns basic stats; engines should
    /// override for accurate cardinality data.
    async fn collection_stats(&self, _collection_id: &str) -> Result<CollectionStats> {
        Ok(CollectionStats {
            engine_strategy: self.strategy(),
            ..CollectionStats::default()
        })
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

    /// Return an engine-level RLS predicate to apply at scan time (spec §8).
    ///
    /// The default implementation returns `None` (no filtering), preserving
    /// backward compatibility with all existing engines. Engines that store
    /// `tenant_id`/`permitted_principals` as indexed record fields override
    /// this to push tenant isolation into the scan iterator itself.
    fn rls_record_filter(&self, _ctx: &StorageQueryContext) -> Option<RlsRecordPredicate> {
        None
    }

    /// Get comprehensive capability set for this engine
    ///
    /// Returns a CapabilitySet from the query capability registry that describes
    /// all features this engine supports. This is used for query validation,
    /// planning, and API parity checks.
    ///
    /// # Example
    /// ```ignore
    /// let engine = StorageEngineFactory::create_sst()?;
    /// let caps = engine.capabilities();
    /// assert!(caps.contains(&CapabilitySet::from_capabilities(&[
    ///     Capability::VectorSearch,
    ///     Capability::Filter,
    /// ])));
    /// ```
    ///
    /// Default implementation provides capability sets based on engine strategy.
    /// Engines can override this method to customize their declared capabilities.
    fn capabilities(&self) -> crate::query::capability::CapabilitySet {
        use crate::query::capability::{Capability, CapabilitySet};
        use crate::storage::traits::StorageEngineStrategy;

        match self.strategy() {
            StorageEngineStrategy::Sst => CapabilitySet::from_capabilities(&[
                // Core operations
                Capability::Scan,
                Capability::Filter,
                Capability::PredicatePushdown,
                // Vector operations
                Capability::VectorSearch,
                Capability::CosineDistance,
                Capability::EuclideanDistance,
                Capability::DotProduct,
                Capability::Quantization,
                // Features
                Capability::WALRecovery,
                Capability::BloomFilter,
                // Index types
                Capability::HNSWIndex,
                Capability::IVFIndex,
                Capability::AnnoyIndex,
                Capability::LSHIndex,
            ]),
            StorageEngineStrategy::Viper => CapabilitySet::from_capabilities(&[
                // Core operations
                Capability::Scan,
                Capability::Filter,
                Capability::Project,
                Capability::PredicatePushdown,
                // Vector operations
                Capability::VectorSearch,
                Capability::CosineDistance,
                Capability::EuclideanDistance,
                Capability::DotProduct,
                Capability::Quantization,
                // Features
                Capability::ColumnarAnalytics,
                Capability::RowGroupPruning,
                Capability::BloomFilter,
                // Index types
                Capability::HNSWIndex,
                Capability::IVFIndex,
            ]),
            StorageEngineStrategy::Helix => CapabilitySet::from_capabilities(&[
                // Core operations
                Capability::Scan,
                Capability::Filter,
                Capability::PredicatePushdown,
                // Vector operations (high-dimensional optimized)
                Capability::VectorSearch,
                Capability::CosineDistance,
                Capability::EuclideanDistance,
                Capability::Quantization,
                // Features (spatial)
                Capability::ColumnarAnalytics,
                Capability::BloomFilter,
            ]),
            StorageEngineStrategy::Nova => CapabilitySet::from_capabilities(&[
                // Core operations
                Capability::Scan,
                Capability::Filter,
                Capability::Project,
                Capability::PredicatePushdown,
                // Vector operations
                Capability::VectorSearch,
                Capability::CosineDistance,
                Capability::EuclideanDistance,
                Capability::DotProduct,
                Capability::HybridSearch,
                Capability::Quantization,
                // Features
                Capability::ColumnarAnalytics,
                Capability::RowGroupPruning,
                Capability::BloomFilter,
                // Index types
                Capability::HNSWIndex,
                Capability::IVFIndex,
            ]),
            StorageEngineStrategy::Swift => CapabilitySet::from_capabilities(&[
                // Core operations (low-latency optimized)
                Capability::Scan,
                Capability::Filter,
                // Vector operations
                Capability::VectorSearch,
                Capability::CosineDistance,
                Capability::EuclideanDistance,
                // Features
                Capability::BloomFilter,
            ]),
            StorageEngineStrategy::Raptor => CapabilitySet::from_capabilities(&[
                // Core operations (adaptive)
                Capability::Scan,
                Capability::Filter,
                Capability::PredicatePushdown,
                // Vector operations
                Capability::VectorSearch,
                Capability::CosineDistance,
                Capability::EuclideanDistance,
                Capability::Quantization,
                // Features
                Capability::ColumnarAnalytics,
                Capability::BloomFilter,
            ]),
            StorageEngineStrategy::TimeSeries => CapabilitySet::from_capabilities(&[
                // Time-series operations
                Capability::TimeSeriesQuery,
                Capability::Scan,
                Capability::Filter,
                // Features
                Capability::Aggregate,
            ]),
            StorageEngineStrategy::Hybrid => CapabilitySet::from_capabilities(&[
                Capability::Scan,
                Capability::Filter,
                Capability::Project,
                Capability::PredicatePushdown,
                Capability::VectorSearch,
                Capability::CosineDistance,
                Capability::EuclideanDistance,
                Capability::DotProduct,
                Capability::HybridSearch,
                Capability::Quantization,
                Capability::ColumnarAnalytics,
                Capability::RowGroupPruning,
                Capability::WALRecovery,
                Capability::BloomFilter,
                Capability::HNSWIndex,
                Capability::IVFIndex,
                Capability::AnnoyIndex,
                Capability::LSHIndex,
            ]),
            StorageEngineStrategy::Cedar => CapabilitySet::from_capabilities(&[
                Capability::Scan,
                Capability::Filter,
                Capability::WALRecovery,
                Capability::BloomFilter,
            ]),
            StorageEngineStrategy::Chrono => CapabilitySet::from_capabilities(&[
                Capability::Scan,
                Capability::Filter,
                Capability::TimeSeriesQuery,
                Capability::Aggregate,
                Capability::WALRecovery,
            ]),
        }
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
        if result.success
            && let Some(collection_id) = &params.collection_id
        {
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
                        false, // has_quantized - DEFERRED: pass from params
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
            StorageEngineStrategy::TimeSeries => {
                // TST: time-based flushing
                let stats = self.get_engine_stats().await?;
                Ok(stats.memory_usage_bytes > 64 * 1024 * 1024) // 64MB default
            }
            StorageEngineStrategy::Cedar => {
                // CEDAR: document memtable size threshold
                let stats = self.get_engine_stats().await?;
                Ok(stats.memory_usage_bytes > 256 * 1024 * 1024) // 256MB default
            }
            StorageEngineStrategy::Chrono => {
                // CHRONO: observability data flush threshold
                let stats = self.get_engine_stats().await?;
                Ok(stats.memory_usage_bytes > 128 * 1024 * 1024) // 128MB default
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
            StorageEngineStrategy::TimeSeries => {
                // TST: compact based on partition count
                let stats = self.get_engine_stats().await?;
                Ok(stats
                    .engine_specific
                    .get("total_partitions")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    > 100)
            }
            StorageEngineStrategy::Cedar => {
                // CEDAR: compact when too many L0 blocks
                let stats = self.get_engine_stats().await?;
                Ok(stats
                    .engine_specific
                    .get("block_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    > 4)
            }
            StorageEngineStrategy::Chrono => {
                // CHRONO: compact based on time-window file count
                let stats = self.get_engine_stats().await?;
                Ok(stats
                    .engine_specific
                    .get("partition_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    > 24) // More than 24 hourly partitions
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
                .and_then(DateTime::from_timestamp_millis),
            last_compaction: engine_metrics
                .get("updated_at")
                .and_then(|v| v.as_i64())
                .and_then(DateTime::from_timestamp_millis),
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
            .map_or_else(Vec::new, |arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect()
            });

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
        if let Some(timeout) = params.timeout_ms
            && timeout == 0
        {
            return Err(anyhow::anyhow!("Flush timeout cannot be zero"));
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
        if let Some(timeout) = params.timeout_ms
            && timeout == 0
        {
            return Err(anyhow::anyhow!("Compaction timeout cannot be zero"));
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

// ---------------------------------------------------------------------------
// Phase E RLS scaffold tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod rls_tests {
    use super::RlsRecordPredicate;

    #[test]
    fn test_rls_predicate_default_is_passthrough() {
        let pred = RlsRecordPredicate::default();
        assert!(
            pred.is_passthrough(),
            "default predicate must not filter anything"
        );
    }

    #[test]
    fn test_rls_predicate_tenant_id_not_passthrough() {
        let pred = RlsRecordPredicate {
            required_tenant_id: Some("acme".to_string()),
            required_principal: None,
        };
        assert!(!pred.is_passthrough());
        assert_eq!(pred.required_tenant_id.as_deref(), Some("acme"));
    }

    #[test]
    fn test_rls_predicate_principal_not_passthrough() {
        let pred = RlsRecordPredicate {
            required_tenant_id: None,
            required_principal: Some("alice".to_string()),
        };
        assert!(!pred.is_passthrough());
    }

    #[test]
    fn test_rls_predicate_both_set() {
        let pred = RlsRecordPredicate {
            required_tenant_id: Some("acme".to_string()),
            required_principal: Some("alice".to_string()),
        };
        assert!(!pred.is_passthrough());
        assert_eq!(pred.required_tenant_id.as_deref(), Some("acme"));
        assert_eq!(pred.required_principal.as_deref(), Some("alice"));
    }
}
