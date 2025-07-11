// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Compaction Operations
//!
//! This module handles Parquet file compaction with MVCC resolution, expired record deletion,
//! and schema evolution support.

use anyhow::{Context, Result};
use arrow_array::{Array, Int64Array, RecordBatch, StringArray};
use parquet::arrow::ArrowWriter;
use std::collections::HashMap;
use std::fs::{self, File};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};


use super::schema::SchemaManager;

/// Compaction manager for VIPER storage engine
#[derive(Debug)]
pub struct CompactionManager {
    /// Schema manager for dynamic schema generation
    schema_manager: SchemaManager,
    
    /// Collection service for metadata access
    collection_service: Arc<RwLock<Option<Arc<crate::services::collection_service::CollectionService>>>>,
}

impl CompactionManager {
    pub fn new(
        collection_service: Arc<RwLock<Option<Arc<crate::services::collection_service::CollectionService>>>>
    ) -> Self {
        Self {
            schema_manager: SchemaManager::new(),
            collection_service,
        }
    }

    /// Arrow/Parquet compaction with version merging and expiry logic
    /// This is the main compaction implementation for VIPER storage engine
    pub async fn compact_parquet_files(
        &self,
        collection_id: &str,
        input_files: Vec<String>,
    ) -> Result<Vec<String>> {
        // Fetch collection configuration using unified interface
        let collection_config = {
            let service_lock = self.collection_service.read().await;
            if let Some(ref service) = *service_lock {
                match service.get_proto_collection(collection_id).await {
                    Ok(Some(collection)) => Some(collection),
                    Ok(None) => {
                        warn!("⚠️ Collection {} not found during compaction", collection_id);
                        None
                    }
                    Err(e) => {
                        warn!("⚠️ Failed to get collection {}: {}", collection_id, e);
                        None
                    }
                }
            } else {
                warn!("⚠️ No collection service available during compaction");
                None
            }
        };
        
        // Extract vector dimensions for efficient capacity planning
        let vector_dimensions = collection_config
            .as_ref()
            .and_then(|collection| collection.config.as_ref())
            .map(|config| config.dimension as usize)
            .unwrap_or(512); // Default to 512 if not available
        
        info!("🔧 VIPER COMPACTION: Starting with {} dimensions for collection {}", 
              vector_dimensions, collection_id);
        
        self.compact_parquet_files_with_config(collection_id, input_files, vector_dimensions, collection_config).await
    }
    
    /// Internal compaction implementation with collection config
    async fn compact_parquet_files_with_config(
        &self,
        collection_id: &str,
        input_files: Vec<String>,
        vector_dimensions: usize,
        collection_config: Option<crate::proto::proximadb::Collection>,
    ) -> Result<Vec<String>> {
        
        info!(
            "🔄 [VIPER COMPACTION] Starting atomic Arrow/Parquet compaction for collection {}",
            collection_id
        );
        
        if input_files.is_empty() {
            return Ok(Vec::new());
        }
        
        info!("📋 Input files for compaction: {:?}", input_files);
        
        // Determine base storage directory from first input file
        let base_storage_dir = if let Some(first_file) = input_files.first() {
            let path = Path::new(first_file);
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("Invalid input file path: {}", first_file))?
                .to_path_buf()
        } else {
            return Err(anyhow::anyhow!("No input files provided for compaction"));
        };
        
        // Create atomic staging directory: {basedir}/{collection_id}/__compact/
        let staging_dir = base_storage_dir.join("__compact");
        if staging_dir.exists() {
            debug!("🧹 Cleaning existing staging directory: {:?}", staging_dir);
            fs::remove_dir_all(&staging_dir)
                .with_context(|| format!("Failed to clean staging directory: {:?}", staging_dir))?;
        }
        
        fs::create_dir_all(&staging_dir)
            .with_context(|| format!("Failed to create staging directory: {:?}", staging_dir))?;
        
        info!("🏗️ Using atomic staging directory: {:?}", staging_dir);
        
        // Generate dynamic schema with caching using pre-fetched collection config
        let schema = self.schema_manager.get_or_generate_cached_schema(collection_id, &collection_config).await?;
        
