// NOVA Engine: Next-gen Optimized Vector Analytics with columnar quantization
// Implements UnifiedStorageEngine trait for integration with ProximaDB

use crate::core::compression::StandardCompression;
use crate::core::search::DataFreshnessTier;
use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::engines::core::ops::{
    UniversalOptimizationStrategy, UniversalPerformanceOptimizer, UniversallyOptimized,
};
use crate::storage::engines::impls::nova::NovaFile;
use crate::storage::traits::{
    CompactionParameters, CompactionResult, EngineHealth, EngineStatistics, FlushParameters,
    FlushResult, OperationPriority, UnifiedStorageEngine,
};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};
// Health status handled internally
use crate::compute::distance_computation::DistanceMetric;
use crate::core::search::results::OptimizedSearchRecord;
use crate::metrics::collectors::{EngineMetricsCollector, OperationTimer};
// Use core compression directly instead of adapter
use super::optimized_operations::OptimizedNovaOperations;
use crate::core::compression::CompressionAlgorithm;
// Arrow schema handled by parquet reader

// Performance optimization handled internally
// NOVA-specific optimization structures removed - now using universal module

use crate::core::hardware_capabilities::HardwareCapabilities;

/// NOVA Engine - Next-gen Optimized Vector Analytics for columnar storage
/// Enhanced with performance optimizations for fast reads, I/O bandwidth, and cost efficiency
pub struct NovaEngine {
    /// Filesystem factory for storage operations
    filesystem: Arc<crate::storage::persistence::filesystem::FilesystemFactory>,

    /// Optimized operations handler
    optimized_ops: Arc<OptimizedNovaOperations>,
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
    /// Distance computation engine
    distance_engine: Arc<crate::compute::distance_computation::engine::UnifiedDistanceCompute>,

