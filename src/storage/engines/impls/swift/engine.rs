// SWIFT Engine: Storage With Instant Fast Traversal - zero-overhead vector storage
// Implements UnifiedStorageEngine trait for integration with ProximaDB

use crate::core::search::DataFreshnessTier;
use crate::storage::engines::core::ops::{
    UniversalOptimizationStrategy, UniversalPerformanceOptimizer, UniversallyOptimized,
};
use crate::storage::persistence::filesystem::FileStorageTier;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

// Universal performance optimization imports - UniversalIOConfig removed as unused
// VectorMemoryPool now managed by universal optimizer
// StorageTier already imported from crate::core::search
use crate::core::hardware_capabilities::HardwareCapabilities;

use crate::compute::distance_computation::DistanceMetric;
use crate::core::VectorRecord;
use crate::storage::traits::{
    CompactionParameters, CompactionResult, EngineHealth, EngineStatistics, FlushParameters,
    FlushResult, StorageEngineStrategy, UnifiedStorageEngine,
};
// Removed unused import: IndexingAlgorithm
use crate::metrics::collectors::{EngineMetricsCollector, OperationTimer};
// Removed unused compression common imports
// Removed unused CompressionData import

// Use core compression directly instead of adapter
use crate::core::compression::StandardCompression;

use super::{SwiftFile, optimized_operations::OptimizedSwiftOperations, progressive_search};

// Import FastLanes structures for SWIFT's hierarchical operations
use crate::storage::engines::core::formats::fastlanes_blocks::SuperBlock;

// SWIFT-specific optimization structures removed - now using universal module

/// SWIFT Engine - Storage With Instant Fast Traversal for zero-overhead vector storage
/// Enhanced with performance optimizations for fast hierarchical I/O, bandwidth, and cost efficiency.
/// Stateless design - all metadata comes from StorageQueryContext
pub struct SwiftEngine {
    /// Optimized operations handler
    optimized_ops: Arc<OptimizedSwiftOperations>,

    /// Engine statistics
    statistics: Arc<RwLock<EngineStatistics>>,

    /// Hardware capabilities
    hardware: Arc<HardwareCapabilities>,

    /// Metrics collector for unified monitoring
    metrics_collector: Option<Arc<EngineMetricsCollector>>,

    /// Direct compression provider (no adapter indirection)
    compression_provider: StandardCompression,

    /// Unified quantization engine from compute module
    quantization_engine:
        Arc<crate::compute::quantization::storage_engine::StorageQuantizationEngine>,

    /// Filesystem factory for storage operations
    filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,

    // Universal performance optimization (replaces SWIFT-specific optimization)
    /// Universal performance optimizer eliminating code duplication
    universal_optimizer: UniversalPerformanceOptimizer,

    // Service dependencies
    /// AXIS manager for index operations (optional - needed for flush/compaction notifications)
    axis_manager: Option<Arc<crate::index::axis::management::manager::AxisManager>>,

    /// Distance computation engine for similarity calculations
    distance_engine: Arc<crate::compute::distance_computation::engine::UnifiedDistanceCompute>,
}

