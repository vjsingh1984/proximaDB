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
use parquet::file::reader::{FileReader, SerializedFileReader};
// Use columnar module's StreamingParquetWriter instead of direct ArrowWriter
use crate::storage::engines::core::formats::columnar::{
    ParquetWriterConfig, QuantizationConfig as ColumnarQuantizationConfig, StreamingParquetWriter,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, trace, warn};

use crate::core::VectorRecord;
use crate::core::search::mvcc_resolution::MvccResolver;

use crate::storage::common::compaction_utils::StorageEngineType as CompactionEngineType;
use crate::storage::common::*;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::transaction_coordinator::{
    StagingConfig, TransactionCoordinator, TransactionStageType,
};

use crate::storage::engines::core::formats::columnar::columnar_schema::ColumnarSchema;

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

/// File metadata for compaction planning
#[derive(Debug, Clone)]
pub struct FileMetadata {
    /// File path
    pub path: String,
    /// File size in bytes
    pub size_bytes: u64,
    /// Number of rows in the file
    pub row_count: u64,
    /// Average row size in bytes
    pub avg_row_size: f64,
    /// File level (0 = fresh flush, higher = more compacted)
    pub level: u32,
    /// File creation timestamp (for ordering within levels)
    pub timestamp: u64,
}

// VIPER filename operations use unified FilenameCodec from compaction_orchestrator

/// Level-based compaction task
#[derive(Debug, Clone)]
pub struct LevelBasedCompactionTask {
    /// Source level being compacted
    pub level: u32,
    /// Input files for compaction
    pub input_files: Vec<String>,
    /// Target level for output files
    pub target_level: u32,
}

/// Compaction plan based on file analysis
#[derive(Debug, Clone)]
pub struct CompactionPlan {
    /// Input file metadata
    pub input_files: Vec<FileMetadata>,
    /// Total rows across all input files
    pub total_rows: u64,
    /// Total size across all input files
    pub total_size_bytes: u64,
    /// Average row size across all files
    pub avg_row_size: f64,
    /// Target number of output files
    pub target_file_count: usize,
    /// Target rows per output file
    pub rows_per_file: u64,
    /// Estimated size per output file
    pub estimated_size_per_file: u64,
}

/// Compaction operations for VIPER storage engine with atomic writes
pub struct Compaction {
    /// Schema for columnar storage
    schema: ColumnarSchema,

    /// Collection service for metadata access
    collection_service:
        Arc<RwLock<Option<Arc<crate::services::collection::manager::CollectionService>>>>,

    /// Filesystem factory for cross-cloud atomic writes
    filesystem_factory: Arc<FilesystemFactory>,

    /// Atomic coordinator for ACID operations
    atomic_coordinator: Arc<TransactionCoordinator>,

    /// New compaction orchestrator
    compaction_orchestrator: Option<Arc<CompactionOrchestrator>>,

    /// Compaction configuration (from server config)
    compaction_config: crate::core::config::CompactionConfig,

    /// Optional metrics updater for non-critical metrics
    metrics_updater: Option<Arc<dyn crate::metrics::InternalMetricsUpdater>>,
}

impl std::fmt::Debug for Compaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Compaction")
            .field("schema", &self.schema)
            .field("collection_service", &self.collection_service)
            .field("filesystem_factory", &self.filesystem_factory)
            .field("atomic_coordinator", &self.atomic_coordinator)
            .field("compaction_config", &self.compaction_config)
            .field("metrics_updater", &self.metrics_updater.is_some())
            .finish()
    }
}

impl Compaction {
    pub async fn new(
        collection_service: Arc<
            RwLock<Option<Arc<crate::services::collection::manager::CollectionService>>>,
        >,
        filesystem_factory: Arc<FilesystemFactory>,
        compaction_config: Option<crate::core::config::CompactionConfig>,
    ) -> Result<Self> {
        // Use provided config or defaults
        let compaction_config =
            compaction_config.unwrap_or_else(crate::core::config::CompactionConfig::default);

        // Create atomic coordinator
        let atomic_coordinator = Arc::new(
            TransactionCoordinator::new(filesystem_factory.clone(), None)
                .await
                .context("Failed to create atomic coordinator")?,
        );

        // Initialize new compaction orchestrator with server configuration
        let orchestrator_config = CompactionConfig {
            level0_threshold: compaction_config.l0_file_threshold,
            level_threshold: (compaction_config.l0_file_threshold as f64
                * compaction_config.level_multiplier) as usize,
            max_level: compaction_config.max_levels as u32,
            max_concurrent_per_collection: 1,
            global_max_concurrent: 3,
            operation_timeout: std::time::Duration::from_secs(1800), // 30 minutes for VIPER
            queue_aware_compaction: true,
            max_queue_wait: std::time::Duration::from_secs(120), // 2 minutes for VIPER
            urgency_threshold: 0.75,
        };

        let orchestrator = Some(Arc::new(CompactionOrchestrator::new(
            filesystem_factory.clone(),
            orchestrator_config,
        )));

        Ok(Self {
            schema: ColumnarSchema::new(),
            collection_service,
            filesystem_factory,
            atomic_coordinator,
            compaction_orchestrator: orchestrator,
            compaction_config,
            metrics_updater: None, // Will be set via set_metrics_updater
        })
    }

    /// Set the metrics updater (optional, for non-critical metrics)
    pub fn set_metrics_updater(
        &mut self,
        updater: Arc<dyn crate::metrics::InternalMetricsUpdater>,
    ) {
        self.metrics_updater = Some(updater);
    }