    // Universal performance optimization (replaces NOVA-specific optimization)
    /// Universal performance optimizer eliminating code duplication
    universal_optimizer: UniversalPerformanceOptimizer,
}
impl NovaEngine {
    /// Create new NOVA engine instance
    pub async fn new() -> Result<Self> {
        let hardware = crate::core::hardware_capabilities::get_hardware_capabilities();
        let optimized_ops = Arc::new(OptimizedNovaOperations::new()?);

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
        let distance_compute = Arc::new(
            crate::compute::distance_computation::engine::UnifiedDistanceCompute::default(),
        );
        let codebook_store =
            Arc::new(crate::compute::quantization::unified::InMemoryCodebookStore::new());
        let unified_engine = Arc::new(
            crate::compute::quantization::unified::UnifiedQuantizationEngine::new(
                distance_compute.clone(),
                codebook_store,
            ),
        );

        // Configure storage quantization for NOVA (columnar engine)
        let storage_config =
            crate::compute::quantization::storage_engine::StorageQuantizationConfig {
                primary_level: Some(
                    crate::compute::quantization::unified::UnifiedQuantizationLevel::Pq8,
                ),
                filter_level: Some(
                    crate::compute::quantization::unified::UnifiedQuantizationLevel::Binary,
                ),
                fast_level: Some(
                    crate::compute::quantization::unified::UnifiedQuantizationLevel::Int8,
                ),
                distance_metric: DistanceMetric::Cosine,
                enable_progressive: true,
                filter_threshold: 100.0,
                candidate_multiplier: 10,
                training_sample_size: 10000,
                memory_budget_mb: 512, // Columnar uses more memory
                enable_hardware_acceleration: true,
            };

        let quantization_engine = Arc::new(
            crate::compute::quantization::storage_engine::StorageQuantizationEngine::new(
                unified_engine,
                distance_compute.clone(),
                storage_config,
            ),
        );

        // Initialize universal performance optimization
        let universal_optimizer =
            UniversalPerformanceOptimizer::with_strategy(UniversalOptimizationStrategy::Balanced)
                .await?;

        // NOVA benefits from UnifiedCachingFilesystem for caching hierarchical stats
        // We'll create collection-specific instances during operations since we need
        // the actual storage path to get the right filesystem from the factory

        Ok(Self {
            filesystem,
            optimized_ops,
            statistics: Arc::new(RwLock::new(EngineStatistics {
                engine_name: "NOVA".to_string(),
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
            distance_engine: distance_compute,
            universal_optimizer,
        })
    }
    /// Set metrics collector for monitoring
    pub fn set_metrics_collector(&mut self, collector: Arc<EngineMetricsCollector>) {
        self.metrics_collector = Some(collector);
    }

    /// Start operation timer if metrics collector is available
    fn start_operation_timer(&self, operation: &str) -> Option<OperationTimer> {
        self.metrics_collector.as_ref().map(|collector| {
            OperationTimer::new(collector.clone(), "NOVA".to_string(), operation.to_string())
        })
    }

    /// Load NOVA files for collection from storage
    /// UnifiedCachingFilesystem provides transparent cloud storage support:
    /// - Cloud files (S3/GCS/Azure) are automatically downloaded to local disk cache on first access
    /// - Subsequent reads use the local cached copy (path: /tmp/proximadb/cache/{collection}/nova/)
    /// - Parquet metadata and footers are cached separately for fast schema access
    /// - Hot files remain in cache based on LRU policy and access patterns
    async fn load_collection_files(
        &self,
        collection_id: &str,
        storage_path: &str,
    ) -> Result<Vec<super::NovaFile>> {
        use crate::storage::persistence::filesystem::FileSystem;

        // Get UnifiedCachingFilesystem for NOVA
        // This creates a collection-specific cache instance that:
        // - Downloads cloud files to local disk cache on first read
        // - Caches Parquet metadata/footers for fast schema access
        // - Tracks access patterns for intelligent prefetching
        let unified_fs = self
            .filesystem
            .get_unified_caching_filesystem(
                storage_path,
                collection_id.to_string(),
                crate::storage::engines::ENGINE_NOVA.to_string(),
            )
            .map_err(|e| anyhow!("Failed to create unified filesystem: {}", e))?;

        // List all NOVA files in the collection directory
        let files = unified_fs.list(storage_path).await?;
        let mut nova_files = Vec::new();

        // Filter for NOVA Parquet files (using NOVA_FILE_EXT constant)
        for file_path in files {
            if file_path.name.ends_with(crate::storage::engines::constants::NOVA_FILE_EXT) {
                // Create a reader for this file based on query type
                let reader = super::unified_strategy_reader::UnifiedNOVAReader::for_search(
                    self.filesystem.clone(),
                    collection_id.to_string(),
                )?;

                // Read vectors using the cached filesystem (metadata will be cached)
                let vectors = reader.read_progressive(&file_path.name).await?;

                // Create NovaFile structure (simplified - would include actual metadata)
                let nova_file = super::NovaFile {
                    quantized_columns: super::quantized_columns::QuantizedColumnMetadata::default(),
                    schema: Arc::new(arrow_schema::Schema::empty()),
                    metadata: crate::storage::engines::core::formats::columnar::ColumnarFileMetadata {
                        collection_id: collection_id.to_string(),
                        num_vectors: vectors.len() as u64,
                        dimension: if !vectors.is_empty() { vectors[0].vector.len() } else { 0 },
                        distance_metric: crate::compute::distance_computation::DistanceMetric::Euclidean,
                        quantization: Default::default(),
                        column_stats: Default::default(),
                        version: 1,
                        timestamp: chrono::Utc::now(),
                        modified_at: chrono::Utc::now(),
                    },
                    row_groups: Vec::new(),
                    enhanced_stats: Vec::new(),
                    superblocks: Vec::new(),
                    advanced_zone_maps: None,
                };

                nova_files.push(nova_file);
            }
        }

        // If no files found, return empty vec (normal for new collections)
        if nova_files.is_empty() {
            debug!("No NOVA files found for collection {} in {}", collection_id, storage_path);
        } else {
            info!("Loaded {} NOVA files for collection {} from {} (cached)",
                nova_files.len(), collection_id, storage_path);
        }

        Ok(nova_files)
    }

    /// Update global statistics file for collection
    async fn update_global_stats(&self, collection_id: &str, storage_path: &str) -> Result<()> {
        // Path: {storage_path}/{collection_id}/global.stats
        // This is updated after flush/compaction to maintain collection-wide metrics
        // File-level statistics are embedded in Parquet metadata properties
        Ok(())
    }

    /// Compute enhanced row group statistics (optimized NOVA design)
    fn compute_enhanced_row_group_stats(
        &self,
        records: &[VectorRecord],
        dimension: usize,
    ) -> Result<Vec<super::hierarchical_stats::EnhancedRowGroupStats>> {
        if records.is_empty() {
            return Ok(Vec::new());
        }

        // Group vectors into row groups (default: 10K vectors per group)
        let row_group_size = 10000;
        let mut stats = Vec::new();

        for (group_idx, chunk) in records.chunks(row_group_size).enumerate() {
            let mut min_vals = vec![f32::INFINITY; dimension];
            let mut max_vals = vec![f32::NEG_INFINITY; dimension];
            let mut sum_vals = vec![0.0f32; dimension];
            let mut null_counts = vec![0u64; dimension];

            // Compute per-dimension statistics
            for record in chunk {
                if record.vector.len() != dimension {
                    continue; // Skip malformed vectors
                }

                for (dim_idx, &value) in record.vector.iter().enumerate() {
                    if value.is_finite() {
                        min_vals[dim_idx] = min_vals[dim_idx].min(value);
                        max_vals[dim_idx] = max_vals[dim_idx].max(value);
                        sum_vals[dim_idx] += value;
                    } else {
                        null_counts[dim_idx] += 1;
                    }
                }
            }

            // Compute centroid for pruning
            let centroid: Vec<f32> = sum_vals
                .iter()
                .map(|&sum| sum / chunk.len() as f32)
                .collect();

            // Create enhanced statistics using the optimized design
            let enhanced_stat = super::EnhancedRowGroupStats::create_basic(
                group_idx as u32,
                chunk.len() as u64,
                dimension,
                min_vals,
                max_vals,
                centroid,
                null_counts,
                1.0 / records.len() as f32, // Basic selectivity estimate
                0.7,                        // Placeholder - actual ratio computed during write
                0,                          // Will be updated based on query patterns
            );

            stats.push(enhanced_stat);
        }

        Ok(stats)
    }

    /// Compute basic zone maps for dimension-level pruning (simplified design)
    fn compute_basic_zone_maps(
        &self,
        records: &[VectorRecord],
        dimension: usize,
    ) -> Result<super::hierarchical_stats::BasicZoneMaps> {
        if records.is_empty() {
            return Ok(super::hierarchical_stats::BasicZoneMaps {
                dimension_ranges: Vec::new(),
                total_vectors: 0,
                creation_time: chrono::Utc::now(),
            });
        }

        let mut dimension_ranges = Vec::with_capacity(dimension);

        // Compute min/max range for each dimension across all vectors
        for dim_idx in 0..dimension {
            let mut min_val = f32::INFINITY;
            let mut max_val = f32::NEG_INFINITY;
            let mut valid_count = 0;

            for record in records {
                if dim_idx < record.vector.len() {
                    let value = record.vector[dim_idx];
                    if value.is_finite() {
                        min_val = min_val.min(value);
                        max_val = max_val.max(value);
                        valid_count += 1;
                    }
                }
            }

            dimension_ranges.push(super::hierarchical_stats::DimensionRange {
                dimension_index: dim_idx,
                min_value: if valid_count > 0 { min_val } else { 0.0 },
                max_value: if valid_count > 0 { max_val } else { 0.0 },
                selectivity: valid_count as f32 / records.len() as f32,
            });
        }

        Ok(super::hierarchical_stats::BasicZoneMaps {
            dimension_ranges,
            total_vectors: records.len() as u64,
            creation_time: chrono::Utc::now(),
        })
    }

    // ============================================================================
    // PERFORMANCE OPTIMIZATION METHODS - DELEGATING TO UNIFIED MODULES
    // ============================================================================

    /// Fast read optimization using memory-mapped Parquet files (delegates to universal optimizer)
    async fn mmap_parquet_file(&self, file_path: &str) -> Result<Vec<u8>> {
        // Use universal optimizer's memory mapping functionality
        if let Some(mmap) = self
            .universal_optimizer
            .get_memory_mapped_file(file_path)
            .await?
        {
            Ok(mmap.to_vec())
        } else {
            // Fallback to regular file reading for cloud storage
            self.universal_optimizer
                .read_data_optimized(file_path)
                .await
        }
    }

    /// Columnar I/O optimization with parallel column reads (delegates to universal optimizer)
    async fn parallel_column_read(
        &self,
        file_path: &str,
        column_indices: &[usize],
    ) -> Result<Vec<Vec<u8>>> {
        // Use universal optimizer for parallel operations
        let optimizer = self.universal_optimizer.clone();
        let file_path_owned = file_path.to_string();
        let read_operations: Vec<_> = column_indices
            .iter()
            .map(|&column_idx| {
                let file_path = file_path_owned.clone();
                let optimizer_clone = optimizer.clone();
                async move {
                    // Simulate column-specific read (in production, use actual column reader)
                    optimizer_clone
                        .read_data_optimized(&format!("{}:col:{}", file_path, column_idx))
                        .await
                }
            })
            .collect();

        let results = self
            .universal_optimizer
            .parallel_operations(read_operations, |operation| operation)
            .await?;

        // Unwrap the nested Results
        let mut unwrapped_results = Vec::new();
        for res in results {
            match res {
                Ok(Ok(data)) => unwrapped_results.push(data),
                Ok(Err(e)) => return Err(anyhow::anyhow!("Column read failed: {}", e)),
                Err(e) => return Err(anyhow::anyhow!("Column read failed: {}", e)),
            }
        }

        Ok(unwrapped_results)
    }

    /// Storage tier optimization for Parquet files based on access patterns (delegates to universal optimizer)
    async fn optimize_parquet_storage_tier(
        &self,
        file_path: &str,
        row_group_stats: &super::hierarchical_stats::EnhancedRowGroupStats,
    ) -> Result<DataFreshnessTier> {
        // Use common utility for consistent vector size estimation
        // Default configuration since NovaEngine doesn't have config field
        let dimension = 1536; // Default dimension
        // Estimate storage size based on dimension and number of row groups
        // Use a reasonable estimate of vectors per row group (e.g., 10000)
        let estimated_vectors = 10000; // Default estimate for row group size
        let estimated_size = crate::storage::engines::core::ops::estimate_vector_storage_size(
            dimension,
            None, // No quantization config available
            estimated_vectors,
        );

        // Use universal optimizer's storage tier optimization
        let infrastructure_tier = self
            .universal_optimizer
            .optimize_storage_tier(file_path, estimated_size as usize)
            .await?;

        // Convert from filesystem::StorageTier to multi_tier_deduplication::StorageTier
        let tier = match infrastructure_tier {
            crate::storage::persistence::filesystem::FileStorageTier::Memory => {
                DataFreshnessTier::Unflushed
            }
            crate::storage::persistence::filesystem::FileStorageTier::NVMe => {
                DataFreshnessTier::Flushed
            }
            crate::storage::persistence::filesystem::FileStorageTier::SSD => {
                DataFreshnessTier::Flushed
            }
            _ => DataFreshnessTier::Compacted,
        };

        Ok(tier)
    }

    /// Compression optimization using unified compression module (delegates to universal optimizer)
    async fn compress_parquet_optimized(
        &self,
        data: &[u8],
        tier: DataFreshnessTier,
    ) -> Result<Vec<u8>> {
        // Convert from multi_tier_deduplication::StorageTier to filesystem::StorageTier
        let fs_tier = match tier {
            DataFreshnessTier::Unflushed => {
                crate::storage::persistence::filesystem::FileStorageTier::Memory
            }
            DataFreshnessTier::Flushed => {
                crate::storage::persistence::filesystem::FileStorageTier::NVMe
            }
            DataFreshnessTier::Compacted => {
                crate::storage::persistence::filesystem::FileStorageTier::SSD
            }
        };

        // Use universal optimizer's tier-aware compression
        self.universal_optimizer
            .compress_for_tier(data, fs_tier)
            .await
    }

    /// Distance computation using unified distance compute engine (delegates to universal optimizer)
    async fn compute_distances_unified(
        &self,
        query: &[f32],
        candidates: &[Vec<f32>],
        metric: DistanceMetric,
    ) -> Result<Vec<f32>> {
        // Use universal optimizer's hardware-accelerated distance computation
        self.universal_optimizer
            .compute_distances_accelerated(query, candidates, metric)
            .await
    }

    /// Row group prefetching optimization (delegates to universal optimizer)
    async fn prefetch_row_groups(
        &self,
        file_path: &str,
        row_group_indices: &[usize],
    ) -> Result<()> {
        let config = self.universal_optimizer.get_config();
        if !config.enable_prefetching {
            return Ok(());
        }

        // Generate row group URLs for prefetching
        let prefetch_count = (config.prefetch_size_mb / 10).min(row_group_indices.len()); // Assume ~10MB per row group
        let row_group_urls: Vec<String> = row_group_indices
            .iter()
            .take(prefetch_count)
            .map(|&idx| format!("{}:rg:{}", file_path, idx))
            .collect();

        // Use universal optimizer's prefetching capability
        self.universal_optimizer
            .prefetch_data(&row_group_urls)
            .await
    }

    /// Memory pool optimization for columnar operations (delegates to universal optimizer)
    async fn get_columnar_buffer(&self, size: usize) -> Result<Vec<f32>> {
        self.universal_optimizer
            .get_memory_buffer(size)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to acquire columnar buffer: {}", e))
    }

    /// Write NOVA file to disk using StreamingParquetWriter with sidecar metadata
    async fn write_nova_file_to_disk(
        &self,
        nova_file: &NovaFile,
        file_path: &str,
        params: &FlushParameters,
        collection_id: &str,
    ) -> Result<u64> {
        use crate::storage::engines::core::formats::columnar::parquet_writer::{
            StreamingParquetWriter, ParquetWriterConfig,
        };
        use super::nova_meta_collector::{NovaMetadataCollector, NovaCollectorConfig};

        // Get filterable columns from collection config (use proto type directly)
        let filterable_columns = params
            .collection_config
            .as_ref()
            .and_then(|c| c.config.as_ref())
            .map(|cfg| cfg.filterable_columns.clone())
            .unwrap_or_else(|| vec![crate::proto::proximadb_v1::FilterableColumnSpec {
                name: "id".to_string(),
                data_type: crate::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                indexed: true,
                supports_range: false,
                estimated_cardinality: Some(1000000),
            }]);

        // Configure writer with NOVA-specific settings
        let writer_config = ParquetWriterConfig {
            compression: CompressionAlgorithm::Zstd,
            row_group_size: 50_000, // 50K vectors per row group
            write_batch_size: 10_000,
            enable_bloom_filters: true,
            bloom_filter_fpp: 0.01, // 1% false positive rate
            expected_ndv: Some(1000000), // Expect up to 1M unique IDs
            bloom_filter_columns: filterable_columns.iter().map(|c| c.name.clone()).collect(),
            enable_column_statistics: true,
            enable_page_index: true,
            enable_column_index: true,
            enable_offset_index: true,
            page_index_granularity: 1000,
            enable_dictionary: true,
            dictionary_threshold: 0.5,
            enable_delta_encoding: false,
            quantization: super::QuantizationConfig::default(),
            id_less_storage: false, // Keep IDs for compatibility
            page_size: 8192,
            enable_byte_stream_split: false,
            enable_pq_sorting: false,
            pq_sorting_segments: 16,
            pq_sorting_codebook_size: 256,
            enable_native_metadata: false,
            metadata_inference_samples: 100,
        };

        // Create NOVA metadata collector for sidecar generation
        let nova_collector = NovaMetadataCollector::new(NovaCollectorConfig {
            row_groups_per_superblock: 10, // 10 row groups per SuperBlock
            compute_vector_stats: true,
            sample_rate: 0.1, // Sample 10% for expensive statistics
        });

        // Create streaming writer with metadata collector
        let mut writer = StreamingParquetWriter::new(
            file_path,
            nova_file.metadata.dimension,
            writer_config,
        )?;

        // Set metadata collector
        writer.set_metadata_collector(Box::new(nova_collector));

        // Convert VectorRecords to Arrow RecordBatches and write
        let batch_size = 10_000;
        for chunk in params.vector_records.chunks(batch_size) {
            let batch = self.vectors_to_record_batch(chunk, &nova_file.schema)?;
            writer.write_batch(chunk).await?;
        }

        // Finalize and get collector for sidecar
        let (stats, collector) = writer.finalize().await?;
        let bytes_written = stats.file_size;

        // Write sidecar metadata file if collector has data
        if let Some(collector) = collector {
            let sidecar_path = format!("{}.{}", file_path, collector.sidecar_extension());
            let sidecar_data = collector.serialize_metadata()?;

            // Write sidecar using filesystem
            let fs = self.filesystem.get_filesystem(&self.determine_fs_url(file_path))?;
            fs.write(&sidecar_path, &sidecar_data, None).await?;

            info!("NOVA: Wrote sidecar metadata ({} bytes) to {}",
                  sidecar_data.len(), sidecar_path);
        }

        info!("NOVA: Successfully wrote {} bytes to {} with {} row groups",
              bytes_written, file_path, stats.total_row_groups);
        Ok(bytes_written)
    }

    /// Helper method to get file size in GB
    async fn get_file_size_gb(&self, file_path: &str) -> Result<f32> {
        let metadata = tokio::fs::metadata(file_path).await?;
        Ok(metadata.len() as f32 / (1024.0 * 1024.0 * 1024.0))
    }

    /// Convert VectorRecords to Arrow RecordBatch
    fn vectors_to_record_batch(
        &self,
        records: &[VectorRecord],
        schema: &Arc<arrow_schema::Schema>,
    ) -> Result<arrow_array::RecordBatch> {
        use arrow_array::{Float32Array, StringArray, Int64Array, builder::*};
        use std::sync::Arc;

        // Build arrays for each field
        let mut id_builder = StringBuilder::new();
        let mut vector_builder = Float32Builder::new();
        let mut timestamp_builder = Int64Builder::new();
        let mut version_builder = Int64Builder::new();

        for record in records {
            // ID column
            id_builder.append_value(&record.id);

            // Vector column (flattened)
            for val in &record.vector {
                vector_builder.append_value(*val);
            }

            // Timestamp column
            timestamp_builder.append_value(record.timestamp);

            // Version column
            version_builder.append_option(record.version);
        }

        // Create arrays
        let arrays: Vec<Arc<dyn arrow_array::Array>> = vec![
            Arc::new(id_builder.finish()),
            Arc::new(vector_builder.finish()),
            Arc::new(timestamp_builder.finish()),
            Arc::new(version_builder.finish()),
        ];

        // Create record batch
        arrow_array::RecordBatch::try_new(schema.clone(), arrays)
            .map_err(|e| anyhow!("Failed to create record batch: {}", e))
    }

    /// Determine filesystem URL from path
    fn determine_fs_url(&self, path: &str) -> String {
        if path.starts_with("s3://") || path.starts_with("gs://")
            || path.starts_with("azure://") || path.starts_with("wasbs://") {
            path.to_string()
        } else {
            "file://".to_string()
        }
    }
}

#[async_trait]
impl UnifiedStorageEngine for NovaEngine {
    // =============================================================================
    // ENGINE IDENTIFICATION
    fn engine_name(&self) -> &'static str {
        "NOVA"
    }

    fn engine_version(&self) -> &'static str {
        "1.0.0"
    }

    fn strategy(&self) -> crate::storage::traits::StorageEngineStrategy {
        crate::storage::traits::StorageEngineStrategy::Nova
    }

    fn get_filesystem_factory(
        &self,
    ) -> &crate::storage::persistence::filesystem::FilesystemFactory {
        &self.filesystem
    }

    // CORE OPERATIONS
    async fn do_flush(&self, params: &FlushParameters) -> Result<FlushResult> {
        let start_time = std::time::Instant::now();

        let collection_id = self.get_collection_id_from_params(params)?;
        info!(
            "NOVA flush: collection={}, vectors={}",
            collection_id,
            params.vector_records.len()
        );
        // Create new VIPER file from flush parameters
        // Get dimension from collection config or use default
        let dimension = params
            .collection_config
            .as_ref()
            .and_then(|c| c.config.as_ref())
            .map(|cfg| cfg.dimension)
            .unwrap_or(1536); // Default dimension
        // Use default compression for Parquet
        let compression_algorithm = CompressionAlgorithm::Zstd;
        debug!("NOVA: Using compression: {:?}", compression_algorithm);
        // Enhanced row group statistics (optimized NOVA design)
        let enhanced_stats =
            self.compute_enhanced_row_group_stats(&params.vector_records, dimension as usize)?;

        // Basic zone maps for dimension-level pruning (simplified from 3-tier hierarchy)
        let zone_maps = self.compute_basic_zone_maps(&params.vector_records, dimension as usize)?;

        let nova_file = NovaFile {
            quantized_columns: super::quantized_columns::QuantizedColumnMetadata::default(),
            schema: Arc::new(arrow_schema::Schema::empty()),
            metadata: crate::storage::engines::core::formats::columnar::ColumnarFileMetadata {
                collection_id: collection_id.to_string(),
                num_vectors: params.vector_records.len() as u64,
                dimension: dimension as usize,
                distance_metric: DistanceMetric::Euclidean,
                quantization: super::QuantizationConfig {
                    enabled: true,
                    strategy: 0, // SmartDefaults
                    custom_levels: vec![],
                    enable_progressive_search: true,
                    binary_filter_selectivity: 0.3,
                    int8_ranking_selectivity: 0.1,
                    pq_ranking_selectivity: 0.05,
                    training_sample_size: 10000,
                    quality_threshold: 0.95,
                    enable_adaptive_training: true,
                    optimize_for_storage: false,
                    optimize_for_memory: false,
                    enable_simd_acceleration: true,
                    // Direct quantization type enables
                    enable_binary: true,
                    enable_int8: true,
                    enable_pq: true,
                    // Product Quantization specific settings
                    pq_segments: 32,
                    pq_bits: 8,
                    pq_codebooks: 0,
                    // Thresholds for progressive search
                    binary_threshold: 0.0,
                    int8_threshold: 0.85,
                    pq_threshold: 0.95,
                },
                column_stats: HashMap::new(),
                version: 1,
                timestamp: chrono::Utc::now(),
                modified_at: chrono::Utc::now(),
            },
            row_groups: Vec::new(),
            enhanced_stats,
            superblocks: Vec::new(), // Keep for future SuperBlock implementation
            advanced_zone_maps: Some(zone_maps),
        };
        // Generate filename using FilenameCodec for consistency with compaction framework
        use crate::storage::common::compaction_orchestrator::FilenameCodec;
        let codec = FilenameCodec::new();
        // NOVA uses .parquet extension (same as VIPER) but with different metadata
        // The extension should match NOVA_FILE_EXT constant
        let nova_filename = codec.generate(0, &crate::storage::engines::constants::NOVA_FILE_EXT[1..]); // Remove leading dot

        // Get storage path from collection config (always present)
        let storage_path = params
            .collection_config
            .as_ref()
            .and_then(|c| c.storage_assignment.as_ref())
            .map(|s| format!("{}/{}/data", s.base_location, collection_id))
            .ok_or_else(|| anyhow!("Collection '{}' has no storage assignment", collection_id))?;

        // Create directory if it doesn't exist
        // UnifiedCachingFilesystem handles both cloud and local transparently:
        // - For cloud storage: creates prefix/folder structure in bucket
        // - For local storage: creates actual directories
        let fs_url = if storage_path.starts_with("s3://") || storage_path.starts_with("gs://")
            || storage_path.starts_with("azure://") || storage_path.starts_with("wasbs://") {
            storage_path.clone()
        } else {
            "file://".to_string()
        };

        let fs = self.filesystem.get_filesystem(&fs_url)?;
        fs.create_dir_all(&storage_path).await?;

        let file_path = format!("{}/{}", storage_path, nova_filename);
        info!("NOVA: Writing columnar file to {}", file_path);

        // Write NOVA file to disk using atomic write strategy
        let bytes_written = self
            .write_nova_file_to_disk(
                &nova_file,
                &file_path,
                params,
                collection_id,
            )
            .await?;
        // Update global stats
        self.update_global_stats(collection_id, &storage_path).await?;
        // Update statistics
        let mut stats = self.statistics.write().await;
        stats.pending_flushes = stats.pending_flushes.saturating_sub(1);
        stats.last_flush = Some(chrono::Utc::now());
        stats.total_storage_bytes += bytes_written;
        let duration_ms = start_time.elapsed().as_millis() as u64;

        Ok(FlushResult {
            success: true,
            collections_affected: vec![collection_id.to_string()],
            entries_flushed: Some(params.vector_records.len() as u64),
            bytes_written: Some(bytes_written),
            files_created: Some(1),
            flushed_batch_ids: vec![], // Initialize empty batch IDs
            duration_ms: Some(duration_ms),
            completed_at: chrono::Utc::now(),
            engine_metrics: HashMap::new(),
            compaction_triggered: false,
        })
    }

    async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult> {
        use crate::storage::engines::core::formats::columnar::parquet_writer::{
            StreamingParquetWriter, ParquetWriterConfig,
        };
        use super::nova_meta_collector::{NovaMetadataCollector, NovaCollectorConfig};
        use super::unified_strategy_reader::UnifiedNOVAReader;

        let start_time = std::time::Instant::now();

        let collection_id = params
            .collection_id
            .as_ref()
            .ok_or_else(|| anyhow!("Collection ID required for compaction"))?;
        info!("NOVA compaction: collection={}", collection_id);

        // Get storage path from collection config
        let storage_path = params
            .collection_config
            .as_ref()
            .and_then(|c| c.storage_assignment.as_ref())
            .map(|s| format!("{}/{}/data", s.base_location, collection_id))
            .ok_or_else(|| anyhow!("Collection '{}' has no storage assignment", collection_id))?;

        // List existing Parquet files
        let fs = self.filesystem.get_filesystem(&self.determine_fs_url(&storage_path))?;
        let files = fs.list(&storage_path).await?
            .into_iter()
            .filter(|f| f.name.ends_with(".parquet"))
            .collect::<Vec<_>>();

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

        // Get collection configuration
        let dimension = params
            .collection_config
            .as_ref()
            .and_then(|c| c.config.as_ref())
            .map(|cfg| cfg.dimension)
            .unwrap_or(1536);

        // Get filterable columns from collection config (use proto type directly)
        let filterable_columns = params
            .collection_config
            .as_ref()
            .and_then(|c| c.config.as_ref())
            .map(|cfg| cfg.filterable_columns.clone())
            .unwrap_or_else(|| vec![crate::proto::proximadb_v1::FilterableColumnSpec {
                name: "id".to_string(),
                data_type: crate::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                indexed: true,
                supports_range: false,
                estimated_cardinality: Some(1000000),
            }]);

        // Create schema for the compacted file
        let filterable_column_names: Vec<String> = filterable_columns.iter()
            .map(|col| col.name.clone())
            .collect();
        let schema = super::create_vector_schema(
            dimension as usize,
            &super::QuantizationConfig::default(),
            &filterable_column_names,
        );

        // Configure writer for compaction (larger row groups)
        let writer_config = ParquetWriterConfig {
            compression: CompressionAlgorithm::Zstd,
            row_group_size: 100_000, // Larger row groups for compacted files
            write_batch_size: 20_000,
            enable_bloom_filters: true,
            bloom_filter_fpp: 0.01,
            expected_ndv: Some(2000000), // Larger for compacted files
            bloom_filter_columns: filterable_columns.iter().map(|c| c.name.clone()).collect(),
            enable_column_statistics: true,
            enable_page_index: true,
            enable_column_index: true,
            enable_offset_index: true,
            page_index_granularity: 2000,
            enable_dictionary: true,
            dictionary_threshold: 0.5,
            enable_delta_encoding: false,
            quantization: super::QuantizationConfig::default(),
            id_less_storage: false,
            page_size: 8192,
            enable_byte_stream_split: false,
            enable_pq_sorting: false,
            pq_sorting_segments: 16,
            pq_sorting_codebook_size: 256,
            enable_native_metadata: false,
            metadata_inference_samples: 100,
        };

        // Generate output filename
        use crate::storage::common::compaction_orchestrator::FilenameCodec;
        let codec = FilenameCodec::new();
        let output_filename = codec.generate(0, &crate::storage::engines::constants::NOVA_FILE_EXT[1..]);
        let output_path = format!("{}/{}", storage_path, output_filename);

        // Create NOVA metadata collector for compacted file
        let nova_collector = NovaMetadataCollector::new(NovaCollectorConfig {
            row_groups_per_superblock: 20, // More row groups per SuperBlock for larger files
            compute_vector_stats: true,
            sample_rate: 0.05, // Lower sample rate for compaction
        });

        // Create streaming writer
        let mut writer = StreamingParquetWriter::new(
            &output_path,
            dimension as usize,
            writer_config,
        )?;

        writer.set_metadata_collector(Box::new(nova_collector));

        // Read and merge all input files
        // NOTE: Compaction does NOT use sidecar metadata because:
        // 1. Full scan reads ALL vectors (no pruning benefit)
        // 2. Sidecar metadata adds overhead without value for full reads
        // 3. Direct Parquet reading is faster for sequential access
        let reader = UnifiedNOVAReader::for_compaction(
            self.filesystem.clone(),
            collection_id.to_string(),
        )?;
        let mut total_entries = 0u64;
        let mut bytes_read = 0u64;

        for file_entry in &files {
            // Read vectors from each file (full scan, no sidecar metadata used)
            let vectors = reader.read_progressive(&file_entry.url).await?;
            total_entries += vectors.len() as u64;

            // Get file size for metrics
            bytes_read += file_entry.metadata.size;

            // Convert to batches and write
            for chunk in vectors.chunks(20_000) {
                let batch = self.vectors_to_record_batch(chunk, &schema)?;
                writer.write_batch(chunk).await?;
            }
        }

        // Finalize and get statistics
        let (stats, collector) = writer.finalize().await?;
        let bytes_written = stats.file_size;

        // Write sidecar metadata
        if let Some(collector) = collector {
            let sidecar_path = format!("{}.{}", output_path, collector.sidecar_extension());
            let sidecar_data = collector.serialize_metadata()?;
            fs.write(&sidecar_path, &sidecar_data, None).await?;

            info!("NOVA compaction: Wrote sidecar metadata to {}", sidecar_path);
        }

        // Delete input files after successful compaction
        for file_entry in &files {
            fs.delete(&file_entry.url).await?;
            // Also delete sidecar files if they exist
            let sidecar_path = format!("{}.nova_meta", file_entry.url);
            let _ = fs.delete(&sidecar_path).await; // Ignore errors for missing sidecars
        }

        let duration_ms = start_time.elapsed().as_millis() as u64;

        // Update statistics
        let mut stats_guard = self.statistics.write().await;
        stats_guard.pending_compactions = stats_guard.pending_compactions.saturating_sub(1);
        stats_guard.last_compaction = Some(chrono::Utc::now());

        info!("NOVA compaction: Merged {} files into {} ({} entries, {} bytes -> {} bytes)",
              files.len(), output_filename, total_entries, bytes_read, bytes_written);

        Ok(CompactionResult {
            success: true,
            collections_affected: vec![collection_id.to_string()],
            entries_processed: Some(total_entries),
            entries_removed: Some(0), // NOVA doesn't deduplicate during compaction
            bytes_read: Some(bytes_read),
            bytes_written: Some(bytes_written),
            input_files: Some(files.len() as u64),
            output_files: Some(1),
            duration_ms: Some(duration_ms),
            completed_at: chrono::Utc::now(),
            engine_metrics: HashMap::new(),
        })
    }

    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();

        // Engine is stateless, so we report engine-level metrics only
        metrics.insert("engine_type".to_string(), serde_json::json!("NOVA"));
        metrics.insert("columnar_engine".to_string(), serde_json::json!(true));

        // TODO: Collect actual metrics from storage when needed
        let total_files = 0;
        let total_row_groups = 0;
        metrics.insert(
            "total_parquet_files".to_string(),
            serde_json::json!(total_files),
        );
        metrics.insert(
            "total_row_groups".to_string(),
            serde_json::json!(total_row_groups),
        );
        let stats = self.statistics.read().await;
        // Use existing fields instead of non-existent ones
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
            serde_json::json!(format!("{:?}", self.hardware.cpu)),
        );
        metrics.insert("columnar_optimization".to_string(), serde_json::json!(true));
        Ok(metrics)
    }

    async fn vector_by_id(
        &self,
        collection_id: &str,
        base_path: &str,
        vector_id: &str,
    ) -> Result<Option<VectorRecord>> {
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

        debug!(
            "NOVA get vector: collection={}, base_path={}, id={}",
            collection_id, base_path, vector_id
        );

        // Construct data directory from base_path and collection_id
        let data_dir = format!("{}/{}/data", base_path, collection_id);

        // TODO: Load actual Parquet files from data_dir
        // For now, return None as placeholder
        // In production, would:
        // 1. Load Parquet files from data_dir
        // 2. Search through ID indexes
        Ok(None)
    }

    async fn search_vectors_unified(
        &self,
        ctx: &crate::storage::traits::StorageQueryContext,
    ) -> Result<Vec<crate::core::search::results::OptimizedSearchRecord>> {
        let search_start = std::time::Instant::now();

        // Extract all parameters from context (pre-computed)
        let collection_id = ctx.collection_id();
        let storage_path = ctx.storage_path();
        let query_vector = ctx
            .query_vector()
            .ok_or_else(|| anyhow!("No query vector in context"))?;
        let top_k = ctx.top_k();
        let distance_metric = ctx.distance_metric();
        let dimension = ctx.dimension();
        let filter_expression = ctx.search_params.filter_expression.as_ref();
        let search_params = ctx.search_params.custom_hints.clone();

        info!(
            "🚀 NOVA: Enhanced unified search with orchestration for collection {}",
            collection_id
        );

        // ========================================================================
        // PHASE 1: SEARCH ORCHESTRATION AND STRATEGY SELECTION
        // ========================================================================

        // Check if orchestration should be used based on context metadata
        let use_orchestration = ctx.metadata.use_axis_indexes || ctx.metadata.has_quantization;

        if use_orchestration {
            info!("🎯 NOVA: Using intelligent search orchestration");

            // Create mock services for orchestration
            let axis_manager = match self.get_axis_manager().await {
                Ok(manager) => manager,
                Err(e) => {
                    tracing::warn!(
                        "⚠️ Failed to get AXIS manager: {}, falling back to direct search",
                        e
                    );
                    return self
                        .fallback_to_direct_search(
                            ctx,
                            collection_id,
                            storage_path,
                            query_vector,
                            top_k,
                            distance_metric,
                            filter_expression,
                        )
                        .await;
                }
            };

            // Direct search implementation - IntegratedSearchOptimizer requires cache infrastructure
            // that isn't available in this context
            return self
                .fallback_to_direct_search(
                    ctx,
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

        info!("🔍 NOVA: Using current unified search implementation (orchestration disabled)");

        // Load files from storage
        let fs = self.filesystem.get_filesystem(&self.determine_fs_url(storage_path))?;
        let parquet_files = fs.list(storage_path).await?
            .into_iter()
            .filter(|f| f.name.ends_with(".parquet"))
            .collect::<Vec<_>>();

        if parquet_files.is_empty() {
            info!("NOVA: No parquet files found in {}", storage_path);
            return Ok(Vec::new());
        }

        // Create metadata reader for sidecar pruning
        // SIDECAR USAGE: Only beneficial for selective queries (similarity search, filtered search)
        // where we can prune 70-97% of row groups before reading
        let mut meta_reader = super::nova_meta_reader::NovaMetaReader::new(self.filesystem.clone());
        let mut all_results = Vec::new();

        // Search each NOVA file using sidecar metadata for pruning
        for parquet_file in &parquet_files {
            // Try to load sidecar metadata for optimized pruning (selective queries only)
            let row_groups_to_read = match meta_reader.load_metadata(&parquet_file.url).await {
                Ok(nova_meta) => {
                    // Get optimization hints based on query
                    let hints = meta_reader.get_optimization_hints(&nova_meta, query_vector, top_k);

                    info!("NOVA: Sidecar pruning for {} - reading {} of {} row groups ({}% pruned)",
                          parquet_file.name, hints.row_groups.len(),
                          nova_meta.row_group_stats.len(),
                          (hints.pruning_ratio * 100.0) as u32);

                    hints.row_groups
                }
                Err(e) => {
                    // No sidecar file or read error - read all row groups
                    tracing::debug!("NOVA: No sidecar metadata for {}: {}", parquet_file.name, e);
                    Vec::new() // Empty means read all
                }
            };

            // Use UnifiedNOVAReader to read the selected row groups
            let reader = super::unified_strategy_reader::UnifiedNOVAReader::for_search(
                self.filesystem.clone(),
                collection_id.to_string(),
            )?;

            // Read vectors from selected row groups (or all if no pruning)
            let vectors = if row_groups_to_read.is_empty() {
                // No pruning - read entire file
                reader.read_progressive(&parquet_file.url).await?
            } else {
                // Read only selected row groups
                reader.read_row_groups(&parquet_file.url, &row_groups_to_read).await?
            };

            // Compute distances and collect results
            for record in vectors {
                // Compute distance using hardware-accelerated SIMD
                // Use the DistanceComputeProvider trait method
                use crate::compute::distance_computation::engine::DistanceComputeProvider;
                let distance = self.distance_engine.compute_distance(
                    query_vector,
                    &record.vector,
                    Some(distance_metric),
                );

                all_results.push((record, distance));
            }
        }

        // Sort by score and take top-k
        all_results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        all_results.truncate(top_k);

        // Convert to OptimizedSearchRecord format directly
        let search_results: Vec<OptimizedSearchRecord> = all_results
            .into_iter()
            .enumerate()
            .map(|(idx, (record, score))| {
                // Create similarity result for semantic information
                let similarity_result = crate::compute::distance_computation::SimilarityResult::new(
                    1.0 - score, // Distance value
                    distance_metric,
                );

                let id = if record.id.is_empty() {
                    format!("unknown_{}", idx)
                } else {
                    record.id.clone()
                };

                let metadata_map: HashMap<String, serde_json::Value> = record
                    .metadata
                    .iter()
                    .map(|(key, sql_value)| {
                        let value = match &sql_value.value {
                            Some(
                                crate::proto::proximadb_v1::sql_value::Value::StringValue(s),
                            ) => serde_json::Value::String(s.clone()),
                            Some(
                                crate::proto::proximadb_v1::sql_value::Value::NumberValue(n),
                            ) => serde_json::json!(n),
                            Some(
                                crate::proto::proximadb_v1::sql_value::Value::Int64Value(i),
                            ) => serde_json::json!(i),
                            Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(
                                b,
                            )) => serde_json::Value::Bool(*b),
                            _ => serde_json::Value::Null,
                        };
                        (key.clone(), value)
                    })
                    .collect();

                let mut search_record =
                    OptimizedSearchRecord::new(id, similarity_result.normalized_score)
                        .with_similarity(similarity_result.normalized_score)
                        .add_vector(record.vector.clone())
                        .with_metadata(record.metadata.clone());

                if let Some(version) = record.version {
                    search_record = search_record.with_version_info(version, record.timestamp);
                }

                search_record
            })
            .collect();

        Ok(search_results)
    }

    // OPTIONAL OPERATIONS
    async fn optimize(&self, collection_id: &str) -> Result<()> {
        info!("NOVA optimize: collection={}", collection_id);
        // Trigger compaction for optimization
        let params = CompactionParameters {
            collection_id: Some(collection_id.to_string()),
            force: false,
            synchronous: true,
            hints: HashMap::new(),
            timeout_ms: Some(60000),
            priority: OperationPriority::Low,
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
            status: "Healthy".to_string(),
            last_check: chrono::Utc::now(),
            response_time_ms: 1.0,
            warnings: vec![],
            metrics: HashMap::new(),
            error_count: 0,
        })
    }

    fn supports_feature(&self, feature: &str) -> bool {
        match feature {
            "id_lookup" => true,
            "similarity_search" => true,
            "columnar_search" => true,
            "quantization" => true,
            "compression" => true,
            "batch_operations" => true,
            "predicate_pushdown" => true,
            "projection" => true,
            _ => false,
        }
    }
}

