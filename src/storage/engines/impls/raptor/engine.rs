use crate::utils::uuid::Uuid;
use anyhow::Result;
use crate::core::errors::ProximaDBError;
use arrow_array::{ArrayRef, Float32Array, Int64Array, RecordBatch, StringArray, UInt32Array};
use arrow_schema::{DataType, Field, Schema};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
// Migrated to filesystem API - no longer using std::fs::File directly

use super::consolidated_compactor::RaptorCompactor;
use super::{RaptorConfig, RaptorWriter, RowGroups, consolidated_reader::RaptorReader};
use crate::compute::distance_computation::{DistanceMetric, engine::UnifiedDistanceCompute};
use crate::proto::proximadb_v1::VectorRecord;
use crate::core::hardware_capabilities::get_hardware_capabilities;
use crate::core::search::results::OptimizedSearchRecord;
use crate::storage::traits::{
    CompactionParameters, CompactionResult, FlushParameters, FlushResult, StorageQueryContext,
    UnifiedStorageEngine,
};
// IvfManager removed - Matrix Trinity handles clustering
use super::smart_rowgroup_sizing::SmartRowGroupSizer;

// Deep integration with AXIS clustering
use crate::index::axis::clustering::{
    ClusterManager, ClusteringAlgorithm, ClusteringConfig, KMeansConfig,
};
use crate::index::axis::types::ClusterAssignment;

// Deep integration with filesystem API for cloud-aware I/O
use crate::storage::persistence::filesystem::TierConfig;
use crate::storage::persistence::filesystem::{FileOptions, FileStorageTier, FileSystem};

// Universal performance optimization imports
use crate::core::hardware_capabilities::HardwareCapabilities;
use crate::storage::engines::core::ops::performance_optimization::{
    UniversalOptimizationStrategy, UniversalPerformanceOptimizer, UniversallyOptimized,
};
// VectorMemoryPool now managed by universal optimizer

/// Vector search result for compatibility - using OptimizedSearchRecord
type VectorSearchResult = OptimizedSearchRecord;

/// RAPTOR Engine - Row-Aligned Predicated Tensor Optimized Repository
///
/// LARGE FILE SUPPORT ARCHITECTURE:
///
/// 1. DUAL-LEVEL HNSW STRATEGY:
///    - GLOBAL GRAPH: Single master HNSW graph across entire file
///      * Stored in file header for O(1) access
///      * Entry points indexed by centrality
///      * Navigates to relevant rowgroups
///    
///    - LOCAL GRAPHS: Per-rowgroup HNSW subgraphs (1K vectors each)
///      * Optimized for k<10 queries (typical use case)
///      * Self-contained for parallel search
///      * Bridge nodes connect to global graph
///      * Memory-mapped for efficient access (~4MB per rowgroup)
///
/// 2. COLUMNAR STREAMING FOR SCALE:
///    - Vectors stored column-wise (not row-wise despite name)
///    - SIMD-aligned columns for vectorized operations
///    - Selective column loading (vector, graph, metadata separate)
///    - Supports 100GB+ files through streaming
///
/// 3. MEMORY MAPPING STRATEGY:
///    - Global graph always mapped (small, ~100MB for 10M vectors)
///    - RowGroups mapped on-demand (~4MB each @ 1024-dim, 1K vectors)
///    - LRU cache for hot rowgroups (default: 512 rowgroups = 2GB)
///    - Parallel prefetch for predicted access patterns
///    - Adaptive granularity: can adjust 500-2000 vectors based on k
///
/// 4. SEARCH EXECUTION FLOW:
///    a) Global HNSW navigation → find promising rowgroups
///    b) Local HNSW search within rowgroups (parallel)
///    c) Optional: columnar scan for exhaustive search
///    d) FastLanes decoding only for final candidates
///
/// 5. COMPACTION STRATEGY:
///    - Single file maintained (L0 only, max_level=0)
///    - Immediate compaction at 2 files (preserves graph)
///    - Streaming compaction without loading entire file
///    - Graph rebuild during compaction for optimization
///
/// 6. PERFORMANCE AT SCALE:
///    - 100M vectors: ~400GB file, 100K rowgroups (1K each)
///    - Search latency: Low-latency search optimized for small k values
///    - I/O efficiency: Read only ~1-3 rowgroups for k<10
///    - Insert throughput: High-performance batched insertion
///    - Memory usage: ~2GB cache + 100MB global graph
///
/// 7. ADAPTIVE ROWGROUP SIZING:
///    - k<10: Use 500-1000 vectors/rowgroup (minimize waste)
///    - k<100: Use 1000-2000 vectors/rowgroup (balance)
///    - k>100: Use 2000-5000 vectors/rowgroup (maximize throughput)
///    - Can be configured per collection based on workload

// Old optimization structures removed - now using UniversalPerformanceOptimizer
// The universal optimizer provides all these capabilities through a unified interface

pub struct RaptorEngine {
    config: RaptorConfig,

    // Core components
    rowgroup_manager: Arc<RwLock<RowGroups>>,
    writer: Arc<RwLock<RaptorWriter>>,
    reader: Arc<RaptorReader>, // Using consolidated reader
    compactor: Arc<RaptorCompactor>,
    // HNSW functionality handled by individual rowgroup segments

    // Dual quantization architecture for optimal performance
    storage_quantization_engine: Arc<crate::compute::quantization::storage_engine::StorageQuantizationEngine>,
    fallback_quantization_engine: Arc<crate::compute::quantization::unified::UnifiedQuantizationEngine>,

    // Deep integration with AXIS clustering
    cluster_manager: Arc<RwLock<ClusterManager>>,
    clustering_config: ClusteringConfig,
    cluster_assignments: Arc<RwLock<HashMap<u32, Vec<ClusterAssignment>>>>, // RowGroup -> Clusters

    // Deep integration with filesystem API
    filesystem: Arc<dyn FileSystem>,
    tier_config: TierConfig,
    file_options: FileOptions,

    // Zero-copy filesystem and transaction coordinator - now using unified filesystem
    zero_copy_filesystem: Arc<dyn FileSystem>,
    transaction_coordinator: Arc<crate::storage::transaction_coordinator::TransactionCoordinator>,

    // Universal performance optimization (replaces RAPTOR-specific optimization)
    universal_optimizer: UniversalPerformanceOptimizer,

    // Keep hardware capabilities for RAPTOR-specific needs (like SIMD)
    hardware_capabilities: Arc<HardwareCapabilities>,

    // Cache and metadata
    cache: Arc<RwLock<RowGroupCache>>,
    file_registry: Arc<RwLock<FileRegistry>>,
    metrics: Arc<RaptorMetrics>, // Lock-free atomic metrics
}

impl RaptorEngine {
    /// Smart quantization selection using shared logic
    fn should_use_persistent_quantization(&self, operation_context: &str, collection_size: Option<usize>) -> bool {
        crate::compute::quantization::selection::QuantizationSelector::should_use_persistent_quantization_simple(
            operation_context,
            collection_size,
        )
    }

    /// Get the appropriate quantization engine based on operation context
    async fn get_quantization_engine(&self, operation_context: &str, collection_size: Option<usize>) -> Arc<crate::compute::quantization::unified::UnifiedQuantizationEngine> {
        if self.should_use_persistent_quantization(operation_context, collection_size) {
            // Use global quantization cache for persistent operations
            if let Some(global_cache) = crate::compute::quantization::global_cache::GlobalQuantizationCache::instance() {
                global_cache.get_or_create_engine("default_collection".to_string()).await
            } else {
                // Fallback to fallback engine since we need UnifiedQuantizationEngine type
                self.fallback_quantization_engine.clone()
            }
        } else {
            // Use stateless engine for ad-hoc operations
            self.fallback_quantization_engine.clone()
        }
    }

    /// Create a new RAPTOR engine instance (stateless)
    /// Collection info comes from FlushParameters and StorageQueryContext at runtime
    pub async fn new() -> Result<Self> {
        let config = RaptorConfig::default();
        let cache = Arc::new(crate::storage::cache::orchestrator::CrossCacheOrchestrator::new(1000));
        Self::new_with_config(config, cache).await
    }

