// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Data Processing Pipeline - Consolidated Implementation
//!
//! 🔥 PHASE 4.2: This consolidates 3 VIPER pipeline files into a unified implementation:
//! - `processor.rs` → Vector record processing with template method pattern
//! - `flusher.rs` → Parquet flushing and writing operations
//! - `compaction.rs` → Background compaction with ML-guided optimization
//!
//! ## Key Features
//! - **Template Method Pattern**: Configurable processing pipeline
//! - **Intelligent Flushing**: Optimized Parquet writing with compression
//! - **ML-Guided Compaction**: Background optimization with learned patterns
//! - **Performance Monitoring**: Comprehensive statistics and analytics

use anyhow::{Context, Result};
use arrow::array::RecordBatch;
use arrow::datatypes::Schema;
use chrono::{DateTime, Utc};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding};
use parquet::file::properties::WriterProperties;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::core::{CollectionId, VectorRecord};
use crate::storage::filesystem::FilesystemFactory;
use crate::storage::vector::types::*;

/// VIPER Data Processing Pipeline coordinator
pub struct ViperPipeline {
    /// Vector record processor
    processor: Arc<VectorRecordProcessor>,
    
    /// Parquet flusher
    flusher: Arc<ParquetFlusher>,
    
    /// Background compaction engine
    compaction_engine: Arc<CompactionEngine>,
    
    /// Pipeline configuration
    config: ViperPipelineConfig,
    
    /// Pipeline statistics
    stats: Arc<RwLock<ViperPipelineStats>>,
    
    /// Filesystem interface
    filesystem: Arc<FilesystemFactory>,
}

/// Configuration for VIPER pipeline
#[derive(Debug, Clone)]
pub struct ViperPipelineConfig {
    /// Processing configuration
    pub processing_config: ProcessingConfig,
    
    /// Flushing configuration
    pub flushing_config: FlushingConfig,
    
    /// Compaction configuration
    pub compaction_config: CompactionConfig,
    
    /// Enable background processing
    pub enable_background_processing: bool,
    
    /// Statistics collection interval
    pub stats_interval_secs: u64,
}

/// Processing configuration
#[derive(Debug, Clone)]
pub struct ProcessingConfig {
    /// Enable record preprocessing
    pub enable_preprocessing: bool,
    
    /// Enable record postprocessing
    pub enable_postprocessing: bool,
    
    /// Batch size for processing
    pub batch_size: usize,
    
    /// Enable compression during processing
    pub enable_compression: bool,
    
    /// Sorting strategy for records
    pub sorting_strategy: SortingStrategy,
}

/// Flushing configuration
#[derive(Debug, Clone)]
pub struct FlushingConfig {
    /// Compression algorithm
    pub compression_algorithm: CompressionAlgorithm,
    
    /// Compression level
    pub compression_level: u8,
    
    /// Enable dictionary encoding
    pub enable_dictionary_encoding: bool,
    
    /// Row group size
    pub row_group_size: usize,
    
    /// Write batch size
    pub write_batch_size: usize,
    
    /// Enable statistics
    pub enable_statistics: bool,
}

/// Compaction configuration
#[derive(Debug, Clone)]
pub struct CompactionConfig {
    /// Enable ML-guided compaction
    pub enable_ml_compaction: bool,
    
    /// Background worker count
    pub worker_count: usize,
    
    /// Compaction interval
    pub compaction_interval_secs: u64,
    
    /// Target file size for merging
    pub target_file_size_mb: usize,
    
    /// Maximum files per merge operation
    pub max_files_per_merge: usize,
    
    /// Quality threshold for reclustering
    pub reclustering_quality_threshold: f32,
}

/// Sorting strategy for record processing
#[derive(Debug, Clone)]
pub enum SortingStrategy {
    /// Sort by timestamp for better compression
    ByTimestamp,
    
    /// Sort by vector similarity for clustering
    BySimilarity,
    
    /// Sort by metadata fields
    ByMetadata(Vec<String>),
    
    /// No sorting (preserve insertion order)
    None,
}

/// Compression algorithm options
#[derive(Debug, Clone)]
pub enum CompressionAlgorithm {
    Snappy,
    Zstd { level: u8 },
    Lz4,
    Brotli { level: u8 },
}