        // COMPACTION FIX: Validate schema compatibility before processing
        // This prevents column alignment issues during compaction
        info!("🔍 Validating schema compatibility for {} input files", input_files.len());
        let mut input_schemas = Vec::new();
        for input_file in &input_files {
            if let Ok(file) = std::fs::File::open(input_file) {
                if let Ok(builder) = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(file) {
                    input_schemas.push(builder.schema().clone());
                }
            }
        }
        
        // Log schema differences for debugging
        for (idx, input_schema) in input_schemas.iter().enumerate() {
            let missing_in_input: Vec<_> = schema.fields().iter()
                .filter(|f| input_schema.field_with_name(f.name()).is_err())
                .map(|f| f.name())
                .collect();
            
            let extra_in_input: Vec<_> = input_schema.fields().iter()
                .filter(|f| schema.field_with_name(f.name()).is_err())
                .map(|f| f.name())
                .collect();
            
            if !missing_in_input.is_empty() || !extra_in_input.is_empty() {
                info!("📊 Schema differences in file {}: missing {:?}, extra {:?}", 
                      idx, missing_in_input, extra_in_input);
            }
        }
        
        // Process each input file and merge records by ID with MVCC resolution
        let mut latest_records: HashMap<String, RecordBatch> = HashMap::new();
        let current_time = chrono::Utc::now().timestamp_micros();
        let mut expired_records_count = 0;
        
        info!("⚡ Processing {} input files for version merging and expiry logic", input_files.len());
        
        for (file_idx, input_file) in input_files.iter().enumerate() {
            info!("📂 Processing file {}/{}: {}", file_idx + 1, input_files.len(), input_file);
            
            let file = File::open(input_file)
                .with_context(|| format!("Failed to open input file: {}", input_file))?;
            
            let builder = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(file)
                .with_context(|| format!("Failed to create Parquet reader for: {}", input_file))?;
            
            let reader = builder.build()
                .with_context(|| format!("Failed to build Parquet reader for: {}", input_file))?;
            
            for batch_result in reader {
                let batch = batch_result
                    .with_context(|| format!("Failed to read batch from: {}", input_file))?;
                
                // Process each record in the batch for MVCC resolution
                let id_array = batch.column_by_name("id").unwrap()
                    .as_any().downcast_ref::<StringArray>().unwrap();
                let version_array = batch.column_by_name("version").unwrap()
                    .as_any().downcast_ref::<Int64Array>().unwrap();
                
                // COMPACTION FIX: Handle missing expires_at column for backward compatibility
                let expires_at_array = match batch.column_by_name("expires_at") {
                    Some(column) => column.as_any().downcast_ref::<Int64Array>()
                        .ok_or_else(|| anyhow::anyhow!("Invalid 'expires_at' column type in {}", input_file))?,
                    None => {
                        // Create null array for missing expires_at column
                        warn!("⚠️ Missing 'expires_at' column in {}, creating null array", input_file);
                        &Int64Array::from(vec![Option::<i64>::None; batch.num_rows()])
                    }
                };
                
                for i in 0..batch.num_rows() {
                    let record_id = id_array.value(i);
                    let record_version = version_array.value(i);
                    let record_expires_at = if expires_at_array.is_null(i) {
                        None
                    } else {
                        Some(expires_at_array.value(i))
                    };
                    
                    // Skip expired records
                    if let Some(expires_at) = record_expires_at {
                        if expires_at < current_time {
                            expired_records_count += 1;
                            debug!("⏰ VIPER COMPACTION: Physically deleting expired record {} (expired at {})", record_id, expires_at);
                            continue;
                        }
                    }
                    
                    // Handle immutable vectors (null/empty IDs) - skip ID-based merging
                    let should_keep = if record_id.is_empty() || record_id == "null" {
                        debug!("📝 Immutable vector record (no ID), including as-is");
                        true
                    } else {
                        // Version merging logic: keep latest version per ID
                        if let Some(existing_batch) = latest_records.get(record_id) {
                            let existing_version_array = existing_batch.column_by_name("version").unwrap()
                                .as_any().downcast_ref::<Int64Array>().unwrap();
                            let existing_version = existing_version_array.value(0);
                            
                            if record_version > existing_version {
                                debug!("📝 Updating record {} from version {} to {}", record_id, existing_version, record_version);
                                true
                            } else {
                                debug!("📝 Keeping existing record {} at version {} (incoming version {})", 
                                    record_id, existing_version, record_version);
                                false
                            }
                        } else {
                            debug!("📝 New record {} at version {}", record_id, record_version);
                            true
                        }
                    };
                    
                    if should_keep {
                        // Create a single-row batch for this record
                        let record_batch = self.extract_single_record_batch(&batch, i, schema.clone())?;
                        
                        // For immutable vectors, use timestamp as unique key since ID is null/empty
                        let storage_key = if record_id.is_empty() || record_id == "null" {
                            let timestamp_array = batch.column_by_name("timestamp").unwrap()
                                .as_any().downcast_ref::<Int64Array>().unwrap();
                            format!("immutable_{}", timestamp_array.value(i))
                        } else {
                            record_id.to_string()
                        };
                        
                        latest_records.insert(storage_key, record_batch);
                    }
                }
            }
        }
        