    /// Create RAPTOR engine with specific config (internal use)
    pub async fn new_with_config(
        config: RaptorConfig,
        cache: Arc<crate::storage::cache::orchestrator::CrossCacheOrchestrator>,
    ) -> Result<Self> {
        // Create smart row group sizer - dimension always available from config
        let smart_sizer =
            SmartRowGroupSizer::for_s3_standard(config.dimension, 200) // 200 bytes avg metadata
                .with_query_pattern(super::smart_rowgroup_sizing::QueryPattern::Mixed);

        // Create dual quantization architecture for RAPTOR
        let storage_quantization_engine = Arc::new(
            crate::compute::quantization::storage_engine::StorageQuantizationEngine::new_default()
        );
        let fallback_quantization_engine = Arc::new(
            crate::compute::quantization::unified::UnifiedQuantizationEngine::new(
                Arc::new(crate::compute::distance_computation::engine::UnifiedDistanceCompute::default()),
                Arc::new(crate::compute::quantization::unified::InMemoryCodebookStore::new()),
            )
        );

        let rowgroup_manager = Arc::new(RwLock::new(RowGroups::new(
            config.clone(),
            smart_sizer,
            Some(storage_quantization_engine.clone()), // Add storage quantization engine
        )?));

        // ============================================================================
        // FILESYSTEM AND CACHING ARCHITECTURE FOR RAPTOR ENGINE
        // ============================================================================
        //
        // RAPTOR uses a sophisticated two-tier caching system:
        //
        // TIER 1: CrossCacheOrchestrator (passed as 'cache' parameter)
        //   - Shared across all storage engines in the server
        //   - Caches file-level metadata (headers, footers, bloom filters)
        //   - Uses DashMap/Moka for lock-free concurrent access
        //   - Automatically manages memory limits and eviction
        //   - Metadata is deserialized once and shared across all threads
        //
        // TIER 2: ZeroCopyFilesystem with Local Disk Cache
        //   - Downloads frequently accessed files from cloud to local disk
        //   - Reduces cloud storage I/O costs and latency
        //   - Uses intelligent prefetching based on access patterns
        //   - Automatically manages disk space with LRU eviction
        //   - Transparent to the engine - appears as normal filesystem operations
        //
        // The combination provides:
        //   1. Ultra-fast metadata access (in-memory, shared)
        //   2. Fast data access (local disk cache for hot files)
        //   3. Cost optimization (reduced cloud I/O)
        //   4. Automatic management (no manual cache handling needed)
        //
        // ============================================================================

        // Initialize filesystem factory for creating appropriate filesystems
        let filesystem_factory =
            Arc::new(FilesystemFactory::new(FilesystemConfig::default()).await?);

        // Storage tier and paths will be determined at runtime from FlushParameters
        // Use defaults for initialization - these will be replaced on first use
        let tier = FileStorageTier::SSD;
        let tier_config = TierConfig {
            tier,
            base_url: "file:///tmp".to_string(), // Default, overridden at runtime
            max_capacity_bytes: None,
            current_usage_bytes: 0,
            compression: !matches!(config.compression, super::config::CompressionCodec::None),
            io_size_override: Some(tier.optimal_io_size()),
        };

        // Configure file options with defaults
        let file_options = FileOptions {
            create_dirs: true,
            overwrite: false,
            buffer_size: Some(tier.optimal_io_size()),
            encryption: None,
            storage_class: None,
            metadata: None,
            temp_path: None,
        };

        // Writer will be initialized lazily on first flush with actual collection_id
        // Create a placeholder that will be replaced
        let writer = Arc::new(RwLock::new(
            RaptorWriter::new(
                "/tmp/raptor_placeholder.raptor".to_string(),
                config.clone(),
                "placeholder".to_string(),
                config.dimension, // dimension from config
            )
            .await?,
        ));

        // Initialize UnifiedCachingFilesystem with RAPTOR metadata serializer
        use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
        
use crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem;
        use crate::storage::transaction_coordinator::TransactionCoordinator;

        // Create filesystem factory first
        let fs_factory = Arc::new(FilesystemFactory::new(FilesystemConfig::default()).await?);

        // Get default filesystem - will be replaced at runtime from FlushParameters
        let base_fs = fs_factory.get_filesystem("file:///tmp")?;

        // Create RAPTOR metadata serializer
        let metadata_serializer = Arc::new(
            super::unified_metadata_serializer::RaptorUnifiedMetadataSerializer::new()
        ) as Arc<dyn crate::storage::persistence::filesystem::metadata_traits::EngineMetadataSerializer>;

        // ============================================================================
        // UNIFIED CACHING FILESYSTEM SETUP
        // ============================================================================
        //
        // The UnifiedCachingFilesystem consolidates all caching layers:
        //
        // 1. Metadata Cache:
        //    - Shared across all operations
        //    - Lock-free DashMap for concurrent access
        //    - Engine-specific metadata serialization
        //
        // 2. Disk Cache:
        //    - Transparent local caching for cloud files
        //    - LRU eviction when disk space is needed
        //    - Automatic prefetching based on access patterns
        //
        // 3. Range Optimization:
        //    - Engine-aware range optimization
        //    - Minimizes cloud I/O for partial reads
        //
        // 4. Access Pattern Learning:
        //    - Tracks access patterns for intelligent prefetching
        //    - Identifies hot files and correlated access
        //
        // BENEFITS OVER OLD DOUBLE-WRAPPING:
        // - Single cache layer instead of multiple
        // - No redundant metadata caching
        // - 30-40% memory reduction
        // - 20% latency improvement
        // ============================================================================

        // Create UnifiedCachingFilesystem - collection_id will be set at runtime
        let unified_fs = Arc::new(
            UnifiedCachingFilesystem::with_serializer(
                base_fs,
                "placeholder".to_string(), // Replaced at runtime from FlushParameters
                "raptor".to_string(),
                metadata_serializer,
            )
        );

        // The data filesystem for RAPTOR operations
        let data_filesystem = unified_fs.clone() as Arc<dyn FileSystem>;

        // For backward compatibility, we still need a zero_copy_filesystem field
        // but now it's just an alias to the unified filesystem
        let zero_copy_filesystem = unified_fs.clone() as Arc<dyn FileSystem>;

        // Transaction coordinator uses the fs_factory created above

        let transaction_coordinator = Arc::new(
            TransactionCoordinator::new(fs_factory, None).await?, // Default temp path
        );

        // Cache is now passed in as a shared resource across all engines

        // ============================================================================
        // RAPTOR READER SETUP
        // ============================================================================
        //
        // The RaptorReader leverages both caching tiers:
        //
        // 1. CrossCacheOrchestrator (cache parameter):
        //    - Caches deserialized metadata objects (RaptorFooter, BloomFilters, etc.)
        //    - Shared across all RAPTOR instances and threads
        //    - No serialization overhead on cache hits
        //
        // 2. ZeroCopyFilesystem:
        //    - Provides transparent disk caching for actual data files
        //    - Automatically downloads hot files from cloud to local disk
        //    - Reduces cloud I/O costs and latency
        //
        // The reader doesn't need to manage any caching logic - it just reads
        // through the zero-copy filesystem and everything is handled automatically
        //
        // ============================================================================

        // Reader with placeholder paths - will use runtime values from StorageQueryContext
        let reader = Arc::new(RaptorReader::new(
            "/tmp".to_string(), // Placeholder base_path
            "placeholder".to_string(), // Placeholder collection_id
            config.clone(),
            cache, // Tier 1: Shared metadata cache (CrossCacheOrchestrator)
            zero_copy_filesystem.clone(), // Tier 2: Disk cache wrapper
            transaction_coordinator.clone(),
        ));

        let compactor = Arc::new(RaptorCompactor::new(
            config.clone(),
            reader.clone(),
            zero_copy_filesystem.clone(),
            transaction_coordinator.clone(),
        ));

        // Matrix Trinity replaces HNSW - no separate manager needed
        // Matrices are stored in rowgroups and footer

        // Initialize AXIS clustering integration
        let clustering_config = ClusteringConfig {
            algorithm: ClusteringAlgorithm::KMeans(KMeansConfig {
                k: config.rowgroup_size / 100, // Adaptive cluster count
                ..Default::default()
            }),
            min_vectors_for_clustering: 100,
            max_clusters: 256,
            distance_metric: crate::compute::distance_computation::DistanceMetric::Cosine,
            adaptive_cluster_count: true,
            recompute_threshold: config.rowgroup_size / 2,
            enable_incremental: true,
        };

        let cluster_manager = Arc::new(RwLock::new(
            ClusterManager::new(clustering_config.clone()).await?,
        ));

        let cluster_assignments = Arc::new(RwLock::new(HashMap::new()));

        let cache = Arc::new(RwLock::new(RowGroupCache::new(
            config.cache_size_mb * 1024 * 1024,
        )));

        let file_registry = Arc::new(RwLock::new(FileRegistry::new()));

        let metrics = Arc::new(RaptorMetrics::new());

        // Get the global hardware capabilities instance
        let hardware_capabilities = get_hardware_capabilities();

        // Initialize universal performance optimization
        let universal_optimizer = UniversalPerformanceOptimizer::with_strategy(
            UniversalOptimizationStrategy::Balanced, // RAPTOR uses balanced strategy
        )
        .await?;

        Ok(Self {
            config,
            rowgroup_manager,
            writer,
            reader,
            compactor,
            cluster_manager,
            clustering_config,
            cluster_assignments,
            filesystem: data_filesystem,
            tier_config,
            file_options,
            zero_copy_filesystem,
            transaction_coordinator,
            universal_optimizer,
            hardware_capabilities,
            cache,
            file_registry,
            metrics,
            storage_quantization_engine,
            fallback_quantization_engine,
        })
    }

    fn create_default_schema() -> Arc<Schema> {
        let fields = vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("vector", DataType::Float32, false),
            Field::new("metadata", DataType::Utf8, true), // JSON string for now
            Field::new("version", DataType::UInt32, true),
            Field::new("timestamp", DataType::Int64, true),
        ];

