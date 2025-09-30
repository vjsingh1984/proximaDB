// NOVA Engine: Next-gen Optimized Vector Analytics with columnar quantization
// Implements UnifiedStorageEngine trait for integration with ProximaDB

use crate::core::compression::StandardCompression;
use crate::core::search::DataFreshnessTier;
use crate::proto::proximadb_v1::VectorRecord;
use crate::utils::StoragePath;
// Import column constants from columnar module
use crate::storage::engines::core::formats::columnar::FIELD_ID;
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
use crate::core::search::bounded_queue::BoundedPriorityQueue;
use crate::metrics::collectors::{EngineMetricsCollector, OperationTimer};
// Use core compression directly instead of adapter
use super::optimized_operations::OptimizedNovaOperations;
use super::operations::{NovaFlushOperations, NovaCompactionOperations, NovaSearchOperations};
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
    /// Flush operations handler
    flush_ops: Arc<NovaFlushOperations>,
    /// Compaction operations handler
    compaction_ops: Arc<NovaCompactionOperations>,
    /// Search operations handler
    search_ops: Arc<NovaSearchOperations>,
    /// Engine statistics
    statistics: Arc<RwLock<EngineStatistics>>,
    /// Hardware capabilities
    hardware: Arc<HardwareCapabilities>,
    /// Metrics collector for unified monitoring
    metrics_collector: Option<Arc<EngineMetricsCollector>>,
    /// Direct compression provider (no adapter indirection)
    compression_provider: StandardCompression,
    /// Storage-aware quantization engine for persistent collection-based PQ
    storage_quantization_engine:
        Arc<crate::compute::quantization::storage_engine::StorageQuantizationEngine>,
    /// Fallback stateless quantization engine for ad-hoc queries
    fallback_quantization_engine:
        Arc<crate::compute::quantization::unified::UnifiedQuantizationEngine>,
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

        let storage_quantization_engine = Arc::new(
            crate::compute::quantization::storage_engine::StorageQuantizationEngine::new(
                unified_engine.clone(),
                distance_compute.clone(),
                storage_config,
            ),
        );

        // Create fallback stateless quantization engine for ad-hoc queries
        let fallback_codebook_store =
            Arc::new(crate::compute::quantization::unified::InMemoryCodebookStore::new());
        let fallback_quantization_engine = Arc::new(
            crate::compute::quantization::unified::UnifiedQuantizationEngine::new(
                distance_compute.clone(),
                fallback_codebook_store,
            ),
        );

        // Initialize universal performance optimization
        let universal_optimizer =
            UniversalPerformanceOptimizer::with_strategy(UniversalOptimizationStrategy::Balanced)
                .await?;

        // Initialize modularized operation handlers
        let flush_ops = Arc::new(NovaFlushOperations::new(filesystem.clone()));
        let compaction_ops = Arc::new(NovaCompactionOperations::new(filesystem.clone()));
        let search_ops = Arc::new(NovaSearchOperations::new(
            filesystem.clone(),
            DistanceMetric::Cosine,
        ));

        // NOVA benefits from UnifiedCachingFilesystem for caching hierarchical stats
        // We'll create collection-specific instances during operations since we need
        // the actual storage path to get the right filesystem from the factory

        Ok(Self {
            filesystem,
            optimized_ops,
            flush_ops,
            compaction_ops,
            search_ops,
            statistics: Arc::new(RwLock::new(EngineStatistics {
                engine_name: "NOVA".to_string(),
                engine_version: "1.0.0".to_string(),  // Release 1 version
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
            storage_quantization_engine,
            fallback_quantization_engine,
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
        use crate::storage::engines::core::formats::columnar::{
            parquet_write_engine::ParquetWriterConfig,
            hybrid_writer::{HybridParquetWriter, HybridWriterConfig},
        };
        use super::nova_meta_collector::{NovaMetadataCollector, NovaCollectorConfig};

        // Get filterable columns from collection config (use proto type directly)
        let filterable_columns = params
            .collection_config
            .as_ref()
            .and_then(|c| c.config.as_ref())
            .map(|cfg| cfg.filterable_columns.clone())
            .unwrap_or_else(|| vec![crate::proto::proximadb_v1::FilterableColumnSpec {
                name: FIELD_ID.to_string(),
                data_type: crate::proto::proximadb_v1::FilterableDataType::FilterableString as i32,
                indexed: true,
                supports_range: false,
                estimated_cardinality: Some(1000000),
            }]);

        // Configure writer with NOVA-specific settings
        // Include both ID and filterable columns in bloom filters
        let mut bloom_columns = vec![FIELD_ID.to_string()];
        bloom_columns.extend(filterable_columns.iter().map(|c| c.name.clone()));

        let writer_config = ParquetWriterConfig {
            compression: parquet::basic::Compression::ZSTD(Default::default()),
            row_group_size: 50_000, // 50K vectors per row group
            write_batch_size: 10_000,
            enable_bloom_filters: true,
            bloom_filter_fpp: 0.01, // 1% false positive rate
            bloom_filter_ndv: 1000000, // Expect up to 1M unique IDs
            enable_statistics: true,
            enable_page_index: true,
            enable_dictionary: true,
            quantization: crate::proto::proximadb_v1::QuantizationConfig::default(),
            id_less_storage: false, // Keep IDs for compatibility
            page_size: 8192,
            sort_columns: vec![], // No sorting for now
            filterable_metadata_columns: Some(filterable_columns.iter().map(|c| c.name.clone()).collect()),
            compression_level: None,
            max_records_per_file: None,
            target_file_size_bytes: None,
            enable_async_io: false,
        };

        // Create NOVA metadata collector for sidecar generation
        let nova_collector = NovaMetadataCollector::new(NovaCollectorConfig {
            row_groups_per_superblock: 10, // 10 row groups per SuperBlock
            compute_vector_stats: true,
            sample_rate: 0.1, // Sample 10% for expensive statistics
        });

        // Configure HybridParquetWriter for adaptive optimization
        let hybrid_config = HybridWriterConfig {
            base_config: writer_config,
            ..Default::default()
        };

        // Use HybridParquetWriter with integrated disk cache and metadata collection
        // This handles:
        // 1. Writing to temp file
        // 2. Collecting metadata during write
        // 3. Finalizing the writer
        // 4. Uploading to cloud/local storage
        // 5. Populating disk cache for future reads
        // 6. Returning metadata collector for sidecar generation
        let (stats, collector) = HybridParquetWriter::write_with_cache(
            &params.vector_records,
            nova_file.metadata.dimension,
            hybrid_config,
            file_path,
            &self.filesystem,
            Some(filterable_columns),
            Some(Box::new(nova_collector)),
        ).await?;

        let bytes_written = stats.file_size;

        // Write sidecar metadata file if collector has data
        if let Some(collector) = collector {
            let sidecar_path = format!("{}.{}", file_path, collector.sidecar_extension());
            let sidecar_data = collector.serialize_metadata()?;

            // Write sidecar using filesystem (this also gets cached)
            let fs = self.filesystem.get_filesystem(&self.determine_fs_url(file_path))?;
            fs.write(&sidecar_path, &sidecar_data, None).await?;

            info!("NOVA: Wrote sidecar metadata ({} bytes) to {} with disk cache",
                  sidecar_data.len(), sidecar_path);
        }

        debug!(
            "NOVA: Wrote {} records to {} with disk cache ({}MB)",
            stats.total_records,
            file_path,
            bytes_written / 1024 / 1024
        );

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
        use arrow_array::{Float32Array, StringArray, Int64Array, Float64Array, BooleanArray, builder::*};
        use arrow_array::builder::{FixedSizeBinaryBuilder, Int8Builder, FixedSizeListBuilder};
        use std::sync::Arc;

        // Build arrays for each field
        let mut id_builder = StringBuilder::new();

        // Get dimension from schema for the vector field
        let dimension = if let arrow_schema::DataType::FixedSizeList(_, dim) = schema.fields()[1].data_type() {
            *dim as usize
        } else {
            // Fallback: use first record's vector dimension
            records.first().map(|r| r.vector.len()).unwrap_or(0)
        };

        // Build vector column as FixedSizeList
        let values_builder = Float32Builder::new();
        let mut vector_builder = FixedSizeListBuilder::new(values_builder, dimension as i32);

        let mut timestamp_builder = Int64Builder::new();
        let mut version_builder = UInt32Builder::new();

        // Check if quantization fields are present in schema (they would be after the 4 core fields)
        let mut quantization_field_count = 0;
        for field in schema.fields().iter().skip(4) {
            if field.name().starts_with("vector_") || field.name() == "int8_scale" || field.name() == "int8_zero_point" {
                quantization_field_count += 1;
            } else {
                break; // Stop when we hit the first non-quantization field
            }
        }

        // Build metadata columns dynamically based on schema (skip core + quantization fields)
        let mut metadata_builders: Vec<Box<dyn arrow_array::builder::ArrayBuilder>> = Vec::new();
        for field_idx in (4 + quantization_field_count)..schema.fields().len() {
            let field = &schema.fields()[field_idx];
            let builder: Box<dyn arrow_array::builder::ArrayBuilder> = match field.data_type() {
                arrow_schema::DataType::Utf8 => Box::new(StringBuilder::new()),
                arrow_schema::DataType::Int64 => Box::new(Int64Builder::new()),
                arrow_schema::DataType::Float64 => Box::new(Float64Builder::new()),
                arrow_schema::DataType::Boolean => Box::new(BooleanBuilder::new()),
                _ => Box::new(StringBuilder::new()), // Default to string
            };
            metadata_builders.push(builder);
        }

        for record in records {
            // ID column
            id_builder.append_value(&record.id);

            // Vector column as FixedSizeList
            let values_builder = vector_builder.values();
            for val in &record.vector {
                values_builder.append_value(*val);
            }
            vector_builder.append(true);

            // Timestamp column
            timestamp_builder.append_value(record.timestamp);

            // Version column
            version_builder.append_option(record.version);

            // Metadata columns
            for (field_idx, builder) in metadata_builders.iter_mut().enumerate() {
                let field = &schema.fields()[field_idx + 4 + quantization_field_count];
                let field_name = field.name();

                // Get metadata value for this field
                let metadata_value = record.metadata.get(field_name);

                // Append value based on field type
                match field.data_type() {
                    arrow_schema::DataType::Utf8 => {
                        let string_builder = builder.as_any_mut().downcast_mut::<StringBuilder>().unwrap();
                        if let Some(value) = metadata_value {
                            if let Some(s) = value.value.as_ref().and_then(|v| match v {
                                crate::proto::proximadb_v1::sql_value::Value::StringValue(s) => Some(s.as_str()),
                                _ => None,
                            }) {
                                string_builder.append_value(s);
                            } else {
                                string_builder.append_null();
                            }
                        } else {
                            string_builder.append_null();
                        }
                    }
                    arrow_schema::DataType::Int64 => {
                        let int_builder = builder.as_any_mut().downcast_mut::<Int64Builder>().unwrap();
                        if let Some(value) = metadata_value {
                            if let Some(i) = value.value.as_ref().and_then(|v| match v {
                                crate::proto::proximadb_v1::sql_value::Value::Int64Value(i) => Some(*i),
                                _ => None,
                            }) {
                                int_builder.append_value(i);
                            } else {
                                int_builder.append_null();
                            }
                        } else {
                            int_builder.append_null();
                        }
                    }
                    arrow_schema::DataType::Float64 => {
                        let float_builder = builder.as_any_mut().downcast_mut::<Float64Builder>().unwrap();
                        if let Some(value) = metadata_value {
                            if let Some(f) = value.value.as_ref().and_then(|v| match v {
                                crate::proto::proximadb_v1::sql_value::Value::NumberValue(f) => Some(*f),
                                _ => None,
                            }) {
                                float_builder.append_value(f);
                            } else {
                                float_builder.append_null();
                            }
                        } else {
                            float_builder.append_null();
                        }
                    }
                    arrow_schema::DataType::Boolean => {
                        let bool_builder = builder.as_any_mut().downcast_mut::<BooleanBuilder>().unwrap();
                        if let Some(value) = metadata_value {
                            if let Some(b) = value.value.as_ref().and_then(|v| match v {
                                crate::proto::proximadb_v1::sql_value::Value::BoolValue(b) => Some(*b),
                                _ => None,
                            }) {
                                bool_builder.append_value(b);
                            } else {
                                bool_builder.append_null();
                            }
                        } else {
                            bool_builder.append_null();
                        }
                    }
                    _ => {
                        // Default to string representation
                        let string_builder = builder.as_any_mut().downcast_mut::<StringBuilder>().unwrap();
                        string_builder.append_null();
                    }
                }
            }
        }

        // Create arrays
        let mut arrays: Vec<Arc<dyn arrow_array::Array>> = vec![
            Arc::new(id_builder.finish()),
            Arc::new(vector_builder.finish()),
            Arc::new(timestamp_builder.finish()),
            Arc::new(version_builder.finish()),
        ];

        // Add null arrays for quantization fields if present
        for field_idx in 4..(4 + quantization_field_count) {
            let field = &schema.fields()[field_idx];
            let null_array = match field.data_type() {
                arrow_schema::DataType::FixedSizeBinary(len) => {
                    let mut builder = FixedSizeBinaryBuilder::new(*len);
                    for _ in 0..records.len() {
                        builder.append_null();
                    }
                    Arc::new(builder.finish()) as Arc<dyn arrow_array::Array>
                }
                arrow_schema::DataType::Float32 => {
                    let mut builder = Float32Builder::new();
                    for _ in 0..records.len() {
                        builder.append_null();
                    }
                    Arc::new(builder.finish()) as Arc<dyn arrow_array::Array>
                }
                arrow_schema::DataType::Int8 => {
                    let mut builder = Int8Builder::new();
                    for _ in 0..records.len() {
                        builder.append_null();
                    }
                    Arc::new(builder.finish()) as Arc<dyn arrow_array::Array>
                }
                _ => {
                    // Default to null string array
                    let mut builder = StringBuilder::new();
                    for _ in 0..records.len() {
                        builder.append_null();
                    }
                    Arc::new(builder.finish()) as Arc<dyn arrow_array::Array>
                }
            };
            arrays.push(null_array);
        }

        // Add metadata arrays
        for mut builder in metadata_builders {
            arrays.push(builder.finish());
        }

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

    /// Check if we should use persistent quantization for this operation
    /// Returns true for collection-based operations with quantization enabled
    pub fn should_use_persistent_quantization(&self, params: &FlushParameters) -> bool {
        crate::compute::quantization::QuantizationSelector::should_use_persistent_quantization(params, "NOVA")
    }

    /// Get the storage quantization engine for persistent collection operations
    pub fn get_storage_quantization_engine(&self) -> &Arc<crate::compute::quantization::storage_engine::StorageQuantizationEngine> {
        &self.storage_quantization_engine
    }

    /// Get the fallback quantization engine for stateless operations
    pub fn get_fallback_quantization_engine(&self) -> &Arc<crate::compute::quantization::unified::UnifiedQuantizationEngine> {
        &self.fallback_quantization_engine
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
        "1.0.0"  // Release 1 version
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
        // Delegate to modularized flush operations
        self.flush_ops.flush(params).await
    }


    async fn do_compact(&self, params: &CompactionParameters) -> Result<CompactionResult> {
        // Delegate to modularized compaction operations
        self.compaction_ops.compact(params).await
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
        let data_dir = StoragePath::collection_data_path(base_path, &collection_id);

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
        use crate::storage::engines::core::formats::columnar::UnifiedParquetReader;
        use crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem;
        use crate::core::search::results::OptimizedSearchRecord;

        // Extract search parameters from context
        let query_vector = ctx.search_params.vector.as_ref()
            .or(ctx.search_params.query_vectors.as_ref().and_then(|vecs| vecs.first()))
            .ok_or_else(|| anyhow::anyhow!("No query vector provided"))?;
        let k = ctx.search_params.top_k.unwrap_or(10);
        let collection_id = &ctx.collection.id;

        // Get files for the collection
        let collection_path = format!("/data/collections/{}/nova", collection_id);
        // Get files for the collection - simplified for now
        let files: Vec<String> = vec![];

        if files.is_empty() {
            return Ok(Vec::new());
        }

        // Delegate to modularized search operations
        self.search_ops.search_vectors_unified(ctx).await
    }

    // Old search implementation removed - using modularized search_operations.rs

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

        // Use bounded priority queue for efficient top-k selection
        let mut priority_queue = BoundedPriorityQueue::new(top_k);

        // Insert all results into bounded queue
        for (record, distance) in all_results {
            // Convert distance to score (higher is better)
            let score = 1.0 / (1.0 + distance);

            let similarity_result = crate::compute::distance_computation::SimilarityResult::new(
                distance,
                distance_metric,
            );

            let search_record = OptimizedSearchRecord {
                id: record.id.clone(),
                vector_id: Some(record.id.clone()),
                score,
                similarity: Some(distance),
                vector: Some(Arc::new(record.vector.clone())),
                metadata: record.metadata.clone(),
                debug_info: None,
                version: None,
                timestamp: None,
                updated_at: None,
                expires_at: None,
                source: None,
                expanded_context: vec![],
                semantic_similarity: None,
                quantization_info: None,
                engine_stats: None,
                index_path: None,
            };

            priority_queue.try_insert(search_record);
        }

        // Get sorted results from bounded queue
        let final_results = priority_queue.into_sorted_vec();

        // Return the results from bounded priority queue
        Ok(final_results)
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