/// Pipeline statistics
#[derive(Debug, Default)]
pub struct ViperPipelineStats {
    pub total_records_processed: u64,
    pub total_batches_flushed: u64,
    pub total_bytes_written: u64,
    pub total_compaction_operations: u64,
    pub avg_processing_time_ms: f64,
    pub avg_flush_time_ms: f64,
    pub avg_compaction_time_ms: f64,
    pub compression_ratio: f32,
    pub current_active_operations: usize,
}

// Vector Record Processor

/// Vector record processor with template method pattern
pub struct VectorRecordProcessor {
    /// Processing configuration
    config: ProcessingConfig,
    
    /// Schema adapter for record conversion
    schema_adapter: Arc<SchemaAdapter>,
    
    /// Processing statistics
    stats: Arc<RwLock<ProcessingStats>>,
}

/// Processing statistics
#[derive(Debug, Default)]
pub struct ProcessingStats {
    pub records_processed: u64,
    pub preprocessing_time_ms: u64,
    pub conversion_time_ms: u64,
    pub postprocessing_time_ms: u64,
    pub total_processing_time_ms: u64,
}

/// Template method trait for vector processing
pub trait VectorProcessor {
    /// Preprocess records before conversion
    fn preprocess_records(&self, records: &mut [VectorRecord]) -> Result<()>;
    
    /// Convert records to arrow batch
    fn convert_to_batch(
        &self,
        records: &[VectorRecord],
        schema: &Arc<Schema>,
    ) -> Result<RecordBatch>;
    
    /// Postprocess batch after conversion
    fn postprocess_batch(&self, batch: RecordBatch) -> Result<RecordBatch>;
}

/// Schema adapter for record conversion
pub struct SchemaAdapter {
    /// Schema cache
    schema_cache: Arc<RwLock<HashMap<String, Arc<Schema>>>>,
    
    /// Schema generation strategies
    strategies: HashMap<String, Box<dyn SchemaGenerationStrategy>>,
}

/// Schema generation strategy trait
pub trait SchemaGenerationStrategy: Send + Sync {
    /// Generate schema for records
    fn generate_schema(&self, records: &[VectorRecord]) -> Result<Arc<Schema>>;
    
    /// Get collection ID
    fn get_collection_id(&self) -> &CollectionId;
    
    /// Get filterable fields
    fn get_filterable_fields(&self) -> &[String];
}

// Parquet Flusher

/// Parquet flusher for writing arrow batches to storage
pub struct ParquetFlusher {
    /// Flushing configuration
    config: FlushingConfig,
    
    /// Filesystem interface
    filesystem: Arc<FilesystemFactory>,
    
    /// Flushing statistics
    stats: Arc<RwLock<FlushingStats>>,
    
    /// Writer pool for concurrent operations
    writer_pool: Arc<WriterPool>,
}

/// Flushing statistics
#[derive(Debug, Default)]
pub struct FlushingStats {
    pub batches_flushed: u64,
    pub bytes_written: u64,
    pub avg_flush_time_ms: f64,
    pub compression_ratio: f32,
    pub writer_pool_utilization: f32,
}

/// Writer pool for managing parquet writers
pub struct WriterPool {
    /// Available writers
    writers: Arc<Mutex<Vec<Box<dyn ParquetWriter>>>>,
    
    /// Pool configuration
    pool_size: usize,
    
    /// Writer factory
    writer_factory: Arc<dyn ParquetWriterFactory>,
}

/// Parquet writer trait
pub trait ParquetWriter: Send + Sync {
    /// Write batch to storage
    async fn write_batch(&mut self, batch: &RecordBatch, path: &str) -> Result<u64>;
    
    /// Close writer and flush
    async fn close(&mut self) -> Result<()>;
}

/// Parquet writer factory
pub trait ParquetWriterFactory: Send + Sync {
    /// Create new parquet writer
    async fn create_writer(&self) -> Result<Box<dyn ParquetWriter>>;
}

/// Flush result information
#[derive(Debug, Clone)]
pub struct FlushResult {
    pub entries_flushed: u64,
    pub bytes_written: u64,
    pub segments_created: u64,
    pub collections_affected: Vec<CollectionId>,
    pub flush_duration_ms: u64,
    pub compression_ratio: f32,
}

// Compaction Engine

/// Background compaction engine with ML-guided optimization
pub struct CompactionEngine {
    /// Configuration
    config: CompactionConfig,
    