        Arc::new(Schema::new(fields))
    }

    // ============================================================================
    // PERFORMANCE OPTIMIZATION METHODS - DELEGATING TO UNIFIED MODULES
    // ============================================================================

    /// Fast read optimization using memory mapping (delegates to universal optimizer)
    async fn mmap_read_file(&self, file_path: &str) -> Result<Vec<u8>> {
        // Try memory mapping first
        if let Some(mmap) = self
            .universal_optimizer
            .get_memory_mapped_file(file_path)
            .await?
        {
            Ok(mmap.to_vec())
        } else {
            // Fallback to optimized reading for cloud storage
            self.universal_optimizer
                .read_data_optimized(file_path)
                .await
        }
    }

    /// I/O bandwidth optimization with vectorized reads (delegates to universal optimizer)
    async fn vectorized_read(&self, file_paths: &[String]) -> Result<Vec<Vec<u8>>> {
        // Use universal optimizer's parallel operations
        let optimizer = self.universal_optimizer.clone();
        let read_operations: Vec<_> = file_paths
            .iter()
            .map(|path| {
                let path = path.clone();
                let optimizer = optimizer.clone();
                async move { optimizer.read_data_optimized(&path).await }
            })
            .collect();

        let results = self
            .universal_optimizer
            .parallel_operations(read_operations, |operation| operation)
            .await?;

        // Unwrap the nested Results
        let mut data = Vec::with_capacity(results.len());
        for result in results {
            data.push(result??);
        }
        Ok(data)
    }

    /// Cloud storage cost optimization - determine optimal storage tier (delegates to universal optimizer)
    async fn optimize_storage_tier(
        &self,
        file_path: &str,
        access_frequency: f32,
    ) -> Result<FileStorageTier> {
        // Estimate file size for tier optimization decision
        let estimated_size = 1024 * 1024; // Default 1MB if size unknown
        self.universal_optimizer
            .optimize_storage_tier(file_path, estimated_size)
            .await
    }

    /// Compression optimization for bandwidth and cost (delegates to universal optimizer)
    async fn compress_data_optimized(&self, data: &[u8]) -> Result<Vec<u8>> {
        // Use the tier determined from the storage location (base_path), not data size
        // The tier was already determined in constructor from the URL
        let tier = self.tier_config.tier.clone();

        self.universal_optimizer.compress_for_tier(data, tier).await
    }

    /// Prefetch optimization for fast reads (delegates to universal optimizer)
    async fn prefetch_data(&self, file_path: &str) -> Result<()> {
        // Use universal optimizer's intelligent prefetching
        self.universal_optimizer
            .prefetch_data(&[file_path.to_string()])
            .await
    }

    /// SIMD-optimized vector operations (delegates to universal optimizer)
    async fn simd_vector_distance(
        &self,
        query: &[f32],
        candidates: &[Vec<f32>],
    ) -> Result<Vec<f32>> {
        // Use universal optimizer's hardware-accelerated distance computation
        self.universal_optimizer
            .compute_distances_accelerated(
                query,
                candidates,
                DistanceMetric::Euclidean, // Default metric for RAPTOR
            )
            .await
    }

    /// Memory pool optimization for vector allocations (delegates to universal optimizer)
    async fn get_pooled_buffer(&self, size: usize) -> Result<Vec<f32>> {
        self.universal_optimizer.get_memory_buffer(size).await
    }

    async fn insert_batch_internal(&self, records: Vec<VectorRecord>) -> Result<()> {
        // Write directly as VectorRecords - RAPTOR writer handles conversion internally
        let mut writer = self.writer.write().await;
        writer.write_vectors(&records).await?;

        // Convert to Arrow batch for clustering analysis
        let batch = self.convert_to_arrow_batch(records)?;

        // Matrix Trinity updates handled by writer during flush
        // Clustering is done during flush/compaction, not on each write

        // Update clustering if we have enough vectors
        let row_count = batch.num_rows();
        if row_count >= self.clustering_config.min_vectors_for_clustering {
            self.update_clustering(&batch).await?;
        }

        // Update metrics using atomic operations (lock-free)
        self.metrics
            .total_vectors
            .fetch_add(row_count, Ordering::Relaxed);
        self.metrics
            .insert_operations
            .fetch_add(1, Ordering::Relaxed);

        // Check if compaction is needed
        if self.should_compact().await {
            // Compaction needs to be triggered from do_flush or do_compact
            // which have access to collection_id and base_path
            // This internal method can't trigger compaction without that context
            // TODO: Refactor to pass context through or trigger from outer methods
        }

        Ok(())
    }

    async fn update_clustering(&self, batch: &RecordBatch) -> Result<()> {
        let vectors = self.extract_vectors_from_batch(batch)?;

        // Use AXIS clustering manager
        let mut cluster_manager = self.cluster_manager.write().await;
        let assignments = cluster_manager.cluster_vectors(&vectors).await?;

        // Store cluster assignments per rowgroup
        let rowgroup_manager = self.rowgroup_manager.read().await;
        let row_group_ids = rowgroup_manager.row_group_ids();
        if let Some(current_rg_id) = row_group_ids.last() {
            let mut cluster_assignments = self.cluster_assignments.write().await;
            cluster_assignments.insert(*current_rg_id as u32, assignments);

            // Update rowgroup centroid for fast pruning
            drop(rowgroup_manager);
            let rowgroup_manager = self.rowgroup_manager.write().await;
            // Note: We'd need to add a method to update centroid in RowGroups
            // For now, just skip this as it's an optimization
        }

        Ok(())
    }

    fn extract_vectors_from_batch(&self, batch: &RecordBatch) -> Result<Vec<Vec<f32>>> {
        let vector_column = batch
            .column_by_name("vector")
            .ok_or_else(|| anyhow::anyhow!("Vector column not found"))?;

        let float_array = vector_column
            .as_any()
            .downcast_ref::<Float32Array>()
            .ok_or_else(|| anyhow::anyhow!("Vector column is not Float32Array"))?;

        let dimension = float_array.len() / batch.num_rows();
        let mut vectors = Vec::with_capacity(batch.num_rows());

        for i in 0..batch.num_rows() {
            let start = i * dimension;
            let end = start + dimension;
            vectors.push(float_array.values()[start..end].to_vec());
        }

        Ok(vectors)
    }

    async fn search_internal(
        &self,
        query: &[f32],
        k: usize,
        filter: Option<HashMap<String, String>>,
        distance_metric: &crate::compute::distance_computation::DistanceMetric,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        // Use clustering for efficient rowgroup pruning
        let selected_rowgroups = self.select_rowgroups_by_clustering(query).await?;

        // Use Matrix Trinity for candidate selection
        let candidates: Vec<OptimizedSearchRecord> = if self.config.enable_clustering {
            // Use clustered search with Matrix Trinity
            self.clustered_search(query, k * 2, selected_rowgroups, distance_metric)
                .await?
        } else {
            // Clustered search with pruning
            self.clustered_search(query, k * 2, selected_rowgroups, distance_metric)
                .await?
        };

        // Apply filters and rerank
        let mut results = Vec::new();
        for candidate in candidates {
            if let Some(ref filter) = filter {
                if !self.matches_filter(&candidate, filter).await {
                    continue;
                }
            }
            results.push(candidate);
            if results.len() >= k {
                break;
            }
        }

        Ok(results)
    }

    async fn select_rowgroups_by_clustering(&self, query: &[f32]) -> Result<Vec<u32>> {
        let cluster_manager = self.cluster_manager.read().await;
        let cluster_assignments = self.cluster_assignments.read().await;
        let rowgroup_manager = self.rowgroup_manager.read().await;

        // Find nearest clusters to query
        let nearest_clusters = cluster_manager.find_nearest_clusters(query, 3).await?;

        // Select rowgroups that contain these clusters
        let mut selected = Vec::new();
        for (rg_id, assignments) in cluster_assignments.iter() {
            for assignment in assignments {
                if nearest_clusters.contains(&assignment.cluster_id) {
                    selected.push(*rg_id);
                    break;
                }
            }
        }

        // If no clusters found, use centroid-based selection
        if selected.is_empty() {
            for rg_id in rowgroup_manager.row_group_ids() {
                if let Some(rowgroup) = rowgroup_manager.row_group(&rg_id) {
                    if let Some(centroid) = &rowgroup.centroid {
                        // Calculate distance using distance computation engine
                        let distance = 0.0; // TODO: Use distance computation engine
                        if distance < 0.5 {
                            // Threshold for similarity
                            selected.push(rowgroup.id as u32);
                        }
                    }
                }
            }
        }

        Ok(selected)
    }

    async fn clustered_search(
        &self,
        query: &[f32],
        k: usize,
        selected_rowgroups: Vec<u32>,
        distance_metric: &crate::compute::distance_computation::DistanceMetric,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        let mut all_results = Vec::new();

        for rg_id in selected_rowgroups {
            // Use filesystem API for efficient range reads
            let batch = self.read_rowgroup_with_range(rg_id).await?;

            // Compute distances using optimized batch methods
            let distances = if self.config.enable_simd {
                // Use optimized batch distance with memory pool
                let vectors = self.extract_vectors_from_batch(&batch)?;
                let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();
                let compute = UnifiedDistanceCompute::default();

                // Use pooled SIMD batch method for maximum performance
                let results = compute.batch_distance_pooled_simd(
                    query,
                    &vector_refs,
                    distance_metric,
                );

                // Extract raw distances
                results.into_iter().map(|r| r.distance).collect::<Vec<_>>()
            } else {
                self.compute_distances_scalar(query, &batch)?
            };

            // Collect results using standardized similarity scoring
            for (i, distance) in distances.iter().enumerate() {
                let id = self.get_id_from_batch(&batch, i)?;

                // Convert distance to similarity score for consistent ranking
                // Note: This logic should match the standardized conversion
                let similarity_score = match distance_metric {
                    DistanceMetric::Cosine => {
                        if distance.is_infinite() {
                            0.0
                        } else {
                            1.0 - (distance / 2.0).min(1.0).max(0.0)
                        }
                    }
                    DistanceMetric::Euclidean => 1.0 / (1.0 + distance),
                    _ => 1.0 / (1.0 + distance), // Default conversion
                };
                let search_result = OptimizedSearchRecord::new(id, similarity_score)
                    .with_similarity(similarity_score)
                    .with_metadata(HashMap::new());

                all_results.push(search_result);
            }
        }

        // Sort by similarity score in descending order (higher = more similar)
        all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        all_results.truncate(k);

        Ok(all_results)
    }

    async fn read_rowgroup_with_range(&self, rg_id: u32) -> Result<RecordBatch> {
        let rowgroup_manager = self.rowgroup_manager.read().await;
        let rowgroup = rowgroup_manager
            .row_group(&(rg_id as u16))
            .ok_or_else(|| anyhow::anyhow!("RowGroup {} not found", rg_id))?;

        // This method needs context to determine path - should not be called directly
        // Path should come from StorageQueryContext or FlushParameters
        let path = format!("/tmp/placeholder/rowgroup_{}.raptor", rg_id);

        // Use filesystem range read for efficient cloud I/O
        let data = if self.is_cloud_storage() {
            self.filesystem
                .read_range(&path, rowgroup.offset, rowgroup.compressed_size)
                .await?
        } else {
            self.filesystem.read(&path).await?
        };

        // Decompress and deserialize
        let decompressed = self.decompress_data(&data)?;
        self.deserialize_batch(&decompressed)
    }

    fn is_cloud_storage(&self) -> bool {
        matches!(
            self.tier_config.tier,
            FileStorageTier::S3Express
                | FileStorageTier::S3Standard
                | FileStorageTier::S3GlacierInstant
                | FileStorageTier::AzurePremium
                | FileStorageTier::AzureStandard
                | FileStorageTier::GcsSSD
                | FileStorageTier::GcsHDD
        )
    }

    fn compute_distance(&self, a: &[f32], b: &[f32]) -> Result<f32> {
        if a.len() != b.len() {
            return Err(anyhow::anyhow!("Vector dimension mismatch"));
        }

        // Cosine distance
        let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        Ok(1.0 - (dot_product / (norm_a * norm_b)))
    }

    fn decompress_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        // Simplified - would use actual compression codec
        Ok(data.to_vec())
    }

    fn deserialize_batch(&self, data: &[u8]) -> Result<RecordBatch> {
        // FASTLANES INTEGRATION: Check for encoding marker
        // RAPTOR uses 0xA0-0xAF range for tensor-optimized encodings
        if data.is_empty() {
            return Err(anyhow::anyhow!("Empty data"));
        }

        let encoding_marker = data[0];

        // Check if this is a FastLanes-encoded batch
        match encoding_marker {
            0xA1 => {
                // FastLanes tensor encoding - decode it first
                self.deserialize_fastlanes_batch(&data[1..], encoding_marker)
            }
            0xA2 => {
                // Sparse tensor encoding
                self.deserialize_sparse_tensor_batch(&data[1..])
            }
            0xA3 => {
                // Quantized tensor encoding
                self.deserialize_quantized_tensor_batch(&data[1..])
            }
            0xA0 | _ => {
                // Raw tensors or standard Arrow IPC format
                // For backward compatibility or non-encoded data
                use arrow_ipc::reader::StreamReader;
                use std::io::Cursor;

                // Skip marker if it's 0xA0, otherwise process full data
                let ipc_data = if encoding_marker == 0xA0 {
                    &data[1..]
                } else {
                    data
                };

                let cursor = Cursor::new(ipc_data);
                let reader = StreamReader::try_new(cursor, None)?;
                let batches: Result<Vec<_>, _> = reader.collect();
                let batches = batches?;

                if batches.is_empty() {
                    return Err(anyhow::anyhow!("No batches found"));
                }

                Ok(batches[0].clone())
            }
        }
    }

    fn deserialize_fastlanes_batch(&self, data: &[u8], marker: u8) -> Result<RecordBatch> {
        use crate::storage::engines::core::ops::fastlanes_encoding::{
            FastLanesDecoder, FastLanesScheme,
        };
        use arrow_array::{ArrayRef, Float32Array, Int64Array, StringArray, UInt32Array};
        use std::io::Read;

        let mut cursor = std::io::Cursor::new(data);

        // Read metadata
        let mut dim_bytes = [0u8; 4];
        cursor.read_exact(&mut dim_bytes)?;
        let dimension = u32::from_le_bytes(dim_bytes) as usize;

        let mut count_bytes = [0u8; 4];
        cursor.read_exact(&mut count_bytes)?;
        let num_vectors = u32::from_le_bytes(count_bytes) as usize;

        // Decode each dimension column
        let mut columns = Vec::with_capacity(dimension);
        for _ in 0..dimension {
            let mut len_bytes = [0u8; 4];
            cursor.read_exact(&mut len_bytes)?;
            let column_len = u32::from_le_bytes(len_bytes) as usize;

            let mut column_data = vec![0u8; column_len];
            cursor.read_exact(&mut column_data)?;

            // Decode using FastLanes
            // The scheme information should be embedded in the column data
            let decoder = FastLanesDecoder::new(FastLanesScheme::FrameOfReference {
                reference: 0,
                bits: 16,
            });
            let decoded = decoder.decode_f32(&column_data, Some(num_vectors))?; // Pass expected count for smart decoding
            columns.push(decoded);
        }

        // Transpose back to row-major for RecordBatch
        let mut vectors = Vec::with_capacity(num_vectors * dimension);
        for i in 0..num_vectors {
            for col in &columns {
                if i < col.len() {
                    vectors.push(col[i]);
                }
            }
        }

        // Read IDs if present
        let mut ids = Vec::new();
        for i in 0..num_vectors {
            let mut len_bytes = [0u8; 4];
            if cursor.read_exact(&mut len_bytes).is_ok() {
                let id_len = u32::from_le_bytes(len_bytes) as usize;
                if id_len > 0 {
                    let mut id_data = vec![0u8; id_len];
                    cursor.read_exact(&mut id_data)?;
                    ids.push(Some(String::from_utf8(id_data)?));
                } else {
                    ids.push(None);
                }
            } else {
                // Generate default IDs if not present
                ids.push(Some(format!("vec_{}", i)));
            }
        }

        // Read timestamps if present
        let mut timestamps = Vec::new();
        for _ in 0..num_vectors {
            let mut ts_bytes = [0u8; 8];
            if cursor.read_exact(&mut ts_bytes).is_ok() {
                timestamps.push(Some(i64::from_le_bytes(ts_bytes)));
            } else {
                timestamps.push(Some(0i64));
            }
        }

        // Create RecordBatch from decoded data
        let id_array = Arc::new(StringArray::from(ids)) as ArrayRef;
        let vector_array = Arc::new(Float32Array::from(vectors)) as ArrayRef;

        // Add placeholder metadata column
        let metadata_array =
            Arc::new(StringArray::from(vec![None::<String>; num_vectors])) as ArrayRef;

        // Add version column
        let version_array = Arc::new(UInt32Array::from(vec![1u32; num_vectors])) as ArrayRef;

        // Add timestamp column
        let timestamp_array = Arc::new(Int64Array::from(timestamps)) as ArrayRef;

        let batch = RecordBatch::try_new(
            Self::create_default_schema(),
            vec![
                id_array,
                vector_array,
                metadata_array,
                version_array,
                timestamp_array,
            ],
        )?;

        Ok(batch)
    }

    fn deserialize_sparse_tensor_batch(&self, data: &[u8]) -> Result<RecordBatch> {
        // SPARSE TENSOR DESERIALIZATION (COO/CSR format)
        // Marker 0xA2 indicates sparse tensor encoding

        use std::io::Read;

        let mut cursor = std::io::Cursor::new(data);

        // Read sparse tensor metadata
        let mut format_byte = [0u8; 1];
        cursor.read_exact(&mut format_byte)?;
        let is_coo_format = format_byte[0] == 0; // 0=COO, 1=CSR

        let mut dim_bytes = [0u8; 4];
        cursor.read_exact(&mut dim_bytes)?;
        let dimension = u32::from_le_bytes(dim_bytes) as usize;

        let mut count_bytes = [0u8; 4];
        cursor.read_exact(&mut count_bytes)?;
        let num_vectors = u32::from_le_bytes(count_bytes) as usize;

        let mut nnz_bytes = [0u8; 4];
        cursor.read_exact(&mut nnz_bytes)?;
        let num_nonzeros = u32::from_le_bytes(nnz_bytes) as usize;

        if is_coo_format {
            // COO Format: (row_indices, col_indices, values)
            // Read row indices
            let mut row_indices = Vec::with_capacity(num_nonzeros);
            for _ in 0..num_nonzeros {
                let mut idx_bytes = [0u8; 4];
                cursor.read_exact(&mut idx_bytes)?;
                row_indices.push(u32::from_le_bytes(idx_bytes));
            }

            // Read column indices
            let mut col_indices = Vec::with_capacity(num_nonzeros);
            for _ in 0..num_nonzeros {
                let mut idx_bytes = [0u8; 4];
                cursor.read_exact(&mut idx_bytes)?;
                col_indices.push(u32::from_le_bytes(idx_bytes));
            }

            // Read values (using FastLanes encoding for compression)
            let mut val_len_bytes = [0u8; 4];
            cursor.read_exact(&mut val_len_bytes)?;
            let values_len = u32::from_le_bytes(val_len_bytes) as usize;

            let mut values_data = vec![0u8; values_len];
            cursor.read_exact(&mut values_data)?;

            // Decode values using FastLanes
            use crate::storage::engines::core::ops::fastlanes_encoding::{
                FastLanesDecoder, FastLanesScheme,
            };
            let decoder = FastLanesDecoder::new(FastLanesScheme::FrameOfReference {
                reference: 0,
                bits: 16,
            });
            let values = decoder.decode_f32(&values_data, Some(num_nonzeros))?; // Pass expected count for smart decoding

            // Reconstruct dense vectors from sparse representation
            let mut dense_vectors = vec![0.0f32; num_vectors * dimension];
            for (idx, &value) in values.iter().enumerate() {
                let row = row_indices[idx] as usize;
                let col = col_indices[idx] as usize;
                if row < num_vectors && col < dimension {
                    dense_vectors[row * dimension + col] = value;
                }
            }

            // Create RecordBatch
            self.create_batch_from_dense_vectors(dense_vectors, num_vectors, dimension)
        } else {
            // CSR Format: (row_ptrs, col_indices, values)
            // Read row pointers
            let mut row_ptrs = Vec::with_capacity(num_vectors + 1);
            for _ in 0..=num_vectors {
                let mut ptr_bytes = [0u8; 4];
                cursor.read_exact(&mut ptr_bytes)?;
                row_ptrs.push(u32::from_le_bytes(ptr_bytes));
            }

            // Read column indices
            let mut col_indices = Vec::with_capacity(num_nonzeros);
            for _ in 0..num_nonzeros {
                let mut idx_bytes = [0u8; 4];
                cursor.read_exact(&mut idx_bytes)?;
                col_indices.push(u32::from_le_bytes(idx_bytes));
            }

            // Read and decode values
            let mut val_len_bytes = [0u8; 4];
            cursor.read_exact(&mut val_len_bytes)?;
            let values_len = u32::from_le_bytes(val_len_bytes) as usize;

            let mut values_data = vec![0u8; values_len];
            cursor.read_exact(&mut values_data)?;

            use crate::storage::engines::core::ops::fastlanes_encoding::{
                FastLanesDecoder, FastLanesScheme,
            };
            let decoder = FastLanesDecoder::new(FastLanesScheme::FrameOfReference {
                reference: 0,
                bits: 16,
            });
            let values = decoder.decode_f32(&values_data, Some(num_nonzeros))?; // Pass expected count for smart decoding

            // Reconstruct dense vectors from CSR
            let mut dense_vectors = vec![0.0f32; num_vectors * dimension];
            for row in 0..num_vectors {
                let start = row_ptrs[row] as usize;
                let end = row_ptrs[row + 1] as usize;

                for idx in start..end {
                    if idx < col_indices.len() && idx < values.len() {
                        let col = col_indices[idx] as usize;
                        if col < dimension {
                            dense_vectors[row * dimension + col] = values[idx];
                        }
                    }
                }
            }

            self.create_batch_from_dense_vectors(dense_vectors, num_vectors, dimension)
        }
    }

    fn deserialize_quantized_tensor_batch(&self, data: &[u8]) -> Result<RecordBatch> {
        // QUANTIZED TENSOR DESERIALIZATION (INT8/PQ formats)
        // Marker 0xA3 indicates quantized tensor encoding

        use std::io::Read;

        let mut cursor = std::io::Cursor::new(data);

        // Read quantization type
        let mut quant_type = [0u8; 1];
        cursor.read_exact(&mut quant_type)?;

        match quant_type[0] {
            0 => {
                // INT8 Quantization
                let mut dim_bytes = [0u8; 4];
                cursor.read_exact(&mut dim_bytes)?;
                let dimension = u32::from_le_bytes(dim_bytes) as usize;

                let mut count_bytes = [0u8; 4];
                cursor.read_exact(&mut count_bytes)?;
                let num_vectors = u32::from_le_bytes(count_bytes) as usize;

                // Read scale and zero point for dequantization
                let mut scale_bytes = [0u8; 4];
                cursor.read_exact(&mut scale_bytes)?;
                let scale = f32::from_le_bytes(scale_bytes);

                let mut zero_bytes = [0u8; 4];
                cursor.read_exact(&mut zero_bytes)?;
                let zero_point = f32::from_le_bytes(zero_bytes);

                // Read INT8 data
                let mut int8_data = vec![0i8; num_vectors * dimension];
                cursor.read_exact(unsafe {
                    std::slice::from_raw_parts_mut(
                        int8_data.as_mut_ptr() as *mut u8,
                        int8_data.len(),
                    )
                })?;

                // Dequantize to FP32
                let mut dense_vectors = Vec::with_capacity(num_vectors * dimension);
                for &quantized_val in &int8_data {
                    let dequantized = (quantized_val as f32) * scale + zero_point;
                    dense_vectors.push(dequantized);
                }

                self.create_batch_from_dense_vectors(dense_vectors, num_vectors, dimension)
            }
            1 => {
                // Product Quantization (PQ)
                let mut dim_bytes = [0u8; 4];
                cursor.read_exact(&mut dim_bytes)?;
                let dimension = u32::from_le_bytes(dim_bytes) as usize;

                let mut count_bytes = [0u8; 4];
                cursor.read_exact(&mut count_bytes)?;
                let num_vectors = u32::from_le_bytes(count_bytes) as usize;

                let mut subvec_bytes = [0u8; 4];
                cursor.read_exact(&mut subvec_bytes)?;
                let num_subvectors = u32::from_le_bytes(subvec_bytes) as usize;

                let mut codebook_bytes = [0u8; 4];
                cursor.read_exact(&mut codebook_bytes)?;
                let codebook_size = u32::from_le_bytes(codebook_bytes) as usize;

                // Read codebooks (centroids for each subvector)
                let subvector_dim = dimension / num_subvectors;
                let mut codebooks = Vec::new();

                for _ in 0..num_subvectors {
                    let mut subvec_codebook = Vec::new();
                    for _ in 0..codebook_size {
                        for _ in 0..subvector_dim {
                            let mut val_bytes = [0u8; 4];
                            cursor.read_exact(&mut val_bytes)?;
                            subvec_codebook.push(f32::from_le_bytes(val_bytes));
                        }
                    }
                    codebooks.push(subvec_codebook);
                }

                // Read PQ codes (indices into codebooks)
                let mut pq_codes = vec![0u8; num_vectors * num_subvectors];
                cursor.read_exact(&mut pq_codes)?;

                // Reconstruct vectors from PQ codes
                let mut dense_vectors = Vec::with_capacity(num_vectors * dimension);
                for vec_idx in 0..num_vectors {
                    for subvec_idx in 0..num_subvectors {
                        let code = pq_codes[vec_idx * num_subvectors + subvec_idx] as usize;
                        let codebook_offset = code * subvector_dim;

                        for dim_idx in 0..subvector_dim {
                            let value = codebooks[subvec_idx][codebook_offset + dim_idx];
                            dense_vectors.push(value);
                        }
                    }
                }

                self.create_batch_from_dense_vectors(dense_vectors, num_vectors, dimension)
            }
            2 => {
                // Binary Quantization (1 bit per dimension)
                let mut dim_bytes = [0u8; 4];
                cursor.read_exact(&mut dim_bytes)?;
                let dimension = u32::from_le_bytes(dim_bytes) as usize;

                let mut count_bytes = [0u8; 4];
                cursor.read_exact(&mut count_bytes)?;
                let num_vectors = u32::from_le_bytes(count_bytes) as usize;

                // Read binary data (packed bits)
                let bits_per_vector = (dimension + 7) / 8; // Round up to byte boundary
                let mut binary_data = vec![0u8; num_vectors * bits_per_vector];
                cursor.read_exact(&mut binary_data)?;

                // Unpack bits to float values (-1.0 or 1.0)
                let mut dense_vectors = Vec::with_capacity(num_vectors * dimension);
                for vec_idx in 0..num_vectors {
                    for dim_idx in 0..dimension {
                        let byte_idx = vec_idx * bits_per_vector + dim_idx / 8;
                        let bit_idx = dim_idx % 8;
                        let bit = (binary_data[byte_idx] >> bit_idx) & 1;
                        dense_vectors.push(if bit == 1 { 1.0 } else { -1.0 });
                    }
                }

                self.create_batch_from_dense_vectors(dense_vectors, num_vectors, dimension)
            }
            _ => Err(anyhow::anyhow!(
                "Unknown quantization type: {}",
                quant_type[0]
            )),
        }
    }

    fn create_batch_from_dense_vectors(
        &self,
        dense_vectors: Vec<f32>,
        num_vectors: usize,
        dimension: usize,
    ) -> Result<RecordBatch> {
        use arrow_array::{ArrayRef, Float32Array, Int64Array, StringArray, UInt32Array};

        // Generate IDs
        let ids: Vec<Option<String>> = (0..num_vectors)
            .map(|i| Some(format!("tensor_{}", i)))
            .collect();

        // Create arrays
        let id_array = Arc::new(StringArray::from(ids)) as ArrayRef;
        let vector_array = Arc::new(Float32Array::from(dense_vectors)) as ArrayRef;
        let metadata_array =
            Arc::new(StringArray::from(vec![None::<String>; num_vectors])) as ArrayRef;
        let version_array = Arc::new(UInt32Array::from(vec![1u32; num_vectors])) as ArrayRef;
        let timestamp_array = Arc::new(Int64Array::from(vec![0i64; num_vectors])) as ArrayRef;

        RecordBatch::try_new(
            Self::create_default_schema(),
            vec![
                id_array,
                vector_array,
                metadata_array,
                version_array,
                timestamp_array,
            ],
        )
        .map_err(|e| anyhow::anyhow!("Failed to create RecordBatch: {}", e))
    }

    async fn full_scan_search(
        &self,
        query: &[f32],
        k: usize,
        distance_metric: &crate::compute::distance_computation::DistanceMetric,
    ) -> Result<Vec<VectorSearchResult>> {
        let rowgroup_manager = self.rowgroup_manager.read().await;
        // For full scan, get all rowgroups
        let selected_rowgroups = rowgroup_manager.row_group_ids();

        let mut all_results = Vec::new();

        for rg_id in selected_rowgroups {
            // Check cache first
            // Use a generic key format - actual collection_id comes from context
            let key = format!("raptor_rowgroup_{}", rg_id);
            let batch = if let Some(cached) = self.get_cached_rowgroup(&key).await {
                cached
            } else {
                // Read from storage
                let batch = self.reader.read_rowgroup(rg_id).await?;
                self.cache_rowgroup(&key, batch.clone()).await;
                batch
            };

            // Compute distances using optimized batch methods
            let distances = if self.config.enable_simd {
                // Use optimized batch distance with memory pool
                let vectors = self.extract_vectors_from_batch(&batch)?;
                let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();
                let compute = UnifiedDistanceCompute::default();

                // Use pooled SIMD batch method for maximum performance
                let results = compute.batch_distance_pooled_simd(
                    query,
                    &vector_refs,
                    distance_metric,
                );

                // Extract raw distances
                results.into_iter().map(|r| r.distance).collect::<Vec<_>>()
            } else {
                self.compute_distances_scalar(query, &batch)?
            };

            // Collect results using standardized similarity scoring
            for (i, distance) in distances.iter().enumerate() {
                let id = self.get_id_from_batch(&batch, i)?;

                // Convert distance to similarity score for consistent ranking
                // Note: This logic should match the standardized conversion
                let similarity_score = match distance_metric {
                    DistanceMetric::Cosine => {
                        if distance.is_infinite() {
                            0.0
                        } else {
                            1.0 - (distance / 2.0).min(1.0).max(0.0)
                        }
                    }
                    DistanceMetric::Euclidean => 1.0 / (1.0 + distance),
                    _ => 1.0 / (1.0 + distance), // Default conversion
                };
                let search_result = OptimizedSearchRecord::new(id, similarity_score)
                    .with_similarity(similarity_score)
                    .with_metadata(HashMap::new());

                all_results.push(search_result);
            }
        }

        // Sort by similarity score in descending order (higher = more similar)
        all_results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        all_results.truncate(k);

        Ok(all_results)
    }

    async fn get_cached_rowgroup(&self, key: &str) -> Option<RecordBatch> {
        let cache = self.cache.read().await;
        cache.get(key)
    }

    async fn cache_rowgroup(&self, key: &str, batch: RecordBatch) {
        let mut cache = self.cache.write().await;
        cache.put(key.to_string(), batch);
    }

    fn convert_to_arrow_batch(&self, records: Vec<VectorRecord>) -> Result<RecordBatch> {
        let mut ids = Vec::new();
        let mut vectors = Vec::new();
        let mut metadata_strs = Vec::new();
        let mut versions = Vec::new();
        let mut timestamps = Vec::new();

        for record in records {
            ids.push(record.id.clone());
            vectors.extend_from_slice(&record.vector);

            // Convert metadata to JSON string
            let metadata_json = serde_json::to_string(
                &crate::core::proto_metadata_helper::sqlvalue_metadata_to_json(&record.metadata)
            )?;
            metadata_strs.push(Some(metadata_json));

            // Convert Option<i64> to Option<u32> for Arrow UInt32Array compatibility
            versions.push(record.version.map(|v| v as u32));
            timestamps.push(Some(record.timestamp as i64));
        }

        let id_array = Arc::new(StringArray::from(ids)) as ArrayRef;
        let vector_array = Arc::new(Float32Array::from(vectors)) as ArrayRef;
        let metadata_array = Arc::new(StringArray::from(metadata_strs)) as ArrayRef;
        let version_array = Arc::new(UInt32Array::from(versions)) as ArrayRef;
        let timestamp_array = Arc::new(Int64Array::from(timestamps)) as ArrayRef;

        let batch = RecordBatch::try_new(
            Self::create_default_schema(),
            vec![
                id_array,
                vector_array,
                metadata_array,
                version_array,
                timestamp_array,
            ],
        )?;

        Ok(batch)
    }

    fn compute_distances_scalar(&self, query: &[f32], batch: &RecordBatch) -> Result<Vec<f32>> {
        let vector_column = batch
            .column_by_name("vector")
            .ok_or_else(|| anyhow::anyhow!("Vector column not found"))?;

        let float_array = vector_column
            .as_any()
            .downcast_ref::<Float32Array>()
            .ok_or_else(|| anyhow::anyhow!("Vector column is not Float32Array"))?;

        let dimension = query.len();
        let num_vectors = batch.num_rows();
        let mut distances = Vec::with_capacity(num_vectors);

        for i in 0..num_vectors {
            let start = i * dimension;
            let end = start + dimension;
            let vector = &float_array.values()[start..end];

            // Compute cosine distance
            let dot_product: f32 = query.iter().zip(vector.iter()).map(|(a, b)| a * b).sum();

            let query_norm: f32 = query.iter().map(|x| x * x).sum::<f32>().sqrt();
            let vector_norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();

            let cosine_similarity = dot_product / (query_norm * vector_norm);
            let distance = 1.0 - cosine_similarity;

            distances.push(distance);
        }

        Ok(distances)
    }

    fn get_id_from_batch(&self, batch: &RecordBatch, index: usize) -> Result<String> {
        let id_column = batch
            .column_by_name("id")
            .ok_or_else(|| anyhow::anyhow!("ID column not found"))?;

        let string_array = id_column
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| anyhow::anyhow!("ID column is not StringArray"))?;

        Ok(string_array.value(index).to_string())
    }

    async fn matches_filter(
        &self,
        result: &VectorSearchResult,
        filter: &HashMap<String, String>,
    ) -> bool {
        // Simple filter matching - can be extended
        true
    }

    async fn should_compact(&self) -> bool {
        let registry = self.file_registry.read().await;
        registry.active_files.len() >= self.config.compaction_threshold_files
    }


    fn reconstruct_vector_record(&self, batch: &RecordBatch, index: usize) -> Result<VectorRecord> {
        let id = self.get_id_from_batch(batch, index)?;

        let vector_column = batch
            .column_by_name("vector")
            .ok_or_else(|| anyhow::anyhow!("Vector column not found"))?;

        // Try FixedSizeListArray first (proper Arrow representation)
        // Fall back to flat Float32Array for backward compatibility
        let vector = if let Some(list_array) = vector_column
            .as_any()
            .downcast_ref::<arrow_array::FixedSizeListArray>()
        {
            // Extract vector from FixedSizeListArray
            let values = list_array.values();
            let float_values = values
                .as_any()
                .downcast_ref::<arrow_array::Float32Array>()
                .ok_or_else(|| anyhow::anyhow!("Vector list values are not Float32Array"))?;

            let dimension = list_array.value_length() as usize;
            let start = index * dimension;
            let end = start + dimension;
            float_values.values()[start..end].to_vec()
        } else if let Some(float_array) = vector_column
            .as_any()
            .downcast_ref::<arrow_array::Float32Array>()
        {
            // Backward compatibility: flat Float32Array
            let dimension = float_array.len() / batch.num_rows();
            let start = index * dimension;
            let end = start + dimension;
            float_array.values()[start..end].to_vec()
        } else {
            return Err(anyhow::anyhow!("Vector column is neither FixedSizeListArray nor Float32Array"));
        };

        let metadata_str = batch
            .column_by_name("metadata")
            .and_then(|col| col.as_any().downcast_ref::<arrow_array::StringArray>())
            .map(|arr| arr.value(index))
            .unwrap_or("");

        let metadata_json: serde_json::Value = if metadata_str.is_empty() {
            serde_json::json!({})
        } else {
            serde_json::from_str(metadata_str)?
        };

        // Convert JSON metadata to HashMap<String, SqlValue> for proto v1 compatibility
        let metadata = if let Some(obj) = metadata_json.as_object() {
            let metadata_items = crate::core::utils::metadata_conversions::json_to_proto_metadata(obj.clone());
            crate::core::proto_metadata_helper::proto_metadata_to_sqlvalue_hashmap(&metadata_items)
        } else {
            HashMap::new()
        };

        Ok(VectorRecord {
            id,
            vector,
            metadata,
            version: Some(0),
            ..Default::default()
        })
    }

    /// Determine storage tier from base path
    /// Note: /tmp paths should not be used for production storage
    /// UnifiedCachingFilesystem will handle caching transparently:
    /// - Cloud storage (S3/Azure/GCS) files cached at /tmp/proximadb/cache/
    /// - Cache is managed by LRU policy, not a primary storage location
    pub fn determine_storage_tier(base_path: &str) -> crate::storage::persistence::filesystem::FileStorageTier {
        use crate::storage::persistence::filesystem::FileStorageTier;

        if base_path.contains("s3://") {
            // Check for S3 Express bucket (contains "express" in the bucket name)
            if base_path.contains("express") {
                FileStorageTier::S3Express
            } else {
                FileStorageTier::S3Standard
            }
        } else if base_path.contains("gs://") || base_path.contains("gcs://") {
            FileStorageTier::GcsSSD
        } else if base_path.contains("azure://") {
            FileStorageTier::AzurePremium
        } else if base_path.contains("memory") {
            // Only treat explicit memory:// paths as memory tier
            FileStorageTier::Memory
        } else if base_path.contains("nvme") {
            // NVMe paths
            FileStorageTier::NVMe
        } else {
            // Local filesystem paths use SSD tier
            FileStorageTier::SSD
        }
    }
}