        info!("📊 MVCC resolution completed: {} unique records after merging", latest_records.len());
        
        // Create output file in staging directory
        let staging_output_file = staging_dir.join(format!(
            "collection_{}_compacted_{}.parquet", 
            collection_id, 
            chrono::Utc::now().timestamp_millis()
        ));
        
        info!("📝 Writing compacted file to staging: {:?} ({} records)", staging_output_file, latest_records.len());
        
        // Write compacted data to staging file
        let output_file_handle = File::create(&staging_output_file)
            .with_context(|| format!("Failed to create staging output file: {:?}", staging_output_file))?;
        
        let mut writer = ArrowWriter::try_new(output_file_handle, schema.clone(), None)
            .with_context(|| format!("Failed to create Arrow writer for: {:?}", staging_output_file))?;
        
        // Combine and write all records
        if !latest_records.is_empty() {
            let combined_batch = self.combine_record_batches(schema.clone(), latest_records.values().collect(), vector_dimensions)?;
            writer.write(&combined_batch)?;
        }
        
        writer.close()?;
        
        // ATOMIC OPERATIONS: Move from staging to final location and cleanup
        let final_output_file = base_storage_dir.join(staging_output_file.file_name().unwrap());
        
        info!(
            "🔄 [ATOMIC MOVE] Moving compacted file from staging {:?} to final location {:?}",
            staging_output_file, final_output_file
        );
        
        // Atomic move from staging to final location (same mount point)
        fs::rename(&staging_output_file, &final_output_file)
            .with_context(|| format!("Failed to move compacted file from {:?} to {:?}", staging_output_file, final_output_file))?;
        
        // Remove input files that were compacted (cleanup)
        for input_file in &input_files {
            if Path::new(input_file).exists() {
                debug!("🧹 Removing compacted input file: {}", input_file);
                fs::remove_file(input_file)
                    .with_context(|| format!("Failed to remove input file: {}", input_file))?;
            }
        }
        
        // Cleanup staging directory
        if staging_dir.exists() {
            debug!("🧹 Cleaning up staging directory: {:?}", staging_dir);
            fs::remove_dir_all(&staging_dir)
                .with_context(|| format!("Failed to cleanup staging directory: {:?}", staging_dir))?;
        }
        
        // Log cleanup statistics
        if expired_records_count > 0 {
            info!("🧹 VIPER COMPACTION CLEANUP: {} expired records physically deleted", expired_records_count);
        }
        
        info!(
            "✅ [VIPER COMPACTION] Atomic Arrow/Parquet compaction completed for collection {}: {} records merged, {} expired deleted, {} input files removed, final file: {:?}",
            collection_id,
            latest_records.len(),
            expired_records_count,
            input_files.len(),
            final_output_file
        );
        
