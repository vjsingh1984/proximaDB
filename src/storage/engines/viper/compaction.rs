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
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn, trace};

use crate::storage::persistence::filesystem::{
    FilesystemFactory
};
use crate::storage::atomic::{UnifiedAtomicCoordinator, StagingConfig, StagingOperationType};


use super::schema::SchemaManager;

/// Result of a compaction operation
#[derive(Debug, Clone)]
pub struct ViperCompactionResult {
    /// Input files that were processed
    pub input_files: Vec<String>,
    /// Output files that were created
    pub output_files: Vec<String>,
    /// Total entries processed
    pub entries_processed: u64,
    /// Entries removed (expired/deleted)
    pub entries_removed: u64,
    /// Bytes read from input files
    pub bytes_read: u64,
    /// Bytes written to output files
    pub bytes_written: u64,
}

/// Compaction manager for VIPER storage engine with atomic writes
#[derive(Debug)]
pub struct CompactionManager {
    /// Schema manager for dynamic schema generation
    schema_manager: SchemaManager,
    
    /// Collection service for metadata access
    collection_service: Arc<RwLock<Option<Arc<crate::services::collection_service::CollectionService>>>>,
    
    /// Filesystem factory for cross-cloud atomic writes
    filesystem_factory: Arc<FilesystemFactory>,
    
    /// Atomic coordinator for ACID operations
    atomic_coordinator: Arc<UnifiedAtomicCoordinator>,
}

impl CompactionManager {
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

    /// Discover files that can be compacted for a collection
    async fn discover_compactable_files(&self, collection_id: &str) -> Result<Vec<String>> {
        debug!("Discovering compactable files for collection: {}", collection_id);
        
        // Get storage assignment for the collection
        let assignment_service = crate::storage::assignment_service::get_assignment_service();
        let storage_assignment = match assignment_service.get_assignment(collection_id).await {
            Some(assignment) => assignment,
            None => {
                debug!("🔍 No storage assignment found for collection {}, skipping compaction", collection_id);
                return Ok(vec![]); // Return empty list instead of failing
            }
        };
        
        debug!("Storage assignment data_url: {}", storage_assignment.data_url);
        info!("🔍 Looking for parquet files in data_url: {}", storage_assignment.data_url);
        
        // Get filesystem for the data directory
        let fs = self.filesystem_factory.get_filesystem(&storage_assignment.data_url)?;
        
        // List all .parquet files in the collection's data directory
        let mut parquet_files = Vec::new();
        
        if fs.exists(&storage_assignment.data_url).await? {
            info!("📂 Data directory exists, listing contents...");
            let entries = fs.list(&storage_assignment.data_url).await?;
            info!("📋 Found {} entries in data directory", entries.len());
            
            for entry in entries {
                info!("📄 Entry: {} (directory: {}, url: {})", 
                      entry.name, entry.metadata.is_directory, entry.url);
                
                if !entry.metadata.is_directory && entry.url.ends_with(".parquet") {
                    info!("✅ Found parquet file: {}", entry.url);
                    parquet_files.push(entry.url);
                } else if entry.url.ends_with(".parquet") {
                    info!("❌ Skipped directory ending with .parquet: {}", entry.url);
                } else {
                    info!("❌ Skipped non-parquet file: {}", entry.url);
                }
            }
        } else {
            warn!("⚠️ Data directory does not exist: {}", storage_assignment.data_url);
        }
        
        info!("🔍 Discovered {} parquet files for compaction in collection {}", 
              parquet_files.len(), collection_id);
        
        Ok(parquet_files)
    }