    /// Discover files organized by level for level-based compaction
    async fn get_parquet_files_by_level(
        &self,
        collection_id: &str,
        collection_config: Option<&crate::proto::proximadb_v1::Collection>,
    ) -> Result<std::collections::HashMap<u32, Vec<FileMetadata>>> {
        use std::collections::HashMap;

        // Use provided collection config or return empty if not available
        let collection = match collection_config {
            Some(col) => col,
            None => {
                return Ok(HashMap::new());
            }
        };

        let storage_assignment = match &collection.storage_assignment {
            Some(assignment) => assignment,
            None => {
                return Ok(HashMap::new());
            }
        };

        let data_url = format!(
            "{}/{}/data",
            storage_assignment.base_location, collection_id
        );
        info!(
            "🔍 VIPER: Looking for parquet files in data directory: {}",
            data_url
        );

        let fs = self.filesystem_factory.get_filesystem(&data_url)?;
        let mut files_by_level: HashMap<u32, Vec<FileMetadata>> = HashMap::new();

        if fs.exists(&data_url).await? {
            let entries = fs.list(&data_url).await?;
            info!(
                "📋 VIPER: Found {} entries in data directory",
                entries.len()
            );

            for entry in entries {
                debug!(
                    "🔍 VIPER: Checking entry: {} (is_dir: {})",
                    entry.name, entry.metadata.is_directory
                );

                // Skip staging directories
                if entry.name.starts_with("__") {
                    debug!("⏭️ VIPER: Skipping staging directory/file: {}", entry.name);
                    continue;
                }

                if !entry.metadata.is_directory && entry.name.ends_with(".parquet") {
                    let codec = FilenameCodec::new();
                    let level = codec.parse_level(&entry.name);
                    let timestamp = codec.parse_timestamp(&entry.name);

                    debug!(
                        "✅ VIPER: Found Parquet file: {} at level {}",
                        entry.name, level
                    );

                    let file_metadata = FileMetadata {
                        path: entry.url.clone(),
                        size_bytes: entry.metadata.size,
                        row_count: 0, // Will be populated later if needed
                        avg_row_size: 0.0,
                        level,
                        timestamp,
                    };

                    files_by_level
                        .entry(level)
                        .or_insert_with(Vec::new)
                        .push(file_metadata);
                }
            }
        } else {
            info!("⚠️ VIPER: Data directory does not exist: {}", data_url);
        }

        info!(
            "🔍 VIPER: Files by level: {:?}",
            files_by_level
                .iter()
                .map(|(k, v)| (k, v.len()))
                .collect::<Vec<_>>()
        );

        Ok(files_by_level)
    }

    /// Check if level-based compaction is needed and return the task
    async fn check_level_based_compaction_needed(
        &self,
        collection_id: &str,
        collection_config: Option<&crate::proto::proximadb_v1::Collection>,
    ) -> Result<Option<LevelBasedCompactionTask>> {
        // Use provided collection config or return None if not available
        let collection = match collection_config {
            Some(col) => col,
            None => return Ok(None),
        };

        let storage_assignment = match &collection.storage_assignment {
            Some(assignment) => assignment,
            None => return Ok(None),
        };

        let data_url = format!(
            "{}/{}/data",
            storage_assignment.base_location, collection_id
        );
        debug!(
            "🔍 VIPER LEVEL-BASED: Delegating to unified framework for collection {}",
            collection_id
        );

        // Use stored VIPER compaction config
        let task_info = CompactionTaskBuilder::check_and_build_compaction_task(
            collection_id,
            &data_url,
            "parquet",
            CompactionEngineType::VIPER,
            &self.compaction_config,
            self.filesystem_factory.clone(),
        )
        .await?;

        Ok(task_info.map(|info| LevelBasedCompactionTask {
            level: info.source_level,
            input_files: info.input_files,
            target_level: info.target_level,
        }))
    }

    /// Discover files that can be compacted for a collection
    async fn discover_compactable_files(
        &self,
        collection_id: &str,
        collection_config: Option<&crate::proto::proximadb_v1::Collection>,
    ) -> Result<Vec<String>> {
        // Use provided collection config or return empty if not available
        let collection = match collection_config {
            Some(col) => col,
            None => {
                return Ok(vec![]); // Return empty list instead of failing
            }
        };

        let storage_assignment = match &collection.storage_assignment {
            Some(assignment) => assignment,
            None => {
                return Ok(vec![]); // Return empty list instead of failing
            }
        };

        let data_url = format!(
            "{}/{}/data",
            storage_assignment.base_location, collection_id
        );
        debug!(
            "🔍 VIPER COMPACTION: Delegating to unified framework for collection {} in {}",
            collection_id, data_url
        );

        // Use unified compaction task builder to get all compactable files
        let compactable_files = CompactionTaskBuilder::get_all_compactable_files(
            collection_id,
            &data_url,
            "parquet",
            CompactionEngineType::VIPER,
            self.filesystem_factory.clone(),
        )
        .await?;

        debug!(
            "📋 VIPER COMPACTION: Unified framework returned {} compactable files for collection {}",
            compactable_files.len(),
            collection_id
        );

        Ok(compactable_files)
    }

    /// Analyze candidate files and create a compaction plan
    async fn analyze_files_and_plan_compaction(
        &self,
        input_files: &[String],
        target_file_size_mb: u64,
    ) -> Result<CompactionPlan> {
        info!(
            "🔍 Analyzing {} files for compaction planning",
            input_files.len()
        );

        let mut file_metadata = Vec::new();
        let fs = self.filesystem_factory.get_filesystem("file:///")?;

        // Analyze each input file
        for (idx, file_path) in input_files.iter().enumerate() {
            // Get file size
            let file_size = match fs.metadata(file_path).await {
                Ok(metadata) => metadata.size,
                Err(e) => {
                    warn!("Failed to get metadata for {}: {}", file_path, e);
                    continue;
                }
            };

            // Read Parquet metadata (just footer, not full file)
            let file_data = fs.read(file_path).await?;
            let parquet_bytes = bytes::Bytes::from(file_data);

            let metadata = match SerializedFileReader::new(parquet_bytes) {
                Ok(reader) => reader.metadata().clone(),
                Err(e) => {
                    warn!("Failed to parse Parquet metadata for {}: {}", file_path, e);
                    continue;
                }
            };

            let row_count = metadata.file_metadata().num_rows() as u64;
            let avg_row_size = if row_count > 0 {
                file_size as f64 / row_count as f64
            } else {
                0.0
            };

            info!(
                "📊 File {}/{}: {} - Size: {:.2}MB, Rows: {}, Avg row size: {:.2}KB",
                idx + 1,
                input_files.len(),
                file_path.split('/').last().unwrap_or("unknown"),
                file_size as f64 / (1024.0 * 1024.0),
                row_count,
                avg_row_size / 1024.0
            );

            file_metadata.push(FileMetadata {
                path: file_path.clone(),
                size_bytes: file_size,
                row_count,
                avg_row_size,
                level: 0, // Default to level 0 for now, will be updated when unified framework is applied
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
            });
        }

        // Calculate totals and averages
        let total_rows: u64 = file_metadata.iter().map(|f| f.row_count).sum();
        let total_size_bytes: u64 = file_metadata.iter().map(|f| f.size_bytes).sum();
        let avg_row_size = if total_rows > 0 {
            total_size_bytes as f64 / total_rows as f64
        } else {
            1024.0 // Default 1KB per row
        };

        // Calculate target file count based on target size
        let target_file_size_bytes = target_file_size_mb * 1024 * 1024;
        let target_file_count =
            ((total_size_bytes as f64 / target_file_size_bytes as f64).ceil() as usize).max(1);
        let rows_per_file = (total_rows as f64 / target_file_count as f64).ceil() as u64;
        let estimated_size_per_file = (rows_per_file as f64 * avg_row_size) as u64;

        info!("📊 Compaction Plan Summary:");
        info!("  - Input files: {}", file_metadata.len());
        info!(
            "  - Total size: {:.2}MB",
            total_size_bytes as f64 / (1024.0 * 1024.0)
        );
        info!("  - Total rows: {}", total_rows);
        info!("  - Average row size: {:.2}KB", avg_row_size / 1024.0);
        info!("  - Target output files: {}", target_file_count);
        info!("  - Rows per output file: {}", rows_per_file);
        info!(
            "  - Estimated size per output file: {:.2}MB",
            estimated_size_per_file as f64 / (1024.0 * 1024.0)
        );

        Ok(CompactionPlan {
            input_files: file_metadata,
            total_rows,
            total_size_bytes,
            avg_row_size,
            target_file_count,
            rows_per_file,
            estimated_size_per_file,
        })
    }