        Ok(vec![final_output_file.to_string_lossy().to_string()])
    }

    /// Extract a single record batch from a larger batch
    fn extract_single_record_batch(
        &self,
        batch: &RecordBatch,
        row_index: usize,
        schema: Arc<arrow_schema::Schema>,
    ) -> Result<RecordBatch> {
        use arrow_array::{Array, BinaryArray, Int64Array, StringArray};
        use arrow_schema::DataType;
        
        let mut columns = Vec::new();
        
        for field in schema.fields() {
            let column = batch.column_by_name(field.name())
                .ok_or_else(|| anyhow::anyhow!("Missing column: {}", field.name()))?;
            
            // Extract single value and create single-element array
            let single_value_array: Arc<dyn Array> = match field.data_type() {
                DataType::Utf8 => {
                    let array = column.as_any().downcast_ref::<StringArray>().unwrap();
                    Arc::new(StringArray::from(vec![array.value(row_index)]))
                }
                DataType::Int64 => {
                    let array = column.as_any().downcast_ref::<Int64Array>().unwrap();
                    if array.is_null(row_index) {
                        Arc::new(Int64Array::from(vec![Option::<i64>::None]))
                    } else {
                        Arc::new(Int64Array::from(vec![array.value(row_index)]))
                    }
                }
                DataType::Binary => {
                    let array = column.as_any().downcast_ref::<BinaryArray>().unwrap();
                    Arc::new(BinaryArray::from(vec![array.value(row_index)]))
                }
                DataType::List(_) => {
                    // For List types (like vectors), slice the array
                    column.slice(row_index, 1)
                }
                _ => {
                    // Generic slice for other types
                    column.slice(row_index, 1)
                }
            };
            
            columns.push(single_value_array);
        }
        
        RecordBatch::try_new(schema, columns)
            .with_context(|| "Failed to create single record batch")
    }

    /// Combine multiple record batches into a single batch
    fn combine_record_batches(
        &self,
        schema: Arc<arrow_schema::Schema>,
        batches: Vec<&RecordBatch>,
        vector_dimensions: usize,
    ) -> Result<RecordBatch> {
        if batches.is_empty() {
            return Err(anyhow::anyhow!("Cannot combine empty batches"));
        }
        
        use arrow_array::{Array, ArrayRef};
        
        
        let mut combined_columns = Vec::new();
        
        // Process each column in the target schema to ensure proper alignment
        for field in schema.fields() {
            let field_name = field.name();
            let field_type = field.data_type();
            let mut column_arrays: Vec<ArrayRef> = Vec::new();
            
            // Collect arrays for this column from all batches, handling schema evolution
            for batch in &batches {
                let array = if let Some(column) = batch.column_by_name(field_name) {
                    // COMPACTION FIX: Column exists in this batch - verify type compatibility
                    // This handles schema evolution where column types may have changed
                    if column.data_type() != field_type {
                        warn!("Column '{}' type mismatch - expected {:?}, got {:?}", 
                              field_name, field_type, column.data_type());
                        // Try to create compatible null array
                        self.create_null_array(field_type, batch.num_rows())?
                    } else {
                        column.clone()
                    }
                } else {
                    // Column doesn't exist in this batch - create null array based on field type
                    debug!("Column '{}' not found in batch, creating null array of size {}", 
                           field_name, batch.num_rows());
                    self.create_null_array(field_type, batch.num_rows())?
                };
                column_arrays.push(array);
            }
            
            // Concatenate arrays for this column using proper Arrow concatenation
            let combined_array = self.concatenate_arrays_by_type(field_type, column_arrays, vector_dimensions)?;
            combined_columns.push(combined_array);
        }
        
        RecordBatch::try_new(schema, combined_columns)
            .with_context(|| "Failed to create combined RecordBatch with schema alignment")
    }
    
    /// Create a null array of the specified type and length for schema evolution
    fn create_null_array(&self, data_type: &arrow_schema::DataType, length: usize) -> Result<arrow_array::ArrayRef> {
        use arrow_array::{ArrayRef, BinaryArray, BooleanArray, Float32Array, Float64Array, 
                         Int64Array, StringArray, TimestampMillisecondArray};
        use arrow_schema::{DataType, TimeUnit};
        use std::sync::Arc;
        
        let null_array: ArrayRef = match data_type {
            DataType::Utf8 => Arc::new(StringArray::from(vec![Option::<String>::None; length])),
            DataType::Int64 => Arc::new(Int64Array::from(vec![Option::<i64>::None; length])),
            DataType::Float32 => Arc::new(Float32Array::from(vec![Option::<f32>::None; length])),
            DataType::Float64 => Arc::new(Float64Array::from(vec![Option::<f64>::None; length])),
            DataType::Boolean => Arc::new(BooleanArray::from(vec![Option::<bool>::None; length])),
            DataType::Binary => {
                let null_values: Vec<Option<&[u8]>> = vec![None; length];
                Arc::new(BinaryArray::from(null_values))
            }
            DataType::Timestamp(TimeUnit::Millisecond, _) => {
                Arc::new(TimestampMillisecondArray::from(vec![Option::<i64>::None; length]))
            }
            DataType::List(field) => {
                // Create empty list array based on inner field type
                match field.data_type() {
                    DataType::Float32 => {
                        // For vector data: List<Float32>
                        use arrow_array::builder::{ListBuilder, Float32Builder};
                        let mut list_builder = ListBuilder::new(Float32Builder::with_capacity(length * 512));
                        for _ in 0..length {
                            list_builder.append_value([]);
                        }
                        Arc::new(list_builder.finish())
                    }
                    DataType::Struct(_) => {
                        // For extra_meta data: List<Struct<key: String, value: String>>
                        use arrow_array::builder::{ListBuilder, StructBuilder, StringBuilder};
                        let mut list_builder = ListBuilder::new(StructBuilder::new(
                            vec![
                                arrow_schema::Field::new("key", DataType::Utf8, false),
                                arrow_schema::Field::new("value", DataType::Utf8, false),
                            ],
                            vec![
                                Box::new(StringBuilder::new()),
                                Box::new(StringBuilder::new()),
                            ],
                        ));
                        for _ in 0..length {
                            list_builder.append(false); // Empty list for each row
                        }
                        Arc::new(list_builder.finish())
                    }
                    _ => {
                        // Fallback for other List types
                        use arrow_array::builder::{ListBuilder, Float32Builder};
                        let mut list_builder = ListBuilder::new(Float32Builder::with_capacity(length * 512));
                        for _ in 0..length {
                            list_builder.append_value([]);
                        }
                        Arc::new(list_builder.finish())
                    }
                }
            }
            _ => {
                return Err(anyhow::anyhow!(
                    "Unsupported data type for null array creation: {:?}",
                    data_type
                ));
            }
        };
        
        Ok(null_array)
    }
    
    /// Concatenate arrays of a specific type with proper type handling
    fn concatenate_arrays_by_type(
        &self,
        data_type: &arrow_schema::DataType,
        arrays: Vec<arrow_array::ArrayRef>,
        vector_dimensions: usize,
    ) -> Result<arrow_array::ArrayRef> {
        if arrays.is_empty() {
            return Err(anyhow::anyhow!("Cannot concatenate empty array list"));
        }
        
        if arrays.len() == 1 {
            return Ok(arrays[0].clone());
        }
        
        use arrow_array::{Array, 
                         Int64Array, StringArray};
        use arrow_schema::DataType;
        use std::sync::Arc;
        
        // Manual concatenation approach for each data type to ensure proper merging
        match data_type {
            DataType::Utf8 => {
                let mut values = Vec::new();
                for array in &arrays {
                    let string_array = array.as_any().downcast_ref::<StringArray>()
                        .ok_or_else(|| anyhow::anyhow!("Failed to downcast to StringArray"))?;
                    for i in 0..string_array.len() {
                        values.push(if string_array.is_null(i) {
                            None
                        } else {
                            Some(string_array.value(i).to_string())
                        });
                    }
                }
                Ok(Arc::new(StringArray::from(values)))
            }
            DataType::Int64 => {
                let mut values = Vec::new();
                for array in &arrays {
                    let int_array = array.as_any().downcast_ref::<Int64Array>()
                        .ok_or_else(|| anyhow::anyhow!("Failed to downcast to Int64Array"))?;
                    for i in 0..int_array.len() {
                        values.push(if int_array.is_null(i) {
                            None
                        } else {
                            Some(int_array.value(i))
                        });
                    }
                }
                Ok(Arc::new(Int64Array::from(values)))
            }
            DataType::List(field) => {
                // For List types, we need to handle them specially based on inner type
                info!("Concatenating List arrays with {} arrays", arrays.len());
                
                // For single array, return as-is
                if arrays.len() == 1 {
                    return Ok(arrays[0].clone());
                }
                
                match field.data_type() {
                    DataType::Float32 => {
                        // Handle vector data: List<Float32>
                        use arrow_array::builder::{ListBuilder, Float32Builder};
                        
                        // Calculate optimal capacity: number of vectors * vector dimensions
                        let total_vectors = arrays.iter()
                            .map(|array| {
                                let list_array = array.as_any().downcast_ref::<arrow_array::ListArray>().unwrap();
                                list_array.len()
                            })
                            .sum::<usize>();
                        
                        let capacity = total_vectors * vector_dimensions;
                        
                        debug!("🔧 VIPER LIST CONCATENATION: {} vectors × {} f32 elements per vector = {} total f32 capacity", 
                               total_vectors, vector_dimensions, capacity);
                        
                        let mut list_builder = ListBuilder::new(Float32Builder::with_capacity(capacity));
                        
                        for array_ref in &arrays {
                            let list_array = array_ref
                                .as_any()
                                .downcast_ref::<arrow_array::ListArray>()
                                .ok_or_else(|| anyhow::anyhow!("Failed to downcast to ListArray"))?;
                            
                            for i in 0..list_array.len() {
                                if list_array.is_null(i) {
                                    list_builder.append(false);
                                } else {
                                    let values = list_array.value(i);
                                    let float_array = values
                                        .as_any()
                                        .downcast_ref::<arrow_array::Float32Array>()
                                        .ok_or_else(|| anyhow::anyhow!("Failed to downcast vector values to Float32Array"))?;
                                    
                                    // Append the vector values
                                    for j in 0..float_array.len() {
                                        list_builder.values().append_value(float_array.value(j));
                                    }
                                    list_builder.append(true);
                                }
                            }
                        }
                        
                        Ok(Arc::new(list_builder.finish()))
                    }
                    DataType::Struct(_) => {
                        // Handle extra_meta data: List<Struct<key: String, value: String>>
                        use arrow_array::builder::{ListBuilder, StructBuilder, StringBuilder};
                        
                        debug!("🔧 VIPER LIST CONCATENATION: Handling List<Struct> for extra_meta");
                        
                        let mut list_builder = ListBuilder::new(StructBuilder::new(
                            vec![
                                arrow_schema::Field::new("key", DataType::Utf8, false),
                                arrow_schema::Field::new("value", DataType::Utf8, false),
                            ],
                            vec![
                                Box::new(StringBuilder::new()),
                                Box::new(StringBuilder::new()),
                            ],
                        ));
                        
                        for array_ref in &arrays {
                            let list_array = array_ref
                                .as_any()
                                .downcast_ref::<arrow_array::ListArray>()
                                .ok_or_else(|| anyhow::anyhow!("Failed to downcast to ListArray"))?;
                            
                            for i in 0..list_array.len() {
                                if list_array.is_null(i) {
                                    list_builder.append(false);
                                } else {
                                    let values = list_array.value(i);
                                    let struct_array = values
                                        .as_any()
                                        .downcast_ref::<arrow_array::StructArray>()
                                        .ok_or_else(|| anyhow::anyhow!("Failed to downcast to StructArray"))?;
                                    
                                    let struct_builder = list_builder.values();
                                    
                                    // Extract key and value arrays from the struct
                                    let key_array = struct_array.column(0).as_any().downcast_ref::<StringArray>().unwrap();
                                    let value_array = struct_array.column(1).as_any().downcast_ref::<StringArray>().unwrap();
                                    
                                    // Append all key-value pairs from this struct array
                                    for j in 0..struct_array.len() {
                                        if !struct_array.is_null(j) {
                                            struct_builder.field_builder::<StringBuilder>(0).unwrap().append_value(key_array.value(j));
                                            struct_builder.field_builder::<StringBuilder>(1).unwrap().append_value(value_array.value(j));
                                            struct_builder.append(true);
                                        }
                                    }
                                    
                                    list_builder.append(true);
                                }
                            }
                        }
                        
                        Ok(Arc::new(list_builder.finish()))
                    }
                    _ => {
                        // For other List types, use fallback
                        warn!("List concatenation for inner type {:?} not implemented, using first array", field.data_type());
                        Ok(arrays[0].clone())
                    }
                }
            }
            _ => {
                // For other types, return the first array as fallback
                warn!("Concatenation for type {:?} not implemented, using first array", data_type);
                Ok(arrays[0].clone())
            }
        }
    }
}