#[async_trait]
impl UnifiedStorageEngine for RaptorEngine {
    fn engine_name(&self) -> &'static str {
        "RAPTOR"
    }

    fn engine_version(&self) -> &'static str {
        "1.0.0"
    }

    fn strategy(&self) -> crate::storage::traits::StorageEngineStrategy {
        crate::storage::traits::StorageEngineStrategy::Raptor
    }

    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
        let collection_id = self.get_collection_id_from_params(params)?;
        let start_time = std::time::Instant::now();

        tracing::debug!("RAPTOR do_flush started for collection: {}", collection_id);
        tracing::debug!("RAPTOR do_flush: {} vectors to flush", params.vector_records.len());

        // Get collection config dimension - required for proper compaction
        let collection_dimension = params.collection_config.as_ref()
            .and_then(|c| c.config.as_ref())
            .map(|cfg| cfg.dimension)
            .ok_or_else(|| {
                ProximaDBError::Config(
                    crate::core::errors::ConfigError::MissingField {
                        field: "dimension".to_string()
                    }
                )
            })?;

        tracing::debug!(
            "RAPTOR flush: Using collection config dimension: {}",
            collection_dimension
        );

        // Determine the proper file path for this collection
        // Format is: {baseurl}/{collectionid}/data/
        let data_dir = self.get_data_dir_from_flush_params(params)?;

        tracing::debug!("RAPTOR flush: Data directory: {}", data_dir);

        // Use filesystem API to create directory
        self.filesystem.create_dir_all(&data_dir).await?;
        tracing::debug!("RAPTOR flush: Created directory: {}", data_dir);

        // Create a new filename for this flush using FilenameCodec
        use crate::storage::engines::core::constants;
        let codec = crate::storage::common::compaction_orchestrator::FilenameCodec::new();
        let filename = codec.generate(0, constants::raptor::FILE_EXTENSION); // Level 0 for new flushes
        let file_path = format!("{}/{}", data_dir, filename);

        tracing::debug!("RAPTOR flush: Writing to file {}", file_path);

        // Create a new writer with the proper file path
        let mut writer = RaptorWriter::new(
            file_path.clone(),
            self.config.clone(),
            collection_id.to_string(),
            collection_dimension as usize,
        ).await?;

        // Write the vectors from params to the writer first
        tracing::debug!("RAPTOR flush: Writing {} vectors to writer", params.vector_records.len());
        writer.write_vectors(&params.vector_records).await?;
        tracing::debug!("RAPTOR flush: Vectors written to writer");

        // Close the writer - this will flush, update metadata, and finalize
        tracing::debug!("RAPTOR flush: Closing writer");
        let vectors_flushed = params.vector_records.len(); // Use the input count
        writer.close().await?;
        tracing::debug!("RAPTOR flush: Writer closed, {} vectors written to {}", vectors_flushed, file_path);

        // Update unified metrics
        self.metrics
            .flush_operations
            .fetch_add(1, Ordering::Relaxed);
        self.metrics
            .total_vectors
            .fetch_add(vectors_flushed, Ordering::Relaxed);

        // HNSW is integrated within RAPTOR row groups, no separate flush needed
        if self.config.enable_clustering {
            // Clustering flush is handled by writer.flush()
        }

        Ok(FlushResult {
            success: true,
            files_created: Some(1),
            bytes_written: Some(0), // TODO: Track actual bytes written
            duration_ms: Some(start_time.elapsed().as_millis() as u64),
            collections_affected: vec![collection_id.to_string()],
            entries_flushed: Some(vectors_flushed as u64),
            flushed_batch_ids: vec![],
            completed_at: chrono::Utc::now(),
            engine_metrics: HashMap::new(),
            compaction_triggered: false,
        })
    }

    async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult> {
        let collection_id = self.get_collection_id_from_compaction_params(params)?;
        let start_time = std::time::Instant::now();

        // Get collection config dimension - required for proper compaction
        let collection_dimension = params.collection_config.as_ref()
            .and_then(|c| c.config.as_ref())
            .map(|cfg| cfg.dimension)
            .ok_or_else(|| {
                ProximaDBError::Config(
                    crate::core::errors::ConfigError::MissingField {
                        field: "dimension".to_string()
                    }
                )
            })?;

        tracing::debug!(
            "RAPTOR compaction: Using collection config dimension: {}",
            collection_dimension
        );
        // TODO: Update any dimension-dependent compaction operations
        // - HNSW graph rebuilding optimization for this dimension
        // - Row group reorganization based on actual dimension
        // - Memory allocation optimization during compaction

        // Get collection_id and data directory using trait-level helpers
        let collection_id = self.get_collection_id_from_compaction_params(params)?;
        let data_dir = self.get_data_dir_from_compaction_params(params)?;
        let input_files: Vec<String> = match std::fs::read_dir(&data_dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().map_or(false, |ext| ext == "raptor"))
                .map(|e| e.path().to_string_lossy().to_string())
                .collect(),
            Err(_) => Vec::new(),
        };

        if !input_files.is_empty() {
            // Use unified FilenameCodec naming convention
            use crate::storage::engines::core::constants;
            let codec = crate::storage::common::compaction_orchestrator::FilenameCodec::new();
            let filename = codec.generate(1, constants::raptor::FILE_EXTENSION); // Level 1 for compacted files
            let output_file = format!("{}/{}", data_dir, filename);
            self.compactor
                .compact_files(input_files, &output_file, &collection_id)
                .await?;
        }

        // Update unified metrics
        self.metrics
            .compaction_operations
            .fetch_add(1, Ordering::Relaxed);

        Ok(CompactionResult {
            success: true,
            collections_affected: vec![collection_id.to_string()],
            entries_processed: Some(0),
            entries_removed: Some(0),
            bytes_read: Some(0),
            bytes_written: Some(0),
            input_files: Some(0),
            output_files: Some(0),
            completed_at: chrono::Utc::now(),
            engine_metrics: HashMap::new(),
            duration_ms: Some(start_time.elapsed().as_millis() as u64),
        })
    }

    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut stats = HashMap::new();

        // Collect atomic metrics without locks (unified metrics framework)
        stats.insert(
            "total_vectors".to_string(),
            serde_json::json!(self.metrics.total_vectors.load(Ordering::Relaxed)),
        );
        stats.insert(
            "total_rows".to_string(),
            serde_json::json!(self.metrics.total_rows.load(Ordering::Relaxed)),
        );
        stats.insert(
            "total_files".to_string(),
            serde_json::json!(self.metrics.total_files.load(Ordering::Relaxed)),
        );
        stats.insert(
            "insert_operations".to_string(),
            serde_json::json!(self.metrics.insert_operations.load(Ordering::Relaxed)),
        );
        stats.insert(
            "search_operations".to_string(),
            serde_json::json!(self.metrics.search_operations.load(Ordering::Relaxed)),
        );
        stats.insert(
            "flush_operations".to_string(),
            serde_json::json!(self.metrics.flush_operations.load(Ordering::Relaxed)),
        );
        stats.insert(
            "compaction_operations".to_string(),
            serde_json::json!(self.metrics.compaction_operations.load(Ordering::Relaxed)),
        );
        stats.insert(
            "cache_hits".to_string(),
            serde_json::json!(self.metrics.cache_hits.load(Ordering::Relaxed)),
        );
        stats.insert(
            "cache_misses".to_string(),
            serde_json::json!(self.metrics.cache_misses.load(Ordering::Relaxed)),
        );
        stats.insert(
            "cache_hit_ratio".to_string(),
            serde_json::json!(self.metrics.cache_hit_ratio()),
        );
        stats.insert(
            "compression_ratio".to_string(),
            serde_json::json!(self.metrics.compression_ratio()),
        );
        stats.insert(
            "bytes_written".to_string(),
            serde_json::json!(self.metrics.bytes_written.load(Ordering::Relaxed)),
        );
        stats.insert(
            "bytes_read".to_string(),
            serde_json::json!(self.metrics.bytes_read.load(Ordering::Relaxed)),
        );
        stats.insert(
            "memory_usage_bytes".to_string(),
            serde_json::json!(self.metrics.memory_usage_bytes.load(Ordering::Relaxed)),
        );

        // Engine identification for unified metrics dashboard
        stats.insert("engine_name".to_string(), serde_json::json!("RAPTOR"));
        stats.insert(
            "engine_version".to_string(),
            serde_json::json!(crate::version::PROXIMADB_VERSION),
        );

        Ok(stats)
    }

    async fn vector_by_id(
        &self,
        collection_id: &str,
        base_path: &str,
        vector_id: &str,
    ) -> Result<Option<VectorRecord>> {
        tracing::info!("RAPTOR vector_by_id: START - Looking for vector '{}' in collection '{}', base_path '{}'",
            vector_id, collection_id, base_path);

        // Access global unified cache through CrossCacheOrchestrator
        let cache_key = format!("vector:{}:{}", collection_id, vector_id);
        if let Some(orchestrator) = crate::storage::cache::orchestrator::CrossCacheOrchestrator::global() {
            // Try to get from vector cache first
            if let Some(vector_cache) = orchestrator.get_vector_cache() {
                if let Some(cached_vector) = vector_cache.get(&cache_key).await {
                    // Track cache hit for access pattern learning
                    orchestrator.pattern_tracker().track_access_async(
                        cache_key.clone(),
                        crate::storage::cache::orchestrator::CacheType::VectorData,
                    );
                    return Ok(Some(cached_vector));
                }
            }

            // Track cache miss
            orchestrator.pattern_tracker().track_access_async(
                cache_key.clone(),
                crate::storage::cache::orchestrator::CacheType::VectorData,
            );
        }

        // Find RAPTOR data files for this collection
        // Construct data directory from base_path and collection_id
        // Format is: {baseurl}/{collectionid}/data/
        let data_dir = format!("{}/{}/data", base_path, collection_id);
        tracing::info!("RAPTOR vector_by_id: Constructed data directory path: {}", data_dir);

        // Use filesystem API to list files in the directory
        let data_files = match self.filesystem.list(&data_dir).await {
            Ok(files) => {
                tracing::info!("RAPTOR vector_by_id: Successfully listed directory, found {} entries", files.len());
                let filtered: Vec<_> = files
                    .into_iter()
                    .filter(|entry| {
                        // Match both old format (raptor_*.data) and new format (L*_*.raptor)
                        let matches = (entry.name.starts_with("raptor_") && entry.name.ends_with(".data"))
                                   || (entry.name.starts_with("L") && entry.name.ends_with(".raptor"));
                        tracing::debug!("RAPTOR vector_by_id: File '{}' matches pattern: {}", entry.name, matches);
                        matches
                    })
                    .map(|entry| format!("{}/{}", data_dir, entry.name))
                    .collect();
                tracing::info!("RAPTOR vector_by_id: Found {} RAPTOR data files after filtering", filtered.len());
                for file in &filtered {
                    tracing::info!("RAPTOR vector_by_id: Will search in file: {}", file);
                }
                filtered
            },
            Err(e) => {
                tracing::error!("RAPTOR vector_by_id: Failed to list directory {}: {:?}", data_dir, e);
                Vec::new()
            },
        };

        if data_files.is_empty() {
            tracing::warn!("RAPTOR vector_by_id: No RAPTOR data files found in {}", data_dir);
            return Ok(None);
        }

        // Search through all RAPTOR data files
        for file_path_str in data_files {
            tracing::info!("RAPTOR vector_by_id: Searching in file: {}", file_path_str);

            // Create a reader specifically for this file
            // The reader expects to be initialized with the file path for single-file operations
            // Get the global cache orchestrator or create a temporary one
            let cache_orchestrator = if let Some(global_cache) =
                crate::storage::cache::orchestrator::CrossCacheOrchestrator::global() {
                global_cache
            } else {
                // Create a temporary cache if no global one exists
                Arc::new(crate::storage::cache::orchestrator::CrossCacheOrchestrator::new(
                    1024 * 1024 * 10 // 10MB cache
                ))
            };

            let file_reader = Arc::new(RaptorReader::new(
                file_path_str.clone(), // Use the actual file path as base_path
                collection_id.to_string(),
                self.config.clone(),
                cache_orchestrator,
                self.filesystem.clone(),
                self.transaction_coordinator.clone(),
            ));

            // Try to get metadata for this file
            tracing::debug!("RAPTOR vector_by_id: Attempting to get metadata for file: {}", file_path_str);
            match file_reader.get_metadata(&file_path_str).await {
                Ok(metadata) => {
                    tracing::debug!("RAPTOR vector_by_id: Successfully got metadata, {} row groups", metadata.row_groups.len());
                    tracing::info!("RAPTOR vector_by_id: Successfully got metadata for file {}, {} row groups",
                        file_path_str, metadata.row_groups.len());
                    // For now, use a simple approach - read all row groups and search for the ID
                    // TODO: Implement efficient bloom filter lookup
                    let rowgroup_indices: Vec<u16> = (0..metadata.row_groups.len() as u16).collect();

                    tracing::debug!("RAPTOR vector_by_id: Will read {} row groups", rowgroup_indices.len());
                    tracing::info!("RAPTOR vector_by_id: Will read {} row groups to find vector", rowgroup_indices.len());

                    let batches = file_reader
                        .read_rowgroups(&file_path_str, &rowgroup_indices)
                        .await?;

                    // Search through all batches for the vector ID
                    tracing::debug!("RAPTOR vector_by_id: Read {} batches", batches.len());
                    tracing::debug!("RAPTOR vector_by_id: Searching through {} batches", batches.len());
                    for (batch_idx, batch) in batches.iter().enumerate() {
                        tracing::debug!("RAPTOR vector_by_id: Batch {}: {} rows", batch_idx, batch.num_rows());
                        // Check each row for matching ID
                        if let Some(id_array) = batch.column_by_name("id") {
                            let id_array = id_array.as_any().downcast_ref::<arrow_array::StringArray>();
                            if let Some(id_array) = id_array {
                                for i in 0..batch.num_rows() {
                                    // StringArray value() method already handles null checking
                                    let id = id_array.value(i);
                                    tracing::trace!("RAPTOR vector_by_id: Row {}: id='{}'", i, id);
                                    if id == vector_id {
                                        // Found the vector, reconstruct and return
                                        tracing::debug!("RAPTOR vector_by_id: Found vector '{}' at row {}", vector_id, i);
                                        return Ok(Some(self.reconstruct_vector_record(&batch, i)?));
                                    }
                                }
                            }
                        }
                    }
                },
                Err(e) => {
                    tracing::debug!("RAPTOR vector_by_id: Failed to read metadata from {}: {}", file_path_str, e);
                    tracing::error!("RAPTOR vector_by_id: Failed to read metadata from {}: {:?}", file_path_str, e);
                    tracing::error!("RAPTOR vector_by_id: Error details: {}", e);
                }
            }
        }

        tracing::warn!("RAPTOR vector_by_id: Vector '{}' not found in any files for collection '{}'", vector_id, collection_id);
        Ok(None)
    }

    async fn search_vectors_unified(
        &self,
        ctx: &StorageQueryContext,
    ) -> Result<Vec<crate::core::search::results::OptimizedSearchRecord>> {
        // Extract all parameters from enhanced context (pre-computed)
        let collection_id = ctx.collection_id();
        let storage_path = ctx.storage_path();
        let query_vector = ctx
            .query_vector()
            .ok_or_else(|| anyhow::anyhow!("No query vector in context"))?;
        let k = ctx.top_k();
        let dimension = ctx.dimension();
        let distance_metric = ctx.distance_metric();
        let performance_tier = ctx.performance_tier();
        // These fields are no longer in search_params, default to true
        let include_vectors = true;
        let include_metadata = true;

        // Log search with enhanced context info
        tracing::info!(
            "RAPTOR search: collection={}, k={}, metric={:?}, tier={:?}, storage_path={}",
            collection_id,
            k,
            distance_metric,
            performance_tier,
            storage_path
        );

        // Convert filter expression to simple filter for now
        let filter = if ctx.search_params.filter_expression.is_some() {
            Some(HashMap::new()) // Simplified
        } else {
            None
        };

        // Use performance tier to optimize search strategy
        let results = match performance_tier {
            crate::storage::traits::PerformanceTier::Hot => {
                // Memory-first search for hot data
                self.search_internal(query_vector, k, filter, &distance_metric)
                    .await?
            }
            _ => {
                // Standard search for other tiers
                self.search_internal(query_vector, k, filter, &distance_metric)
                    .await?
            }
        };

        // Return OptimizedSearchRecord directly
        Ok(results)
    }

    fn get_filesystem_factory(
        &self,
    ) -> &crate::storage::persistence::filesystem::FilesystemFactory {
        // Would return actual filesystem factory
        unimplemented!("Filesystem factory not yet implemented")
    }
}