    /// Arrow/Parquet compaction with version merging and expiry logic
    /// This is the main compaction implementation for VIPER storage engine
    pub async fn compact_parquet_files(
        &self,
        collection_id: &str,
        input_files: Vec<String>,
    ) -> Result<ViperCompactionResult> {
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
        _vector_dimensions: usize,
        collection_config: Option<crate::proto::proximadb::Collection>,
    ) -> Result<ViperCompactionResult> {
        
        info!("Starting atomic Arrow/Parquet compaction for collection {}", collection_id);
        info!(
            "🔄 [VIPER COMPACTION] Starting atomic Arrow/Parquet compaction for collection {}",
            collection_id
        );
        
        // If no input files specified, discover them from storage
        let input_files = if input_files.is_empty() {
            self.discover_compactable_files(collection_id).await?
        } else {
            input_files
        };
        
        let discovered_input_files = input_files.clone(); // Save for result
        let mut total_bytes_read = 0u64;
        let mut total_records_processed = 0usize;
        
        if input_files.is_empty() {
            info!("📋 No files found for compaction in collection {}", collection_id);
            return Ok(ViperCompactionResult {
                input_files: vec![],
                output_files: vec![],
                entries_processed: 0,
                entries_removed: 0,
                bytes_read: 0,
                bytes_written: 0,
            });
        }
        
        info!("📋 Input files for compaction: {:?}", input_files);
        
        // Determine base storage directory from storage assignment
        let assignment_service = crate::storage::assignment_service::get_assignment_service();
        let storage_assignment = match assignment_service.get_assignment(collection_id).await {
            Some(assignment) => assignment,
            None => {
                info!("🔍 No storage assignment found for collection {}, compaction not needed", collection_id);
                return Ok(ViperCompactionResult {
                    input_files: vec![],
                    output_files: vec![],
                    entries_processed: 0,
                    entries_removed: 0,
                    bytes_read: 0,
                    bytes_written: 0,
                });
            }
        };
        
        // The output file should go in the same directory as the input files
        let base_storage_url = &storage_assignment.data_url;
        
        // Atomic write will handle staging internally
        
        // Generate dynamic schema with caching using pre-fetched collection config
        let schema = self.schema_manager.get_or_generate_cached_schema(collection_id, &collection_config).await?;
        
        // COMPACTION FIX: Validate schema compatibility before processing
        // This prevents column alignment issues during compaction
        info!("🔍 Validating schema compatibility for {} input files", input_files.len());
        let fs = self.filesystem_factory.get_filesystem("file:///")?;
        let mut input_schemas = Vec::new();
        for input_file in &input_files {
            if let Ok(file_data) = fs.read(input_file).await {
                let parquet_bytes = bytes::Bytes::from(file_data);
                if let Ok(builder) = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(parquet_bytes) {
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
        // Instead of storing RecordBatches, store the actual data for each record
        #[derive(Clone)]
        struct RecordData {
            id: Option<String>, // ID is optional
            version: i64,
            row_data: HashMap<String, serde_json::Value>, // Store all column data
        }
        
        let mut latest_records: HashMap<String, RecordData> = HashMap::new();
        let mut all_records_ordered: Vec<RecordData> = Vec::new(); // Maintain order for records without IDs
        let current_time = chrono::Utc::now().timestamp_micros();
        let mut expired_records_count = 0;
        
        info!("Processing {} input files for version merging and expiry logic", input_files.len());
        info!("⚡ Processing {} input files for version merging and expiry logic", input_files.len());
        
        for (file_idx, input_file) in input_files.iter().enumerate() {
            debug!("Processing file {}/{}: {}", file_idx + 1, input_files.len(), input_file);
            info!("📂 Processing file {}/{}: {}", file_idx + 1, input_files.len(), input_file);
            
            let file_data = fs.read(input_file).await
                .with_context(|| format!("Failed to read input file: {}", input_file))?;
            total_bytes_read += file_data.len() as u64;
            
            info!("  📊 Read {} bytes from file", file_data.len());
            let parquet_bytes = bytes::Bytes::from(file_data);
            
            let builder = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(parquet_bytes)
                .with_context(|| format!("Failed to create Parquet reader for: {}", input_file))?;
            
            let reader = builder.build()
                .with_context(|| format!("Failed to build Parquet reader for: {}", input_file))?;
            
            for batch_result in reader {
                let batch = batch_result
                    .with_context(|| format!("Failed to read batch from: {}", input_file))?;
                
                debug!("Got batch with {} rows", batch.num_rows());
                
                // Process each record in the batch for MVCC resolution
                let id_array = batch.column_by_name("id").unwrap()
                    .as_any().downcast_ref::<StringArray>().unwrap();
                // Handle version as Int8 (as written by flush) or Int64 (legacy)
                let version_array = batch.column_by_name("version").unwrap();
                let version_iter: Box<dyn Iterator<Item = Option<i64>>> = 
                    if let Some(arr) = version_array.as_any().downcast_ref::<arrow_array::Int8Array>() {
                        Box::new(arr.iter().map(|v| v.map(|i| i as i64)))
                    } else if let Some(arr) = version_array.as_any().downcast_ref::<Int64Array>() {
                        Box::new(arr.iter())
                    } else {
                        return Err(anyhow::anyhow!("Invalid version column type"));
                    };
                
                // COMPACTION FIX: Handle missing expires_at column for backward compatibility
                let null_expires_array;
                let expires_at_array = match batch.column_by_name("expires_at") {
                    Some(column) => column.as_any().downcast_ref::<Int64Array>()
                        .ok_or_else(|| anyhow::anyhow!("Invalid 'expires_at' column type in {}", input_file))?,
                    None => {
                        // Create null array for missing expires_at column
                        warn!("⚠️ Missing 'expires_at' column in {}, creating null array", input_file);
                        null_expires_array = Int64Array::from(vec![Option::<i64>::None; batch.num_rows()]);
                        &null_expires_array
                    }
                };
                
                let version_values: Vec<Option<i64>> = version_iter.collect();
                debug!("Collected {} version values", version_values.len());
                
                for i in 0..batch.num_rows() {
                    trace!("Processing row {}/{} in batch", i + 1, batch.num_rows());
                    total_records_processed += 1;
                    let record_id = if id_array.is_null(i) {
                        trace!("Row {} has NULL id", i);
                        None
                    } else {
                        let id_str = id_array.value(i);
                        trace!("Row {} has id: '{}'", i, id_str);
                        if id_str.is_empty() || id_str == "null" {
                            None
                        } else {
                            Some(id_str.to_string())
                        }
                    };
                    trace!("Record ID extracted: {:?}", record_id);
                    let record_version = version_values[i].unwrap_or(1);
                    trace!("Record version: {}", record_version);
                    let record_expires_at = if expires_at_array.is_null(i) {
                        trace!("expires_at is NULL for row {}", i);
                        None
                    } else {
                        let expires_value = expires_at_array.value(i);
                        trace!("expires_at value for row {}: {}", i, expires_value);
                        Some(expires_value)
                    };
                    
                    // Skip expired records (treat 0 as no expiry)
                    trace!("Current time: {}, Record expires_at: {:?}", current_time, record_expires_at);
                    if let Some(expires_at) = record_expires_at {
                        // Only consider expired if expires_at > 0 (0 means no expiry)
                        if expires_at > 0 && expires_at < current_time {
                            expired_records_count += 1;
                            debug!("Deleting expired record {:?} (expired at {} < current {})", record_id, expires_at, current_time);
                            debug!("⏰ VIPER COMPACTION: Physically deleting expired record {:?} (expired at {})", record_id, expires_at);
                            continue;
                        }
                    }
                    
                    trace!("About to check should_keep for row {}", i);
                    trace!("Latest records count before check: {}", latest_records.len());
                    
                    // Handle immutable vectors (null/empty IDs) - skip ID-based merging
                    let should_keep = if record_id.is_none() {
                        debug!("📝 Immutable vector record (no ID), including as-is");
                        true
                    } else if let Some(ref id_str) = record_id {
                        // Version merging logic: keep latest version per ID
                        if let Some(existing_record) = latest_records.get(id_str) {
                            let existing_version = existing_record.version;
                            
                            if record_version > existing_version {
                                debug!("📝 Updating record {} from version {} to {}", id_str, existing_version, record_version);
                                true
                            } else {
                                debug!("📝 Keeping existing record {} at version {} (incoming version {})", 
                                    id_str, existing_version, record_version);
                                false
                            }
                        } else {
                            debug!("📝 New record {} at version {}", id_str, record_version);
                            true
                        }
                    } else {
                        // Should not reach here, but keep for safety
                        true
                    };
                    
                    trace!("Row {} - should_keep: {}, id: {:?}, version: {}", 
                             i, should_keep, record_id, record_version);
                    
                    if !should_keep {
                        trace!("Skipping row {} - should_keep is false", i);
                    }
                    
                    if should_keep {
                        trace!("Row {} should be kept, starting extraction", i);
                        // Extract all column data for this record
                        let mut row_data = HashMap::new();
                        
                        // Extract data from each column
                        trace!("Extracting data for row {} ({} columns)", i, batch.schema().fields().len());
                        for (col_idx, field) in batch.schema().fields().iter().enumerate() {
                            trace!("  Column {}: {}", col_idx, field.name());
                            let column = batch.column(col_idx);
                            let value = self.extract_column_value(column, i, field.data_type())?;
                            row_data.insert(field.name().to_string(), value);
                        }
                        trace!("Row {} extracted successfully", i);
                        trace!("Record ID: {:?}, Version: {}", record_id, record_version);
                        
                        let record_data = RecordData {
                            id: record_id.clone(),
                            version: record_version,
                            row_data,
                        };
                        
                        // For immutable vectors (no ID), add to ordered list
                        if record_id.is_none() {
                            debug!("Adding record without ID to ordered list");
                            all_records_ordered.push(record_data);
                            trace!("all_records_ordered now has {} records", all_records_ordered.len());
                        } else if let Some(id_str) = record_id {
                            // For records with IDs, use MVCC logic
                            debug!("Adding record with ID '{}' to latest_records", id_str);
                            latest_records.insert(id_str, record_data);
                            trace!("latest_records now has {} records", latest_records.len());
                        } else {
                            error!("Record has neither None ID nor Some ID - this shouldn't happen!");
                        }
                    } else {
                        trace!("End of row {} processing (should_keep was false)", i);
                    }
                }
                trace!("End of batch processing");
            }
            trace!("End of reader loop for file: {}", input_file);
        }
        debug!("End of all file processing");
        
        // Combine records with IDs (sorted by ID) and records without IDs (in order)
        let mut final_records: Vec<RecordData> = Vec::new();
        
        // First add all records with IDs, sorted by ID for consistent output
        let mut sorted_id_records: Vec<_> = latest_records.into_iter().collect();
        sorted_id_records.sort_by(|a, b| a.0.cmp(&b.0));
        
        let num_id_records = sorted_id_records.len();
        let num_no_id_records = all_records_ordered.len();
        
        for (_, record) in sorted_id_records {
            final_records.push(record);
        }
        
        // Then add all records without IDs in their original order
        final_records.extend(all_records_ordered);
        
        let total_records = final_records.len();
        info!("Final records count: {} (latest_records had {} entries, all_records_ordered had {} entries)",
                 total_records, num_id_records, num_no_id_records);
        info!("📊 MVCC resolution completed: {} records after merging (expired: {})", 
              total_records, expired_records_count);
        
        if final_records.is_empty() {
            warn!("⚠️ COMPACTION: No records to compact! All records expired or invalid?");
        } else {
            info!("📊 COMPACTION: Will write {} records to compacted file", total_records);
        }
        
        // Build Parquet data in memory
        let mut parquet_data = Vec::new();
        {
            let mut writer = ArrowWriter::try_new(&mut parquet_data, schema.clone(), None)
                .with_context(|| "Failed to create Arrow writer")?;
            
            // Build the final RecordBatch from collected data
            if !final_records.is_empty() {
                debug!("Building RecordBatch from {} records", final_records.len());
        info!("📊 Building RecordBatch from {} records", final_records.len());
                
                // Extract row data from records
                let row_data_vec: Vec<HashMap<String, serde_json::Value>> = final_records.iter()
                    .map(|r| r.row_data.clone())
                    .collect();
                
                let batch = self.build_record_batch_from_data(schema.clone(), &row_data_vec)?;
                debug!("Built RecordBatch with {} rows, writing to compacted file", batch.num_rows());
        info!("📊 Built RecordBatch with {} rows, writing to compacted file", batch.num_rows());
                
                if batch.num_rows() == 0 {
                    error!("❌ RecordBatch is empty despite having {} input records!", final_records.len());
                } else if batch.num_rows() != final_records.len() {
                    error!("❌ RecordBatch row count mismatch! Expected {}, got {}", 
                           final_records.len(), batch.num_rows());
                }
                
                writer.write(&batch)?;
            } else {
                warn!("⚠️ No records to write to compacted file!");
            }
            
            writer.close()?;
        }
        
        // Generate final output filename
        let output_filename = format!("compacted_{}_{}.parquet", 
            collection_id, 
            chrono::Utc::now().timestamp_millis()
        );
        
        info!(
            "🔄 [ATOMIC COMPACTION] Starting atomic compaction operation for collection {}",
            collection_id
        );
        
        // Begin atomic operation for compaction
        // Note: base_storage_url already includes collection path, so don't set collection_id
        let staging_config = StagingConfig {
            base_url: base_storage_url.clone(),
            collection_id: None,  // Don't duplicate collection path
            operation_type: StagingOperationType::Compaction,
            custom_staging_dir: None,
            auto_cleanup: true,
            max_orphaned_age_hours: 24,
        };
        
        let atomic_op = self.atomic_coordinator
            .begin_atomic_operation(&staging_config)
            .await
            .context("Failed to begin atomic compaction operation")?;
        
        info!("📝 Writing compacted file to staging: {}", output_filename);
        
        // Write compacted data to staging directory
        self.atomic_coordinator
            .write_to_staging(&atomic_op.operation_id, &output_filename, &parquet_data)
            .await
            .context("Failed to write compacted file to staging")?;
        
        // Finalize atomic operation - this will atomically move the file to final location
        self.atomic_coordinator
            .finalize_atomic_operation(&atomic_op.operation_id)
            .await
            .context("Failed to finalize atomic compaction")?;
        
        let final_path = format!("{}/{}", base_storage_url, output_filename);
        
        // Add a small delay to ensure filesystem operations complete
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        
        // Verify the compacted file exists before deleting input files
        info!("🔍 Verifying compacted file exists before cleanup: {}", final_path);
        
        // List files in the data directory to debug
        info!("📂 Listing files in data directory after finalization:");
        if let Ok(entries) = fs.list(base_storage_url).await {
            for entry in entries {
                if !entry.metadata.is_directory {
                    info!("  - {} (parquet: {})", entry.name, entry.name.ends_with(".parquet"));
                }
            }
        }
        
        if !fs.exists(&final_path).await? {
            // Check if it's still in staging
            let staging_path = format!("{}/___temp/__compact", base_storage_url);
            if let Ok(staging_entries) = fs.list(&staging_path).await {
                warn!("⚠️ Files still in staging directory:");
                for entry in staging_entries {
                    warn!("  - {}", entry.name);
                }
            }
            
            return Err(anyhow::anyhow!(
                "Compacted file {} does not exist after atomic operation - aborting input file deletion for safety",
                final_path
            ));
        }
        
        // Remove input files that were compacted (cleanup)
        // This happens AFTER the atomic operation to ensure data safety
        // In case of failure during deletion, we'll have duplicate data rather than data loss
        info!("🧹 Removing {} input files after successful compaction", input_files.len());
        let mut deleted_count = 0;
        let mut failed_deletions = Vec::new();
        
        for input_file in &input_files {
            if fs.exists(input_file).await? {
                match fs.delete(input_file).await {
                    Ok(_) => {
                        debug!("✅ Removed compacted input file: {}", input_file);
                        deleted_count += 1;
                    }
                    Err(e) => {
                        warn!("⚠️ Failed to remove input file {}: {}", input_file, e);
                        failed_deletions.push(input_file.clone());
                    }
                }
            }
        }
        
        if !failed_deletions.is_empty() {
            warn!(
                "⚠️ VIPER COMPACTION: {} input files could not be deleted. Manual cleanup may be required: {:?}",
                failed_deletions.len(),
                failed_deletions
            );
        }
        
        // Log cleanup statistics
        if expired_records_count > 0 {
            info!("🧹 VIPER COMPACTION CLEANUP: {} expired records physically deleted", expired_records_count);
        }
        
        let bytes_written = match fs.metadata(&final_path).await {
            Ok(metadata) => metadata.size,
            Err(_) => 0,
        };
        
        info!(
            "✅ [VIPER COMPACTION] Atomic Arrow/Parquet compaction completed for collection {}: {} records merged, {} expired deleted, {}/{} input files removed, final file: {:?}",
            collection_id,
            total_records,
            expired_records_count,
            deleted_count,
            input_files.len(),
            final_path
        );
        
        Ok(ViperCompactionResult {
            input_files: discovered_input_files,
            output_files: vec![final_path],
            entries_processed: total_records_processed as u64,
            entries_removed: expired_records_count as u64,
            bytes_read: total_bytes_read,
            bytes_written,
        })
    }


    
    /// Extract a value from a column at a specific row index
    fn extract_column_value(
        &self,
        column: &dyn arrow_array::Array,
        row_idx: usize,
        data_type: &arrow_schema::DataType,
    ) -> Result<serde_json::Value> {
        use arrow_array::{StringArray, Int64Array, Int8Array, Float32Array, BooleanArray};
        use arrow_schema::DataType;
        
        trace!("extract_column_value: row {}, type {:?}", row_idx, data_type);
        
        if column.is_null(row_idx) {
            trace!("  -> NULL value");
            return Ok(serde_json::Value::Null);
        }
        
        match data_type {
            DataType::Utf8 => {
                let array = column.as_any().downcast_ref::<StringArray>()
                    .ok_or_else(|| anyhow::anyhow!("Failed to downcast to StringArray"))?;
                Ok(serde_json::json!(array.value(row_idx)))
            }
            DataType::Int64 => {
                let array = column.as_any().downcast_ref::<Int64Array>()
                    .ok_or_else(|| anyhow::anyhow!("Failed to downcast to Int64Array"))?;
                Ok(serde_json::json!(array.value(row_idx)))
            }
            DataType::Int8 => {
                let array = column.as_any().downcast_ref::<Int8Array>()
                    .ok_or_else(|| anyhow::anyhow!("Failed to downcast to Int8Array"))?;
                Ok(serde_json::json!(array.value(row_idx)))
            }
            DataType::Float32 => {
                let array = column.as_any().downcast_ref::<Float32Array>()
                    .ok_or_else(|| anyhow::anyhow!("Failed to downcast to Float32Array"))?;
                Ok(serde_json::json!(array.value(row_idx)))
            }
            DataType::Boolean => {
                let array = column.as_any().downcast_ref::<BooleanArray>()
                    .ok_or_else(|| anyhow::anyhow!("Failed to downcast to BooleanArray"))?;
                Ok(serde_json::json!(array.value(row_idx)))
            }
            DataType::List(field) => {
                match field.data_type() {
                    DataType::Float32 => {
                        // Handle vector data
                        trace!("  -> Extracting List<Float32> vector");
                        let list_array = column.as_any().downcast_ref::<arrow_array::ListArray>()
                            .ok_or_else(|| anyhow::anyhow!("Failed to downcast to ListArray"))?;
                        trace!("  -> List array ok, getting value at index {}", row_idx);
                        let values = list_array.value(row_idx);
                        let float_array = values.as_any().downcast_ref::<Float32Array>()
                            .ok_or_else(|| anyhow::anyhow!("Failed to downcast vector to Float32Array"))?;
                        
                        trace!("  -> Float array has {} elements", float_array.len());
                        let vector: Vec<f32> = (0..float_array.len())
                            .map(|i| float_array.value(i))
                            .collect();
                        trace!("  -> Vector extracted: {} dimensions", vector.len());
                        Ok(serde_json::json!(vector))
                    }
                    DataType::Struct(_) => {
                        // Handle extra_meta - extract key-value pairs
                        trace!("  -> Extracting List<Struct> (extra_meta)");
                        let list_array = column.as_any().downcast_ref::<arrow_array::ListArray>()
                            .ok_or_else(|| anyhow::anyhow!("Failed to downcast to ListArray"))?;
                        
                        if list_array.is_null(row_idx) {
                            return Ok(serde_json::json!([]));
                        }
                        
                        let struct_array = list_array.value(row_idx);
                        let struct_array = struct_array.as_any().downcast_ref::<arrow_array::StructArray>()
                            .ok_or_else(|| anyhow::anyhow!("Failed to downcast to StructArray"))?;
                        
                        let key_array = struct_array.column_by_name("key")
                            .ok_or_else(|| anyhow::anyhow!("Missing 'key' column in struct"))?
                            .as_any().downcast_ref::<arrow_array::StringArray>()
                            .ok_or_else(|| anyhow::anyhow!("Failed to downcast key to StringArray"))?;
                        
                        let value_array = struct_array.column_by_name("value")
                            .ok_or_else(|| anyhow::anyhow!("Missing 'value' column in struct"))?
                            .as_any().downcast_ref::<arrow_array::StringArray>()
                            .ok_or_else(|| anyhow::anyhow!("Failed to downcast value to StringArray"))?;
                        
                        let mut metadata = Vec::new();
                        for i in 0..struct_array.len() {
                            if !key_array.is_null(i) && !value_array.is_null(i) {
                                metadata.push(serde_json::json!({
                                    "key": key_array.value(i),
                                    "value": value_array.value(i)
                                }));
                            }
                        }
                        
                        trace!("  -> Extracted {} metadata items", metadata.len());
                        Ok(serde_json::json!(metadata))
                    }
                    _ => {
                        // For other list types, store as null for now
                        trace!("  -> Unknown list type, returning null");
                        Ok(serde_json::Value::Null)
                    }
                }
            }
            _ => {
                // For unsupported types, store as null
                Ok(serde_json::Value::Null)
            }
        }
    }
    
    /// Build a RecordBatch from collected record data
    fn build_record_batch_from_data(
        &self,
        schema: Arc<arrow_schema::Schema>,
        records: &[HashMap<String, serde_json::Value>],
    ) -> Result<RecordBatch> {
        use arrow_array::{StringArray, Int64Array, Int8Array, Float32Array, BooleanArray};
        use arrow_schema::DataType;
        
        let mut column_builders: Vec<(String, Vec<serde_json::Value>)> = Vec::new();
        
        // Initialize builders for each field in schema
        for field in schema.fields() {
            column_builders.push((field.name().to_string(), Vec::with_capacity(records.len())));
        }
        
        // Collect data for each column
        for record in records {
            for (field_name, values) in &mut column_builders {
                let value = record.get(field_name)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                values.push(value);
            }
        }
        
        // Build Arrow arrays for each column
        let mut arrays: Vec<Arc<dyn arrow_array::Array>> = Vec::new();
        
        for (field_idx, field) in schema.fields().iter().enumerate() {
            let (_, values) = &column_builders[field_idx];
            
            let array: Arc<dyn arrow_array::Array> = match field.data_type() {
                DataType::Utf8 => {
                    let string_values: Vec<Option<String>> = values.iter()
                        .map(|v| if v.is_null() { None } else { v.as_str().map(|s| s.to_string()) })
                        .collect();
                    Arc::new(StringArray::from(string_values))
                }
                DataType::Int64 => {
                    let int_values: Vec<Option<i64>> = values.iter()
                        .map(|v| if v.is_null() { None } else { v.as_i64() })
                        .collect();
                    Arc::new(Int64Array::from(int_values))
                }
                DataType::Int8 => {
                    let int_values: Vec<Option<i8>> = values.iter()
                        .map(|v| if v.is_null() { None } else { v.as_i64().map(|i| i as i8) })
                        .collect();
                    Arc::new(Int8Array::from(int_values))
                }
                DataType::Float32 => {
                    let float_values: Vec<Option<f32>> = values.iter()
                        .map(|v| if v.is_null() { None } else { v.as_f64().map(|f| f as f32) })
                        .collect();
                    Arc::new(Float32Array::from(float_values))
                }
                DataType::Boolean => {
                    let bool_values: Vec<Option<bool>> = values.iter()
                        .map(|v| if v.is_null() { None } else { v.as_bool() })
                        .collect();
                    Arc::new(BooleanArray::from(bool_values))
                }
                DataType::List(inner_field) => {
                    match inner_field.data_type() {
                        DataType::Float32 => {
                            // Handle vector data
                            use arrow_array::builder::{ListBuilder, Float32Builder};
                            let mut list_builder = ListBuilder::new(Float32Builder::new());
                            
                            for value in values {
                                if value.is_null() {
                                    list_builder.append(false);
                                } else if let Some(vec_array) = value.as_array() {
                                    let float_values: Vec<f32> = vec_array.iter()
                                        .filter_map(|v| v.as_f64().map(|f| f as f32))
                                        .collect();
                                    
                                    for &val in &float_values {
                                        list_builder.values().append_value(val);
                                    }
                                    list_builder.append(true);
                                } else {
                                    list_builder.append(false);
                                }
                            }
                            
                            Arc::new(list_builder.finish())
                        }
                        DataType::Struct(_) => {
                            // Handle extra_meta - build from extracted metadata
                            trace!("Building List<Struct> array for extra_meta");
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
                            
                            for value in values {
                                if let Some(metadata_array) = value.as_array() {
                                    let struct_builder = list_builder.values();
                                    for item in metadata_array {
                                        if let Some(obj) = item.as_object() {
                                            if let (Some(key), Some(val)) = (obj.get("key"), obj.get("value")) {
                                                if let (Some(key_str), Some(val_str)) = (key.as_str(), val.as_str()) {
                                                    struct_builder.field_builder::<StringBuilder>(0)
                                                        .unwrap()
                                                        .append_value(key_str);
                                                    struct_builder.field_builder::<StringBuilder>(1)
                                                        .unwrap()
                                                        .append_value(val_str);
                                                    struct_builder.append(true);
                                                }
                                            }
                                        }
                                    }
                                    list_builder.append(true);
                                } else {
                                    // Empty metadata list
                                    list_builder.append(false);
                                }
                            }
                            
                            Arc::new(list_builder.finish())
                        }
                        _ => {
                            return Err(anyhow::anyhow!("Unsupported list type: {:?}", inner_field.data_type()));
                        }
                    }
                }
                _ => {
                    return Err(anyhow::anyhow!("Unsupported column type: {:?}", field.data_type()));
                }
            };
            
            arrays.push(array);
        }
        
        RecordBatch::try_new(schema, arrays)
            .context("Failed to create RecordBatch from collected data")
    }

}