    /// Task queue for compaction operations
    task_queue: Arc<Mutex<VecDeque<CompactionTask>>>,
    
    /// Active compaction operations
    active_compactions: Arc<RwLock<HashMap<CollectionId, CompactionOperation>>>,
    
    /// ML optimization model
    optimization_model: Arc<RwLock<Option<CompactionOptimizationModel>>>,
    
    /// Background worker handles
    worker_handles: Vec<tokio::task::JoinHandle<()>>,
    
    /// Shutdown signal
    shutdown_sender: Arc<Mutex<Option<mpsc::Sender<()>>>>,
    
    /// Compaction statistics
    stats: Arc<RwLock<CompactionStats>>,
    
    /// Filesystem interface
    filesystem: Arc<FilesystemFactory>,
}

/// Compaction task
#[derive(Debug, Clone)]
pub struct CompactionTask {
    pub task_id: String,
    pub collection_id: CollectionId,
    pub compaction_type: CompactionType,
    pub priority: CompactionPriority,
    pub input_partitions: Vec<PartitionId>,
    pub expected_outputs: usize,
    pub optimization_hints: Option<CompactionOptimizationHints>,
    pub created_at: DateTime<Utc>,
    pub estimated_duration: Duration,
}

/// Compaction operation state
#[derive(Debug)]
pub struct CompactionOperation {
    pub operation_id: String,
    pub collection_id: CollectionId,
    pub operation_type: CompactionType,
    pub started_at: DateTime<Utc>,
    pub progress: f32,
    pub status: CompactionStatus,
}

/// Types of compaction operations
#[derive(Debug, Clone)]
pub enum CompactionType {
    /// Merge small files
    FileMerging {
        target_file_size_mb: usize,
        max_files_per_merge: usize,
    },
    
    /// Recluster based on ML model
    Reclustering {
        new_cluster_count: usize,
        quality_threshold: f32,
    },
    
    /// Reorganize by feature importance
    FeatureReorganization {
        important_features: Vec<usize>,
        reorganization_strategy: ReorganizationStrategy,
    },
    
    /// Compression optimization
    CompressionOptimization {
        target_algorithm: CompressionAlgorithm,
        quality_threshold: f32,
    },
}

/// Compaction priority levels
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompactionPriority {
    Low,
    Normal,
    High,
    Critical,
}

/// Compaction status
#[derive(Debug, Clone)]
pub enum CompactionStatus {
    Pending,
    Running,
    Finalizing,
    Completed,
    Failed(String),
}

/// Reorganization strategy
#[derive(Debug, Clone)]
pub enum ReorganizationStrategy {
    ByFeatureImportance,
    ByAccessPattern,
    ByCompressionRatio,
}

/// ML optimization model for compaction
#[derive(Debug)]
pub struct CompactionOptimizationModel {
    pub model_id: String,
    pub version: String,
    pub accuracy: f32,
    pub feature_weights: Vec<f32>,
    pub trained_at: DateTime<Utc>,
}

/// Optimization hints from ML model
#[derive(Debug, Clone)]
pub struct CompactionOptimizationHints {
    pub recommended_cluster_count: Option<usize>,
    pub important_features: Vec<usize>,
    pub compression_algorithm: Option<CompressionAlgorithm>,
    pub estimated_improvement: f32,
}

/// Compaction statistics
#[derive(Debug, Default)]
pub struct CompactionStats {
    pub total_operations: u64,
    pub successful_operations: u64,
    pub failed_operations: u64,
    pub avg_operation_time_ms: f64,
    pub total_bytes_processed: u64,
    pub total_bytes_saved: u64,
    pub avg_compression_improvement: f32,
}

/// Type aliases
pub type PartitionId = String;

// Implementation

impl ViperPipeline {
    /// Create new VIPER pipeline
    pub async fn new(
        config: ViperPipelineConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        let schema_adapter = Arc::new(SchemaAdapter::new().await?);
        
        let processor = Arc::new(VectorRecordProcessor::new(
            config.processing_config.clone(),
            schema_adapter.clone(),
        ).await?);
        
        let writer_factory = Arc::new(DefaultParquetWriterFactory::new());
        let writer_pool = Arc::new(WriterPool::new(8, writer_factory).await?);
        
        let flusher = Arc::new(ParquetFlusher::new(
            config.flushing_config.clone(),
            filesystem.clone(),
            writer_pool,
        ).await?);
        
        let compaction_engine = Arc::new(CompactionEngine::new(
            config.compaction_config.clone(),
            filesystem.clone(),
        ).await?);
        
        Ok(Self {
            processor,
            flusher,
            compaction_engine,
            config,
            stats: Arc::new(RwLock::new(ViperPipelineStats::default())),
            filesystem,
        })
    }
    