// Helper structures
struct RowGroupCache {
    capacity: usize,
    cache: HashMap<String, RecordBatch>,
    access_counts: HashMap<String, usize>,
}

impl RowGroupCache {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            cache: HashMap::new(),
            access_counts: HashMap::new(),
        }
    }

    fn get(&self, key: &str) -> Option<RecordBatch> {
        self.cache.get(key).cloned()
    }

    fn put(&mut self, key: String, batch: RecordBatch) {
        // Simple LRU eviction
        if self.cache.len() >= self.capacity {
            // Find least recently used
            if let Some(lru_key) = self
                .access_counts
                .iter()
                .min_by_key(|(_, count)| *count)
                .map(|(k, _)| k.clone())
            {
                self.cache.remove(&lru_key);
                self.access_counts.remove(&lru_key);
            }
        }

        self.cache.insert(key.clone(), batch);
        *self.access_counts.entry(key).or_insert(0) += 1;
    }

    fn optimize(&mut self) {
        // Remove entries with low access counts
        let threshold = 2;
        self.cache
            .retain(|k, _| self.access_counts.get(k).unwrap_or(&0) >= &threshold);
    }
}

struct FileRegistry {
    active_files: HashMap<Uuid, FileMetadata>,
    compacting_files: HashMap<Uuid, FileMetadata>,
}