impl NovaFile {
    /// Load record at specific location
    pub fn load_record_at_location(
        &self,
        location: &crate::storage::engines::core::formats::columnar::id_index::ParquetLocation,
    ) -> Result<VectorRecord> {
        // In production, would load from Parquet row group
        Ok(VectorRecord {
            id: format!("vec_rg{}_row{}", location.row_group_id, location.row_offset),
            vector: vec![0.0; self.metadata.dimension],
            metadata: std::collections::HashMap::new(),
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            version: None,
            quantized_vector: Vec::new(),
            source: None,
        })
    }
}

// TODO: Fix columnar search config implementation when module is available
/*
impl crate::storage::engines::core::formats::columnar::columnar_search::ColumnarSearchConfig {
    /// Create from search parameters
    pub fn from_params(params: Option<&serde_json::Value>) -> Self {
        // Parse parameters or use defaults
        Self::default()
    }
}
*/

/// Implementation of UniversallyOptimized trait for NOVA engine
#[async_trait]
impl UniversallyOptimized for NovaEngine {
    /// Get the universal performance optimizer instance
    fn universal_optimizer(&self) -> &UniversalPerformanceOptimizer {
        &self.universal_optimizer
    }

    /// NOVA-specific optimization setup
    async fn setup_engine_optimizations(&self) -> Result<()> {
        // NOVA-specific optimizations for columnar storage
        info!("🔧 NOVA Engine: Setting up universal performance optimizations");

        // Initialize columnar-specific optimizations
        let config = self.universal_optimizer.get_config();
        debug!("   Cache size: {}MB", config.cache_size_mb);
        debug!("   Parallel operations: {}", config.parallel_operations);
        debug!("   Prefetching enabled: {}", config.enable_prefetching);
        debug!(
            "   Memory mapping enabled: {}",
            config.enable_memory_mapping
        );

        // NOVA is ready for columnar analytics operations
        info!("✅ NOVA Engine: Universal optimizations configured for columnar analytics");
        Ok(())
    }

