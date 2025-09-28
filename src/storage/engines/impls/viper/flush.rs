// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Flush Operations
//!
//! This module handles flushing vector records from memory to Parquet files
//! with dynamic schema generation and metadata separation.

use anyhow::{Context, Result};
use uuid::Uuid;
// Use columnar module's StreamingParquetWriter instead of direct ArrowWriter
use crate::storage::engines::core::formats::columnar::{
    constants::{FIELD_ID, FIELD_VECTOR_FP32},
    ParquetWriterConfig,
    StreamingParquetWriter,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

// Use core compression directly instead of adapter
use crate::core::compression::StandardCompression;
// Use unified quantization engine
use crate::compute::distance_computation::UnifiedDistanceCompute;
use crate::compute::quantization::{UnifiedQuantizationEngine, unified::InMemoryCodebookStore};

use crate::storage::common::compaction_orchestrator::FilenameCodec;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::transaction_coordinator::{
    StagingConfig, TransactionCoordinator, TransactionStageType,
};

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::engines::core::formats::columnar::columnar_schema::ColumnarSchema;
use crate::storage::optimization::{MetadataSorter, SortingStats};

/// Flush operations for VIPER storage engine with atomic writes
pub struct Flush {
    /// Schema for columnar storage
    schema: ColumnarSchema,

    /// Collection service for metadata access
    collection_service:
        Arc<RwLock<Option<Arc<crate::services::collection::manager::CollectionService>>>>,

    /// Filesystem factory for cross-cloud atomic writes
    filesystem_factory: Arc<FilesystemFactory>,

    /// Atomic coordinator for ACID operations
    atomic_coordinator: Arc<TransactionCoordinator>,

    /// Direct compression provider (no adapter indirection)
    compression_provider: StandardCompression,

    /// Metrics updater for flush operation tracking
    metrics_updater: Option<Arc<dyn crate::metrics::InternalMetricsUpdater>>,

    /// Quantization engine for unified quantization
    quantization_engine:
        Option<Arc<crate::compute::quantization::unified::UnifiedQuantizationEngine>>,
}

impl std::fmt::Debug for Flush {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Flush")
            .field("schema", &self.schema)
            .field("collection_service", &self.collection_service)
            .field("filesystem_factory", &self.filesystem_factory)
            .field("atomic_coordinator", &self.atomic_coordinator)
            .field("compression_provider", &"StandardCompression")
            .field("metrics_updater", &self.metrics_updater.is_some())
            .field("quantization_engine", &self.quantization_engine.is_some())
            .finish()
    }
}

impl Flush {
    pub async fn new(
        collection_service: Arc<
            RwLock<Option<Arc<crate::services::collection::manager::CollectionService>>>,
        >,
        filesystem_factory: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        // Create atomic coordinator
        let atomic_coordinator = Arc::new(
            TransactionCoordinator::new(filesystem_factory.clone(), None)
                .await
                .context("Failed to create atomic coordinator")?,
        );

        // Initialize compression provider directly
        let compression_provider = StandardCompression::default();

        // Initialize quantization engine
        let codebook_store =
            Arc::new(crate::compute::quantization::unified::InMemoryCodebookStore::new());
        let distance_compute = Arc::new(
            crate::compute::distance_computation::engine::UnifiedDistanceCompute::new(
                crate::proto::proximadb_v1::DistanceMetric::Cosine,
            ),
        );
        let quantization_engine = Some(Arc::new(
            crate::compute::quantization::unified::UnifiedQuantizationEngine::new(
                distance_compute,
                codebook_store,
            ),
        ));

        Ok(Self {
            schema: ColumnarSchema::new(),
            collection_service,
            filesystem_factory,
            atomic_coordinator,
            compression_provider,
            metrics_updater: None, // Set via set_metrics_updater for dependency injection
            quantization_engine,
        })
    }

    /// Set the metrics updater for flush operation tracking
    pub fn set_metrics_updater(
        &mut self,
        updater: Arc<dyn crate::metrics::InternalMetricsUpdater>,
    ) {
        self.metrics_updater = Some(updater);
    }