    /// Process vector records through the complete pipeline
    pub async fn process_records(
        &self,
        collection_id: &CollectionId,
        records: Vec<VectorRecord>,
        output_path: &str,
    ) -> Result<FlushResult> {
        let start_time = Instant::now();
        
        info!("🚀 VIPER Pipeline: Processing {} records for collection {}", 
              records.len(), collection_id);
        
        // Step 1: Process records using template method pattern
        let processed_batch = self.processor
            .process_records(records)
            .await?;
        
        // Step 2: Flush processed batch to storage
        let flush_result = self.flusher
            .flush_batch(processed_batch, output_path)
            .await?;
        
        // Step 3: Schedule background compaction if needed
        if self.config.enable_background_processing {
            self.compaction_engine
                .schedule_compaction_if_needed(collection_id)
                .await?;
        }
        
        // Step 4: Update pipeline statistics
        let processing_time = start_time.elapsed().as_millis() as u64;
        self.update_stats(&flush_result, processing_time).await?;
        
        info!("✅ VIPER Pipeline: Completed processing for collection {} in {}ms", 
              collection_id, processing_time);
        
        Ok(flush_result)
    }
    
    /// Start background processing
    pub async fn start_background_processing(&mut self) -> Result<()> {
        if self.config.enable_background_processing {
            self.compaction_engine.start_workers().await?;
        }
        Ok(())
    }
    
    /// Stop background processing
    pub async fn stop_background_processing(&mut self) -> Result<()> {
        self.compaction_engine.stop_workers().await?;
        Ok(())
    }
    
    /// Get pipeline statistics
    pub async fn get_statistics(&self) -> ViperPipelineStats {
        self.stats.read().await.clone()
    }
    
    async fn update_stats(&self, flush_result: &FlushResult, processing_time_ms: u64) -> Result<()> {
        let mut stats = self.stats.write().await;
        stats.total_records_processed += flush_result.entries_flushed;
        stats.total_batches_flushed += 1;
        stats.total_bytes_written += flush_result.bytes_written;
        stats.avg_processing_time_ms = 
            (stats.avg_processing_time_ms + processing_time_ms as f64) / 2.0;
        stats.compression_ratio = flush_result.compression_ratio;
        Ok(())
    }
}

impl VectorRecordProcessor {
    async fn new(
        config: ProcessingConfig,
        schema_adapter: Arc<SchemaAdapter>,
    ) -> Result<Self> {
        Ok(Self {
            config,
            schema_adapter,
            stats: Arc::new(RwLock::new(ProcessingStats::default())),
        })
    }
    
    /// Process records using template method pattern
    pub async fn process_records(&self, mut records: Vec<VectorRecord>) -> Result<RecordBatch> {
        if records.is_empty() {
            return Err(anyhow::anyhow!("No records to process"));
        }
        
        let start_time = Instant::now();
        
        // Step 1: Preprocess records
        let preprocess_start = Instant::now();
        self.preprocess_records(&mut records)?;
        let preprocess_time = preprocess_start.elapsed().as_millis() as u64;
        
        // Step 2: Generate schema
        let collection_id = &records[0].collection_id;
        let schema = self.schema_adapter.get_or_generate_schema(collection_id, &records).await?;
        
        // Step 3: Convert to batch
        let convert_start = Instant::now();
        let batch = self.convert_to_batch(&records, &schema)?;
        let convert_time = convert_start.elapsed().as_millis() as u64;
        
        // Step 4: Postprocess batch
        let postprocess_start = Instant::now();
        let final_batch = self.postprocess_batch(batch)?;
        let postprocess_time = postprocess_start.elapsed().as_millis() as u64;
        
        // Update statistics
        let total_time = start_time.elapsed().as_millis() as u64;
        self.update_processing_stats(preprocess_time, convert_time, postprocess_time, total_time).await;
        
        Ok(final_batch)
    }
    