    /// Arrow/Parquet compaction with version merging and expiry logic
    /// This is the main compaction implementation for VIPER storage engine
    pub async fn compact_parquet_files(
        &self,
        collection_id: &str,
        input_files: Vec<String>,
        collection_config: Option<&crate::proto::proximadb_v1::Collection>,
    ) -> Result<ViperCompactionResult> {
        // Note: Files are already filtered in discover_compactable_files
        // Only AXIS-ready files reach this point
        debug!(
            "🔧 VIPER COMPACTION: Starting compaction with {} pre-filtered files for collection {}",
            input_files.len(),
            collection_id
        );

        // Extract vector dimensions - REQUIRED for compaction
        let vector_dimensions = collection_config
            .as_ref()
            .and_then(|collection| collection.config.as_ref())
            .map(|config| config.dimension)
            .ok_or_else(|| {
                error!("CRITICAL: Collection config missing for collection {} during compaction. Cannot proceed without vector dimensions!", collection_id);
                anyhow::anyhow!("Collection config with dimension is required for compaction")
            })?;

        info!(
            "🔧 VIPER COMPACTION: Starting with {} dimensions for collection {}",
            vector_dimensions, collection_id
        );

        self.compact_parquet_files_with_config(
            collection_id,
            input_files,
            vector_dimensions as usize,
            collection_config.cloned(),
        )
        .await
    }