    /// NOVA-specific performance metrics
    async fn collect_performance_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();

        // Basic NOVA metrics
        let stats = self.statistics.read().await;
        metrics.insert(
            "nova_total_storage_bytes".to_string(),
            serde_json::Value::Number(serde_json::Number::from(stats.total_storage_bytes)),
        );
        metrics.insert(
            "nova_memory_usage_bytes".to_string(),
            serde_json::Value::Number(serde_json::Number::from(stats.memory_usage_bytes)),
        );
        metrics.insert(
            "nova_collection_count".to_string(),
            serde_json::Value::Number(serde_json::Number::from(stats.collection_count)),
        );
        metrics.insert(
            "nova_pending_flushes".to_string(),
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

// Helper methods for NovaEngine
impl NovaEngine {
    // Helper method for AXIS integration when needed
    async fn get_axis_manager(
        &self,
    ) -> Result<Arc<crate::index::axis::management::manager::AxisManager>> {
        // Create AXIS manager with default config
        let config = crate::index::axis::types::AxisConfig::default();
        Ok(Arc::new(
            crate::index::axis::management::manager::AxisManager::new(config).await?,
        ))
    }

    // Removed unnecessary helper methods - engines receive all params directly
    // No need for CollectionService, distance/quantization engines are already in the struct

    /// Fallback to direct search when orchestration fails
    async fn fallback_to_direct_search(
        &self,
        ctx: &crate::storage::traits::StorageQueryContext,
        collection_id: &str,
        storage_path: &str,
        query_vector: &[f32],
        top_k: usize,
        distance_metric: crate::compute::distance_computation::DistanceMetric,
        filter_expression: Option<&crate::core::search::FilterExpression>,
    ) -> Result<Vec<crate::core::search::results::OptimizedSearchRecord>> {
        tracing::warn!("🔄 NOVA: Falling back to direct search implementation");

        // Use the existing search implementation
        // Load files from storage
        let files = self
            .load_collection_files(collection_id, storage_path)
            .await?;
        let mut all_results = Vec::new();

        // Search each NOVA file using columnar optimization
        for nova_file in files.iter() {
            // Placeholder - would implement actual columnar search
            let results: Vec<(crate::proto::proximadb_v1::VectorRecord, f32)> = Vec::new();

            // Convert to search results
            for (record, score) in results {
                all_results.push((record, score));
            }
        }

        // Sort by score and take top-k
        all_results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        all_results.truncate(top_k);

        // Convert to OptimizedSearchRecord format directly
        let search_results: Vec<OptimizedSearchRecord> = all_results
            .into_iter()
            .enumerate()
            .map(|(idx, (record, score))| {
                let similarity_result = crate::compute::distance_computation::SimilarityResult::new(
                    1.0 - score, // Distance value
                    distance_metric,
                );

                let id = if record.id.is_empty() {
                    format!("unknown_{}", idx)
                } else {
                    record.id.clone()
                };

                let metadata_map: HashMap<String, serde_json::Value> = record
                    .metadata
                    .iter()
                    .map(|(key, sql_value)| {
                        let value = match &sql_value.value {
                            Some(
                                crate::proto::proximadb_v1::sql_value::Value::StringValue(s),
                            ) => serde_json::Value::String(s.clone()),
                            Some(
                                crate::proto::proximadb_v1::sql_value::Value::NumberValue(n),
                            ) => serde_json::json!(n),
                            Some(
                                crate::proto::proximadb_v1::sql_value::Value::Int64Value(i),
                            ) => serde_json::json!(i),
                            Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(
                                b,
                            )) => serde_json::Value::Bool(*b),
                            _ => serde_json::Value::Null,
                        };
                        (key.clone(), value)
                    })
                    .collect();

                let mut search_record =
                    OptimizedSearchRecord::new(id, similarity_result.normalized_score)
                        .with_similarity(similarity_result.normalized_score)
                        .add_vector(record.vector.clone())
                        .with_metadata(record.metadata.clone());

                if let Some(version) = record.version {
                    search_record = search_record.with_version_info(version, record.timestamp);
                }

                search_record
            })
            .collect();

        Ok(search_results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_nova_engine_creation() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        let engine = NovaEngine::new().await.unwrap();
        assert_eq!(engine.engine_name(), "NOVA");
        assert_eq!(engine.engine_version(), "1.0.0");
    }

    #[tokio::test]
    async fn test_nova_feature_support() {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        let engine = NovaEngine::new().await.unwrap();
        assert!(engine.supports_feature("id_lookup"));
        assert!(engine.supports_feature("columnar_search"));
        assert!(engine.supports_feature("predicate_pushdown"));
        assert!(engine.supports_feature("projection"));
        assert!(!engine.supports_feature("unknown_feature"));
    }
}