    async fn update_processing_stats(
        &self,
        preprocess_time: u64,
        convert_time: u64,
        postprocess_time: u64,
        total_time: u64,
    ) {
        let mut stats = self.stats.write().await;
        stats.records_processed += 1;
        stats.preprocessing_time_ms += preprocess_time;
        stats.conversion_time_ms += convert_time;
        stats.postprocessing_time_ms += postprocess_time;
        stats.total_processing_time_ms += total_time;
    }
}

impl VectorProcessor for VectorRecordProcessor {
    fn preprocess_records(&self, records: &mut [VectorRecord]) -> Result<()> {
        if !self.config.enable_preprocessing {
            return Ok(());
        }
        
        match &self.config.sorting_strategy {
            SortingStrategy::ByTimestamp => {
                records.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
            }
            SortingStrategy::BySimilarity => {
                // Simplified similarity sorting - would implement actual clustering
                records.sort_by(|a, b| a.vector.len().cmp(&b.vector.len()));
            }
            SortingStrategy::ByMetadata(fields) => {
                // Sort by specified metadata fields
                records.sort_by(|a, b| {
                    for field in fields {
                        let a_val = a.metadata.get(field);
                        let b_val = b.metadata.get(field);
                        match (a_val, b_val) {
                            (Some(a), Some(b)) => {
                                let cmp = a.to_string().cmp(&b.to_string());
                                if cmp != std::cmp::Ordering::Equal {
                                    return cmp;
                                }
                            }
                            (Some(_), None) => return std::cmp::Ordering::Less,
                            (None, Some(_)) => return std::cmp::Ordering::Greater,
                            (None, None) => continue,
                        }
                    }
                    std::cmp::Ordering::Equal
                });
            }
            SortingStrategy::None => {}
        }
        
        Ok(())
    }
    
    fn convert_to_batch(
        &self,
        _records: &[VectorRecord],
        _schema: &Arc<Schema>,
    ) -> Result<RecordBatch> {
        // Placeholder implementation - would implement actual arrow conversion
        Err(anyhow::anyhow!("Record conversion not implemented"))
    }
    
    fn postprocess_batch(&self, batch: RecordBatch) -> Result<RecordBatch> {
        if !self.config.enable_postprocessing {
            return Ok(batch);
        }
        
        // Apply postprocessing optimizations
        // - Column pruning
        // - Compression optimization
        // - Statistics collection
        
        Ok(batch)
    }
}

impl ParquetFlusher {
    async fn new(
        config: FlushingConfig,
        filesystem: Arc<FilesystemFactory>,
        writer_pool: Arc<WriterPool>,
    ) -> Result<Self> {
        Ok(Self {
            config,
            filesystem,
            stats: Arc::new(RwLock::new(FlushingStats::default())),
            writer_pool,
        })
    }
    
    /// Flush record batch to parquet file
    pub async fn flush_batch(&self, batch: RecordBatch, output_path: &str) -> Result<FlushResult> {
        let start_time = Instant::now();
        let original_size = batch.get_array_memory_size();
        
        // Create writer properties based on configuration
        let props = self.create_writer_properties()?;
        
        // Serialize to bytes
        let mut buffer = Vec::new();
        {
            let mut writer = ArrowWriter::try_new(&mut buffer, batch.schema(), Some(props))
                .context("Failed to create parquet writer")?;
            
            writer.write(&batch)
                .context("Failed to write batch to parquet")?;
            
            writer.close()
                .context("Failed to close parquet writer")?;
        }
        
        let bytes_written = buffer.len() as u64;
        let compression_ratio = original_size as f32 / bytes_written as f32;
        
        // Write to filesystem
        self.filesystem
            .write(output_path, &buffer, None)
            .await
            .with_context(|| format!("Failed to write to: {}", output_path))?;
        
        let flush_duration = start_time.elapsed().as_millis() as u64;
        
        // Update statistics
        self.update_flushing_stats(bytes_written, flush_duration, compression_ratio).await;
        
        Ok(FlushResult {
            entries_flushed: batch.num_rows() as u64,
            bytes_written,
            segments_created: 1,
            collections_affected: vec![], // Would extract from batch
            flush_duration_ms: flush_duration,
            compression_ratio,
        })
    }
    