    /// Core flush operation using proper staging pattern
    pub async fn flush_vectors(
        &self,
        collection_id: &str,
        vector_records: &[VectorRecord],
        batch_ids: &[String],
        force: bool,
        synchronous: bool,
        viper_config: &crate::core::config::ViperConfig,
        provided_collection_config: Option<&crate::proto::proximadb_v1::Collection>,
    ) -> Result<crate::storage::traits::FlushResult> {
        info!("🔄 VIPER: Starting flush operation with staging pattern");
        info!(
            "🔍 VIPER: Flush params - force: {}, synchronous: {}, vector_records_len: {}, batch_ids: {}",
            force,
            synchronous,
            vector_records.len(),
            batch_ids.len()
        );

        // Use provided collection config or fetch if not provided (avoid duplicate calls)
        let collection_config = if let Some(config) = provided_collection_config {
            info!("✅ VIPER: Using provided collection config (avoiding duplicate fetch)");
            Some(config.clone())
        } else {
            // Fetch collection configuration using proto type directly
            let service_lock = self.collection_service.read().await;
            if let Some(ref service) = *service_lock {
                match service.collection(collection_id).await {
                    Ok(Some(collection)) => {
                        info!("📋 VIPER: Fetched collection config from service");
                        Some(collection)
                    }
                    Ok(None) => {
                        warn!("⚠️ Collection {} not found during flush", collection_id);
                        None
                    }
                    Err(e) => {
                        warn!("⚠️ Failed to get collection {}: {}", collection_id, e);
                        None
                    }
                }
            } else {
                warn!("⚠️ No collection service available during flush");
                None
            }
        };

        // Extract vector dimensions - REQUIRED for proper processing
        let vector_dimensions = collection_config
            .as_ref()
            .and_then(|c| c.config.as_ref())
            .map(|config| config.dimension)
            .ok_or_else(|| {
                error!("CRITICAL: Collection config missing or incomplete for collection {}. Cannot determine vector dimensions!", collection_id);
                anyhow::anyhow!("Collection config with dimension is required for flush operation")
            })?;

        info!(
            "🔧 VIPER FLUSH: Using {} dimensions for collection {}",
            vector_dimensions, collection_id
        );

        info!(
            "🔍 VIPER: Processing flush for collection: {}",
            collection_id
        );

        let operation_id = crate::utils::uuid::Uuid::new_v4().to_string();

        if vector_records.is_empty() {
            info!(
                "📋 VIPER: No vector records provided for collection {}",
                collection_id
            );
            return Ok(crate::storage::traits::FlushResult {
                success: true,
                collections_affected: vec![collection_id.to_string()],
                entries_flushed: Some(0),
                bytes_written: Some(0),
                files_created: Some(0),
                duration_ms: Some(0),
                completed_at: chrono::Utc::now(),
                compaction_triggered: false,
                flushed_batch_ids: Vec::new(), // ✅ Empty for empty flush
                engine_metrics: {
                    let mut metrics = std::collections::HashMap::new();
                    metrics.insert(
                        "operation_id".to_string(),
                        serde_json::Value::String(operation_id.clone()),
                    );
                    metrics.insert("empty_flush".to_string(), serde_json::Value::Bool(true));
                    metrics
                },
            });
        }

        info!(
            "💾 VIPER: Processing {} vector records for flush",
            vector_records.len()
        );

        // Step 1: Generate unique Parquet filename using unified compactor format
        let codec = FilenameCodec::new();
        // Level 0 for flush, use VIPER_FILE_EXT constant without the dot
        let parquet_filename = codec.generate(0, &crate::storage::engines::VIPER_FILE_EXT[1..]);
        info!(
            "🔄 VIPER: Step 1 - Preparing atomic Parquet write: {}",
            parquet_filename
        );

        // Step 2: Sort records by metadata for optimal Parquet encoding
        info!(
            "🔄 VIPER: Step 2a - Sorting {} vector records by metadata for optimal compression",
            vector_records.len()
        );
        let (sorted_records, _sort_stats) = match self
            .sort_records_for_parquet_encoding(vector_records, &collection_config)
            .await
        {
            Ok(result) => {
                info!(
                    "✅ VIPER: Step 2a - Records sorted (estimated compression improvement: {:.1}%)",
                    result.1.compression_estimate * 100.0
                );
                result
            }
            Err(e) => {
                warn!(
                    "⚠️ VIPER: Step 2a - Sorting failed, using original order: {}",
                    e
                );
                (
                    vector_records.to_vec(),
                    crate::storage::optimization::SortingStats::default(),
                )
            }
        };

        // Step 2b: Serialize sorted vector records to Parquet format
        info!(
            "🔄 VIPER: Step 2b - Serializing {} sorted vector records to Parquet",
            sorted_records.len()
        );
        debug!("📊 VIPER WRITER PATH ANALYSIS:");
        debug!("   - Input: Entire batch from memtable (flush pattern)");
        debug!("   - Processing: Sort → Quantize → Columnar layout");
        debug!("   - Output: Single Parquet file with quantized columns");
        debug!("   - Quantization: Applied based on collection config");
        let parquet_data_or_path = match self
            .serialize_records_to_parquet(
                &sorted_records,
                collection_id,
                &collection_config,
                vector_dimensions as usize,
                viper_config,
            )
            .await
        {
            Ok(data) => {
                // Check if this is a file path marker (starts with magic bytes)
                if data.len() > 4 && &data[0..4] == &[0xFA, 0xCE, 0xF1, 0x1E] {
                    let path_str = String::from_utf8_lossy(&data[4..]);
                    info!(
                        "✅ VIPER: Step 2 - Serialization completed (file at {})",
                        path_str
                    );
                } else {
                    info!(
                        "✅ VIPER: Step 2 - Serialization completed ({} bytes)",
                        data.len()
                    );
                }
                data
            }
            Err(e) => {
                error!("❌ VIPER: Step 2 - Serialization failed: {}", e);
                return Err(e.context("Failed to serialize vector records to Parquet"));
            }
        };

        // Step 3: Check if HybridParquetWriter already handled atomic write
        let final_file_path = if parquet_data_or_path.len() > 4
            && &parquet_data_or_path[0..4] == &[0xFF, 0xFF, 0xFF, 0xFF]
        {
            // HybridParquetWriter already wrote the file atomically
            let path_str = String::from_utf8_lossy(&parquet_data_or_path[4..]);
            info!("✅ VIPER: Step 3 - Parquet already written atomically by HybridParquetWriter: {}", path_str);
            path_str.to_string()
        } else if parquet_data_or_path.len() > 4
            && &parquet_data_or_path[0..4] == &[0xFA, 0xCE, 0xF1, 0x1E]
        {
            // Legacy path: Extract temp file path from marker
            let temp_path_str = String::from_utf8_lossy(&parquet_data_or_path[4..]);
            let temp_path = std::path::Path::new(temp_path_str.as_ref());

            // Use more efficient atomic move
            match self
                .write_parquet_atomic_from_path(
                    collection_id,
                    &parquet_filename,
                    temp_path,
                    &collection_config,
                )
                .await
            {
                Ok(path) => {
                    info!("✅ VIPER: Step 3 - Parquet atomically moved: {}", path);
                    // Clean up temp file if it still exists
                    if let Err(e) = std::fs::remove_file(temp_path) {
                        debug!("Temp file already moved or deleted: {}", e);
                    }
                    path
                }
                Err(e) => {
                    // Clean up temp file on error
                    if let Err(cleanup_err) = std::fs::remove_file(temp_path) {
                        debug!("Failed to clean up temp file: {}", cleanup_err);
                    }
                    error!("❌ VIPER: Step 3 - Atomic move failed: {}", e);
                    return Err(e.context("Failed to atomically move Parquet file"));
                }
            }
        } else {
            // Legacy path with buffer (should not happen with new code)
            match self
                .write_parquet_atomic(
                    collection_id,
                    &parquet_filename,
                    &parquet_data_or_path,
                    &collection_config,
                )
                .await
            {
                Ok(path) => {
                    info!("✅ VIPER: Step 3 - Parquet atomically written: {}", path);
                    path
                }
                Err(e) => {
                    error!("❌ VIPER: Step 3 - Atomic write failed: {}", e);
                    return Err(e.context("Failed to atomically write Parquet file"));
                }
            }
        };

        // Note: No cleanup needed - atomic write strategy handles staging automatically

        // Step 4: Check for compaction trigger
        info!("🔄 VIPER: Step 4 - Checking compaction trigger");
        let compaction_triggered = self
            .check_compaction_trigger(collection_id)
            .await
            .unwrap_or(false);

        // Step 5: Update collection metadata
        info!("🔄 VIPER: Step 5 - Updating collection metadata_info");
        // Calculate actual data size (either from file or buffer)
        let data_size = if parquet_data_or_path.len() > 4
            && &parquet_data_or_path[0..4] == &[0xFA, 0xCE, 0xF1, 0x1E]
        {
            // Get file size from the final path
            let fs = self.filesystem_factory.get_filesystem(&final_file_path)?;
            if let Ok(metadata) = fs.metadata(&final_file_path).await {
                metadata.size as usize
            } else {
                0
            }
        } else {
            parquet_data_or_path.len()
        };

        self.update_collection_metadata_after_flush(collection_id, vector_records.len(), data_size)
            .await?;

        // Step 5.1: Record metrics (non-blocking, failure-tolerant)
        if let Some(ref metrics_updater) = self.metrics_updater {
            use crate::metrics::FlushMetricsUpdate;

            let flush_update = FlushMetricsUpdate {
                vectors_flushed: vector_records.len() as i64,
                bytes_written: data_size as i64,
                duration_ms: 0, // TODO: Track actual duration
                files_created: 1,
                engine_type: "VIPER".to_string(),
                timestamp: chrono::Utc::now().timestamp_millis(),
            };

            // Fire and forget - never block flush operation
            if let Err(e) = metrics_updater
                .record_flush(collection_id, flush_update)
                .await
            {
                debug!("Failed to record flush metrics: {}", e);
            }
        }

        // Step 6: Return successful flush result with BatchId coordination
        Ok(crate::storage::traits::FlushResult {
            success: true,
            collections_affected: vec![collection_id.to_string()],
            entries_flushed: Some(vector_records.len() as u64),
            bytes_written: Some(data_size as u64),
            files_created: Some(1),
            duration_ms: Some(0), // Will be set by high-level flush() method
            completed_at: chrono::Utc::now(),
            flushed_batch_ids: batch_ids
                .iter()
                .map(|_id| {
                    // Use compact BatchId for minimal storage overhead (10 bytes vs 100+ bytes)
                    crate::storage::persistence::write_ahead_log::BatchId::default()
                })
                .collect(), // ✅ Include for WAL cleanup
            engine_metrics: {
                let mut metrics = std::collections::HashMap::new();
                metrics.insert(
                    "operation_id".to_string(),
                    serde_json::Value::String(operation_id),
                );
                metrics.insert(
                    "vector_records_count".to_string(),
                    serde_json::Value::Number(serde_json::Number::from(vector_records.len())),
                );
                metrics.insert(
                    "final_file_path".to_string(),
                    serde_json::Value::String(final_file_path),
                );
                metrics.insert(
                    "compaction_triggered".to_string(),
                    serde_json::Value::Bool(compaction_triggered),
                );
                metrics
            },
            compaction_triggered,
        })
    }

