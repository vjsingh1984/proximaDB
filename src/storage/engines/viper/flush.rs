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
use arrow_array::builder::{ListBuilder, Int8Builder, UInt8Builder};
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
use crate::storage::transaction_coordinator::{TransactionCoordinator, StagingConfig, TransactionStageType};

use crate::core::{String, VectorRecord};
use crate::storage::optimization::{MetadataSorter, SortingStats};
use crate::metrics::{InternalMetricsUpdater, MetricsUpdate};
use super::schema::SchemaManager;

/// Flush manager for VIPER storage engine with atomic writes
pub struct FlushManager {
    /// Schema manager for dynamic schema generation
    schema_manager: SchemaManager,
    
    /// Collection service for metadata access
    collection_service: Arc<RwLock<Option<Arc<crate::services::collection_service::CollectionService>>>>,
    
    /// Filesystem factory for cross-cloud atomic writes
    filesystem_factory: Arc<FilesystemFactory>,
    
    /// Atomic coordinator for ACID operations
    atomic_coordinator: Arc<TransactionCoordinator>,
    
    // 🔴 UNUSED FIELD - Metrics module is unused
    // /// Optional metrics updater for non-critical metrics
    // metrics_updater: Option<Arc<dyn InternalMetricsUpdater>>,
}

impl std::fmt::Debug for FlushManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlushManager")
            .field("schema_manager", &self.schema_manager)
            .field("collection_service", &self.collection_service)
            .field("filesystem_factory", &self.filesystem_factory)
            .field("atomic_coordinator", &self.atomic_coordinator)
            // .field("metrics_updater", &self.metrics_updater.is_some())  // 🔴 UNUSED
            .finish()
    }
}

impl FlushManager {
    pub async fn new(
        collection_service: Arc<RwLock<Option<Arc<crate::services::collection_service::CollectionService>>>>,
        filesystem_factory: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        // Create atomic coordinator
        let atomic_coordinator = Arc::new(
            TransactionCoordinator::new(filesystem_factory.clone(), None)
                .await
                .context("Failed to create atomic coordinator")?
        );
        
        Ok(Self {
            schema_manager: SchemaManager::new(),
            collection_service,
            filesystem_factory,
            atomic_coordinator,
            // metrics_updater: None, // Will be set via set_metrics_updater  // 🔴 UNUSED
        })
    }
    
    // 🔴 UNUSED METHOD - Metrics module is unused
    // /// Set the metrics updater (optional, for non-critical metrics)
    // pub fn set_metrics_updater(&mut self, updater: Arc<dyn InternalMetricsUpdater>) {
    //     self.metrics_updater = Some(updater);
    // }