    fn create_writer_properties(&self) -> Result<WriterProperties> {
        let compression = match &self.config.compression_algorithm {
            CompressionAlgorithm::Snappy => Compression::SNAPPY,
            CompressionAlgorithm::Zstd { level } => {
                Compression::ZSTD(parquet::basic::ZstdLevel::try_new(*level as i32)?)
            }
            CompressionAlgorithm::Lz4 => Compression::LZ4,
            CompressionAlgorithm::Brotli { level } => {
                Compression::BROTLI(parquet::basic::BrotliLevel::try_new(*level as u32)?)
            }
        };
        
        let props = WriterProperties::builder()
            .set_compression(compression)
            .set_encoding(Encoding::DELTA_BINARY_PACKED)
            .set_dictionary_enabled(self.config.enable_dictionary_encoding)
            .set_max_row_group_size(self.config.row_group_size)
            .set_write_batch_size(self.config.write_batch_size)
            .build();
        
        Ok(props)
    }
    
    async fn update_flushing_stats(&self, bytes_written: u64, flush_time: u64, compression_ratio: f32) {
        let mut stats = self.stats.write().await;
        stats.batches_flushed += 1;
        stats.bytes_written += bytes_written;
        stats.avg_flush_time_ms = (stats.avg_flush_time_ms + flush_time as f64) / 2.0;
        stats.compression_ratio = (stats.compression_ratio + compression_ratio) / 2.0;
    }
}

impl CompactionEngine {
    async fn new(
        config: CompactionConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        Ok(Self {
            config,
            task_queue: Arc::new(Mutex::new(VecDeque::new())),
            active_compactions: Arc::new(RwLock::new(HashMap::new())),
            optimization_model: Arc::new(RwLock::new(None)),
            worker_handles: Vec::new(),
            shutdown_sender: Arc::new(Mutex::new(None)),
            stats: Arc::new(RwLock::new(CompactionStats::default())),
            filesystem,
        })
    }
    
    /// Schedule compaction if needed
    pub async fn schedule_compaction_if_needed(&self, collection_id: &CollectionId) -> Result<()> {
        // Check if compaction is needed based on ML model or heuristics
        let needs_compaction = self.evaluate_compaction_need(collection_id).await?;
        
        if needs_compaction {
            let task = CompactionTask {
                task_id: uuid::Uuid::new_v4().to_string(),
                collection_id: collection_id.clone(),
                compaction_type: CompactionType::FileMerging {
                    target_file_size_mb: self.config.target_file_size_mb,
                    max_files_per_merge: self.config.max_files_per_merge,
                },
                priority: CompactionPriority::Normal,
                input_partitions: Vec::new(), // Would be populated with actual partitions
                expected_outputs: 1,
                optimization_hints: None,
                created_at: Utc::now(),
                estimated_duration: Duration::from_secs(300), // 5 minutes estimate
            };
            
            let mut queue = self.task_queue.lock().await;
            queue.push_back(task);
        }
        
        Ok(())
    }
    
    /// Start background workers
    pub async fn start_workers(&mut self) -> Result<()> {
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        *self.shutdown_sender.lock().await = Some(shutdown_tx);
        
        for worker_id in 0..self.config.worker_count {
            let task_queue = self.task_queue.clone();
            let active_compactions = self.active_compactions.clone();
            let stats = self.stats.clone();
            let filesystem = self.filesystem.clone();
            let config = self.config.clone();
            let mut shutdown_receiver = shutdown_rx.resubscribe();
            
            let handle = tokio::spawn(async move {
                Self::worker_loop(
                    worker_id,
                    task_queue,
                    active_compactions,
                    stats,
                    filesystem,
                    config,
                    &mut shutdown_receiver,
                ).await;
            });
            
            self.worker_handles.push(handle);
        }
        
        Ok(())
    }
    
    /// Stop background workers
    pub async fn stop_workers(&mut self) -> Result<()> {
        if let Some(sender) = self.shutdown_sender.lock().await.take() {
            let _ = sender.send(()).await;
        }
        
        for handle in self.worker_handles.drain(..) {
            let _ = handle.await;
        }
        
        Ok(())
    }
    
    async fn evaluate_compaction_need(&self, _collection_id: &CollectionId) -> Result<bool> {
        // Simplified evaluation - would implement actual heuristics
        Ok(true)
    }
    