    /// Internal compaction implementation with collection config
    async fn compact_parquet_files_with_config(
        &self,
        collection_id: &str,
        input_files: Vec<String>,
        _vector_dimensions: usize,
        collection_config: Option<crate::proto::proximadb_v1::Collection>,
    ) -> Result<ViperCompactionResult> {
        info!(
            "Starting atomic Arrow/Parquet compaction for collection {}",
            collection_id
        );
        info!(
            "🔄 [VIPER COMPACTION] Starting atomic Arrow/Parquet compaction for collection {}",
            collection_id
        );

        // If no input files specified, discover them from storage
        let input_files = if input_files.is_empty() {
            self.discover_compactable_files(collection_id, collection_config.as_ref())
                .await?
        } else {
            input_files
        };

        if input_files.is_empty() {
            info!(
                "📋 No files found for compaction in collection {}",
                collection_id
            );
            return Ok(ViperCompactionResult {
                input_files: vec![],
                output_files: vec![],
                entries_processed: 0,
                entries_removed: 0,
                bytes_read: 0,
                bytes_written: 0,
            });
        }

        // Analyze files and create compaction plan (target 128MB files)
        let compaction_plan = self
            .analyze_files_and_plan_compaction(&input_files, 128)
            .await?;

        let discovered_input_files = input_files.clone(); // Save for result
        let mut total_bytes_read = 0u64;
        let mut total_records_processed = 0usize;

        info!("📋 Input files for compaction: {:?}", input_files);

        // Use provided collection config or return empty result if not available
        let collection = match &collection_config {
            Some(col) => col,
            None => {
                info!(
                    "🔍 Collection config not provided for {}, skipping compaction_info",
                    collection_id
                );
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

        let storage_assignment = match &collection.storage_assignment {
            Some(assignment) => assignment,
            None => {
                info!(
                    "🔍 No storage assignment found for collection {}, compaction not needed",
                    collection_id
                );
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
        let base_storage_url = format!(
            "{}/{}/data",
            storage_assignment.base_location, collection_id
        );

        // Atomic write will handle staging internally

        // Use the schema from the compaction instance
        let schema = &self.schema;

        // COMPACTION FIX: Validate schema compatibility before processing
        // This prevents column alignment issues during compaction
        info!(
            "🔍 Validating schema compatibility for {} input files",
            input_files.len()
        );
        let fs = self.filesystem_factory.get_filesystem("file:///")?;
        let mut input_schemas = Vec::new();
        for input_file in &input_files {
            if let Ok(file_data) = fs.read(input_file).await {
                let parquet_bytes = bytes::Bytes::from(file_data);
                if let Ok(builder) =
                    parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(
                        parquet_bytes,
                    )
                {
                    input_schemas.push(builder.schema().clone());
                }
            }
        }

        // Log schema differences for debugging
        // TODO: Re-enable schema validation after refactoring to get Arrow Schema from ColumnarSchema
        for (idx, _input_schema) in input_schemas.iter().enumerate() {
            debug!("📊 Processing schema validation for file {}", idx);
            // Schema validation temporarily disabled due to type mismatch
            // Need to extract Arrow Schema from ColumnarSchema first
        }

        // Process each input file and merge records by ID with MVCC resolution
        // Instead of storing RecordBatches, store the actual data for each record
        #[derive(Clone)]
        struct RecordData {
            id: Option<String>, // ID is optional
            version: i64,
            timestamp: Option<i64>, // For duplicate resolution
            row_data: HashMap<String, serde_json::Value>, // Store all column data
        }

        let mut latest_records: HashMap<String, RecordData> = HashMap::new();
        let mut all_records_ordered: Vec<RecordData> = Vec::new(); // Maintain order for records without IDs
        let current_time = chrono::Utc::now().timestamp(); // i64 seconds to match parquet expires_at column
        let mut expired_records_count = 0;

        info!(
            "Processing {} input files for version merging and expiry logic",
            input_files.len()
        );
        info!(
            "⚡ Processing {} input files for version merging and expiry logic",
            input_files.len()
        );

        for (file_idx, input_file) in input_files.iter().enumerate() {
            debug!(
                "Processing file {}/{}: {}",
                file_idx + 1,
                input_files.len(),
                input_file
            );
            info!(
                "📂 Processing file {}/{}: {}",
                file_idx + 1,
                input_files.len(),
                input_file
            );

            let file_data = fs
                .read(input_file)
                .await
                .with_context(|| format!("Failed to read input file: {}", input_file))?;
            total_bytes_read += file_data.len() as u64;

            info!("  📊 Read {} bytes from file", file_data.len());
            let parquet_bytes = bytes::Bytes::from(file_data);

            let builder = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(
                parquet_bytes,
            )
            .with_context(|| format!("Failed to create Parquet reader for: {}", input_file))?;

            let reader = builder
                .build()
                .with_context(|| format!("Failed to build Parquet reader for: {}", input_file))?;

            for batch_result in reader {
                let batch = batch_result
                    .with_context(|| format!("Failed to read batch from: {}", input_file))?;

                debug!("Got batch with {} rows", batch.num_rows());

                // Process each record in the batch for MVCC resolution
                let id_array = batch
                    .column_by_name("id")
                    .unwrap()
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .unwrap();
                // Handle version as Int8 (as written by flush) or Int64 (legacy)
                let version_array = batch.column_by_name("version").unwrap();
                let version_iter: Box<dyn Iterator<Item = Option<i64>>> = if let Some(arr) =
                    version_array
                        .as_any()
                        .downcast_ref::<arrow_array::Int8Array>()
                {
                    Box::new(arr.iter().map(|v| v.map(|i| i as i64)))
                } else if let Some(arr) = version_array.as_any().downcast_ref::<Int64Array>() {
                    Box::new(arr.iter())
                } else {
                    return Err(anyhow::anyhow!("Invalid version column type"));
                };

                // COMPACTION FIX: Handle missing expires_at column for backward compatibility
                let null_expires_array;
                let expires_at_array = match batch.column_by_name("expires_at") {
                    Some(column) => {
                        column
                            .as_any()
                            .downcast_ref::<Int64Array>()
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "Invalid 'expires_at' column type in {}",
                                    input_file
                                )
                            })?
                    }
                    None => {
                        // Create null array for missing expires_at column
                        warn!(
                            "⚠️ Missing 'expires_at' column in {}, creating null array",
                            input_file
                        );
                        null_expires_array =
                            Int64Array::from(vec![Option::<i64>::None; batch.num_rows()]);
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
                    let record_version = version_values[i];
                    trace!("Record version: {:?}", record_version.unwrap_or(-1));

                    // Extract timestamp for duplicate resolution
                    let record_timestamp = batch
                        .column_by_name("timestamp")
                        .and_then(|col| col.as_any().downcast_ref::<Int64Array>())
                        .map(|arr| {
                            if arr.is_null(i) {
                                None
                            } else {
                                Some(arr.value(i))
                            }
                        });

                    let record_expires_at = if expires_at_array.is_null(i) {
                        trace!("expires_at is NULL for row {}", i);
                        None
                    } else {
                        let expires_value = expires_at_array.value(i);
                        trace!("expires_at value for row {}: {}", i, expires_value);
                        Some(expires_value)
                    };

                    // Check for tombstone records (deleted records marked with special metadata)
                    let is_tombstone = batch
                        .column_by_name("is_deleted")
                        .and_then(|col| col.as_any().downcast_ref::<arrow_array::BooleanArray>())
                        .map(|arr| !arr.is_null(i) && arr.value(i));

                    if is_tombstone.unwrap_or(false) {
                        expired_records_count += 1; // Count tombstones as removed records
                        debug!(
                            "🪦 VIPER COMPACTION: Physically deleting tombstoned record {:?}",
                            record_id
                        );
                        continue;
                    }

                    // Skip expired records (treat 0 as no expiry)
                    trace!(
                        "Current time: {}, Record expires_at: {:?}",
                        current_time, record_expires_at
                    );
                    if let Some(expires_at) = record_expires_at {
                        // Only consider expired if expires_at > 0 (0 means no expiry)
                        if expires_at > 0 && expires_at < current_time {
                            expired_records_count += 1;
                            debug!(
                                "Deleting expired record {:?} (expired at {} < current {})",
                                record_id, expires_at, current_time
                            );
                            debug!(
                                "⏰ VIPER COMPACTION: Physically deleting expired record {:?} (expired at {})",
                                record_id, expires_at
                            );
                            continue;
                        }
                    }

                    trace!("About to check should_keep for row {}", i);
                    trace!(
                        "Latest records count before check: {}",
                        latest_records.len()
                    );

                    // Handle immutable vectors (null/empty IDs) - skip ID-based merging
                    let should_keep = if record_id.as_ref().map_or(true, |id| id.is_empty()) {
                        debug!("📝 Immutable vector record (no ID), including as-is");
                        true
                    } else if let Some(ref id_str) = record_id {
                        // Use centralized MVCC resolution for version merging logic
                        if let Some(existing_record) = latest_records.get(id_str) {
                            // Create temporary VectorRecord instances for comparison
                            let existing_vector_record = VectorRecord {
                                id: id_str.clone(),
                                version: existing_record.version,
                                timestamp: existing_record.timestamp.unwrap_or(0),
                                vector: vec![],
                                metadata: std::collections::HashMap::new(),
                                updated_at: existing_record.timestamp.map(|t| t as i64),
                                expires_at: None,
                                quantized_vector: vec![],
                                source: None,
                            };

                            let current_vector_record = VectorRecord {
                                id: id_str.clone(),
                                version: record_version.unwrap_or(0),
                                timestamp: record_timestamp.flatten().unwrap_or(0),
                                vector: vec![],
                                metadata: std::collections::HashMap::new(),
                                updated_at: record_timestamp.flatten().map(|t| t as i64),
                                expires_at: None,
                                quantized_vector: vec![],
                                source: None,
                            };

                            // Use centralized MVCC resolver for comparison
                            let resolver = MvccResolver::new();
                            let should_replace = resolver
                                .compare_records(&current_vector_record, &existing_vector_record);

                            if should_replace {
                                debug!(
                                    "📝 Centralized MVCC: Updating record {} from version {} to {}",
                                    id_str,
                                    existing_record.version,
                                    record_version.unwrap_or(-1)
                                );
                            } else {
                                debug!(
                                    "📝 Centralized MVCC: Keeping existing record {} at version {} (incoming version {})",
                                    id_str,
                                    existing_record.version,
                                    record_version.unwrap_or(-1)
                                );
                            }

                            should_replace
                        } else {
                            debug!(
                                "📝 New record {} at version {}",
                                id_str,
                                record_version.unwrap_or(-1)
                            );
                            true
                        }
                    } else {
                        // Should not reach here, but keep for safety
                        true
                    };

                    trace!(
                        "Row {} - should_keep: {}, id: {:?}, version: {}",
                        i,
                        should_keep,
                        record_id,
                        record_version.unwrap_or(-1)
                    );

                    if !should_keep {
                        trace!("Skipping row {} - should_keep is false", i);
                    }

                    if should_keep {
                        trace!("Row {} should be kept, starting extraction", i);
                        // Extract all column data for this record
                        let mut row_data = HashMap::new();

                        // Extract data from each column
                        trace!(
                            "Extracting data for row {} ({} columns)",
                            i,
                            batch.schema().fields().len()
                        );
                        for (col_idx, field) in batch.schema().fields().iter().enumerate() {
                            trace!("  Column {}: {}", col_idx, field.name());
                            let column = batch.column(col_idx);
                            let value = self.extract_column_value(column, i)?;
                            row_data.insert(field.name().to_string(), value);
                        }
                        trace!("Row {} extracted successfully", i);
                        trace!(
                            "Record ID: {:?}, Version: {}",
                            record_id,
                            record_version.unwrap_or(-1)
                        );

                        let record_data = RecordData {
                            id: record_id.clone(),
                            version: record_version.unwrap_or(0),
                            timestamp: record_timestamp.flatten(),
                            row_data,
                        };

                        // For immutable vectors (no ID), add to ordered list
                        if record_id.as_ref().map_or(true, |id| id.is_empty()) {
                            debug!("Adding record without ID to ordered list");
                            all_records_ordered.push(record_data);
                            trace!(
                                "all_records_ordered now has {} records",
                                all_records_ordered.len()
                            );
                        } else if let Some(id_str) = record_id {
                            // For records with IDs, use MVCC logic
                            debug!("Adding record with ID '{}' to latest_records", id_str);
                            latest_records.insert(id_str, record_data);
                            trace!("latest_records now has {} records", latest_records.len());
                        } else {
                            error!(
                                "Record has neither None ID nor Some ID - this shouldn't happen!"
                            );
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
        info!(
            "Final records count: {} (latest_records had {} entries, all_records_ordered had {} entries)",
            total_records, num_id_records, num_no_id_records
        );
        info!(
            "📊 MVCC resolution completed: {} records after merging (expired: {})",
            total_records, expired_records_count
        );

        if final_records.is_empty() {
            warn!("⚠️ COMPACTION: No records to compact! All records expired or invalid?");
            return Ok(ViperCompactionResult {
                input_files: discovered_input_files,
                output_files: vec![],
                entries_processed: total_records_processed as u64,
                entries_removed: expired_records_count as u64,
                bytes_read: total_bytes_read,
                bytes_written: 0,
            });
        } else {
            info!(
                "📊 COMPACTION: Will write {} records to {} output files",
                total_records, compaction_plan.target_file_count
            );
        }

        info!(
            "🔄 [ATOMIC COMPACTION] Starting atomic compaction operation for collection {}",
            collection_id
        );
        debug!(
            "🔄 [DEBUG] Starting atomic compaction for collection: {}",
            collection_id
        );
        debug!("    base_storage_url: {}", base_storage_url);

        // Begin atomic operation for compaction
        // Note: base_storage_url already includes collection path, so don't set collection_id
        let staging_config = StagingConfig {
            base_url: base_storage_url.clone(),
            collection_id: None, // Don't duplicate collection path
            operation_type: TransactionStageType::Compaction,
            custom_staging_dir: None,
            auto_cleanup: true,
            max_orphaned_age_hours: 24,
            skip_uuid_subdir: true, // Use simple __compact directory without UUID subdirectory
        };

        debug!("📋 [DEBUG] StagingConfig for compaction:");
        debug!("    base_url: {}", staging_config.base_url);
        debug!("    collection_id: {:?}", staging_config.collection_id);
        debug!("    operation_type: {:?}", staging_config.operation_type);
        debug!("    skip_uuid_subdir: {}", staging_config.skip_uuid_subdir);

        let atomic_op = self
            .atomic_coordinator
            .begin_atomic_operation(&staging_config)
            .await
            .context("Failed to begin atomic compaction operation")?;

        debug!("📦 [DEBUG] Atomic operation created:");
        debug!("    operation_id: {}", atomic_op.operation_id);
        debug!("    staging_url: {}", atomic_op.staging_url);
        debug!("    final_url: {}", atomic_op.final_url);

        // Split records across multiple output files
        let mut output_files = Vec::new();
        let mut total_bytes_written = 0u64;
        let timestamp_base = chrono::Utc::now().timestamp_millis();

        for file_idx in 0..compaction_plan.target_file_count {
            let start_idx = file_idx * compaction_plan.rows_per_file as usize;
            let end_idx =
                ((file_idx + 1) * compaction_plan.rows_per_file as usize).min(final_records.len());

            if start_idx >= final_records.len() {
                break;
            }

            let file_records = &final_records[start_idx..end_idx];
            let file_record_count = file_records.len();

            info!(
                "📊 Building output file {}/{} with {} records (rows {}-{})",
                file_idx + 1,
                compaction_plan.target_file_count,
                file_record_count,
                start_idx,
                end_idx - 1
            );

            // Create a temporary file for StreamingParquetWriter
            let temp_dir = std::env::temp_dir();
            let temp_file_path = temp_dir.join(format!(
                "viper_compaction_{}.parquet.tmp",
                crate::utils::uuid::Uuid::new_v4()
            ));
            debug!(
                "Creating temporary compacted Parquet file: {:?}",
                temp_file_path
            );

            {
                debug!("🔍 VIPER COMPACTION: Creating StreamingParquetWriter with bloom filters");
                debug!("📊 VIPER COMPACTION PATH ANALYSIS:");
                debug!("   - Input: Streaming Parquet files from multiple sources");
                debug!("   - Processing: Merge → MVCC resolution → Quantize → Columnar layout");
                debug!(
                    "   - Output: Consolidated Parquet files with optimized layout and bloom filters"
                );
                debug!("   - Bloom filters: Enabled for efficient ID lookups");
                debug!("   - Compression: Collection config drives Parquet compression");

                // Extract compression configuration from collection metadata
                let compression_algorithm = if let Some(ref collection) = collection_config {
                    if let Some(ref config) = collection.config {
                        if let Some(ref storage_config) = config.storage_config {
                            if storage_config.compression != 0 {
                                use crate::proto::proximadb_v1::CompressionAlgorithm;

                                debug!(
                                    "   ✅ Found compression config: algorithm={}",
                                    storage_config.compression
                                );

                                // Convert proto compression to core compression algorithm
                                match CompressionAlgorithm::try_from(storage_config.compression) {
                                    Ok(CompressionAlgorithm::CompressionZstd) => {
                                        crate::core::compression::CompressionAlgorithm::Zstd
                                    }
                                    Ok(CompressionAlgorithm::CompressionLz4) => {
                                        crate::core::compression::CompressionAlgorithm::Lz4
                                    }
                                    Ok(CompressionAlgorithm::CompressionSnappy) => {
                                        crate::core::compression::CompressionAlgorithm::Snappy
                                    }
                                    Ok(CompressionAlgorithm::CompressionGzip) => {
                                        crate::core::compression::CompressionAlgorithm::Gzip
                                    }
                                    Ok(CompressionAlgorithm::CompressionBrotli) => {
                                        crate::core::compression::CompressionAlgorithm::Brotli
                                    }
                                    _ => {
                                        debug!("   ⚠️ Unknown compression algorithm, using Zstd");
                                        crate::core::compression::CompressionAlgorithm::Zstd
                                    }
                                }
                            } else {
                                crate::core::compression::CompressionAlgorithm::Zstd
                            }
                        } else {
                            crate::core::compression::CompressionAlgorithm::Zstd
                        }
                    } else {
                        crate::core::compression::CompressionAlgorithm::Zstd
                    }
                } else {
                    crate::core::compression::CompressionAlgorithm::Zstd
                };

                // Check if quantization is enabled
                let has_quantization = collection_config
                    .as_ref()
                    .and_then(|c| c.config.as_ref())
                    .and_then(|cfg| cfg.quantization.as_ref())
                    .map(|q| q.enabled)
                    .unwrap_or(false);

                // Configure ParquetWriterConfig for compaction
                let writer_config = ParquetWriterConfig {
                    row_group_size: 100000,     // Larger row groups for compacted files
                    enable_bloom_filters: true, // Essential for efficient ID lookups
                    bloom_filter_fpp: 0.01,
                    expected_ndv: Some(file_record_count),
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
                    quantization: ColumnarQuantizationConfig {
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
                        pq_codebooks: 0,
                        binary_threshold: 0.5,
                        int8_threshold: 0.3,
                        pq_threshold: 0.1,
                    },
                    id_less_storage: false, // Keep IDs for customer APIs
                    write_batch_size: 10000,
                    page_size: 1024 * 1024, // 1MB pages
                    enable_byte_stream_split: !has_quantization,
                    enable_pq_sorting: true, // Enable sorting for better compression in compaction
                    pq_sorting_segments: 16,
                    pq_sorting_codebook_size: 256,
                    enable_native_metadata: true,
                    metadata_inference_samples: 100,
                };

                // Get dimension from collection metadata config - REQUIRED
                let dimension = collection_config.as_ref()
                    .and_then(|c| c.config.as_ref())
                    .map(|cfg| cfg.dimension)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Collection dimension not found in config for compaction. \
                            Collection config must include dimension to ensure correct vector processing."
                        )
                    })?;

                // Create StreamingParquetWriter with temp file
                let mut writer = StreamingParquetWriter::new(
                    &temp_file_path,
                    dimension as usize,
                    writer_config,
                )?;

                // Convert RecordData to VectorRecord for writing
                let vector_records: Vec<VectorRecord> = file_records
                    .iter()
                    .filter_map(|record| {
                        // Extract vector from row_data
                        let vector = record
                            .row_data
                            .get("vector")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_f64().map(|f| f as f32))
                                    .collect::<Vec<f32>>()
                            })
                            .unwrap_or_default();

                        // Convert metadata to HashMap<String, SqlValue>
                        let metadata: std::collections::HashMap<
                            String,
                            crate::storage::engines::impls::viper::FilterValue,
                        > = record
                            .row_data
                            .iter()
                            .filter(|(k, _)| *k != "vector")
                            .map(|(k, v)| {
                                (
                                    k.clone(),
                                    crate::storage::engines::impls::viper::FilterValue::Equals(
                                        serde_json::Value::String(v.to_string()),
                                    ),
                                )
                            })
                            .collect();

                        Some(VectorRecord {
                            id: record.id.clone().unwrap_or_default(),
                            vector,
                            metadata,
                            version: record.version,
                            updated_at: record.timestamp.map(|t| t as i64),
                            expires_at: None,
                            quantized_vector: vec![],
                            source: None,
                            timestamp: record.timestamp.unwrap_or(0),
                        })
                    })
                    .collect();

                // Write records using StreamingParquetWriter's batch method
                // This provides bloom filters and optimized columnar layout
                writer.write_batch(&vector_records).await?;

                // Finalize the writer to flush all data and get statistics
                let stats = writer.finalize().await?;

                debug!(
                    "   ✅ VIPER Compaction: Wrote file {} with StreamingParquetWriter",
                    file_idx
                );
                debug!("      Records: {}", stats.total_records);
                debug!("      Row groups: {}", stats.total_row_groups);
                debug!("      File size: {} bytes", stats.file_size);
                debug!("      Compression ratio: {:.2}", stats.compression_ratio);
                debug!("      Bloom filters: {}", stats.bloom_filter_count);
            }

            // Read the temporary file into a buffer for atomic write
            // TODO: Refactor write_parquet_atomic to accept file path directly
            let parquet_data = std::fs::read(&temp_file_path)
                .context("Failed to read temporary compacted Parquet file")?;

            // Clean up temporary file after reading
            if let Err(e) = std::fs::remove_file(&temp_file_path) {
                debug!("Failed to remove temporary file: {}", e);
            }

            // Generate output filename using unified compactor format
            // Determine target level based on source level
            use crate::storage::common::compaction_orchestrator::FilenameCodec;
            let codec = FilenameCodec::new();

            // If compacting L0 files, output goes to L1
            // If compacting L1 files, output goes to L2, etc.
            let source_level = input_files
                .iter()
                .filter_map(|f| {
                    // Extract filename from full path
                    f.rsplit('/')
                        .next()
                        .map(|filename| codec.parse_level(filename))
                })
                .max();
            let target_level = (source_level.unwrap_or(0) + 1).min(7); // Max level is typically 7

            let output_filename = codec.generate(target_level, "parquet");

            info!(
                "📊 Compacting from L{} to L{}",
                source_level.unwrap_or(0),
                target_level
            );

            info!(
                "📝 Writing compacted file {} to staging: {} ({:.2}MB)",
                file_idx + 1,
                output_filename,
                parquet_data.len() as f64 / (1024.0 * 1024.0)
            );

            // Write to staging
            self.atomic_coordinator
                .write_to_staging(&atomic_op.operation_id, &output_filename, &parquet_data)
                .await
                .context(format!(
                    "Failed to write compacted file {} to staging",
                    file_idx
                ))?;

            let final_path = format!("{}/{}", base_storage_url, output_filename);
            output_files.push(final_path.clone());
            total_bytes_written += parquet_data.len() as u64;
        }

        // Finalize atomic operation - this will atomically move all files to final location
        debug!(
            "🔄 [DEBUG] About to finalize atomic operation: {}",
            atomic_op.operation_id
        );
        self.atomic_coordinator
            .finalize_atomic_operation(&atomic_op.operation_id)
            .await
            .context("Failed to finalize atomic compaction_info")?;
        info!("✅ [DEBUG] Atomic operation finalized successfully");

        // Add a small delay to ensure filesystem operations complete
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Verify all compacted files exist before deleting input files
        info!(
            "🔍 Verifying {} compacted files exist before cleanup",
            output_files.len()
        );
        debug!(
            "🔍 [DEBUG] Verifying {} compacted files exist",
            output_files.len()
        );

        for (idx, output_file) in output_files.iter().enumerate() {
            debug!("    [DEBUG] Checking file {}: {}", idx, output_file);
            let exists = fs.exists(output_file).await?;
            debug!("    [DEBUG] File exists: {}", exists);
            if !exists {
                debug!("    ❌ [DEBUG] File verification failed!");
                return Err(anyhow::anyhow!(
                    "Compacted file {} ({}/{}) does not exist after atomic operation - aborting input file deletion for safety",
                    output_file,
                    idx + 1,
                    output_files.len()
                ));
            }
        }

        info!(
            "✅ All {} output files verified successfully",
            output_files.len()
        );

        // Remove input files that were compacted (cleanup)
        // This happens AFTER the atomic operation to ensure data safety
        // In case of failure during deletion, we'll have duplicate data rather than data loss
        info!(
            "🧹 Removing {} input files after successful compaction_info",
            input_files.len()
        );
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
            info!(
                "🧹 VIPER COMPACTION CLEANUP: {} expired/tombstoned records physically deleted",
                expired_records_count
            );
        }

        // Log compaction results summary
        info!("📊 COMPACTION RESULTS:");
        info!(
            "  - Input files: {} ({:.2}MB total)",
            compaction_plan.input_files.len(),
            compaction_plan.total_size_bytes as f64 / (1024.0 * 1024.0)
        );
        info!(
            "  - Output files: {} ({:.2}MB total)",
            output_files.len(),
            total_bytes_written as f64 / (1024.0 * 1024.0)
        );
        info!("  - Records processed: {}", total_records_processed);
        info!("  - Records written: {}", total_records);
        info!("  - Records expired/removed: {}", expired_records_count);
        info!(
            "  - Average output file size: {:.2}MB",
            (total_bytes_written as f64 / output_files.len() as f64) / (1024.0 * 1024.0)
        );
        info!(
            "  - Input files deleted: {}/{}",
            deleted_count,
            input_files.len()
        );

        info!(
            "✅ [VIPER COMPACTION] Atomic Arrow/Parquet compaction completed for collection {}: {} records merged into {} files, {} expired deleted, {}/{} input files removed",
            collection_id,
            total_records,
            output_files.len(),
            expired_records_count,
            deleted_count,
            input_files.len()
        );

        // Record compaction metrics (non-blocking, failure-tolerant)
        if let Some(ref metrics_updater) = self.metrics_updater {
            let compaction_update = crate::metrics::CompactionMetricsUpdate {
                files_before: input_files.len() as i32,
                files_after: output_files.len() as i32,
                bytes_before: total_bytes_read as i64,
                bytes_after: total_bytes_written as i64,
                duration_ms: 0, // TODO: Track actual duration
                timestamp: chrono::Utc::now().timestamp_millis(),
            };

            // Fire and forget - never block compaction operation
            metrics_updater
                .record_compaction(collection_id, compaction_update)
                .await;
        }

        let result = ViperCompactionResult {
            input_files: discovered_input_files.clone(),
            output_files: output_files.clone(),
            entries_processed: total_records_processed as u64,
            entries_removed: expired_records_count as u64,
            bytes_read: total_bytes_read,
            bytes_written: total_bytes_written,
        };

        // Notify EventLog about successful compaction
        if !output_files.is_empty() {
            let flush_handler =
                crate::storage::engines::impls::viper::eventlog_flush::ViperFlushNotifier::new();
            flush_handler.notify_compaction_complete(
                collection_id,
                output_files.clone(),
                total_records_processed,
            );

            // Clean up old files from EventLog tracking
            if let Err(e) = flush_handler
                .cleanup_compacted_files(collection_id, discovered_input_files.clone())
                .await
            {
                warn!("Failed to cleanup compacted files from EventLog: {}", e);
            }

            info!(
                "📨 Notified EventLog about VIPER compaction completion for collection {}",
                collection_id
            );
        }

        Ok(result)
    }