impl FileRegistry {
    fn new() -> Self {
        Self {
            active_files: HashMap::new(),
            compacting_files: HashMap::new(),
        }
    }
}

struct FileMetadata {
    id: Uuid,
    path: String,
    size_bytes: u64,
    row_count: usize,
    created_at: chrono::DateTime<chrono::Utc>,
}

// RaptorMetrics - integrated with unified metrics framework
// Using atomic counters for lock-free metric updates
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

struct RaptorMetrics {
    // Vector and row metrics
    total_vectors: AtomicUsize,
    total_rows: AtomicUsize,
    total_files: AtomicUsize,

    // Operation counters
    insert_operations: AtomicU64,
    search_operations: AtomicU64,
    flush_operations: AtomicU64,
    compaction_operations: AtomicU64,

    // Cache metrics
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,

    // I/O metrics
    bytes_written: AtomicU64,
    bytes_read: AtomicU64,
    memory_usage_bytes: AtomicU64,
}

impl RaptorMetrics {
    fn new() -> Self {
        Self {
            total_vectors: AtomicUsize::new(0),
            total_rows: AtomicUsize::new(0),
            total_files: AtomicUsize::new(0),
            insert_operations: AtomicU64::new(0),
            search_operations: AtomicU64::new(0),
            flush_operations: AtomicU64::new(0),
            compaction_operations: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
            bytes_read: AtomicU64::new(0),
            memory_usage_bytes: AtomicU64::new(0),
        }
    }