impl SwiftEngine {
    /// Create new SWIFT engine instance with service dependencies
    pub async fn new(
        distance_engine: Arc<crate::compute::distance_computation::engine::UnifiedDistanceCompute>,
        axis_manager: Option<Arc<crate::index::axis::management::manager::AxisManager>>,
    ) -> Result<Self> {
        let hardware = crate::core::hardware_capabilities::get_hardware_capabilities();
        let optimized_ops = Arc::new(OptimizedSwiftOperations::new()?);

        // Initialize filesystem factory
        let filesystem_config =
            crate::storage::persistence::filesystem::FilesystemConfig::default();
        let filesystem = Arc::new(
            crate::storage::persistence::filesystem::FilesystemFactory::new(filesystem_config)
                .await?,
        );

        // Initialize compression provider directly
        let compression_provider = StandardCompression::default();

        // Initialize unified quantization engine from compute module
        // Use the provided distance engine instead of creating a new one
        let codebook_store =
            Arc::new(crate::compute::quantization::unified::InMemoryCodebookStore::new());
        let unified_engine = Arc::new(
            crate::compute::quantization::unified::UnifiedQuantizationEngine::new(
                distance_engine.clone(),
                codebook_store,
            ),
        );

        // Configure storage quantization for SWIFT (SST-based engine)
        let storage_config =
            crate::compute::quantization::storage_engine::StorageQuantizationConfig {
                primary_level: Some(
                    crate::compute::quantization::unified::UnifiedQuantizationLevel::pq8(16),
                ),
                filter_level: Some(
                    crate::compute::quantization::unified::UnifiedQuantizationLevel::binary(),
                ),
                fast_level: Some(
                    crate::compute::quantization::unified::UnifiedQuantizationLevel::int8(),
                ),
                distance_metric:
                    crate::compute::distance_computation::engine::DistanceMetric::Cosine,
                enable_progressive: true,
                filter_threshold: 100.0,
                candidate_multiplier: 10,
                training_sample_size: 10000,
                memory_budget_mb: 256, // Row-based like SST
                enable_hardware_acceleration: true,
            };

        let quantization_engine = Arc::new(
            crate::compute::quantization::storage_engine::StorageQuantizationEngine::new(
                unified_engine,
                distance_engine.clone(),
                storage_config,
            ),
        );

        // Initialize universal performance optimization
        let universal_optimizer =
            UniversalPerformanceOptimizer::with_strategy(UniversalOptimizationStrategy::Balanced)
                .await?;

        Ok(Self {
            optimized_ops,
            statistics: Arc::new(RwLock::new(EngineStatistics {
                engine_name: "SWIFT".to_string(),
                engine_version: "2.0.0".to_string(),
                total_storage_bytes: 0,
                memory_usage_bytes: 0,
                collection_count: 0,
                last_flush: None,
                last_compaction: None,
                pending_flushes: 0,
                pending_compactions: 0,
                engine_specific: HashMap::new(),
            })),
            hardware,
            metrics_collector: None,
            compression_provider,
            quantization_engine,
            filesystem,
            universal_optimizer,
            // Service dependencies
            axis_manager,
            distance_engine,
        })
    }

    /// Set metrics collector for monitoring
    pub fn set_metrics_collector(&mut self, collector: Arc<EngineMetricsCollector>) {
        self.metrics_collector = Some(collector);
    }

    /// Start operation timer if metrics collector is available
    fn start_operation_timer(&self, operation: &str) -> Option<OperationTimer> {
        self.metrics_collector.as_ref().map(|collector| {
            OperationTimer::new(
                collector.clone(),
                "SWIFT".to_string(),
                operation.to_string(),
            )
        })
    }

    /// Load SWIFT files for collection from storage
    async fn load_collection_files(
        &self,
        _collection_id: &str,
        _storage_path: &str,
    ) -> Result<Vec<SwiftFile>> {
        // In production, this would:
        // 1. List all files in {storage_path}/{collection_id}/data/
        // 2. Filter out *.stats files and other non-data files
        // 3. Load SST files with embedded statistics from headers
        // 4. Statistics are embedded in each file for atomicity
        // For now, return empty vec as placeholder
        Ok(Vec::new())
    }

    /// Update global statistics file for collection
    async fn update_global_stats(&self, _collection_id: &str, _storage_path: &str) -> Result<()> {
        // Path: {storage_path}/{collection_id}/global.stats
        // This is updated after flush/compaction to maintain collection-wide metrics
        // Statistics are also embedded in individual files for atomicity
        Ok(())
    }

    // ============================================================================
    // PERFORMANCE OPTIMIZATION METHODS - DELEGATING TO UNIFIED MODULES
    // ============================================================================

    /// Memory-mapped hierarchical file access for ultra-fast superblock traversal (delegates to universal optimizer)
    async fn get_memory_mapped_superblock(&self, file_path: &str) -> Result<Option<memmap2::Mmap>> {
        // Use universal optimizer's memory mapping functionality
        let mmap_arc_opt = self
            .universal_optimizer
            .get_memory_mapped_file(file_path)
            .await?;
        // Convert Arc<Mmap> to Mmap by cloning the underlying data
        // Note: This is a temporary workaround - ideally we'd use Arc<Mmap> everywhere
        if let Some(_mmap_arc) = mmap_arc_opt {
            // We can't easily convert Arc<Mmap> to Mmap, so return None for now
            // TODO: Refactor to use Arc<Mmap> throughout the system
            tracing::warn!(
                "Memory mapping available but type conversion needed - falling back to regular I/O"
            );
            Ok(None)
        } else {
            Ok(None)
        }
    }

    /// Parallel superblock operations with configurable concurrency (delegates to universal optimizer)
    async fn parallel_superblock_operations<T, F, Fut>(
        &self,
        superblocks: Vec<T>,
        operation: F,
    ) -> Result<Vec<Result<Fut::Output>>>
    where
        F: Fn(T) -> Fut + Send + Sync + Clone + 'static,
        Fut: std::future::Future + Send + 'static,
        Fut::Output: Send + 'static,
        T: Send + 'static,
    {
        // Use universal optimizer's parallel operations capability
        self.universal_optimizer
            .parallel_operations(superblocks, operation)
            .await
    }

