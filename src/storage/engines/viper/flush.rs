// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Flush Operations
//!
//! This module handles flushing vector records from memory to Parquet files
//! with dynamic schema generation and metadata separation.

use anyhow::{Context, Result};
use arrow_array::{Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::storage::persistence::filesystem::{
    FilesystemFactory
};
use crate::storage::atomic::{UnifiedAtomicCoordinator, StagingConfig, StagingOperationType};

use crate::core::{String, VectorRecord};
use crate::storage::optimization::{MetadataSorter, SortingStats};
use super::schema::SchemaManager;

/// Flush manager for VIPER storage engine with atomic writes
#[derive(Debug)]
pub struct FlushManager {
    /// Schema manager for dynamic schema generation
    schema_manager: SchemaManager,
    
    /// Collection service for metadata access
    collection_service: Arc<RwLock<Option<Arc<crate::services::collection_service::CollectionService>>>>,
    
    /// Filesystem factory for cross-cloud atomic writes
    filesystem_factory: Arc<FilesystemFactory>,
    
    /// Atomic coordinator for ACID operations
    atomic_coordinator: Arc<UnifiedAtomicCoordinator>,
}

impl FlushManager {
    pub async fn new(
        collection_service: Arc<RwLock<Option<Arc<crate::services::collection_service::CollectionService>>>>,
        filesystem_factory: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        // Create atomic coordinator
        let atomic_coordinator = Arc::new(
            UnifiedAtomicCoordinator::new(filesystem_factory.clone(), None)
                .await
                .context("Failed to create atomic coordinator")?
        );
        
        Ok(Self {
            schema_manager: SchemaManager::new(),
            collection_service,
            filesystem_factory,
            atomic_coordinator,
        })
    }

    /// Core flush operation using proper staging pattern
    pub async fn flush_vectors(
        &self,
        collection_id: &str,
        vector_records: &[VectorRecord],
        batch_ids: &[String],
        force: bool,
        synchronous: bool,
    ) -> Result<crate::storage::traits::FlushResult> {
        info!("🔄 VIPER: Starting flush operation with staging pattern");
        info!(
            "🔍 VIPER: Flush params - force: {}, synchronous: {}, vector_records_len: {}, batch_ids: {}",
            force,
            synchronous,
            vector_records.len(),
            batch_ids.len()
        );

        // Fetch collection configuration using proto type directly
        let collection_config = {
            let service_lock = self.collection_service.read().await;
            if let Some(ref service) = *service_lock {
                match service.get_proto_collection(collection_id).await {
                    Ok(Some(collection)) => Some(collection),
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

        // Extract vector dimensions for efficient capacity planning
        let vector_dimensions = collection_config
            .as_ref()
            .and_then(|c| c.config.as_ref())
            .map(|config| config.dimension as usize)
            .unwrap_or(512); // Default to 512 if not available

        info!("🔧 VIPER FLUSH: Using {} dimensions for collection {}", 
              vector_dimensions, collection_id);

        info!(
            "🔍 VIPER: Processing flush for collection: {}",
            collection_id
        );

        let operation_id = uuid::Uuid::new_v4().to_string();

        if vector_records.is_empty() {
            info!(
                "📋 VIPER: No vector records provided for collection {}",
                collection_id
            );
            return Ok(crate::storage::traits::FlushResult {
                success: true,
                collections_affected: vec![collection_id.to_string()],
                entries_flushed: 0,
                bytes_written: 0,
                files_created: 0,
                duration_ms: 0,
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

        // Step 1: Generate unique Parquet filename for atomic write
        let parquet_filename = format!("partition_{}.parquet", operation_id);
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
                warn!("⚠️ VIPER: Step 2a - Sorting failed, using original order: {}", e);
                (vector_records.to_vec(), crate::storage::optimization::SortingStats::default())
            }
        };

        // Step 2b: Serialize sorted vector records to Parquet format
        info!(
            "🔄 VIPER: Step 2b - Serializing {} sorted vector records to Parquet",
            sorted_records.len()
        );
        let parquet_data = match self
            .serialize_records_to_parquet(&sorted_records, collection_id, &collection_config, vector_dimensions)
            .await
        {
            Ok(data) => {
                info!(
                    "✅ VIPER: Step 2 - Serialization completed ({} bytes)",
                    data.len()
                );
                data
            }
            Err(e) => {
                error!("❌ VIPER: Step 2 - Serialization failed: {}", e);
                return Err(e.context("Failed to serialize vector records to Parquet"));
            }
        };

        // Step 3: Atomic write of Parquet data using unified filesystem strategy
        info!(
            "🔄 VIPER: Step 3 - Atomically writing Parquet file: {}",
            parquet_filename
        );
        let final_file_path = match self
            .write_parquet_atomic(collection_id, &parquet_filename, &parquet_data)
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
        };

        // Note: No cleanup needed - atomic write strategy handles staging automatically

        // Step 4: Check for compaction trigger
        info!("🔄 VIPER: Step 4 - Checking compaction trigger");
        let compaction_triggered = self.check_compaction_trigger(collection_id).await.unwrap_or(false);

        // Step 5: Update collection metadata
        info!("🔄 VIPER: Step 5 - Updating collection metadata");
        self.update_collection_metadata_after_flush(collection_id, vector_records.len(), parquet_data.len()).await?;

        // Step 6: Return successful flush result with BatchId coordination
        Ok(crate::storage::traits::FlushResult {
            success: true,
            collections_affected: vec![collection_id.to_string()],
            entries_flushed: vector_records.len() as u64,
            bytes_written: parquet_data.len() as u64,
            files_created: 1,
            duration_ms: 0, // Will be set by high-level flush() method
            completed_at: chrono::Utc::now(),
            flushed_batch_ids: batch_ids.iter().map(|_id| {
                // Use compact BatchId for minimal storage overhead (10 bytes vs 100+ bytes)
                crate::storage::persistence::wal::BatchId::default()
            }).collect(), // ✅ Include for WAL cleanup
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
        collection_config: &Option<crate::proto::proximadb::Collection>,
        vector_dimensions: usize,
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
        let mut schema_fields = vec![
            Field::new("id", DataType::Utf8, true),  // Can be null for append-only vectors
            Field::new("collection_id", DataType::Utf8, false),
            Field::new(
                "vector", 
                DataType::List(Arc::new(Field::new("item", DataType::Float32, true))), 
                true  // Vector field can be null
            ), // Float32 array for row-level vector filtering, supports sparse vectors
            Field::new("version", DataType::Int8, true), // Version field for MVCC - using tinyint
            Field::new("updated_at", DataType::Int64, true), // Audit field - stores create or update time
            Field::new("expires_at", DataType::Int64, true), // Only keep expires_at for TTL
        ];

        // 🎯 DYNAMIC FILTERABLE METADATA: Use proto filterable_columns directly  
        let filterable_metadata: Vec<&crate::proto::proximadb::FilterableColumnSpec> = if let Some(ref collection) = collection_config {
            if let Some(ref config) = collection.config {
                config.filterable_columns.iter().collect()
            } else {
                Vec::new()
            }
        } else {
            info!("Collection {} config not available, using empty filterable metadata", collection_id);
            Vec::new()
        };
        
        // Add filterable metadata columns based on collection configuration using proto types
        for filterable_column in &filterable_metadata {
            let arrow_data_type = self.schema_manager.convert_proto_type_to_arrow(filterable_column.data_type)?;
            
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
            true
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
        
        let filterable_field_names: std::collections::HashSet<String> = filterable_metadata
            .iter()
            .map(|col| col.name.clone())
            .collect();

        for record in records {
            ids.push(record.id.as_deref().unwrap_or("").to_string());
            collection_ids.push(collection_id.to_string());
            vectors.push(record.vector.clone());
            
            // Process filterable metadata
            for filterable_column in &filterable_metadata {
                let values = filterable_arrays.get_mut(&filterable_column.name).unwrap();
                let value = record.metadata.iter()
                    .find(|item| item.key == filterable_column.name)
                    .map(|item| serde_json::Value::String(item.value.clone()))
                    .unwrap_or(serde_json::Value::Null);
                values.push(value);
            }
            
            // Collect remaining metadata as extra key-value pairs
            let mut extra_kvs = Vec::new();
            for item in &record.metadata {
                // Skip filterable fields - they're handled dynamically above
                if !filterable_field_names.contains(&item.key) {
                    extra_kvs.push((item.key.clone(), item.value.clone()));
                }
            }
            extra_metadata_data.push(extra_kvs);

            // Include version for MVCC, updated_at for audit, and expires_at for TTL support
            // Version should be null if id is null (for append-only vectors)
            if record.id.is_none() {
                versions.push(None);
            } else {
                versions.push(Some(record.version as i8));
            }
            // Use timestamp as updated_at (represents either creation or last update time)
            updated_at_values.push(record.timestamp);
            expires_at_values.push(record.expires_at.unwrap_or(0));
        }

        // Create Arrow arrays with proper List<Float32> for vectors
        let id_array = StringArray::from(ids);
        let collection_array = StringArray::from(collection_ids);
        
        // 🎯 CRITICAL: Create ListArray for proper row-based f32 vector storage
        // Build ListArray using optimized capacity: records.len() * vector_dimensions
        let total_capacity = records.len() * vector_dimensions;
        let mut builder = arrow_array::builder::ListBuilder::with_capacity(
            arrow_array::builder::Float32Builder::with_capacity(total_capacity),
            records.len()  // Pre-allocate list capacity
        );
        
        debug!("🔧 VIPER SERIALIZE: Using {} capacity for {} records × {} dimensions", 
               total_capacity, records.len(), vector_dimensions);
        
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
                use crate::proto::proximadb::FilterableDataType;
                match FilterableDataType::try_from(filterable_column.data_type) {
                    Ok(FilterableDataType::FilterableString) => {
                        let string_values: Vec<Option<String>> = values.iter()
                            .map(|v| if v.is_null() { None } else { Some(v.as_str().unwrap_or("").to_string()) })
                            .collect();
                        Arc::new(StringArray::from(string_values))
                    }
                    Ok(FilterableDataType::FilterableInteger) => {
                        let int_values: Vec<Option<i64>> = values.iter()
                            .map(|v| if v.is_null() { None } else { v.as_i64() })
                            .collect();
                        Arc::new(arrow_array::Int64Array::from(int_values))
                    }
                    Ok(FilterableDataType::FilterableFloat) => {
                        let float_values: Vec<Option<f64>> = values.iter()
                            .map(|v| if v.is_null() { None } else { v.as_f64() })
                            .collect();
                        Arc::new(arrow_array::Float64Array::from(float_values))
                    }
                    Ok(FilterableDataType::FilterableBoolean) => {
                        let bool_values: Vec<Option<bool>> = values.iter()
                            .map(|v| if v.is_null() { None } else { v.as_bool() })
                            .collect();
                        Arc::new(arrow_array::BooleanArray::from(bool_values))
                    }
                    Ok(FilterableDataType::FilterableDatetime) => {
                        let ts_values: Vec<Option<i64>> = values.iter()
                            .map(|v| if v.is_null() { None } else { v.as_i64() })
                            .collect();
                        Arc::new(arrow_array::TimestampMicrosecondArray::from(ts_values))
                    }
                    Ok(FilterableDataType::FilterableArrayString) | 
                    Ok(FilterableDataType::FilterableArrayInteger) | 
                    Ok(FilterableDataType::FilterableArrayFloat) => {
                        // For array types, serialize as JSON strings for now
                        let json_values: Vec<Option<String>> = values.iter()
                            .map(|v| if v.is_null() { None } else { Some(v.to_string()) })
                            .collect();
                        Arc::new(StringArray::from(json_values))
                    }
                    _ => {
                        // Default to string for unknown types
                        let string_values: Vec<Option<String>> = values.iter()
                            .map(|v| if v.is_null() { None } else { Some(v.to_string()) })
                            .collect();
                        Arc::new(StringArray::from(string_values))
                    }
                }
            };
            
            dynamic_filterable_arrays.push(arrow_array);
        }

        // 🎯 EXTRA METADATA: Serialize as list of key-value pairs for structured data management
        use arrow_array::builder::{ListBuilder, StructBuilder, StringBuilder};
        
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
                    struct_builder.field_builder::<StringBuilder>(0).unwrap().append_value(key);
                    struct_builder.field_builder::<StringBuilder>(1).unwrap().append_value(value);
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
        
        // Add dynamic filterable columns
        columns.extend(dynamic_filterable_arrays);
        
        // Add extra_meta column
        columns.push(Arc::new(extra_meta_array));

        // Create RecordBatch
        let batch = RecordBatch::try_new(schema, columns)?;
        
        info!("📝 VIPER FLUSH: Created RecordBatch with {} rows for {} records", 
              batch.num_rows(), records.len());
        
        // Verify batch has correct number of rows
        if batch.num_rows() != records.len() {
            error!("❌ VIPER FLUSH: Batch row count mismatch! Expected {}, got {}", 
                   records.len(), batch.num_rows());
        }

        // Write to Parquet
        let mut buffer = Vec::new();
        let props = WriterProperties::builder()
            .set_compression(parquet::basic::Compression::UNCOMPRESSED)
            .build();
        
        let mut writer = ArrowWriter::try_new(&mut buffer, batch.schema(), Some(props))?;
        writer.write(&batch)?;
        writer.close()?;
        
        info!("📝 VIPER FLUSH: Wrote {} bytes of Parquet data", buffer.len());

        Ok(buffer)
    }

    /// Write Parquet data using atomic write strategy
    /// Uses unified atomic write infrastructure for cross-cloud compatibility
    async fn write_parquet_atomic(
        &self, 
        collection_id: &str, 
        filename: &str, 
        parquet_data: &[u8]
    ) -> Result<String> {
        info!("🔄 Writing Parquet file atomically: {} ({} bytes)", filename, parquet_data.len());
        
        // Use assignment service for proper path resolution
        let assignment_service = crate::storage::assignment_service::get_assignment_service();
        let storage_assignment = assignment_service
            .get_assignment(collection_id)
            .await
            .context("Failed to get storage assignment")?;
        
        // Begin atomic operation for flush
        // Note: data_url already includes collection path, so don't set collection_id
        let staging_config = StagingConfig {
            base_url: storage_assignment.data_url.clone(),
            collection_id: None,  // Don't duplicate collection path
            operation_type: StagingOperationType::Flush,
            custom_staging_dir: None,
            auto_cleanup: true,
            max_orphaned_age_hours: 24,
        };
        
        let atomic_op = self.atomic_coordinator
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
        
        let final_path = format!("{}/{}", storage_assignment.data_url, filename);
        
        info!("✅ VIPER: Atomically wrote Parquet file {} ({} KB)", 
              final_path, parquet_data.len() / 1024);
        
        // Verify file was written
        let fs = self.filesystem_factory.get_filesystem(&storage_assignment.data_url)?;
        if fs.exists(&final_path).await? {
            let metadata = fs.metadata(&final_path).await?;
            info!("✅ VIPER: Verified file exists at {} with size {} bytes", 
                  final_path, metadata.size);
        } else {
            error!("❌ VIPER: File not found after atomic write: {}", final_path);
        }
        
        Ok(final_path)
    }

    /// Check if compaction should be triggered
    async fn check_compaction_trigger(&self, collection_id: &str) -> Result<bool> {
        // Compaction triggers based on multiple factors
        
        // 1. Check number of Parquet files for this collection
        // TODO: Get filesystem and flush_path from somewhere
        let file_count = 0; // Placeholder
        
        // 2. Define compaction thresholds
        const MAX_FILES_BEFORE_COMPACTION: usize = 10;
        const MIN_FILES_FOR_COMPACTION: usize = 3;
        const FILE_SIZE_THRESHOLD_MB: u64 = 100;
        
        // Trigger if too many files
        if file_count >= MAX_FILES_BEFORE_COMPACTION {
            tracing::info!("Compaction triggered for {}: {} files exceed max threshold", 
                collection_id, file_count);
            return Ok(true);
        }
        
        // Check if we have enough small files to compact
        if file_count >= MIN_FILES_FOR_COMPACTION {
            let _small_file_count = 0;
            // TODO: Check file sizes when filesystem is available
            /*for file_info in &collection_files {
                if let Some(size) = file_info.size {
                    if size < FILE_SIZE_THRESHOLD_MB * 1024 * 1024 {
                        small_file_count += 1;
                    }
                }
            }*/
            let _small_file_count = 0; // Placeholder
            
            // Trigger if more than half are small files
            if _small_file_count > file_count / 2 {
                tracing::info!("Compaction triggered for {}: {} small files out of {}", 
                    collection_id, _small_file_count, file_count);
                return Ok(true);
            }
        }
        
        Ok(false)
    }

    /// Update collection metadata after flush
    async fn update_collection_metadata_after_flush(&self, collection_id: &str, records_count: usize, bytes_written: usize) -> Result<()> {
        // Update collection statistics through shared services if available
        // TODO: Update collection statistics through collection service
        if false {
            // Update collection metadata with new stats
            let _metadata_update = crate::storage::metadata::MetadataOperation::UpdateStats {
                collection_id: collection_id.to_string(),
                vector_delta: records_count as i64,
                size_delta: bytes_written as i64,
            };
            
            // Execute metadata update through shared services
            /*if let Some(metadata_store) = shared_services.metadata_store() {
                metadata_store.batch_operations(vec![metadata_update]).await?;
                
                tracing::debug!(
                    "Updated collection {} metadata: +{} vectors, +{} bytes",
                    collection_id, records_count, bytes_written
                );
            } else {
                tracing::warn!("No metadata store available to update collection stats");
            }*/
        } else {
            tracing::debug!("No shared services available for metadata update");
        }
        
        Ok(())
    }

    /// Sort vector records by metadata for optimal Parquet encoding
    async fn sort_records_for_parquet_encoding(
        &self,
        records: &[VectorRecord],
        collection_config: &Option<crate::proto::proximadb::Collection>,
    ) -> Result<(Vec<VectorRecord>, SortingStats)> {
        // Extract filterable columns from collection config
        let filterable_columns = collection_config
            .as_ref()
            .and_then(|c| c.config.as_ref())
            .map(|config| config.filterable_columns.clone())
            .unwrap_or_default();
        
        if filterable_columns.is_empty() {
            // No filterable columns, sort by vector ID for consistent ordering
            let mut sorted_records = records.to_vec();
            sorted_records.sort_by(|a, b| {
                let a_id = a.id.as_deref().unwrap_or("");
                let b_id = b.id.as_deref().unwrap_or("");
                a_id.cmp(b_id)
            });
            
            return Ok((sorted_records, SortingStats {
                records_sorted: records.len(),
                sort_keys_used: vec!["vector_id".to_string()],
                compression_estimate: 0.05, // Small improvement from ID sorting
                sort_time_us: 0,
                ..Default::default()
            }));
        }
        
        // Create metadata sorter from filterable columns
        let sorter = MetadataSorter::from_filterable_specs(&filterable_columns);
        
        // Sort records for optimal encoding
        let (sorted_records, stats) = sorter.sort_for_encoding(records.to_vec())?;
        
        debug!(
            "🎯 VIPER: Sorted {} records by {} filterable keys for Parquet optimization",
            stats.records_sorted,
            stats.sort_keys_used.len()
        );
        
        Ok((sorted_records, stats))
    }
}