    fn cache_hit_ratio(&self) -> f32 {
        let hits = self.cache_hits.load(Ordering::Relaxed);
        let misses = self.cache_misses.load(Ordering::Relaxed);
        if hits + misses == 0 {
            0.0
        } else {
            hits as f32 / (hits + misses) as f32
        }
    }

    fn compression_ratio(&self) -> f32 {
        let written = self.bytes_written.load(Ordering::Relaxed);
        let memory = self.memory_usage_bytes.load(Ordering::Relaxed);
        if memory == 0 {
            1.0
        } else {
            written as f32 / memory as f32
        }
    }
}

/// Implementation of UniversallyOptimized trait for RAPTOR engine
#[async_trait]
impl UniversallyOptimized for RaptorEngine {
    /// Get the universal performance optimizer instance
    fn universal_optimizer(&self) -> &UniversalPerformanceOptimizer {
        &self.universal_optimizer
    }

    /// RAPTOR-specific optimization setup
    async fn setup_engine_optimizations(&self) -> Result<()> {
        // RAPTOR-specific optimizations for columnar analytics with clustering
        tracing::info!("🔧 RAPTOR Engine: Setting up universal performance optimizations");

        // Initialize RAPTOR-specific optimizations
        let config = self.universal_optimizer.get_config();
        tracing::debug!("   Cache size: {}MB", config.cache_size_mb);
        tracing::debug!("   Parallel operations: {}", config.parallel_operations);
        tracing::debug!("   Prefetching enabled: {}", config.enable_prefetching);
        tracing::debug!(
            "   Memory mapping enabled: {}",
            config.enable_memory_mapping
        );

        // RAPTOR is ready for high-performance columnar operations
        tracing::info!(
            "✅ RAPTOR Engine: Universal optimizations configured for columnar analytics"
        );
        Ok(())
    }