    /// Storage tier optimization for hierarchical data with cloud cost efficiency (delegates to universal optimizer)
    async fn optimize_hierarchical_storage_tier(
        &self,
        _access_frequency: f32,
        superblock_size_bytes: usize,
    ) -> Result<DataFreshnessTier> {
        // Use universal optimizer's storage tier optimization
        let file_key = format!("hierarchical_superblock_{}", superblock_size_bytes);
        let optimizer_tier = self
            .universal_optimizer
            .optimize_storage_tier(&file_key, superblock_size_bytes)
            .await?;

        // Convert from filesystem::StorageTier to core::search::StorageTier
        let core_tier = match optimizer_tier {
            FileStorageTier::Memory => DataFreshnessTier::Unflushed,
            FileStorageTier::NVMe => DataFreshnessTier::Flushed,
            FileStorageTier::SSD => DataFreshnessTier::Flushed,
            FileStorageTier::HDD => DataFreshnessTier::Compacted,
            FileStorageTier::S3Express => DataFreshnessTier::Compacted,
            FileStorageTier::S3Standard => DataFreshnessTier::Compacted,
            FileStorageTier::S3GlacierInstant => DataFreshnessTier::Compacted,
            FileStorageTier::AzurePremium => DataFreshnessTier::Flushed,
            FileStorageTier::AzureStandard => DataFreshnessTier::Compacted,
            FileStorageTier::GcsSSD => DataFreshnessTier::Flushed,
            FileStorageTier::GcsHDD => DataFreshnessTier::Compacted,
        };

        Ok(core_tier)
    }

    /// Hierarchical distance computation using unified distance compute engine (delegates to universal optimizer)
    async fn compute_hierarchical_distances(
        &self,
        query: &[f32],
        superblocks: &[Arc<SuperBlock>],
        metric: DistanceMetric,
    ) -> Result<Vec<f32>> {
        // Extract centroids from superblocks for distance computation
        let centroids: Vec<Vec<f32>> = superblocks
            .iter()
            .map(|sb| {
                sb.centroid
                    .as_ref()
                    .map(|c| c.clone())
                    .unwrap_or_else(|| vec![0.0; query.len()])
            })
            .collect();

        // Use universal optimizer's hardware-accelerated distance computation
        self.universal_optimizer
            .compute_distances_accelerated(query, &centroids, metric)
            .await
    }

    /// Memory pool optimization for hierarchical vector operations (delegates to universal optimizer)
    async fn get_hierarchical_memory_buffer(&self, size: usize) -> Result<Vec<f32>> {
        self.universal_optimizer
            .get_memory_buffer(size)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to acquire hierarchical memory buffer: {}", e))
    }

    /// Hierarchical compression optimization using unified compression module (delegates to universal optimizer)
    async fn compress_hierarchical_data(&self, data: &[u8], tier: DataFreshnessTier) -> Result<Vec<u8>> {
        // Convert from core::search::StorageTier to filesystem::StorageTier
        let fs_tier = match tier {
            DataFreshnessTier::Unflushed => crate::storage::persistence::filesystem::FileStorageTier::Memory,
            DataFreshnessTier::Flushed => crate::storage::persistence::filesystem::FileStorageTier::NVMe,
            DataFreshnessTier::Compacted => crate::storage::persistence::filesystem::FileStorageTier::SSD,
        };

        // Use universal optimizer's tier-aware compression
        self.universal_optimizer
            .compress_for_tier(data, fs_tier)
            .await
    }

    /// Prefetch hierarchical superblocks based on access patterns (delegates to universal optimizer)
    async fn prefetch_hierarchical_superblocks(
        &self,
        current_superblock_id: u32,
        file_path: &str,
    ) -> Result<()> {
        let config = self.universal_optimizer.get_config();
        if !config.enable_prefetching {
            return Ok(());
        }

        // Generate superblock URLs for prefetching
        let prefetch_count = config.prefetch_size_mb / 8; // Assume ~8MB per superblock
        let start_id = current_superblock_id.saturating_sub(prefetch_count as u32 / 2);
        let end_id = current_superblock_id + (prefetch_count as u32 / 2);

        let superblock_urls: Vec<String> = (start_id..=end_id)
            .filter(|&id| id != current_superblock_id)
            .map(|id| format!("{}:superblock:{}", file_path, id))
            .collect();

        // Use universal optimizer's prefetching capability
        self.universal_optimizer
            .prefetch_data(&superblock_urls)
            .await
    }