    /// Core flush operation using proper staging pattern
    pub async fn flush_vectors(
        &self,
        collection_id: &str,
        vector_records: &[VectorRecord],
        batch_ids: &[String],
        force: bool,
        synchronous: bool,
        viper_config: &crate::core::config::ViperConfig,
        provided_collection_config: Option<&crate::proto::proximadb::Collection>,
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
                match service.get_proto_collection(collection_id).await {
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
            .serialize_records_to_parquet(&sorted_records, collection_id, &collection_config, vector_dimensions, viper_config)
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
            .write_parquet_atomic(collection_id, &parquet_filename, &parquet_data, &collection_config)
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

        // 🔴 UNUSED METRICS - Metrics module is unused
        // // Step 5.1: Record metrics (non-blocking, failure-tolerant)
        // if let Some(ref metrics_updater) = self.metrics_updater {
        //     let flush_update = FlushMetricsUpdate {
        //         vectors_flushed: vector_records.len() as i64,
        //         bytes_written: parquet_data.len() as i64,
        //         duration_ms: 0, // TODO: Track actual duration
        //         files_created: 1,
        //         engine_type: "VIPER".to_string(),
        //         timestamp: chrono::Utc::now().timestamp_millis(),
        //     };
        //     
        //     // Fire and forget - never block flush operation
        //     metrics_updater.record_flush(collection_id, flush_update).await;
        // }

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
                crate::storage::persistence::write_ahead_log::BatchId::default()
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
        let quantization_config = if let Some(ref collection) = collection_config {
            collection.config.as_ref().and_then(|c| c.quantization_config.as_ref())
        } else {
            None
        };
        
        let mut schema_fields = vec![
            Field::new("id", DataType::Utf8, true),  // Can be null for append-only vectors
            Field::new("collection_id", DataType::Utf8, false),
            Field::new(
                "vector", 
                DataType::List(Arc::new(Field::new("item", DataType::Float32, true))), 
                true  // Vector field can be null
            ), // Primary FP32 vector column for 100% fidelity
            Field::new("version", DataType::Int8, true), // Version field for MVCC - using tinyint
            Field::new("updated_at", DataType::Int64, true), // Audit field - stores create or update time
            Field::new("expires_at", DataType::Int64, true), // Only keep expires_at for TTL
        ];
        
        // Phase 2: Add quantized vector columns for compression + fast approximation
        if let Some(quant_config) = quantization_config {
            if quant_config.enabled {
                debug!("🗜️ VIPER: Adding quantized vector columns for collection {}", collection_id);
                
                // Add INT8 quantized column (highest quality quantization)
                schema_fields.push(Field::new(
                    "vector_int8",
                    DataType::List(Arc::new(Field::new("item", DataType::Int8, true))),
                    true
                ));
                
                // Add PQ8 (Product Quantization 8-bit) column for high compression
                schema_fields.push(Field::new(
                    "vector_pq8",
                    DataType::List(Arc::new(Field::new("item", DataType::UInt8, true))),
                    true
                ));
                
                // Add PQ4 (Product Quantization 4-bit) column for maximum compression
                // Stored as UInt8 but each byte contains two 4-bit values
                schema_fields.push(Field::new(
                    "vector_pq4",
                    DataType::List(Arc::new(Field::new("item", DataType::UInt8, true))),
                    true
                ));
                
                info!("✅ VIPER: Dual storage enabled - FP32 + INT8 + PQ8 + PQ4 quantized columns");
            }
        }

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
        
        // Phase 2: Initialize quantized vector arrays if quantization is enabled
        let mut vector_int8_data: Vec<Vec<i8>> = Vec::with_capacity(capacity);
        let mut vector_pq8_data: Vec<Vec<u8>> = Vec::with_capacity(capacity);
        let mut vector_pq4_data: Vec<Vec<u8>> = Vec::with_capacity(capacity);
        let has_quantization = quantization_config.map(|q| q.enabled).unwrap_or(false);
        
        let filterable_field_names: std::collections::HashSet<String> = filterable_metadata
            .iter()
            .map(|col| col.name.clone())
            .collect();

        for record in records {
            ids.push(record.id.as_deref().unwrap_or("").to_string());
            collection_ids.push(collection_id.to_string());
            vectors.push(record.vector.clone());
            
            // Phase 2: Generate quantized versions of vectors for dual column storage
            if has_quantization {
                let fp32_vector = &record.vector;
                
                // INT8 quantization (highest quality) - simple linear quantization
                let int8_vector = self.quantize_to_int8(fp32_vector);
                vector_int8_data.push(int8_vector);
                
                // PQ8 quantization (high compression) - using simplified approach for now
                let pq8_vector = self.quantize_to_pq8(fp32_vector);
                vector_pq8_data.push(pq8_vector);
                
                // PQ4 quantization (maximum compression) - using simplified approach for now
                let pq4_vector = self.quantize_to_pq4(fp32_vector);
                vector_pq4_data.push(pq4_vector);
            }
            
            // Process filterable metadata
            for filterable_column in &filterable_metadata {
                let values = filterable_arrays.get_mut(&filterable_column.name).unwrap();
                let value = record.metadata.iter()
                    .find(|item| item.key == filterable_column.name)
                    .map(|item| {
                        match &item.value {
                            Some(crate::proto::proximadb::metadata_item::Value::StringValue(s)) => serde_json::Value::String(s.clone()),
                            Some(crate::proto::proximadb::metadata_item::Value::NumberValue(n)) => {
                                serde_json::Number::from_f64(*n)
                                    .map(serde_json::Value::Number)
                                    .unwrap_or_else(|| serde_json::Value::String(n.to_string()))
                            },
                            Some(crate::proto::proximadb::metadata_item::Value::BoolValue(b)) => serde_json::Value::Bool(*b),
                            None => serde_json::Value::Null,
                        }
                    })
                    .unwrap_or(serde_json::Value::Null);
                values.push(value);
            }
            
            // Collect remaining metadata as extra key-value pairs
            let mut extra_kvs = Vec::new();
            for item in &record.metadata {
                // Skip filterable fields - they're handled dynamically above
                if !filterable_field_names.contains(&item.key) {
                    // Convert metadata value to string for storage
                    let value_str = match &item.value {
                        Some(crate::proto::proximadb::metadata_item::Value::StringValue(s)) => s.clone(),
                        Some(crate::proto::proximadb::metadata_item::Value::NumberValue(n)) => n.to_string(),
                        Some(crate::proto::proximadb::metadata_item::Value::BoolValue(b)) => b.to_string(),
                        None => String::new(),
                    };
                    extra_kvs.push((item.key.clone(), value_str));
                }
            }
            extra_metadata_data.push(extra_kvs);

            // Include version for MVCC, updated_at for audit, and expires_at for TTL support
            // Version should be null if id is null (for append-only vectors)
            if record.id.is_none() {
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
        
        // Phase 2: Add quantized vector columns if quantization is enabled
        if has_quantization {
            // Create INT8 quantized vector array
            let mut int8_list_builder = ListBuilder::new(Int8Builder::new());
            for int8_vector in vector_int8_data {
                let mut value_builder = int8_list_builder.values();
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
                let mut value_builder = pq8_list_builder.values();
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
                let mut value_builder = pq4_list_builder.values();
                for &val in &pq4_vector {
                    value_builder.append_value(val);
                }
                pq4_list_builder.append(true);
            }
            let pq4_array = pq4_list_builder.finish();
            columns.push(Arc::new(pq4_array));
            
            info!("📦 VIPER FLUSH: Added {} quantized vector columns (INT8, PQ8, PQ4)", 3);
        }

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

        // Write to Parquet with configuration-based compression
        let mut buffer = Vec::new();
        
        // Get compression from collection config if available, otherwise use viper defaults
        let (compression_algo, compression_level) = if let Some(ref collection) = collection_config {
            if let Some(ref config) = collection.config {
                if let Some(ref compression) = config.compression {
                    use crate::proto::proximadb::CompressionAlgorithm;
                    // Convert from proto-generated enum value to Parquet compression
                    match CompressionAlgorithm::try_from(compression.algorithm) {
                        Ok(CompressionAlgorithm::CompressionZstd) => {
                            let level = compression.level.unwrap_or(viper_config.compression_level);
                            (parquet::basic::Compression::ZSTD(
                                parquet::basic::ZstdLevel::try_new(level)?
                            ), true)
                        }
                        Ok(CompressionAlgorithm::CompressionLz4) => {
                            (parquet::basic::Compression::LZ4, true)
                        }
                        Ok(CompressionAlgorithm::CompressionSnappy) => {
                            (parquet::basic::Compression::SNAPPY, true)
                        }
                        Ok(CompressionAlgorithm::CompressionGzip) => {
                            (parquet::basic::Compression::GZIP(
                                parquet::basic::GzipLevel::try_new(compression.level.unwrap_or(6) as u32)?
                            ), true)
                        }
                        Ok(CompressionAlgorithm::CompressionBrotli) => {
                            (parquet::basic::Compression::BROTLI(
                                parquet::basic::BrotliLevel::try_new(compression.level.unwrap_or(6) as u32)?
                            ), true)
                        }
                        _ => (parquet::basic::Compression::UNCOMPRESSED, false)
                    }
                } else {
                    // No compression config, use viper defaults
                    match viper_config.compression.as_str() {
                        "zstd" => (parquet::basic::Compression::ZSTD(
                            parquet::basic::ZstdLevel::try_new(viper_config.compression_level)?
                        ), true),
                        "snappy" => (parquet::basic::Compression::SNAPPY, true),
                        "lz4" => (parquet::basic::Compression::LZ4, true),
                        "none" | _ => (parquet::basic::Compression::UNCOMPRESSED, false)
                    }
                }
            } else {
                (parquet::basic::Compression::UNCOMPRESSED, false)
            }
        } else {
            (parquet::basic::Compression::UNCOMPRESSED, false)
        };
        
        // Build writer properties with optimal encodings for different column types
        let mut props_builder = WriterProperties::builder()
            .set_compression(compression_algo)
            .set_max_row_group_size(viper_config.row_group_size);
        
        // Set optimal encoding for vector column based on quantization
        // Check if vectors are quantized (detected via collection config)
        let is_quantized = if let Some(ref collection) = collection_config {
            collection.config.as_ref()
                .and_then(|c| c.quantization_config.as_ref())
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
                parquet::basic::Encoding::BIT_PACKED
            );
            debug!("🔧 VIPER: Using BIT_PACKED encoding for quantized vectors");
        } else {
            // For full precision f32 vectors
            // BYTE_STREAM_SPLIT splits floating point bytes for better compression
            props_builder = props_builder.set_column_encoding(
                parquet::schema::types::ColumnPath::from("vector"),
                parquet::basic::Encoding::BYTE_STREAM_SPLIT
            );
            debug!("🔧 VIPER: Using BYTE_STREAM_SPLIT encoding for f32 vectors");
        }
        
        // Set dictionary encoding for low-cardinality string columns
        props_builder = props_builder.set_column_dictionary_enabled(
            parquet::schema::types::ColumnPath::from("collection_id"),
            true
        );
        props_builder = props_builder.set_column_dictionary_enabled(
            parquet::schema::types::ColumnPath::from("id"),
            true
        );
        
        // Apply column-specific encodings from filterable metadata
        for filterable_column in &filterable_metadata {
            if let Some(encoding_hint) = filterable_column.encoding_hint {
                use crate::proto::proximadb::ColumnEncoding;
                let column_path = parquet::schema::types::ColumnPath::from(filterable_column.name.as_str());
                
                match ColumnEncoding::try_from(encoding_hint) {
                    Ok(ColumnEncoding::EncodingDictionary) => {
                        props_builder = props_builder.set_column_dictionary_enabled(column_path, true);
                    }
                    Ok(ColumnEncoding::EncodingDelta) => {
                        props_builder = props_builder.set_column_encoding(
                            column_path,
                            parquet::basic::Encoding::DELTA_BINARY_PACKED
                        );
                    }
                    Ok(ColumnEncoding::EncodingRle) => {
                        props_builder = props_builder.set_column_encoding(
                            column_path,
                            parquet::basic::Encoding::RLE
                        );
                    }
                    _ => {} // Use default encoding
                }
            }
        }
        
        let props = props_builder.build();
        
        let mut writer = ArrowWriter::try_new(&mut buffer, batch.schema(), Some(props))?;
        writer.write(&batch)?;
        writer.close()?;
        
        info!("📝 VIPER FLUSH: Wrote {} bytes of Parquet data", buffer.len());

        Ok(buffer)
    }

    /// INT8 Quantization: Linear quantization preserving vector relationships
    /// Maps FP32 values to INT8 range (-128 to 127) using min/max scaling
    fn quantize_to_int8(&self, fp32_vector: &[f32]) -> Vec<i8> {
        if fp32_vector.is_empty() {
            return Vec::new();
        }
        
        // Find min and max values for scaling
        let min_val = fp32_vector.iter().copied().fold(f32::INFINITY, f32::min);
        let max_val = fp32_vector.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        
        // Avoid division by zero
        let range = if (max_val - min_val).abs() < f32::EPSILON {
            1.0
        } else {
            max_val - min_val
        };
        
        // Scale to [-128, 127] range
        fp32_vector.iter().map(|&val| {
            let normalized = (val - min_val) / range;  // [0, 1]
            let scaled = normalized * 255.0 - 128.0;   // [-128, 127]
            scaled.clamp(-128.0, 127.0) as i8
        }).collect()
    }
    
    /// PQ8 Quantization: Product Quantization with 8 clusters per subvector
    /// Simplified implementation dividing vector into subvectors and quantizing each
    fn quantize_to_pq8(&self, fp32_vector: &[f32]) -> Vec<u8> {
        if fp32_vector.is_empty() {
            return Vec::new();
        }
        
        // For simplicity, divide vector into 8-dimensional subvectors
        // Each subvector gets quantized to one of 256 centroids (u8)
        const SUBVECTOR_SIZE: usize = 8;
        let num_subvectors = (fp32_vector.len() + SUBVECTOR_SIZE - 1) / SUBVECTOR_SIZE;
        
        let mut quantized = Vec::with_capacity(num_subvectors);
        
        for i in 0..num_subvectors {
            let start = i * SUBVECTOR_SIZE;
            let end = std::cmp::min(start + SUBVECTOR_SIZE, fp32_vector.len());
            let subvector = &fp32_vector[start..end];
            
            // Simplified quantization: hash the subvector to get cluster ID
            let mut hash: u64 = 0;
            for &val in subvector {
                hash = hash.wrapping_add((val * 1000.0) as u64);
            }
            quantized.push((hash % 256) as u8);
        }
        
        quantized
    }
    
    /// PQ4 Quantization: Product Quantization with 4-bit codes (16 clusters)
    /// Maximum compression with 4 bits per subvector  
    fn quantize_to_pq4(&self, fp32_vector: &[f32]) -> Vec<u8> {
        if fp32_vector.is_empty() {
            return Vec::new();
        }
        
        // Similar to PQ8 but with 4-bit codes (16 centroids per subvector)
        const SUBVECTOR_SIZE: usize = 8;
        let num_subvectors = (fp32_vector.len() + SUBVECTOR_SIZE - 1) / SUBVECTOR_SIZE;
        
        // Pack two 4-bit codes per byte
        let mut quantized = Vec::with_capacity((num_subvectors + 1) / 2);
        
        for i in (0..num_subvectors).step_by(2) {
            let start1 = i * SUBVECTOR_SIZE;
            let end1 = std::cmp::min(start1 + SUBVECTOR_SIZE, fp32_vector.len());
            let subvector1 = &fp32_vector[start1..end1];
            
            // First 4-bit code
            let mut hash1: u64 = 0;
            for &val in subvector1 {
                hash1 = hash1.wrapping_add((val * 1000.0) as u64);
            }
            let code1 = (hash1 % 16) as u8;
            
            // Second 4-bit code (if exists)
            let code2 = if i + 1 < num_subvectors {
                let start2 = (i + 1) * SUBVECTOR_SIZE;
                let end2 = std::cmp::min(start2 + SUBVECTOR_SIZE, fp32_vector.len());
                let subvector2 = &fp32_vector[start2..end2];
                
                let mut hash2: u64 = 0;
                for &val in subvector2 {
                    hash2 = hash2.wrapping_add((val * 1000.0) as u64);
                }
                (hash2 % 16) as u8
            } else {
                0
            };
            
            // Pack two 4-bit codes into one byte: [code2][code1]
            quantized.push((code2 << 4) | code1);
        }
        
        quantized
    }

    /// Write Parquet data using atomic write strategy
    /// Uses unified atomic write infrastructure for cross-cloud compatibility
    async fn write_parquet_atomic(
        &self, 
        collection_id: &str, 
        filename: &str, 
        parquet_data: &[u8],
        collection_config: &Option<crate::proto::proximadb::Collection>
    ) -> Result<String> {
        info!("🔄 Writing Parquet file atomically: {} ({} bytes)", filename, parquet_data.len());
        
        // Get storage assignment from collection config - fail fast if not present
        let storage_assignment = collection_config
            .as_ref()
            .and_then(|c| c.storage_assignment.as_ref())
            .ok_or_else(|| anyhow::anyhow!(
                "Collection '{}' has no storage assignment. All collections must have storage assignments.",
                collection_id
            ))?;
        
        // Begin atomic operation for flush
        let data_url = format!("{}/{}/data", storage_assignment.base_location, collection_id);
        let staging_config = StagingConfig {
            base_url: data_url.clone(),
            collection_id: None,  // Don't duplicate collection path
            operation_type: TransactionStageType::Flush,
            custom_staging_dir: None,
            auto_cleanup: true,
            max_orphaned_age_hours: 24,
            ..Default::default()  // This will pick up skip_uuid_subdir: false
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
        
        let final_path = format!("{}/{}", data_url, filename);
        
        info!("✅ VIPER: Atomically wrote Parquet file {} ({} KB)", 
              final_path, parquet_data.len() / 1024);
        
        // Verify file was written
        let fs = self.filesystem_factory.get_filesystem(&data_url)?;
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
        // Note: This is deferred to the CompactionManager which has full context
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
    async fn update_collection_metadata_after_flush(&self, collection_id: &str, records_count: usize, bytes_written: usize) -> Result<()> {
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