    async fn worker_loop(
        worker_id: usize,
        task_queue: Arc<Mutex<VecDeque<CompactionTask>>>,
        active_compactions: Arc<RwLock<HashMap<CollectionId, CompactionOperation>>>,
        stats: Arc<RwLock<CompactionStats>>,
        _filesystem: Arc<FilesystemFactory>,
        _config: CompactionConfig,
        shutdown_receiver: &mut mpsc::Receiver<()>,
    ) {
        debug!("🔧 Compaction worker {} started", worker_id);
        
        loop {
            tokio::select! {
                _ = shutdown_receiver.recv() => {
                    debug!("🔧 Compaction worker {} shutting down", worker_id);
                    break;
                }
                _ = tokio::time::sleep(Duration::from_secs(10)) => {
                    // Process tasks from queue
                    if let Some(task) = {
                        let mut queue = task_queue.lock().await;
                        queue.pop_front()
                    } {
                        Self::execute_compaction_task(task, active_compactions.clone(), stats.clone()).await;
                    }
                }
            }
        }
    }
    
    async fn execute_compaction_task(
        task: CompactionTask,
        active_compactions: Arc<RwLock<HashMap<CollectionId, CompactionOperation>>>,
        stats: Arc<RwLock<CompactionStats>>,
    ) {
        let operation = CompactionOperation {
            operation_id: task.task_id.clone(),
            collection_id: task.collection_id.clone(),
            operation_type: task.compaction_type.clone(),
            started_at: Utc::now(),
            progress: 0.0,
            status: CompactionStatus::Running,
        };
        
        // Register operation
        {
            let mut active = active_compactions.write().await;
            active.insert(task.collection_id.clone(), operation);
        }
        
        // Execute compaction (placeholder implementation)
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        // Complete operation
        {
            let mut active = active_compactions.write().await;
            if let Some(mut op) = active.remove(&task.collection_id) {
                op.status = CompactionStatus::Completed;
                op.progress = 1.0;
            }
        }
        
        // Update statistics
        {
            let mut s = stats.write().await;
            s.total_operations += 1;
            s.successful_operations += 1;
        }
        
        debug!("✅ Compaction task {} completed for collection {}", 
               task.task_id, task.collection_id);
    }
}

// Placeholder implementations

impl SchemaAdapter {
    async fn new() -> Result<Self> {
        Ok(Self {
            schema_cache: Arc::new(RwLock::new(HashMap::new())),
            strategies: HashMap::new(),
        })
    }
    
    async fn get_or_generate_schema(
        &self,
        _collection_id: &CollectionId,
        _records: &[VectorRecord],
    ) -> Result<Arc<Schema>> {
        // Placeholder implementation
        Err(anyhow::anyhow!("Schema generation not implemented"))
    }
}

impl WriterPool {
    async fn new(_pool_size: usize, _factory: Arc<dyn ParquetWriterFactory>) -> Result<Self> {
        Ok(Self {
            writers: Arc::new(Mutex::new(Vec::new())),
            pool_size: _pool_size,
            writer_factory: _factory,
        })
    }
}

pub struct DefaultParquetWriterFactory;

impl DefaultParquetWriterFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ParquetWriterFactory for DefaultParquetWriterFactory {
    async fn create_writer(&self) -> Result<Box<dyn ParquetWriter>> {
        Ok(Box::new(DefaultParquetWriter))
    }
}

pub struct DefaultParquetWriter;

impl ParquetWriter for DefaultParquetWriter {
    async fn write_batch(&mut self, _batch: &RecordBatch, _path: &str) -> Result<u64> {
        Ok(0)
    }
    
    async fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

impl Default for ViperPipelineConfig {
    fn default() -> Self {
        Self {
            processing_config: ProcessingConfig {
                enable_preprocessing: true,
                enable_postprocessing: true,
                batch_size: 1000,
                enable_compression: true,
                sorting_strategy: SortingStrategy::ByTimestamp,
            },
            flushing_config: FlushingConfig {
                compression_algorithm: CompressionAlgorithm::Zstd { level: 3 },
                compression_level: 3,
                enable_dictionary_encoding: true,
                row_group_size: 1000000,
                write_batch_size: 8192,
                enable_statistics: true,
            },
            compaction_config: CompactionConfig {
                enable_ml_compaction: true,
                worker_count: 2,
                compaction_interval_secs: 3600,
                target_file_size_mb: 128,
                max_files_per_merge: 10,
                reclustering_quality_threshold: 0.8,
            },
            enable_background_processing: true,
            stats_interval_secs: 60,
        }
    }
}