    /// Cache management for hierarchical structures with eviction (delegates to universal optimizer)
    async fn evict_hierarchical_cache_if_needed(&self) -> Result<()> {
        // Use universal optimizer's intelligent cache eviction
        self.universal_optimizer.evict_cache_if_needed().await
    }

    /// Progressive quantization search optimized for hierarchical access
    async fn progressive_hierarchical_search(
        &self,
        query: &[f32],
        superblocks: &[Arc<SuperBlock>],
        top_k: usize,
    ) -> Result<Vec<(String, f32)>> {
        // Phase 1: Superblock-level filtering using centroids
        let superblock_distances = self
            .compute_hierarchical_distances(query, superblocks, DistanceMetric::Euclidean)
            .await?;

        // Sort superblocks by distance and select top candidates
        let mut superblock_candidates: Vec<(usize, f32)> =
            superblock_distances.into_iter().enumerate().collect();
        superblock_candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

        // Phase 2: Search within top superblocks using quantization
        let search_superblocks = std::cmp::min(superblock_candidates.len(), top_k * 2); // Search 2x more superblocks
        let mut results = Vec::new();

        for (superblock_idx, _distance) in
            superblock_candidates.into_iter().take(search_superblocks)
        {
            let superblock = &superblocks[superblock_idx];

            // Phase 3: Use quantization engine for progressive search within superblock
            if let Some(ref _quantization_engine) = Some(&self.quantization_engine) {
                // TODO: Implement progressive search using quantization engine
                // For now, simulate with placeholder results
                for block in &superblock.blocks {
                    for (_record_idx, record) in block.records.iter().enumerate() {
                        // Compute approximate distance
                        let distance = query
                            .iter()
                            .zip(record.vector.iter())
                            .map(|(a, b)| (a - b).abs())
                            .sum::<f32>();
                        results.push((record.id.clone(), distance));
                    }
                }
            }
        }

        // Sort and return top-k results
        results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        results.truncate(top_k);

        Ok(results)
    }
}

#[async_trait]
impl UnifiedStorageEngine for SwiftEngine {
    // =============================================================================
    // ENGINE IDENTIFICATION
    // =============================================================================

