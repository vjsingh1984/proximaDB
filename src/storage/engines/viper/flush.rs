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

use crate::core::{CollectionId, VectorRecord};
use super::types::*;
use super::schema::SchemaManager;

/// Flush manager for VIPER storage engine
#[derive(Debug)]
pub struct FlushManager {
    /// Schema manager for dynamic schema generation
    schema_manager: SchemaManager,
    
    /// Collection service for metadata access
    collection_service: Arc<RwLock<Option<Arc<crate::services::collection_service::CollectionService>>>>,
}

impl FlushManager {
    pub fn new(
        collection_service: Arc<RwLock<Option<Arc<crate::services::collection_service::CollectionService>>>>
    ) -> Self {
        Self {
            schema_manager: SchemaManager::new(),
            collection_service,
        }
    }

    /// Core flush operation using proper staging pattern
    pub async fn flush_vectors(
        &self,
        collection_id: &CollectionId,
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

        // Fetch collection configuration using unified interface
        let collection_config = {
            let service_lock = self.collection_service.read().await;
            if let Some(ref service) = *service_lock {
                match service.get_collection_unified(collection_id).await {
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
                collections_affected: vec![collection_id.clone()],
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

        // Step 1: Ensure __flush staging directory exists
        info!(
            "🔄 VIPER: Step 1 - Creating staging directory for collection {}",
            collection_id
        );
        let staging_dir = match self
            .ensure_staging_directory(collection_id, "__flush")
            .await
        {
            Ok(dir) => {
                info!("✅ VIPER: Step 1 - Staging directory created: {}", dir);
                dir
            }
            Err(e) => {
                error!(
                    "❌ VIPER: Step 1 - Failed to create staging directory: {}",
                    e
                );
                return Err(e.context("Failed to create __flush staging directory"));
            }
        };

        // Step 2: Serialize vector records to Parquet format
        info!(
            "🔄 VIPER: Step 2 - Serializing {} vector records to Parquet",
            vector_records.len()
        );
        let parquet_data = match self
            .serialize_records_to_parquet(vector_records, collection_id, &collection_config, vector_dimensions)
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

        // Step 3: Write Parquet data to __flush staging directory
        let parquet_filename = format!("partition_{}.parquet", operation_id);
        info!(
            "🔄 VIPER: Step 3 - Writing Parquet to staging: {}",
            parquet_filename
        );
        let staging_file_path = match self
            .write_to_staging(&staging_dir, &parquet_filename, &parquet_data)
            .await
        {
            Ok(path) => {
                info!("✅ VIPER: Step 3 - Parquet written to staging: {}", path);
                path
            }
            Err(e) => {
                error!("❌ VIPER: Step 3 - Writing to staging failed: {}", e);
                return Err(e.context("Failed to write Parquet to staging"));
            }
        };

        // Step 4: Atomic move from staging to final destination
        info!("🔄 VIPER: Step 4 - Atomic move from staging to final destination");
        let final_file_path = match self
            .atomic_move_from_staging(collection_id, &staging_file_path, &parquet_filename)
            .await
        {
            Ok(path) => {
                info!("✅ VIPER: Step 4 - Atomic move completed: {}", path);
                path
            }
            Err(e) => {
                error!("❌ VIPER: Step 4 - Atomic move failed: {}", e);
                return Err(e.context("Failed to atomic move from staging"));
            }
        };

        // Step 5: Cleanup staging directory
        info!("🔄 VIPER: Step 5 - Cleaning up staging directory");
        if let Err(e) = self.cleanup_staging_directory(&staging_dir).await {
            warn!("⚠️ VIPER: Step 5 - Cleanup warning: {}", e);
        } else {
            info!("✅ VIPER: Step 5 - Staging cleanup completed");
        }

        // Step 6: Check for compaction trigger
        info!("🔄 VIPER: Step 6 - Checking compaction trigger");
        let compaction_triggered = self.check_compaction_trigger(collection_id).await.unwrap_or(false);

        // Step 7: Update collection metadata
        info!("🔄 VIPER: Step 7 - Updating collection metadata");
        self.update_collection_metadata_after_flush(collection_id, vector_records.len(), parquet_data.len()).await?;

        // Step 8: Return successful flush result with BatchId coordination
        Ok(crate::storage::traits::FlushResult {
            success: true,
            collections_affected: vec![collection_id.clone()],
            entries_flushed: vector_records.len() as u64,
            bytes_written: parquet_data.len() as u64,
            files_created: 1,
            duration_ms: 0, // Will be set by high-level flush() method
            completed_at: chrono::Utc::now(),
            flushed_batch_ids: batch_ids.iter().map(|id| {
                // Convert string to BatchId - this is a temporary solution
                // In production, batch_ids should already be proper BatchId objects
                crate::storage::persistence::wal::BatchId::new(
                    collection_id.to_string(),
                    0, // Default sequence start
                    0, // Default sequence end  
                )
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
        collection_id: &CollectionId,
        collection_config: &Option<crate::core::Collection>,
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
            Field::new("id", DataType::Utf8, false),
            Field::new("collection_id", DataType::Utf8, false),
            Field::new(
                "vector", 
                DataType::List(Arc::new(Field::new("item", DataType::Float32, false))), 
                false
            ), // Float32 array for row-level vector filtering
            Field::new("timestamp", DataType::Int64, false),
            Field::new("created_at", DataType::Int64, false),
            Field::new("updated_at", DataType::Int64, false),
            Field::new("version", DataType::Int64, false),
        ];

        // 🎯 DYNAMIC FILTERABLE METADATA: Use pre-fetched collection config to avoid service calls
        let filterable_metadata: Vec<FilterableColumn> = if let Some(ref collection) = collection_config {
            let config_json = serde_json::to_string(&collection.config)
                .context("Failed to serialize collection config")?;
            self.schema_manager.parse_filterable_columns(&config_json)?
        } else {
            info!("Collection {} config not available, using empty filterable metadata", collection_id);
            Vec::new()
        };
        
        // Add filterable metadata columns based on collection configuration
        for filterable_column in &filterable_metadata {
            let arrow_data_type = match filterable_column.data_type {
                FilterableDataType::String => DataType::Utf8,
                FilterableDataType::Integer => DataType::Int64,
                FilterableDataType::Float => DataType::Float64,
                FilterableDataType::Boolean => DataType::Boolean,
                FilterableDataType::DateTime => DataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None),
                FilterableDataType::Array(ref inner_type) => {
                    let inner_arrow_type = match **inner_type {
                        FilterableDataType::String => DataType::Utf8,
                        FilterableDataType::Integer => DataType::Int64,
                        FilterableDataType::Float => DataType::Float64,
                        FilterableDataType::Boolean => DataType::Boolean,
                        _ => DataType::Utf8, // Default fallback
                    };
                    DataType::List(Arc::new(Field::new("item", inner_arrow_type, false)))
                }
            };
            
            schema_fields.push(Field::new(
                &filterable_column.name,
                arrow_data_type,
                true, // Filterable metadata is always nullable
            ));
        }
        
        // Add extra_meta column for remaining metadata
        schema_fields.push(Field::new("extra_meta", DataType::Utf8, true));
        
        let schema = Arc::new(Schema::new(schema_fields));

        // Process records for Arrow array creation
        let mut ids = Vec::new();
        let mut collection_ids = Vec::new();
        let mut vectors = Vec::new();
        let mut timestamps = Vec::new();
        let mut created_ats = Vec::new();
        let mut updated_ats = Vec::new();
        let mut versions = Vec::new();
        let mut filterable_arrays: HashMap<String, Vec<serde_json::Value>> = HashMap::new();
        let mut extra_metadata_data = Vec::new();

        // Initialize filterable arrays
        for filterable_column in &filterable_metadata {
            filterable_arrays.insert(filterable_column.name.clone(), Vec::new());
        }
        
        let filterable_field_names: std::collections::HashSet<String> = filterable_metadata
            .iter()
            .map(|col| col.name.clone())
            .collect();

        for record in records {
            ids.push(record.id.clone());
            collection_ids.push(record.collection_id.clone());
            vectors.push(record.vector.clone());
            
            // Process filterable metadata
            for filterable_column in &filterable_metadata {
                let values = filterable_arrays.get_mut(&filterable_column.name).unwrap();
                let value = record.metadata.get(&filterable_column.name)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                values.push(value);
            }
            
            // Collect remaining metadata as extra key-value pairs
            let mut extra_kvs = Vec::new();
            for (key, value) in &record.metadata {
                // Skip filterable fields - they're handled dynamically above
                if !filterable_field_names.contains(key) {
                    extra_kvs.push((key.clone(), value.to_string()));
                }
            }
            extra_metadata_data.push(extra_kvs);

            timestamps.push(record.timestamp);
            created_ats.push(record.created_at);
            updated_ats.push(record.updated_at);
            versions.push(record.version);
        }

        // Create Arrow arrays with proper List<Float32> for vectors
        let id_array = StringArray::from(ids);
        let collection_array = StringArray::from(collection_ids);
        
        // 🎯 CRITICAL: Create ListArray for proper row-based f32 vector storage
        // Build ListArray using optimized capacity: records.len() * vector_dimensions
        let total_capacity = records.len() * vector_dimensions;
        let vector_list_builder = arrow_array::builder::ListBuilder::new(
            arrow_array::builder::Float32Builder::with_capacity(total_capacity)
        );
        let mut builder = vector_list_builder;
        
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
        
        let timestamp_array = Int64Array::from(timestamps);
        let created_array = Int64Array::from(created_ats);
        let updated_array = Int64Array::from(updated_ats);
        let version_array = Int64Array::from(versions);

        // 🎯 DYNAMIC FILTERABLE METADATA: Create Arrow arrays for each filterable column
        let mut dynamic_filterable_arrays: Vec<Arc<dyn Array>> = Vec::new();
        for filterable_column in &filterable_metadata {
            let values = filterable_arrays.get(&filterable_column.name).unwrap();
            
            let arrow_array: Arc<dyn Array> = match filterable_column.data_type {
                FilterableDataType::String => {
                    let string_values: Vec<Option<String>> = values.iter()
                        .map(|v| if v.is_null() { None } else { Some(v.as_str().unwrap_or("").to_string()) })
                        .collect();
                    Arc::new(StringArray::from(string_values))
                }
                FilterableDataType::Integer => {
                    let int_values: Vec<Option<i64>> = values.iter()
                        .map(|v| if v.is_null() { None } else { v.as_i64() })
                        .collect();
                    Arc::new(arrow_array::Int64Array::from(int_values))
                }
                FilterableDataType::Float => {
                    let float_values: Vec<Option<f64>> = values.iter()
                        .map(|v| if v.is_null() { None } else { v.as_f64() })
                        .collect();
                    Arc::new(arrow_array::Float64Array::from(float_values))
                }
                FilterableDataType::Boolean => {
                    let bool_values: Vec<Option<bool>> = values.iter()
                        .map(|v| if v.is_null() { None } else { v.as_bool() })
                        .collect();
                    Arc::new(arrow_array::BooleanArray::from(bool_values))
                }
                FilterableDataType::DateTime => {
                    let ts_values: Vec<Option<i64>> = values.iter()
                        .map(|v| if v.is_null() { None } else { v.as_i64() })
                        .collect();
                    Arc::new(arrow_array::TimestampMicrosecondArray::from(ts_values))
                }
                FilterableDataType::Array(_) => {
                    // For array types, serialize as JSON strings for now
                    let json_values: Vec<Option<String>> = values.iter()
                        .map(|v| if v.is_null() { None } else { Some(v.to_string()) })
                        .collect();
                    Arc::new(StringArray::from(json_values))
                }
            };
            
            dynamic_filterable_arrays.push(arrow_array);
        }

        // 🎯 EXTRA METADATA: Serialize as JSON strings for flexible storage
        let extra_meta_strings: Vec<Option<String>> = extra_metadata_data.iter()
            .map(|kvs| {
                if kvs.is_empty() {
                    None
                } else {
                    let json_obj: serde_json::Value = kvs.iter()
                        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                        .collect::<serde_json::Map<String, serde_json::Value>>()
                        .into();
                    Some(json_obj.to_string())
                }
            })
            .collect();
        let extra_meta_array = StringArray::from(extra_meta_strings);

        // Combine all arrays into columns
        let mut columns: Vec<Arc<dyn Array>> = vec![
            Arc::new(id_array),
            Arc::new(collection_array),
            Arc::new(vector_array),
            Arc::new(timestamp_array),
            Arc::new(created_array),
            Arc::new(updated_array),
            Arc::new(version_array),
        ];
        
        // Add dynamic filterable columns
        columns.extend(dynamic_filterable_arrays);
        
        // Add extra_meta column
        columns.push(Arc::new(extra_meta_array));

        // Create RecordBatch
        let batch = RecordBatch::try_new(schema, columns)?;

        // Write to Parquet
        let mut buffer = Vec::new();
        let props = WriterProperties::builder()
            .set_compression(parquet::basic::Compression::SNAPPY)
            .build();
        
        let mut writer = ArrowWriter::try_new(&mut buffer, batch.schema(), Some(props))?;
        writer.write(&batch)?;
        writer.close()?;

        Ok(buffer)
    }

    /// Ensure staging directory exists
    async fn ensure_staging_directory(&self, collection_id: &CollectionId, stage_name: &str) -> Result<String> {
        let staging_dir = format!("/tmp/viper_staging/{}_{}", collection_id, stage_name);
        std::fs::create_dir_all(&staging_dir)?;
        Ok(staging_dir)
    }

    /// Write data to staging directory
    async fn write_to_staging(&self, staging_dir: &str, filename: &str, data: &[u8]) -> Result<String> {
        let file_path = format!("{}/{}", staging_dir, filename);
        std::fs::write(&file_path, data)?;
        Ok(file_path)
    }

    /// Atomic move from staging to final destination
    async fn atomic_move_from_staging(&self, collection_id: &CollectionId, staging_path: &str, filename: &str) -> Result<String> {
        let final_dir = format!("/tmp/viper_final/{}", collection_id);
        std::fs::create_dir_all(&final_dir)?;
        let final_path = format!("{}/{}", final_dir, filename);
        std::fs::rename(staging_path, &final_path)?;
        Ok(final_path)
    }

    /// Cleanup staging directory
    async fn cleanup_staging_directory(&self, staging_dir: &str) -> Result<()> {
        std::fs::remove_dir_all(staging_dir)?;
        Ok(())
    }

    /// Check if compaction should be triggered
    async fn check_compaction_trigger(&self, _collection_id: &CollectionId) -> Result<bool> {
        // TODO: Implement compaction trigger logic
        Ok(false)
    }

    /// Update collection metadata after flush
    async fn update_collection_metadata_after_flush(&self, _collection_id: &CollectionId, _records_count: usize, _bytes_written: usize) -> Result<()> {
        // TODO: Implement metadata update logic
        Ok(())
    }
}