    /// Extract a value from a column at a specific row index
    fn extract_column_value(
        &self,
        column: &dyn arrow_array::Array,
        row_idx: usize,
        // data_type removed -  &arrow_schema::DataType,
    ) -> Result<serde_json::Value> {
        use arrow_array::{BooleanArray, Float32Array, Int8Array, Int64Array, StringArray};
        use arrow_schema::DataType;

        let column_data_type = column.data_type();
        trace!(
            "extract_column_value: row {}, type {:?}",
            row_idx, column_data_type
        );

        if column.is_null(row_idx) {
            trace!("  -> NULL value");
            return Ok(serde_json::Value::Null);
        }

        match column_data_type {
            DataType::Utf8 => {
                let array = column
                    .as_any()
                    .downcast_ref::<StringArray>()
                    .ok_or_else(|| anyhow::anyhow!("Failed to downcast to StringArray"))?;
                Ok(serde_json::json!(array.value(row_idx)))
            }
            DataType::Int64 => {
                let array = column
                    .as_any()
                    .downcast_ref::<Int64Array>()
                    .ok_or_else(|| anyhow::anyhow!("Failed to downcast to Int64Array"))?;
                Ok(serde_json::json!(array.value(row_idx)))
            }
            DataType::Int8 => {
                let array = column
                    .as_any()
                    .downcast_ref::<Int8Array>()
                    .ok_or_else(|| anyhow::anyhow!("Failed to downcast to Int8Array"))?;
                Ok(serde_json::json!(array.value(row_idx)))
            }
            DataType::Float32 => {
                let array = column
                    .as_any()
                    .downcast_ref::<Float32Array>()
                    .ok_or_else(|| anyhow::anyhow!("Failed to downcast to Float32Array"))?;
                Ok(serde_json::json!(array.value(row_idx)))
            }
            DataType::Boolean => {
                let array = column
                    .as_any()
                    .downcast_ref::<BooleanArray>()
                    .ok_or_else(|| anyhow::anyhow!("Failed to downcast to BooleanArray"))?;
                Ok(serde_json::json!(array.value(row_idx)))
            }
            DataType::List(field) => {
                match field.data_type() {
                    DataType::Float32 => {
                        // Handle vector data
                        trace!("  -> Extracting List<Float32> vector");
                        let list_array = column
                            .as_any()
                            .downcast_ref::<arrow_array::ListArray>()
                            .ok_or_else(|| {
                            anyhow::anyhow!("Failed to downcast to ListArray")
                        })?;
                        trace!("  -> List array ok, getting value at index {}", row_idx);
                        let values = list_array.value(row_idx);
                        let float_array = values
                            .as_any()
                            .downcast_ref::<Float32Array>()
                            .ok_or_else(|| {
                                anyhow::anyhow!("Failed to downcast vector to Float32Array")
                            })?;

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
                        let list_array = column
                            .as_any()
                            .downcast_ref::<arrow_array::ListArray>()
                            .ok_or_else(|| {
                            anyhow::anyhow!("Failed to downcast to ListArray")
                        })?;

                        if list_array.is_null(row_idx) {
                            return Ok(serde_json::json!([]));
                        }

                        let struct_array = list_array.value(row_idx);
                        let struct_array = struct_array
                            .as_any()
                            .downcast_ref::<arrow_array::StructArray>()
                            .ok_or_else(|| anyhow::anyhow!("Failed to downcast to StructArray"))?;

                        let key_array = struct_array
                            .column_by_name("key")
                            .ok_or_else(|| anyhow::anyhow!("Missing 'key' column in struct"))?
                            .as_any()
                            .downcast_ref::<arrow_array::StringArray>()
                            .ok_or_else(|| {
                                anyhow::anyhow!("Failed to downcast key to StringArray")
                            })?;

                        let value_array = struct_array
                            .column_by_name("value")
                            .ok_or_else(|| anyhow::anyhow!("Missing 'value' column in struct"))?
                            .as_any()
                            .downcast_ref::<arrow_array::StringArray>()
                            .ok_or_else(|| {
                                anyhow::anyhow!("Failed to downcast value to StringArray")
                            })?;

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
        use arrow_array::{BooleanArray, Float32Array, Int8Array, Int64Array, StringArray};
        use arrow_schema::DataType;

        let mut column_builders: Vec<(String, Vec<serde_json::Value>)> = Vec::new();

        // Initialize builders for each field in schema
        for field in schema.fields() {
            column_builders.push((field.name().to_string(), Vec::with_capacity(records.len())));
        }

        // Collect data for each column
        for record in records {
            for (field, values) in &mut column_builders {
                let value = record
                    .get(field)
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
                DataType::Int64 => {
                    let int_values: Vec<Option<i64>> = values
                        .iter()
                        .map(|v| if v.is_null() { None } else { v.as_i64() })
                        .collect();
                    Arc::new(Int64Array::from(int_values))
                }
                DataType::Int8 => {
                    let int_values: Vec<Option<i8>> = values
                        .iter()
                        .map(|v| {
                            if v.is_null() {
                                None
                            } else {
                                v.as_i64().map(|i| i as i8)
                            }
                        })
                        .collect();
                    Arc::new(Int8Array::from(int_values))
                }
                DataType::Float32 => {
                    let float_values: Vec<Option<f32>> = values
                        .iter()
                        .map(|v| {
                            if v.is_null() {
                                None
                            } else {
                                v.as_f64().map(|f| f as f32)
                            }
                        })
                        .collect();
                    Arc::new(Float32Array::from(float_values))
                }
                DataType::Boolean => {
                    let bool_values: Vec<Option<bool>> = values
                        .iter()
                        .map(|v| if v.is_null() { None } else { v.as_bool() })
                        .collect();
                    Arc::new(BooleanArray::from(bool_values))
                }
                DataType::List(inner_field) => {
                    match inner_field.data_type() {
                        DataType::Float32 => {
                            // Handle vector data
                            use arrow_array::builder::{Float32Builder, ListBuilder};
                            let mut list_builder = ListBuilder::new(Float32Builder::new());

                            for value in values {
                                if value.is_null() {
                                    list_builder.append(false);
                                } else if let Some(vec_array) = value.as_array() {
                                    let float_values: Vec<f32> = vec_array
                                        .iter()
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
                            use arrow_array::builder::{ListBuilder, StringBuilder, StructBuilder};

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
                                    for value in metadata_array {
                                        if let Some(obj) = value.as_object() {
                                            if let (Some(key), Some(val)) =
                                                (obj.get("key"), obj.get("value"))
                                            {
                                                if let (Some(key_str), Some(val_str)) =
                                                    (key.as_str(), val.as_str())
                                                {
                                                    struct_builder
                                                        .field_builder::<StringBuilder>(0)
                                                        .unwrap()
                                                        .append_value(key_str);
                                                    struct_builder
                                                        .field_builder::<StringBuilder>(1)
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
                            return Err(anyhow::anyhow!(
                                "Unsupported list type: {:?}",
                                inner_field.data_type()
                            ));
                        }
                    }
                }
                _ => {
                    return Err(anyhow::anyhow!(
                        "Unsupported column type: {:?}",
                        field.data_type()
                    ));
                }
            };

            arrays.push(array);
        }

        RecordBatch::try_new(schema, arrays)
            .context("Failed to create RecordBatch from collected data")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compaction_plan_single_output() {
        // Test case where all files fit in one output file
        let input_files = vec![
            FileMetadata {
                path: "file1.parquet".to_string(),
                size_bytes: 10 * 1024 * 1024, // 10MB
                row_count: 1000,
                avg_row_size: 10240.0,
                level: 0,
                timestamp: 1000,
            },
            FileMetadata {
                path: "file2.parquet".to_string(),
                size_bytes: 20 * 1024 * 1024, // 20MB
                row_count: 2000,
                avg_row_size: 10240.0,
                level: 0,
                timestamp: 2000,
            },
        ];

        // Total: 30MB, 3000 rows
        let plan = CompactionPlan {
            input_files: input_files.clone(),
            total_rows: 3000,
            total_size_bytes: 30 * 1024 * 1024,
            avg_row_size: 10240.0,
            target_file_count: 1, // 30MB fits in one 128MB file
            rows_per_file: 3000,
            estimated_size_per_file: 30 * 1024 * 1024,
        };

        assert_eq!(plan.target_file_count, 1);
        assert_eq!(plan.rows_per_file, 3000);
    }

    #[test]
    fn test_compaction_plan_multiple_outputs() {
        // Test case where files need to be split
        let input_files = vec![
            FileMetadata {
                path: "large1.parquet".to_string(),
                size_bytes: 150 * 1024 * 1024, // 150MB
                row_count: 15000,
                avg_row_size: 10240.0,
                level: 1,
                timestamp: 3000,
            },
            FileMetadata {
                path: "large2.parquet".to_string(),
                size_bytes: 150 * 1024 * 1024, // 150MB
                row_count: 15000,
                avg_row_size: 10240.0,
                level: 1,
                timestamp: 4000,
            },
        ];

        // Total: 300MB, 30000 rows
        // With 128MB target: 300/128 = 2.34, rounds up to 3 files
        let plan = CompactionPlan {
            input_files,
            total_rows: 30000,
            total_size_bytes: 300 * 1024 * 1024,
            avg_row_size: 10240.0,
            target_file_count: 3,
            rows_per_file: 10000, // 30000/3
            estimated_size_per_file: 100 * 1024 * 1024,
        };

        assert_eq!(plan.target_file_count, 3);
        assert_eq!(plan.rows_per_file, 10000);
    }
}