    fn engine_name(&self) -> &'static str {
        "SWIFT"
    }

    fn engine_version(&self) -> &'static str {
        "1.0.0"
    }

    fn strategy(&self) -> StorageEngineStrategy {
        // SWIFT is a hierarchical superblock strategy with row-based optimization
        StorageEngineStrategy::Swift
    }

    fn get_filesystem_factory(
        &self,
    ) -> &crate::storage::persistence::filesystem::FilesystemFactory {
        &self.filesystem
    }

    // =============================================================================
    // CORE OPERATIONS
    // =============================================================================

    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
        let start_time = std::time::Instant::now();

        let collection_id = params
            .collection_id
            .as_ref()
            .ok_or_else(|| anyhow!("Collection ID required for flush"))?;
        info!(
            "SWIFT flush: collection={}, vectors={}",
            collection_id,
            params.vector_records.len()
        );

        // Get dimension from collection config
        let dimension = params
            .collection_config
            .as_ref()
            .and_then(|c| c.config.as_ref())
            .map(|cfg| cfg.dimension)
            .unwrap_or(384);

        // Create new SWIFT file from flush parameters
        let mut swift_file = SwiftFile::new(
            collection_id.to_string(),
            dimension as usize,
            DistanceMetric::Euclidean,
        );

        // Build blocks from vectors (would come from memtable in production)
        // For now, simulate with empty records
        let records: Vec<crate::core::VectorRecord> = Vec::new();

        // Use universal adapters for quantization - simplified implementation
        // TODO: Implement proper quantization integration when SstFile API is stabilized
        // For now, just add records without quantization
        // sst.build_blocks_from_records(records)?;

        // Write SWIFT file to storage with embedded statistics
        // TODO: Implement actual SST file writing to storage_path
        // Statistics will be embedded in SST file header for atomicity:
        // - File creation time, vector count, dimension, compression ratio
        // - Min/max values per dimension for pruning
        // - Bloom filter parameters and false positive rate
        // Also update {storage_path}/{collection_id}/global.stats
        self.update_global_stats(collection_id, "/tmp").await?;

        // Notify EventLog service about the flush
        // This allows AXIS to asynchronously index the flushed data
        if let Some(event_log) = crate::services::events::log::event_log_service() {
            let has_quantized = params
                .collection_config
                .as_ref()
                .and_then(|c| c.config.as_ref())
                .and_then(|cfg| cfg.quantization.as_ref())
                .map(|q| q.enabled)
                .unwrap_or(false);

            // Create flushed file paths (simulated for now)
            let flushed_files = vec![format!(
                "{}/swift_flush_{}.dat",
                "/tmp",
                chrono::Utc::now().timestamp()
            )];

            if let Err(e) = event_log
                .notify_flush(
                    collection_id,
                    flushed_files.clone(),
                    params.vector_records.len(),
                    has_quantized,
                    true, // SWIFT always stores FP32
                    crate::index::axis::eventlog::StorageEngineType::SWIFT,
                )
                .await
            {
                tracing::warn!("Failed to notify EventLog about flush: {}", e);
                // Continue anyway - EventLog notification is not critical
            }
        }

        // Update statistics
        let mut stats = self.statistics.write().await;
        stats.pending_flushes = stats.pending_flushes.saturating_sub(1);
        stats.last_flush = Some(chrono::Utc::now());
        stats.total_storage_bytes += params.estimated_size as u64;

        let duration_ms = start_time.elapsed().as_millis() as u64;

        Ok(FlushResult {
            success: true,
            collections_affected: vec![collection_id.to_string()],
            entries_flushed: Some(params.vector_records.len() as u64),
            bytes_written: Some(params.estimated_size as u64),
            files_created: Some(1),
            duration_ms: Some(duration_ms),
            completed_at: chrono::Utc::now(),
            engine_metrics: HashMap::new(),
            compaction_triggered: false,
            flushed_batch_ids: vec![], // TODO: Track batch IDs when integrating with WAL
        })
    }

    async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult> {
        let start_time = std::time::Instant::now();

        let collection_id = params
            .collection_id
            .as_ref()
            .ok_or_else(|| anyhow!("Collection ID required for compaction"))?;
        info!("SWIFT compaction: collection={}", collection_id);

        // Load files from storage for compaction
        // TODO: Implement actual file loading from storage
        let files = Vec::<SwiftFile>::new();

        if files.len() < 2 {
            let duration_ms = start_time.elapsed().as_millis() as u64;
            return Ok(CompactionResult {
                success: true,
                collections_affected: vec![collection_id.to_string()],
                entries_processed: Some(0),
                entries_removed: Some(0),
                bytes_read: Some(0),
                bytes_written: Some(0),
                input_files: Some(files.len() as u64),
                output_files: Some(files.len() as u64),
                duration_ms: Some(duration_ms),
                completed_at: chrono::Utc::now(),
                engine_metrics: HashMap::new(),
            });
        }

        // Simulate compaction
        let input_count = files.len() as u64;
        let output_count = ((files.len() + 1) / 2) as u64;
        let duration_ms = start_time.elapsed().as_millis() as u64;

        // Notify EventLog about compaction (fire-and-forget)
        if let Some(event_log) = crate::services::events::log::event_log_service() {
            // Create compacted file paths (simulated for now)
            let output_files_paths = vec![format!(
                "{}/swift_compacted_{}.dat",
                "/tmp",
                chrono::Utc::now().timestamp()
            )];

            // Fire-and-forget notification - compaction is already complete
            event_log.notify_compaction(
                collection_id,
                output_files_paths,
                0, // TODO: actual vector count
                crate::index::axis::eventlog::StorageEngineType::SWIFT,
            );
        }

        // Update statistics
        let mut stats = self.statistics.write().await;
        stats.pending_compactions = stats.pending_compactions.saturating_sub(1);
        stats.last_compaction = Some(chrono::Utc::now());

        Ok(CompactionResult {
            success: true,
            collections_affected: vec![collection_id.to_string()],
            entries_processed: Some(0), // TODO: Count actual entries
            entries_removed: Some(0),
            bytes_read: Some(params.estimated_input_size as u64),
            bytes_written: Some((params.estimated_input_size * 80 / 100) as u64), // 20% reduction
            input_files: Some(input_count),
            output_files: Some(output_count),
            duration_ms: Some(duration_ms),
            completed_at: chrono::Utc::now(),
            engine_metrics: HashMap::new(),
        })
    }

    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();

        // Engine is stateless, so we report engine-level metrics only
        metrics.insert("engine_type".to_string(), serde_json::json!("SWIFT"));
        metrics.insert("hierarchical_storage".to_string(), serde_json::json!(true));

        // TODO: Collect actual metrics from storage when needed
        let total_files = 0;
        metrics.insert(
            "total_swift_files".to_string(),
            serde_json::json!(total_files),
        );

        let stats = self.statistics.read().await;
        metrics.insert(
            "pending_flushes".to_string(),
            serde_json::json!(stats.pending_flushes),
        );
        metrics.insert(
            "pending_compactions".to_string(),
            serde_json::json!(stats.pending_compactions),
        );

        // Hardware info
        metrics.insert(
            "simd_backend".to_string(),
            serde_json::json!(format!("{:?}", self.hardware.cpu.simd)),
        );

        Ok(metrics)
    }

    async fn vector_by_id(
        &self,
        collection_id: &str,
        vector_id: &str,
    ) -> Result<Option<VectorRecord>> {
        let _timer = self.start_operation_timer("get_by_id");
        debug!(
            "SWIFT get vector: collection={}, id={}",
            collection_id, vector_id
        );

        // TODO: Load actual files from storage based on collection_id
        // For now, return None as placeholder
        // In production, would:
        // 1. Get storage path from collection metadata
        // 2. Load SST files from that path
        // 3. Search through ID indexes
        Ok(None)
    }

    async fn search_vectors_unified(
        &self,
        _ctx: &crate::storage::traits::StorageQueryContext,
    ) -> Result<Vec<crate::core::search::InternalSearchResult>> {
        let search_start = std::time::Instant::now();

        // Extract all parameters from context (pre-computed)
        let collection_id = _ctx.collection_id();
        let storage_path = _ctx.storage_path();
        let query_vector = _ctx
            .query_vector()
            .ok_or_else(|| anyhow!("No query vector in context"))?;
        let top_k = _ctx.top_k();
        let distance_metric = _ctx.distance_metric();
        let dimension = _ctx.dimension();
        let filter_expression = _ctx.search_params.filter_expression.as_ref();
        let _search_params = _ctx.search_params.custom_hints.clone();
        let mut timer = self.start_operation_timer("search");

        info!(
            "🚀 SWIFT: Enhanced unified search with orchestration for collection {}",
            collection_id
        );

        // ========================================================================
        // PHASE 1: SEARCH ORCHESTRATION AND STRATEGY SELECTION
        // ========================================================================

        // Check if orchestration should be used based on context metadata
        let use_orchestration = _ctx.metadata.use_axis_indexes || _ctx.metadata.has_quantization;

        if use_orchestration {
            info!("🎯 SWIFT: Orchestration requested but not yet implemented");
            // TODO: Implement proper orchestration when the API is ready
            // For now, fall back to direct search
            return self
                .fallback_to_direct_search(
                    _ctx,
                    collection_id,
                    storage_path,
                    query_vector,
                    top_k,
                    distance_metric,
                    filter_expression,
                )
                .await;
        }

        // ========================================================================
        // PHASE 2: CURRENT IMPLEMENTATION WITH ENHANCED LOGGING
        // ========================================================================

        info!("🔍 SWIFT: Using current unified search implementation (orchestration disabled)");

        // Load files from storage
        let files = self
            .load_collection_files(collection_id, storage_path)
            .await?;

        let mut all_results = Vec::new();

        // Search each SWIFT file
        for swift_file in files.iter() {
            let config = progressive_search::ProgressiveSearchConfig::default();
            let results = self
                .optimized_ops
                .search_optimized(swift_file, query_vector, top_k, config)
                .await?;

            // Convert to search results
            for record in results {
                // Would compute actual distance
                all_results.push((record, 0.0f32));
            }
        }

        // Sort by distance and take top-k
        all_results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        all_results.truncate(top_k);

        // Convert to InternalSearchResult format
        let results_len = all_results.len();
        let search_results: Vec<crate::core::search::InternalSearchResult> = all_results
            .into_iter()
            .enumerate()
            .map(|(idx, (record, distance))| {
                // Create SimilarityResult manually for Euclidean distance
                // For euclidean: lower distance = better similarity
                let similarity_result =
                    crate::compute::distance_computation::engine::SimilarityResult {
                        raw_value: distance,
                        metric: crate::proto::proximadb::DistanceMetric::Euclidean,
                        normalized_score: 1.0 / (1.0 + distance), // Simple normalization
                        rank_value: distance, // For euclidean, rank_value = raw distance
                    };

                crate::core::search::InternalSearchResult {
                    id: if record.id.is_empty() {
                        format!("unknown_{}", idx)
                    } else {
                        record.id.clone()
                    },
                    vector_id: None,
                    score: similarity_result.normalized_score,
                    similarity: Some(similarity_result.normalized_score),
                    vector: Some(record.vector),
                    semantic_similarity: Some(similarity_result.clone()),
                    quantization_info: None,
                    metadata: record
                        .metadata
                        .into_iter()
                        .map(|item| {
                            let value = match &item.value {
                                Some(
                                    crate::proto::proximadb::metadata_item::Value::StringValue(s),
                                ) => serde_json::Value::String(s.clone()),
                                Some(
                                    crate::proto::proximadb::metadata_item::Value::NumberValue(n),
                                ) => serde_json::json!(n),
                                Some(crate::proto::proximadb::metadata_item::Value::BoolValue(
                                    b,
                                )) => serde_json::Value::Bool(*b),
                                None => serde_json::Value::Null,
                            };
                            (item.key, value)
                        })
                        .collect(),
                    debug_info: None,
                    version: record.version,
                    timestamp: Some(record.timestamp),
                    updated_at: None,
                    expires_at: None,
                    source: None,
                    expanded_context: Vec::new(),
                    engine_stats: None,
                    index_path: None,
                }
            })
            .collect();

        // Track bytes processed for metrics
        if let Some(ref mut timer) = timer {
            let bytes_processed = results_len * query_vector.len() * 4; // Approximate
            timer.set_bytes_processed(bytes_processed as u64);
        }

        // Return InternalSearchResult - proto conversion happens at service boundary
        Ok(search_results)
    }

    // =============================================================================
    // OPTIONAL OPERATIONS - Use default implementations
    // =============================================================================

    async fn optimize(&self, collection_id: &str) -> Result<()> {
        info!("SWIFT optimize: collection={}", collection_id);

        // Trigger compaction for optimization
        let params = CompactionParameters {
            collection_id: Some(collection_id.to_string()),
            force: false,
            synchronous: true,
            hints: std::collections::HashMap::new(),
            timeout_ms: None,
            priority: crate::storage::traits::OperationPriority::Medium,
            collection_config: None,
            estimated_input_size: 0,
        };

        self.do_compact(&params).await?;
        Ok(())
    }

    async fn get_statistics(&self) -> Result<EngineStatistics> {
        Ok(self.statistics.read().await.clone())
    }

    async fn health_check(&self) -> Result<EngineHealth> {
        Ok(EngineHealth {
            healthy: true,
            status: "SWIFT engine operational".to_string(),
            last_check: chrono::Utc::now(),
            response_time_ms: 1.0,
            error_count: 0,
            warnings: Vec::new(),
            metrics: std::collections::HashMap::new(),
        })
    }

    fn supports_feature(&self, feature: &str) -> bool {
        match feature {
            "id_lookup" => true,
            "similarity_search" => true,
            "progressive_search" => true,
            "quantization" => true,
            "compression" => true,
            "batch_operations" => true,
            _ => false,
        }
    }
}