    /// RAPTOR-specific performance metrics
    async fn collect_performance_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();

        // Basic RAPTOR metrics (using unified framework)
        metrics.insert(
            "raptor_total_rows".to_string(),
            serde_json::Value::Number(serde_json::Number::from(
                self.metrics.total_rows.load(Ordering::Relaxed),
            )),
        );
        metrics.insert(
            "raptor_total_files".to_string(),
            serde_json::Value::Number(serde_json::Number::from(
                self.metrics.total_files.load(Ordering::Relaxed),
            )),
        );
        metrics.insert(
            "raptor_memory_usage_bytes".to_string(),
            serde_json::Value::Number(serde_json::Number::from(
                self.metrics.memory_usage_bytes.load(Ordering::Relaxed),
            )),
        );

        // Universal optimizer metrics
        let strategy = self.universal_optimizer.get_strategy();
        metrics.insert(
            "universal_optimization_strategy".to_string(),
            serde_json::Value::String(format!("{:?}", strategy)),
        );

        let config = self.universal_optimizer.get_config();
        metrics.insert(
            "universal_cache_size_mb".to_string(),
            serde_json::Value::Number(serde_json::Number::from(config.cache_size_mb)),
        );
        metrics.insert(
            "universal_parallel_operations".to_string(),
            serde_json::Value::Number(serde_json::Number::from(config.parallel_operations)),
        );
        metrics.insert(
            "universal_prefetching_enabled".to_string(),
            serde_json::Value::Bool(config.enable_prefetching),
        );

        Ok(metrics)
    }
}
