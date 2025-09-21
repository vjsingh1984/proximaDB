// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Flush Operations
//!
//! This module handles flushing vector records from memory to Parquet files
//! with dynamic schema generation and metadata separation.

use anyhow::{Context, Result};
use arrow_array::builder::{Int8Builder, UInt8Builder};
use arrow_array::{Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
// Use columnar module's StreamingParquetWriter instead of direct ArrowWriter
use crate::storage::engines::core::formats::columnar::{
    ParquetWriterConfig, StreamingParquetWriter,
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

use crate::core::{String, VectorRecord};
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

        // Step 3: Atomic write/move of Parquet data using unified filesystem strategy
        info!(
            "🔄 VIPER: Step 3 - Atomically writing/moving Parquet file: {}",
            parquet_filename
        );

        // Check if we have a file path or raw data
        let final_file_path = if parquet_data_or_path.len() > 4
            && &parquet_data_or_path[0..4] == &[0xFA, 0xCE, 0xF1, 0x1E]
        {
            // Extract temp file path from marker
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

        // 🎯 OPTIMIZED PARQUET SCHEMA: Designed for multi-stage query execution
        //
        // QUERY OPTIMIZATION ORDER:
        // 1. FILTERABLE METADATA → Parquet predicate pushdown (fastest, reduces I/O)
        // 2. VECTOR SEARCH → Similarity search on reduced candidate set
        // 3. EXTRA_METADATA → Post-processing filter (slowest, applied to smallest set)
        //
        // This ordering maximizes performance by eliminating rows early using efficient
        // columnar filters before expensive vector operations
        // Check if quantization is enabled to determine schema columns
        let quantization = if let Some(collection) = collection_config {
            collection
                .config
                .as_ref()
                .and_then(|c| c.quantization.as_ref())
        } else {
            None
        };

        let mut schema_fields = vec![
            Field::new("id", DataType::Utf8, true), // Can be null for append-only vectors
            Field::new("collection_id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::List(Arc::new(Field::new("item", DataType::Float32, true))),
                true, // Vector field can be null
            ), // Primary FP32 vector column for 100% fidelity
            Field::new("version", DataType::Int8, true), // Version field for MVCC - using tinyint
            Field::new("updated_at", DataType::Int64, true), // Audit field - stores create or update time
            Field::new("expires_at", DataType::Int64, true), // Only keep expires_at for TTL
        ];

        // Phase 2: Add quantized vector columns for compression + fast approximation
        if let Some(quant_config) = quantization {
            if quant_config.enabled {
                debug!(
                    "🗜️ VIPER: Adding quantized vector columns for collection {}",
                    collection_id
                );

                // Add INT8 quantized column (highest quality quantization)
                schema_fields.push(Field::new(
                    "vector_int8",
                    DataType::List(Arc::new(Field::new("item", DataType::Int8, true))),
                    true,
                ));

                // Add PQ8 (Product Quantization 8-bit) column for high compression
                schema_fields.push(Field::new(
                    "vector_pq8",
                    DataType::List(Arc::new(Field::new("item", DataType::UInt8, true))),
                    true,
                ));

                // Add PQ4 (Product Quantization 4-bit) column for maximum compression
                // Stored as UInt8 but each byte contains two 4-bit values
                schema_fields.push(Field::new(
                    "vector_pq4",
                    DataType::List(Arc::new(Field::new("item", DataType::UInt8, true))),
                    true,
                ));

                info!("✅ VIPER: Dual storage enabled - FP32 + INT8 + PQ8 + PQ4 quantized columns");
            }
        }

        // 🎯 DYNAMIC FILTERABLE METADATA: Use proto filterable_columns directly
        let filterable_metadata: Vec<&crate::proto::proximadb_v1::FilterableColumnSpec> =
            if let Some(collection) = collection_config {
                if let Some(ref config) = collection.config {
                    config.filterable_columns.iter().collect()
                } else {
                    Vec::new()
                }
            } else {
                info!(
                    "Collection {} config not available, using empty filterable metadata_info",
                    collection_id
                );
                Vec::new()
            };

        // Add filterable metadata columns based on collection configuration using proto types
        for filterable_column in &filterable_metadata {
            // TODO: Implement convert_proto_type_to_arrow method on ColumnarSchema
            // For now, use String as default for filterable columns
            let arrow_data_type = arrow_schema::DataType::Utf8;

            schema_fields.push(Field::new(
                &filterable_column.name,
                arrow_data_type,
                true, // Filterable metadata is always nullable
            ));
        }

        // Add extra_meta column for remaining metadata as list of key-value pairs
        let key_value_struct = DataType::Struct(arrow_schema::Fields::from(vec![
            Field::new("key", DataType::Utf8, false),
            Field::new("value", DataType::Utf8, false),
        ]));
        schema_fields.push(Field::new(
            "extra_meta",
            DataType::List(Arc::new(Field::new("item", key_value_struct, true))),
            true,
        ));

        let schema = Arc::new(Schema::new(schema_fields));

        // Process records for Arrow array creation - pre-allocate with capacity for performance
        let capacity = records.len();
        let mut ids = Vec::with_capacity(capacity);
        let mut collection_ids = Vec::with_capacity(capacity);
        let mut vectors = Vec::with_capacity(capacity);
        let mut versions: Vec<Option<i8>> = Vec::with_capacity(capacity);
        let mut updated_at_values = Vec::with_capacity(capacity);
        let mut expires_at_values = Vec::with_capacity(capacity);
        let mut filterable_arrays: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
        let mut extra_metadata_data = Vec::with_capacity(capacity);

        // Initialize filterable arrays with capacity
        for filterable_column in &filterable_metadata {
            filterable_arrays.insert(filterable_column.name.clone(), Vec::with_capacity(capacity));
        }

        // Phase 2: Initialize quantized vector arrays if quantization is enabled
        let mut vector_int8_data: Vec<Vec<i8>> = Vec::with_capacity(capacity);
        let mut vector_pq8_data: Vec<Vec<u8>> = Vec::with_capacity(capacity);
        let mut vector_pq4_data: Vec<Vec<u8>> = Vec::with_capacity(capacity);
        let has_quantization = quantization.as_ref().map(|q| q.enabled).unwrap_or(false);

        let filterable_field_names: std::collections::HashSet<String> = filterable_metadata
            .iter()
            .map(|col| col.name.clone())
            .collect();

        for record in records {
            ids.push(record.id.clone());
            collection_ids.push(collection_id.to_string());
            vectors.push(record.vector.clone());

            // Phase 2: Generate quantized versions using unified quantization infrastructure
            if has_quantization {
                // Use collection-specific quantization config for VIPER columnar optimization
                let fp32_vector = &record.vector;

                if let Some(quant_config) = quantization {
                    debug!(
                        "🔧 VIPER: Applying collection quantization config for vector {}",
                        &record.id
                    );

                    info!(
                        "🎯 VIPER: Collection quantization enabled - strategy={:?}",
                        quant_config.strategy
                    );

                    // Apply collection-aware quantization using config parameters
                    let int8_vector = self.quantize_to_int8(fp32_vector, quant_config);
                    vector_int8_data.push(int8_vector);

                    // Use unified quantization engine for PQ8
                    let distance_compute = Arc::new(UnifiedDistanceCompute::default());
                    let codebook_store = Arc::new(InMemoryCodebookStore::new());
                    let quant_engine =
                        UnifiedQuantizationEngine::new(distance_compute, codebook_store);

                    // Default to 8 subvectors for PQ - TODO: get from unified config
                    let num_subvectors = 8usize;

                    let pq8_vector = quant_engine.quantize_to_pq(fp32_vector, num_subvectors, 8)?;
                    vector_pq8_data.push(pq8_vector);

                    let pq4_vector = quant_engine.quantize_to_pq(fp32_vector, num_subvectors, 4)?;
                    vector_pq4_data.push(pq4_vector);
                } else {
                    return Err(anyhow::anyhow!(
                        "Quantization enabled but no collection config provided"
                    ));
                }
            }

            // Process filterable metadata
            for filterable_column in &filterable_metadata {
                let values = filterable_arrays.get_mut(&filterable_column.name).unwrap();
                let value = record
                    .metadata
                    .get(&filterable_column.name)
                    .map(|sql_value| {
                        // Convert SqlValue to serde_json::Value
                        // This is a placeholder - actual implementation depends on SqlValue structure
                        serde_json::Value::String(format!("{:?}", sql_value))
                    })
                    .unwrap_or(serde_json::Value::Null);
                values.push(value);
            }

            // Collect remaining metadata as extra key-value pairs
            let mut extra_kvs = Vec::new();
            for (key, sql_value) in &record.metadata {
                // Skip filterable fields - they're handled dynamically above
                if !filterable_field_names.contains(key) {
                    // Convert SqlValue to string for storage
                    let value_str = format!("{:?}", sql_value); // Placeholder conversion
                    extra_kvs.push((key.clone(), value_str));
                }
            }
            extra_metadata_data.push(extra_kvs);

            // Include version for MVCC, updated_at for audit, and expires_at for TTL support
            // Version should be null if id is null (for append-only vectors)
            if record.id.is_empty() {
                versions.push(None);
            } else {
                versions.push(record.version.map(|v| v as i8));
            }
            // Use timestamp as updated_at (represents either creation or last update time)
            updated_at_values.push(record.timestamp as i64);
            expires_at_values.push(record.expires_at.unwrap_or(0) as i64);
        }

        // Create Arrow arrays with proper List<Float32> for vectors
        let id_array = StringArray::from(ids);
        let collection_array = StringArray::from(collection_ids);

        // 🎯 CRITICAL: Create ListArray for proper row-based f32 vector storage
        // Build ListArray using optimized capacity: records.len() * vector_dimensions
        let total_capacity = records.len() * vector_dimensions;
        let mut builder = arrow_array::builder::ListBuilder::with_capacity(
            arrow_array::builder::Float32Builder::with_capacity(total_capacity),
            records.len(), // Pre-allocate list capacity
        );

        debug!(
            "🔧 VIPER SERIALIZE: Using {} capacity for {} records × {} dimensions",
            total_capacity,
            records.len(),
            vector_dimensions
        );

        let mut _value_idx = 0;
        for record in records {
            let values = builder.values();
            for &val in &record.vector {
                values.append_value(val);
            }
            builder.append(true);
        }

        let vector_array = builder.finish();

        let version_array = arrow_array::Int8Array::from(versions);
        let updated_at_array = Int64Array::from(updated_at_values);
        let expires_at_array = Int64Array::from(expires_at_values);

        // 🎯 DYNAMIC FILTERABLE METADATA: Create Arrow arrays for each filterable column
        let mut dynamic_filterable_arrays: Vec<Arc<dyn Array>> = Vec::new();
        for filterable_column in &filterable_metadata {
            let values = filterable_arrays.get(&filterable_column.name).unwrap();

            let arrow_array: Arc<dyn Array> = {
                use crate::proto::proximadb_v1::FilterableDataType;
                match FilterableDataType::try_from(filterable_column.data_type) {
                    Ok(FilterableDataType::FilterableString) => {
                        let string_values: Vec<Option<String>> = values
                            .iter()
                            .map(|v| {
                                if v.is_null() {
                                    None
                                } else {
                                    v.as_str().map(|s| s.to_string())
                                }
                            })
                            .collect();
                        Arc::new(StringArray::from(string_values))
                    }
                    Ok(FilterableDataType::FilterableInteger) => {
                        let int_values: Vec<Option<i64>> = values
                            .iter()
                            .map(|v| if v.is_null() { None } else { v.as_i64() })
                            .collect();
                        Arc::new(arrow_array::Int64Array::from(int_values))
                    }
                    Ok(FilterableDataType::FilterableFloat) => {
                        let float_values: Vec<Option<f64>> = values
                            .iter()
                            .map(|v| if v.is_null() { None } else { v.as_f64() })
                            .collect();
                        Arc::new(arrow_array::Float64Array::from(float_values))
                    }
                    Ok(FilterableDataType::FilterableBoolean) => {
                        let bool_values: Vec<Option<bool>> = values
                            .iter()
                            .map(|v| if v.is_null() { None } else { v.as_bool() })
                            .collect();
                        Arc::new(arrow_array::BooleanArray::from(bool_values))
                    }
                    Ok(FilterableDataType::FilterableDatetime) => {
                        let ts_values: Vec<Option<i64>> = values
                            .iter()
                            .map(|v| if v.is_null() { None } else { v.as_i64() })
                            .collect();
                        Arc::new(arrow_array::TimestampMicrosecondArray::from(ts_values))
                    }
                    Ok(FilterableDataType::FilterableArrayString)
                    | Ok(FilterableDataType::FilterableArrayInteger)
                    | Ok(FilterableDataType::FilterableArrayFloat) => {
                        // For array types, serialize as JSON strings for now
                        let json_values: Vec<Option<String>> = values
                            .iter()
                            .map(|v| {
                                if v.is_null() {
                                    None
                                } else {
                                    Some(v.to_string())
                                }
                            })
                            .collect();
                        Arc::new(StringArray::from(json_values))
                    }
                    _ => {
                        // Default to string for unknown types
                        let string_values: Vec<Option<String>> = values
                            .iter()
                            .map(|v| {
                                if v.is_null() {
                                    None
                                } else {
                                    Some(v.to_string())
                                }
                            })
                            .collect();
                        Arc::new(StringArray::from(string_values))
                    }
                }
            };

            dynamic_filterable_arrays.push(arrow_array);
        }

        // 🎯 EXTRA METADATA: Serialize as list of key-value pairs for structured data management
        use arrow_array::builder::{ListBuilder, StringBuilder, StructBuilder};

        let mut extra_meta_builder = ListBuilder::new(StructBuilder::new(
            vec![
                Field::new("key", DataType::Utf8, false),
                Field::new("value", DataType::Utf8, false),
            ],
            vec![
                Box::new(StringBuilder::new()),
                Box::new(StringBuilder::new()),
            ],
        ));

        for kvs in extra_metadata_data {
            if kvs.is_empty() {
                extra_meta_builder.append(false); // NULL value for empty metadata
            } else {
                let struct_builder = extra_meta_builder.values();

                for (key, value) in kvs {
                    struct_builder
                        .field_builder::<StringBuilder>(0)
                        .unwrap()
                        .append_value(key);
                    struct_builder
                        .field_builder::<StringBuilder>(1)
                        .unwrap()
                        .append_value(value);
                    struct_builder.append(true);
                }
                extra_meta_builder.append(true);
            }
        }

        let extra_meta_array = extra_meta_builder.finish();

        // Combine all arrays into columns
        let mut columns: Vec<Arc<dyn Array>> = vec![
            Arc::new(id_array),
            Arc::new(collection_array),
            Arc::new(vector_array),
            Arc::new(version_array),
            Arc::new(updated_at_array),
            Arc::new(expires_at_array),
        ];

        info!(
            "🔍 VIPER FLUSH DEBUG: Base columns count: {}",
            columns.len()
        );

        // Add dynamic filterable columns
        info!(
            "🔍 VIPER FLUSH DEBUG: Adding {} dynamic filterable arrays",
            dynamic_filterable_arrays.len()
        );
        columns.extend(dynamic_filterable_arrays);

        // Phase 2: Add quantized vector columns if quantization is enabled
        if has_quantization {
            // Create INT8 quantized vector array
            let mut int8_list_builder = ListBuilder::new(Int8Builder::new());
            for int8_vector in vector_int8_data {
                let value_builder = int8_list_builder.values();
                for &val in &int8_vector {
                    value_builder.append_value(val);
                }
                int8_list_builder.append(true);
            }
            let int8_array = int8_list_builder.finish();
            columns.push(Arc::new(int8_array));

            // Create PQ8 quantized vector array
            let mut pq8_list_builder = ListBuilder::new(UInt8Builder::new());
            for pq8_vector in vector_pq8_data {
                let value_builder = pq8_list_builder.values();
                for &val in &pq8_vector {
                    value_builder.append_value(val);
                }
                pq8_list_builder.append(true);
            }
            let pq8_array = pq8_list_builder.finish();
            columns.push(Arc::new(pq8_array));

            // Create PQ4 quantized vector array
            let mut pq4_list_builder = ListBuilder::new(UInt8Builder::new());
            for pq4_vector in vector_pq4_data {
                let value_builder = pq4_list_builder.values();
                for &val in &pq4_vector {
                    value_builder.append_value(val);
                }
                pq4_list_builder.append(true);
            }
            let pq4_array = pq4_list_builder.finish();
            columns.push(Arc::new(pq4_array));

            info!(
                "📦 VIPER FLUSH: Added {} quantized vector columns (INT8, PQ8, PQ4)",
                3
            );
        }

        // Add extra_meta column
        columns.push(Arc::new(extra_meta_array));

        // Debug: Log the schema and columns count
        info!(
            "🔍 VIPER FLUSH DEBUG: Schema has {} fields, columns array has {} items",
            schema.fields().len(),
            columns.len()
        );
        info!(
            "🔍 VIPER FLUSH DEBUG: Schema fields: {:?}",
            schema.fields().iter().map(|f| f.name()).collect::<Vec<_>>()
        );
        info!(
            "🔍 VIPER FLUSH DEBUG: has_quantization={}, filterable_metadata.len()={}",
            has_quantization,
            filterable_metadata.len()
        );

        // Create RecordBatch
        let batch = RecordBatch::try_new(schema, columns)?;

        info!(
            "📝 VIPER FLUSH: Created RecordBatch with {} rows for {} records",
            batch.num_rows(),
            records.len()
        );

        // Verify batch has correct number of rows
        if batch.num_rows() != records.len() {
            error!(
                "❌ VIPER FLUSH: Batch row count mismatch! Expected {}, got {}",
                records.len(),
                batch.num_rows()
            );
        }

        // Create a temporary file for StreamingParquetWriter
        let temp_dir = std::env::temp_dir();
        let temp_file_path = temp_dir.join(format!(
            "viper_flush_{}.parquet.tmp",
            crate::utils::uuid::Uuid::new_v4()
        ));
        debug!("Creating temporary Parquet file: {:?}", temp_file_path);

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
                compression_algorithm,
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
            props_builder = self.apply_mixed_compression_strategy(props_builder, &batch)?;
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
                parquet::schema::types::ColumnPath::from("vector"),
                parquet::basic::Encoding::RLE,
            );
            debug!("🔧 VIPER: Using BIT_PACKED encoding for quantized vectors");
        } else {
            // For full precision f32 vectors
            // BYTE_STREAM_SPLIT splits floating point bytes for better compression
            props_builder = props_builder.set_column_encoding(
                parquet::schema::types::ColumnPath::from("vector"),
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
            .set_column_dictionary_enabled(parquet::schema::types::ColumnPath::from("id"), true);

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
        let writer_config = ParquetWriterConfig {
            row_group_size: viper_config.row_group_size,
            enable_bloom_filters: true, // Enable for efficient ID lookups
            bloom_filter_fpp: 0.01,
            expected_ndv: Some(records.len()),
            bloom_filter_columns: vec!["id".to_string()],
            compression: compression_algorithm,
            enable_column_statistics: true,
            enable_page_index: true,
            enable_column_index: true,
            enable_offset_index: true,
            page_index_granularity: 10000,
            enable_dictionary: true,
            dictionary_threshold: 0.5,
            enable_delta_encoding: true,
            quantization: crate::proto::proximadb_v1::QuantizationConfig {
                enabled: has_quantization,
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
                enable_int8: has_quantization,
                enable_pq: has_quantization,
                pq_segments: 32,
                pq_bits: 8,
                pq_codebooks: 256,
                binary_threshold: 0.5,
                int8_threshold: 0.3,
                pq_threshold: 0.1,
            },
            id_less_storage: false, // Keep IDs for customer APIs
            write_batch_size: 10000,
            page_size: 1024 * 1024, // 1MB pages
            enable_byte_stream_split: !is_quantized,
            enable_pq_sorting: false,
            pq_sorting_segments: 16,
            pq_sorting_codebook_size: 256,
            enable_native_metadata: true,
            metadata_inference_samples: 100,
        };

        // Get dimension from the first record
        let dimension = if !records.is_empty() {
            records[0].vector.len()
        } else {
            return Ok(Vec::new());
        };

        // Create StreamingParquetWriter with temp file
        let mut writer = StreamingParquetWriter::new(&temp_file_path, dimension, writer_config)?;

        // Write all records using the columnar writer's optimized batching
        writer.write_batch(records).await?;

        // Finalize the writer to flush all data
        let stats = writer.finalize().await?;

        debug!("   ✅ VIPER Parquet written using StreamingParquetWriter:");
        debug!("      File: {:?}", temp_file_path);
        debug!("      Records: {}", stats.0.total_records);
        debug!("      Row groups: {}", stats.0.total_row_groups);
        debug!("      File size: {} bytes", stats.0.file_size);
        debug!("      Compression ratio: {:.2}", stats.0.compression_ratio);
        debug!("      Bloom filters: {}", stats.0.bloom_filter_count);

        info!(
            "📝 VIPER FLUSH: Created Parquet file with bloom filters at {:?}",
            temp_file_path
        );

        // Return temp file path for atomic move
        // Wrap in a special marker to indicate this is a file path, not buffer data
        let mut result = vec![0xFA, 0xCE, 0xF1, 0x1E]; // Magic bytes to indicate file path mode
        result.extend_from_slice(temp_file_path.to_string_lossy().as_bytes());
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