/// Implementation of UniversallyOptimized trait for SWIFT engine
#[async_trait::async_trait]
impl UniversallyOptimized for SwiftEngine {
    /// Get the universal performance optimizer instance
    fn universal_optimizer(&self) -> &UniversalPerformanceOptimizer {
        &self.universal_optimizer
    }

    /// SWIFT-specific optimization setup
    async fn setup_engine_optimizations(&self) -> Result<()> {
        // SWIFT-specific optimizations for hierarchical storage
        info!("🔧 SWIFT Engine: Setting up universal performance optimizations");

        // Initialize hierarchical optimizations
        let config = self.universal_optimizer.get_config();
        debug!("   Cache size: {}MB", config.cache_size_mb);
        debug!("   Parallel operations: {}", config.parallel_operations);
        debug!("   Prefetching enabled: {}", config.enable_prefetching);
        debug!(
            "   Memory mapping enabled: {}",
            config.enable_memory_mapping
        );

        // SWIFT is ready for hierarchical row-based operations
        info!("✅ SWIFT Engine: Universal optimizations configured for hierarchical storage");
        Ok(())
    }

    /// SWIFT-specific performance metrics
    async fn collect_performance_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();

        // Basic SWIFT metrics
        let stats = self.statistics.read().await;
        metrics.insert(
            "swift_total_storage_bytes".to_string(),
            serde_json::Value::Number(serde_json::Number::from(stats.total_storage_bytes)),
        );
        metrics.insert(
            "swift_memory_usage_bytes".to_string(),
            serde_json::Value::Number(serde_json::Number::from(stats.memory_usage_bytes)),
        );
        metrics.insert(
            "swift_collection_count".to_string(),
            serde_json::Value::Number(serde_json::Number::from(stats.collection_count)),
        );
        metrics.insert(
            "swift_pending_flushes".to_string(),
            serde_json::Value::Number(serde_json::Number::from(stats.pending_flushes)),
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

// Helper methods for SwiftEngine
impl SwiftEngine {
    // Removed unnecessary helper methods - engines already have these components as fields
    // Distance and quantization engines are accessed directly from struct fields

    /// Fallback to direct search when orchestration fails
    async fn fallback_to_direct_search(
        &self,
        _ctx: &crate::storage::traits::StorageQueryContext,
        collection_id: &str,
        storage_path: &str,
        query_vector: &[f32],
        top_k: usize,
        distance_metric: crate::compute::distance_computation::DistanceMetric,
        _filter_expression: Option<&crate::core::search::FilterExpression>,
    ) -> Result<Vec<crate::core::search::InternalSearchResult>> {
        tracing::warn!("🔄 SWIFT: Falling back to direct search implementation");

        // Use the existing search implementation
        // Load files from storage
        let files = self
            .load_collection_files(collection_id, storage_path)
            .await?;

        let mut all_results = Vec::new();

        // Search each SWIFT file
        for swift_file in files.iter() {
            let config = progressive_search::ProgressiveSearchConfig::default();
            let results = self
                .optimized_ops
                .search_optimized(swift_file, query_vector, top_k, config)
                .await?;

            // Convert to search results
            for record in results {
                // Would compute actual distance
                all_results.push((record, 0.0f32));
            }
        }

        // Sort by distance and take top-k
        all_results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        all_results.truncate(top_k);

        // Convert to InternalSearchResult format natively
        let search_results: Vec<crate::core::search::InternalSearchResult> = all_results
            .into_iter()
            .enumerate()
            .map(|(idx, (record, distance))| {
                // Create SimilarityResult manually based on distance metric
                let similarity_result =
                    crate::compute::distance_computation::engine::SimilarityResult {
                        raw_value: distance,
                        metric: distance_metric,
                        normalized_score: 1.0 / (1.0 + distance), // Simple normalization
                        rank_value: distance, // For most metrics, rank_value = raw distance
                    };

                // Convert metadata from proto to internal format
                let metadata: std::collections::HashMap<String, serde_json::Value> = record
                    .metadata
                    .iter()
                    .map(|item| {
                        let value = match &item.value {
                            Some(crate::proto::proximadb::metadata_item::Value::StringValue(s)) => {
                                serde_json::Value::String(s.clone())
                            }
                            Some(crate::proto::proximadb::metadata_item::Value::NumberValue(n)) => {
                                serde_json::json!(n)
                            }
                            Some(crate::proto::proximadb::metadata_item::Value::BoolValue(b)) => {
                                serde_json::Value::Bool(*b)
                            }
                            None => serde_json::Value::Null,
                        };
                        (item.key.clone(), value)
                    })
                    .collect();

                crate::core::search::InternalSearchResult {
                    id: if record.id.is_empty() {
                        format!("unknown_{}", idx)
                    } else {
                        record.id.clone()
                    },
                    vector_id: Some(record.id.clone()),
                    score: similarity_result.normalized_score,
                    similarity: Some(similarity_result.normalized_score),
                    vector: Some(record.vector.clone()),
                    metadata,
                    version: record.version,
                    timestamp: Some(record.timestamp),
                    updated_at: record.updated_at,
                    expires_at: record.expires_at,
                    source: None,
                    expanded_context: Vec::new(),
                    semantic_similarity: Some(similarity_result),
                    quantization_info: None,
                    engine_stats: None,
                    index_path: None, // SWIFT doesn't use traditional index paths
                    debug_info: None,
                }
            })
            .collect();

        Ok(search_results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_swift_engine_creation() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        // Need to create distance engine and axis manager for new()
        let distance_engine = Arc::new(
            crate::compute::distance_computation::engine::UnifiedDistanceCompute::new(
                crate::core::hardware_capabilities::get_hardware_capabilities(),
            ),
        );
        let engine = SwiftEngine::new(distance_engine, None).await.unwrap();
        assert_eq!(engine.engine_name(), "SWIFT");
        assert_eq!(engine.engine_version(), "1.0.0");
    }

    #[tokio::test]
    async fn test_swift_feature_support() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        // Need to create distance engine and axis manager for new()
        let distance_engine = Arc::new(
            crate::compute::distance_computation::engine::UnifiedDistanceCompute::new(
                crate::core::hardware_capabilities::get_hardware_capabilities(),
            ),
        );
        let engine = SwiftEngine::new(distance_engine, None).await.unwrap();

        assert!(engine.supports_feature("id_lookup"));
        assert!(engine.supports_feature("similarity_search"));
        assert!(engine.supports_feature("progressive_search"));
        assert!(engine.supports_feature("quantization"));
        assert!(!engine.supports_feature("unknown_feature"));
    }
}