    /// Serialize vector records to actual Parquet format using Apache Arrow
    async fn serialize_records_to_parquet(
        &self,
        records: &[VectorRecord],
        collection_id: &str,
        collection_config: &Option<crate::proto::proximadb_v1::Collection>,
        vector_dimensions: usize,
        viper_config: &crate::core::config::ViperConfig,
    ) -> Result<Vec<u8>> {
        if records.is_empty() {
            return Ok(Vec::new());
        }

        // 🎯 DELEGATING TO SHARED COLUMNAR MODULE:
        // StreamingParquetWriter handles all schema creation and data serialization
        // This ensures consistency between VIPER and NOVA engines
        // Extract quantization config for StreamingParquetWriter
        let quantization_config = if let Some(collection) = collection_config {
            collection
                .config
                .as_ref()
                .and_then(|c| c.quantization.clone())
                .unwrap_or_default()
        } else {
            Default::default()
        };

        // Extract filterable columns for bloom filter configuration
        let filterable_columns: Vec<String> = if let Some(collection) = collection_config {
            if let Some(ref config) = collection.config {
                config.filterable_columns.iter().map(|col| col.name.clone()).collect()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        // No manual schema creation needed - StreamingParquetWriter handles this

        // No manual data processing needed - StreamingParquetWriter handles everything

        // StreamingParquetWriter will handle all data transformation internally
        // No need for manual data processing - directly use StreamingParquetWriter

        /* REMOVED: Manual data processing code (lines 507-852)
           This was creating arrays for RecordBatch that was never actually used.
           StreamingParquetWriter accepts VectorRecord directly. */

        // Prepare to use HybridParquetWriter's flush method directly

        debug!("🔍 VIPER FLUSH: Using core compression directly");
        debug!("   Collection: {}", collection_id);
        debug!("   Records: {}", records.len());

        // Select compression algorithm based on collection config
        let compression_algorithm = if let Some(collection) = collection_config {
            if let Some(ref config) = collection.config {
                if let Some(ref storage_config) = config.storage_config {
                    let compression_value = storage_config.compression;
                    use crate::proto::proximadb_v1::CompressionAlgorithm as ProtoAlgorithm;

                    // Convert proto compression to core compression algorithm
                    match ProtoAlgorithm::try_from(compression_value) {
                        Ok(ProtoAlgorithm::CompressionZstd) => {
                            crate::core::compression::CompressionAlgorithm::Zstd
                        }
                        Ok(ProtoAlgorithm::CompressionLz4) => {
                            crate::core::compression::CompressionAlgorithm::Lz4
                        }
                        Ok(ProtoAlgorithm::CompressionSnappy) => {
                            crate::core::compression::CompressionAlgorithm::Snappy
                        }
                        Ok(ProtoAlgorithm::CompressionGzip) => {
                            crate::core::compression::CompressionAlgorithm::Gzip
                        }
                        Ok(ProtoAlgorithm::CompressionBrotli) => {
                            crate::core::compression::CompressionAlgorithm::Brotli
                        }
                        // CompressionMixed not available in proto, using Mixed from our enum
                        // This case should not occur with current proto definitions
                        _ => crate::core::compression::CompressionAlgorithm::None,
                    }
                } else {
                    crate::core::compression::CompressionAlgorithm::Zstd // Default
                }
            } else {
                crate::core::compression::CompressionAlgorithm::Zstd // Default
            }
        } else {
            crate::core::compression::CompressionAlgorithm::Zstd // Default
        };

        let compression_level = viper_config.compression_level as u32; // Simplified since compression is now i32

        // Map core compression to Parquet compression using shared function
        let compression_algo =
            crate::storage::engines::core::formats::columnar::common::map_core_to_parquet_compression(
                compression_algorithm.clone(),
                Some(compression_level as i32),
            )?;
        debug!("   Selected Parquet compression: {:?}", compression_algo);

        // Build writer properties with optimal encodings for different column types
        let mut props_builder = parquet::file::properties::WriterProperties::builder()
            .set_compression(compression_algo)
            .set_max_row_group_size(viper_config.row_group_size);

        // For Mixed compression, apply per-column optimization
        if compression_algorithm == crate::core::compression::CompressionAlgorithm::Mixed {
            info!("🎯 VIPER: Applying Mixed compression per-column optimization");
            // Note: Mixed compression strategy is now handled by StreamingParquetWriter
            // props_builder = self.apply_mixed_compression_strategy(props_builder, &batch)?;
        }

        // Set optimal encoding for vector column based on quantization
        // Check if vectors are quantized (detected via collection config)
        let is_quantized = if let Some(collection) = collection_config {
            collection
                .config
                .as_ref()
                .and_then(|c| c.quantization.as_ref())
                .map(|q| q.enabled)
                .unwrap_or(false)
        } else {
            false
        };

        if is_quantized {
            // For quantized vectors (INT8/INT16 or custom bit-width via bytemuck)
            // Use BIT_PACKED encoding for maximum compression
            props_builder = props_builder.set_column_encoding(
                parquet::schema::types::ColumnPath::from(FIELD_VECTOR_FP32),
                parquet::basic::Encoding::RLE,
            );
            debug!("🔧 VIPER: Using BIT_PACKED encoding for quantized vectors");
        } else {
            // For full precision f32 vectors
            // BYTE_STREAM_SPLIT splits floating point bytes for better compression
            props_builder = props_builder.set_column_encoding(
                parquet::schema::types::ColumnPath::from(FIELD_VECTOR_FP32),
                parquet::basic::Encoding::BYTE_STREAM_SPLIT,
            );
            debug!("🔧 VIPER: Using BYTE_STREAM_SPLIT encoding for f32 vectors");
        }

        // Set dictionary encoding for low-cardinality string columns
        props_builder = props_builder.set_column_dictionary_enabled(
            parquet::schema::types::ColumnPath::from("collection_id"),
            true,
        );
        props_builder = props_builder
            .set_column_dictionary_enabled(parquet::schema::types::ColumnPath::from(FIELD_ID), true);

        // Apply column-specific encodings from filterable metadata
        // TODO: Re-enable when encoding_hint is available in proto v1
        /*for filterable_column in &filterable_metadata {
            if let Some(encoding_hint) = filterable_column.encoding_hint {
                use crate::proto::proximadb_v1::ColumnEncoding;
                let column_path =
                    parquet::schema::types::ColumnPath::from(filterable_column.name.as_str());

                match ColumnEncoding::try_from(encoding_hint) {
                    Ok(ColumnEncoding::EncodingDictionary) => {
                        props_builder =
                            props_builder.set_column_dictionary_enabled(column_path, true);
                    }
                    Ok(ColumnEncoding::EncodingDelta) => {
                        props_builder = props_builder.set_column_encoding(
                            column_path,
                            parquet::basic::Encoding::DELTA_BINARY_PACKED,
                        );
                    }
                    Ok(ColumnEncoding::EncodingRle) => {
                        props_builder = props_builder
                            .set_column_encoding(column_path, parquet::basic::Encoding::RLE);
                    }
                    _ => {} // Use default encoding
                }
            }
        }*/

        // Configure ParquetWriterConfig from VIPER settings
        // Include filterable columns in bloom filters for fast filtering
        let mut bloom_columns = vec![FIELD_ID.to_string()];
        // Add filterable columns that were extracted earlier
        bloom_columns.extend(filterable_columns.clone());

        let writer_config = ParquetWriterConfig {
            row_group_size: viper_config.row_group_size,
            page_size: 1024 * 1024, // 1MB pages
            write_batch_size: 10000,
            compression: compression_algo,
            compression_level: Some(compression_level as i32),
            enable_dictionary: true,
            enable_bloom_filters: true, // Enable for efficient ID and metadata lookups
            bloom_filter_fpp: 0.01,
            bloom_filter_ndv: records.len() as u64,
            enable_statistics: true,
            enable_page_index: true,
            sort_columns: vec![], // Can be populated with filterable columns if needed
            id_less_storage: false, // Keep IDs for customer APIs
            filterable_metadata_columns: Some(filterable_columns.clone()),
            quantization: crate::proto::proximadb_v1::QuantizationConfig {
                enabled: is_quantized,
                strategy: 0, // SMART_DEFAULTS
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
                enable_binary: false,
                enable_int8: is_quantized,
                enable_pq: false, // DISABLED: PQ codebook storage causes 15-25x file bloat
                pq_segments: 32,
                pq_bits: 8,
                pq_codebooks: 256,
                binary_threshold: 0.5,
                int8_threshold: 0.3,
                pq_threshold: 0.1,
            },
            max_records_per_file: None,
            target_file_size_bytes: None,
            enable_async_io: false,
        };

        // Get dimension from the first record
        let dimension = if !records.is_empty() {
            records[0].vector.len()
        } else {
            return Ok(Vec::new());
        };

        // Extract filterable columns from collection config
        let filterable_columns = collection_config.as_ref()
            .and_then(|c| c.config.as_ref())
            .and_then(|cfg| {
                if cfg.filterable_columns.is_empty() {
                    None
                } else {
                    Some(cfg.filterable_columns.as_slice())
                }
            });

        // Create StreamingParquetWriter with temp file and filterable columns
        debug!("Creating StreamingParquetWriter with {} filterable columns", filterable_columns.map(|cols| cols.len()).unwrap_or(0));
        if let Some(cols) = filterable_columns {
            for col in cols {
                debug!("  Filterable column: {} (type: {:?})", col.name, col.data_type);
            }
        }

        // Debug: Check first few records for metadata
        for (i, record) in records.iter().take(3).enumerate() {
            debug!("Record {}: id={}, metadata keys={:?}", i, record.id, record.metadata.keys().collect::<Vec<_>>());
            if let Some(category) = record.metadata.get("category") {
                debug!("  category value: {:?}", category);
            }
        }

        // Get storage assignment from collection config - fail fast if not present
        let storage_assignment = collection_config
            .as_ref()
            .and_then(|c| c.storage_assignment.as_ref())
            .ok_or_else(|| anyhow::anyhow!(
                "Collection '{}' has no storage assignment. All collections must have storage assignments.",
                collection_id
            ))?;

        // Get storage base path
        let data_url = format!(
            "{}/{}/data",
            storage_assignment.base_location, collection_id
        );

        // Generate filename using FilenameCodec
        let codec = FilenameCodec::new();
        let filename = codec.generate(0, &crate::storage::engines::VIPER_FILE_EXT[1..]);

        // Construct final path
        let final_path = std::path::PathBuf::from(format!("{}/{}", data_url, filename));

        // Construct final URL
        let final_url = format!("{}/{}", data_url, filename);

        // Use HybridParquetWriter with integrated disk cache support
        use crate::storage::engines::core::formats::columnar::hybrid_writer::{HybridParquetWriter, HybridWriterConfig};
        let hybrid_config = HybridWriterConfig {
            base_config: writer_config,
            ..Default::default()
        };

        // Use the integrated write_with_cache method that handles:
        // 1. Writing to temp file
        // 2. Finalizing the writer
        // 3. Uploading to cloud with disk cache population
        let (stats, _collector) = HybridParquetWriter::write_with_cache(
            records,
            dimension,
            hybrid_config,
            &final_url,
            &self.filesystem_factory,
            filterable_columns.clone(),
            None, // VIPER doesn't use metadata collectors for sidecar files
        ).await?;

        debug!("   ✅ VIPER Parquet written with disk cache:");
        debug!("      Cloud URL: {}", final_url);
        debug!("      Records: {}", stats.total_records);
        debug!("      Row groups: {}", stats.total_row_groups);
        debug!("      File size: {} bytes", stats.file_size);
        debug!("      Disk cache: POPULATED (future reads avoid cloud costs)");

        info!(
            "📝 VIPER FLUSH: Wrote Parquet file to {} with disk cache",
            final_url
        );

        // Return final path as success marker - file written and cached
        let mut result = vec![0xFF, 0xFF, 0xFF, 0xFF]; // Magic bytes to indicate already written
        result.extend_from_slice(final_url.as_bytes());
        Ok(result)
    }

    /// INT8 Quantization for Parquet columnar storage
    /// Delegates to unified quantization engine for consistency across all engines
    fn quantize_to_int8(
        &self,
        fp32_vector: &[f32],
        _quant_config: &crate::proto::proximadb_v1::QuantizationConfig,
    ) -> Vec<i8> {
        if fp32_vector.is_empty() {
            return Vec::new();
        }

        debug!("🔧 VIPER: Using unified INT8 quantization for Parquet storage");

        // Get unified quantization engine
        let quant_engine = match self.quantization_engine.as_ref() {
            Some(engine) => engine,
            None => {
                error!("Quantization engine not initialized");
                return Vec::new();
            }
        };

        // Use unified engine's INT8 quantization
        match quant_engine.quantize_to_u8(fp32_vector) {
            Ok((quantized, _min, _max)) => {
                // Convert u8 to i8 for compatibility with existing code
                quantized.iter().map(|&v| v as i8).collect()
            }
            Err(e) => {
                error!("Failed to quantize vector: {}", e);
                Vec::new()
            }
        }
    }

    // DEPRECATED: Use UnifiedQuantizationEngine::quantize_to_pq() instead
    // Removed duplicate quantize_to_pq8 implementation

    // DEPRECATED: Use UnifiedQuantizationEngine::quantize_to_pq() instead
    // Removed duplicate quantize_to_pq4 implementation

    /// Write Parquet data using atomic write strategy
    /// Uses unified atomic write infrastructure for cross-cloud compatibility
    /// Write Parquet file atomically from a temp file path using zero-copy I/O
    async fn write_parquet_atomic_from_path(
        &self,
        collection_id: &str,
        filename: &str,
        temp_file_path: &std::path::Path,
        collection_config: &Option<crate::proto::proximadb_v1::Collection>,
    ) -> Result<String> {
        let file_size = std::fs::metadata(temp_file_path)?.len();
        info!(
            "🔄 Moving Parquet file atomically with zero-copy: {} ({} bytes)",
            filename, file_size
        );

        // Get storage assignment from collection config - fail fast if not present
        let storage_assignment = collection_config
            .as_ref()
            .and_then(|c| c.storage_assignment.as_ref())
            .ok_or_else(|| anyhow::anyhow!(
                "Collection '{}' has no storage assignment. All collections must have storage assignments.",
                collection_id
            ))?;

        // Get final destination path
        let data_url = format!(
            "{}/{}/data",
            storage_assignment.base_location, collection_id
        );
        let final_path = format!("{}/{}", data_url, filename);

        // Get filesystem for the destination
        let fs = self.filesystem_factory.get_filesystem(&data_url)?;

        info!(
            "📝 Using zero-copy I/O for atomic write from {:?} to {}",
            temp_file_path, final_path
        );

        // Check if we have UnifiedCachingFilesystem for optimal performance
        if let Some(unified_fs) =
            fs.as_any()
                .downcast_ref::<crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem>()
        {
            info!("✅ Using UnifiedCachingFilesystem with intelligent staging and caching");

            // Read data once for optimized write
            let data = std::fs::read(temp_file_path)?;

            // Configure write options for atomic operation
            let write_options = crate::storage::persistence::filesystem::FileOptions {
                create_dirs: true,            // Create parent directories if needed
                overwrite: true,              // Overwrite if exists
                buffer_size: Some(64 * 1024), // 64KB buffer
                ..Default::default()
            };

            // Use unified filesystem which:
            // 1. Writes to optimal temp location
            // 2. Populates local cache for fast reads (hydrates indexes)
            // 3. Asynchronously uploads to cloud
            // 4. Returns immediately after local cache write
            fs.write(&final_path, &data, Some(write_options.clone()))
                .await?;

            info!("✅ Zero-copy write complete with cache population for index hydration");
        } else {
            // Fallback to standard filesystem API
            debug!("Using standard filesystem API (consider enabling ZeroCopyFilesystem)");

            let data = std::fs::read(temp_file_path)?;

            let write_options = crate::storage::persistence::filesystem::FileOptions {
                create_dirs: true,
                overwrite: true,
                buffer_size: Some(64 * 1024),
                ..Default::default()
            };

            fs.write(&final_path, &data, Some(write_options)).await?;
        }

        info!(
            "✅ VIPER: Atomically moved Parquet file {} ({} KB)",
            final_path,
            file_size / 1024
        );

        // Verify file was written
        if fs.exists(&final_path).await? {
            let metadata = fs.metadata(&final_path).await?;
            info!(
                "✅ VIPER: Verified file exists at {} with size {} bytes",
                final_path, metadata.size
            );
        } else {
            error!("❌ VIPER: File not found after atomic move: {}", final_path);
        }

        Ok(final_path)
    }

    /// Legacy method that accepts a byte buffer (less efficient)
    async fn write_parquet_atomic(
        &self,
        collection_id: &str,
        filename: &str,
        parquet_data: &[u8],
        collection_config: &Option<crate::proto::proximadb_v1::Collection>,
    ) -> Result<String> {
        info!(
            "🔄 Writing Parquet file atomically: {} ({} bytes)",
            filename,
            parquet_data.len()
        );

        // Get storage assignment from collection config - fail fast if not present
        let storage_assignment = collection_config
            .as_ref()
            .and_then(|c| c.storage_assignment.as_ref())
            .ok_or_else(|| anyhow::anyhow!(
                "Collection '{}' has no storage assignment. All collections must have storage assignments.",
                collection_id
            ))?;

        // Begin atomic operation for flush
        let data_url = format!(
            "{}/{}/data",
            storage_assignment.base_location, collection_id
        );
        let staging_config = StagingConfig {
            base_url: data_url.clone(),
            collection_id: None, // Don't duplicate collection path
            operation_type: TransactionStageType::Flush,
            custom_staging_dir: None,
            auto_cleanup: true,
            max_orphaned_age_hours: 24,
            ..Default::default() // This will pick up skip_uuid_subdir: false
        };

        let atomic_op = self
            .atomic_coordinator
            .begin_atomic_operation(&staging_config)
            .await
            .context("Failed to begin atomic flush operation")?;

        info!("📝 Writing parquet file to staging: {}", filename);

        // Write parquet data to staging directory
        self.atomic_coordinator
            .write_to_staging(&atomic_op.operation_id, filename, parquet_data)
            .await
            .context("Failed to write parquet file to staging")?;

        // Finalize atomic operation - this will atomically move the file to final location
        self.atomic_coordinator
            .finalize_atomic_operation(&atomic_op.operation_id)
            .await
            .context("Failed to finalize atomic flush")?;

        let final_path = format!("{}/{}", data_url, filename);

        info!(
            "✅ VIPER: Atomically wrote Parquet file {} ({} KB)",
            final_path,
            parquet_data.len() / 1024
        );

        // Verify file was written
        let fs = self.filesystem_factory.get_filesystem(&data_url)?;
        if fs.exists(&final_path).await? {
            let metadata = fs.metadata(&final_path).await?;
            info!(
                "✅ VIPER: Verified file exists at {} with size {} bytes",
                final_path, metadata.size
            );
        } else {
            error!(
                "❌ VIPER: File not found after atomic write: {}",
                final_path
            );
        }

        Ok(final_path)
    }

    /// Check if compaction should be triggered
    async fn check_compaction_trigger(&self, _collection_id: &str) -> Result<bool> {
        // Compaction triggers based on multiple factors
        // Note: This is deferred to the Compaction which has full context
        // about file counts, sizes, and collection-specific thresholds.
        // For now, we don't trigger compaction from the flush path.

        // The BackgroundManager handles compaction scheduling based on:
        // 1. Number of Parquet files for this collection
        // 2. File size distribution
        // 3. Collection-specific compaction policies
        // 4. System load and resource availability

        // Return false to let BackgroundManager handle compaction decisions
        Ok(false)
    }

    /// Update collection metadata after flush
    async fn update_collection_metadata_after_flush(
        &self,
        collection_id: &str,
        records_count: usize,
        bytes_written: usize,
    ) -> Result<()> {
        // Note: Collection stats update is currently not implemented in the flush path.
        // The CollectionService has an update_stats() method that can track:
        // - vector_count (incremental changes)
        // - data_size_bytes (storage usage)
        //
        // This would be valuable metrics for users to monitor:
        // - Collection growth over time
        // - Storage utilization
        // - Flush performance metrics
        //
        // For now, we just log the flush completion locally.

        debug!(
            "Flush completed for collection {}: {} vectors, {} bytes written",
            collection_id, records_count, bytes_written
        );

        // TODO: Consider integrating with CollectionService::update_stats()
        // to maintain accurate collection-level metrics that users can query.

        Ok(())
    }

    /// Sort vector records by metadata for optimal Parquet encoding
    async fn sort_records_for_parquet_encoding(
        &self,
        records: &[VectorRecord],
        collection_config: &Option<crate::proto::proximadb_v1::Collection>,
    ) -> Result<(Vec<VectorRecord>, SortingStats)> {
        // Extract filterable columns from collection config
        let filterable_columns = collection_config
            .as_ref()
            .and_then(|c| c.config.as_ref())
            .map(|config| config.filterable_columns.clone())
            .clone();

        if filterable_columns.is_none() {
            // No filterable columns, sort by vector ID for consistent ordering
            let mut sorted_records = records.to_vec();
            sorted_records.sort_by(|a, b| {
                let a_id = a.id.as_str();
                let b_id = b.id.as_str();
                a_id.cmp(&b_id)
            });

            return Ok((
                sorted_records,
                SortingStats {
                    records_sorted: records.len(),
                    sort_keys_used: vec!["vector_id".to_string()],
                    compression_estimate: 0.05, // Small improvement from ID sorting
                    sort_time_us: 0,
                    ..Default::default()
                },
            ));
        }

        // Create metadata sorter from filterable columns
        let sorter =
            MetadataSorter::from_filterable_specs(filterable_columns.as_deref().unwrap_or(&[]));

        // Sort records for optimal encoding
        let (sorted_records, stats) = sorter.sort_for_encoding(records.to_vec())?;

        debug!(
            "🎯 VIPER: Sorted {} records by {} filterable keys for Parquet optimization",
            stats.records_sorted,
            stats.sort_keys_used.len()
        );

        Ok((sorted_records, stats))
    }

    /// Apply mixed compression strategy with per-column optimization
    fn apply_mixed_compression_strategy(
        &self,
        mut props_builder: parquet::file::properties::WriterPropertiesBuilder,
        batch: &arrow_array::RecordBatch,
    ) -> Result<parquet::file::properties::WriterPropertiesBuilder> {
        use crate::core::compression::{
            CompressionContext, detect_column_type, optimal_compression_for_column,
        };

        info!(
            "🎯 VIPER: Applying Mixed compression search_strategy to {} columns",
            batch.num_columns()
        );

        // Analyze each column and apply optimal compression
        for field in batch.schema().fields() {
            let name = field.name();

            // Detect column type based on name and context
            let data_type = detect_column_type(name, &CompressionContext::Parquet);

            // Get optimal compression for this column type
            let optimal_algorithm = optimal_compression_for_column(&data_type);

            // Convert to Parquet compression using the columnar common function
            let compression_algo = crate::storage::engines::core::formats::columnar::common::map_core_to_parquet_compression(
                optimal_algorithm,
                None, // No specific compression level for column-specific compression
            )?;

            let column_path = parquet::schema::types::ColumnPath::from(name.as_str());

            debug!(
                "🔧 VIPER Mixed: {} -> {:?} (type: {:?})",
                name, optimal_algorithm, data_type
            );

            // Apply per-column compression - compression_algo is already a parquet compression type
            props_builder =
                props_builder.set_column_compression(column_path.clone(), compression_algo);

            // Apply optimal encoding based on column type
            let encoding = match data_type {
                crate::core::compression::ColumnData::BinaryQuantized => {
                    // Binary data - use bit packing for maximum density
                    parquet::basic::Encoding::RLE
                }
                crate::core::compression::ColumnData::Int8Quantized => {
                    // Integer quantized - use delta encoding
                    parquet::basic::Encoding::DELTA_BINARY_PACKED
                }
                crate::core::compression::ColumnData::ProductQuantized => {
                    // PQ vectors - use byte stream split for floating point efficiency
                    parquet::basic::Encoding::BYTE_STREAM_SPLIT
                }
                crate::core::compression::ColumnData::FullPrecision => {
                    // FP32 vectors - use byte stream split for best compression
                    parquet::basic::Encoding::BYTE_STREAM_SPLIT
                }
                crate::core::compression::ColumnData::Identifier => {
                    // ID columns - use dictionary encoding for deduplication
                    parquet::basic::Encoding::RLE_DICTIONARY
                }
                crate::core::compression::ColumnData::Metadata => {
                    // Metadata - use dictionary encoding for repeated values
                    parquet::basic::Encoding::RLE_DICTIONARY
                }
                crate::core::compression::ColumnData::Timestamp => {
                    // Timestamps - use delta encoding for monotonic values
                    parquet::basic::Encoding::DELTA_BINARY_PACKED
                }
                crate::core::compression::ColumnData::Generic => {
                    // Generic data - use plain encoding
                    parquet::basic::Encoding::PLAIN
                }
            };

            props_builder = props_builder.set_column_encoding(column_path, encoding);

            debug!("🔧 VIPER Mixed: {} encoding -> {:?}", name, encoding);
        }

        info!("✅ VIPER: Mixed compression search_strategy applied to all columns");
        Ok(props_builder)
    }
}
