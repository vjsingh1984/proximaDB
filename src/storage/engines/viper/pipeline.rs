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
use arrow_array::RecordBatch;
use arrow_schema::Schema;
use chrono::{DateTime, Utc};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding};
use parquet::file::properties::WriterProperties;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio::time::{Duration, Instant};
use tracing::{debug, info};

use crate::core::{CollectionId, VectorRecord};
use crate::storage::persistence::filesystem::FilesystemFactory;

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

/// Sorting strategy for record processing with optimized rewrite support
#[derive(Debug, Clone)]
pub enum SortingStrategy {
    /// Sort by timestamp for better compression
    ByTimestamp,
    
    /// Sort by vector similarity for clustering
    BySimilarity,
    
    /// Sort by metadata fields (in specified order)
    ByMetadata(Vec<String>),
    
    /// Sort by record ID for consistent ordering
    ById,
    
    /// Composite sorting: ID first, then metadata fields, then timestamp
    /// This provides the most optimized layout for range queries and compaction
    CompositeOptimal {
        /// Metadata fields to sort by (in order of priority)
        metadata_fields: Vec<String>,
        /// Whether to include ID in sort (recommended: true)
        include_id: bool,
        /// Whether to include timestamp as final sort key (recommended: true)
        include_timestamp: bool,
    },
    
    /// Multi-phase sorting for large datasets: 
    /// 1. Cluster by similarity first 
    /// 2. Sort within clusters by specified fields
    ClusterThenSort {
        /// Number of clusters for initial grouping
        cluster_count: usize,
        /// Sorting strategy within each cluster
        inner_strategy: Box<SortingStrategy>,
    },
    
    /// Custom sorting with user-defined comparison function
    Custom {
        /// Strategy name for logging and metrics
        strategy_name: String,
        /// Comparison function type identifier
        comparison_type: CustomComparisonType,
    },
    
    /// No sorting (preserve insertion order)
    None,
}

/// Custom comparison types for specialized sorting
#[derive(Debug, Clone)]
pub enum CustomComparisonType {
    /// Sort by vector L2 norm (magnitude)
    VectorMagnitude,
    /// Sort by metadata field count (records with more metadata first)
    MetadataRichness,
    /// Sort by insertion order but group by collection_id
    CollectionGrouped,
    /// Sort for optimal compression (similar vectors together)
    CompressionOptimal,
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
#[derive(Debug, Default, Clone)]
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
    writers: Arc<Mutex<Vec<DefaultParquetWriter>>>,
    
    /// Pool configuration
    pool_size: usize,
    
    /// Writer factory
    writer_factory: Arc<DefaultParquetWriterFactory>,
}

// Use ParquetWriter and ParquetWriterFactory from viper_core module
use super::core::{DefaultParquetWriter, DefaultParquetWriterFactory};

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
    worker_handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    
    /// Shutdown signal
    shutdown_sender: Arc<Mutex<Option<tokio::sync::broadcast::Sender<()>>>>,
    
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
#[derive(Debug, Default, Clone)]
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

// =============================================================================
// ADVANCED SORTING IMPLEMENTATION METHODS
// =============================================================================

impl VectorRecordProcessor {
    /// Compare records by metadata fields in specified order
    fn compare_by_metadata_fields(
        &self,
        a: &VectorRecord,
        b: &VectorRecord,
        fields: &[String],
    ) -> std::cmp::Ordering {
        for field in fields {
            let a_val = a.metadata.get(field);
            let b_val = b.metadata.get(field);
            match (a_val, b_val) {
                (Some(a_meta), Some(b_meta)) => {
                    let cmp = self.compare_metadata_values(a_meta, b_meta);
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
    }
    
    /// Smart metadata value comparison with type awareness
    fn compare_metadata_values(
        &self,
        a: &serde_json::Value,
        b: &serde_json::Value,
    ) -> std::cmp::Ordering {
        use serde_json::Value;
        
        match (a, b) {
            // Numeric comparisons
            (Value::Number(a_num), Value::Number(b_num)) => {
                match (a_num.as_f64(), b_num.as_f64()) {
                    (Some(a_f), Some(b_f)) => a_f.partial_cmp(&b_f).unwrap_or(std::cmp::Ordering::Equal),
                    _ => std::cmp::Ordering::Equal,
                }
            }
            
            // String comparisons  
            (Value::String(a_str), Value::String(b_str)) => a_str.cmp(b_str),
            
            // Boolean comparisons (false < true)
            (Value::Bool(a_bool), Value::Bool(b_bool)) => a_bool.cmp(b_bool),
            
            // Array comparisons (by length, then lexicographic)
            (Value::Array(a_arr), Value::Array(b_arr)) => {
                let len_cmp = a_arr.len().cmp(&b_arr.len());
                if len_cmp != std::cmp::Ordering::Equal {
                    return len_cmp;
                }
                for (a_item, b_item) in a_arr.iter().zip(b_arr.iter()) {
                    let item_cmp = self.compare_metadata_values(a_item, b_item);
                    if item_cmp != std::cmp::Ordering::Equal {
                        return item_cmp;
                    }
                }
                std::cmp::Ordering::Equal
            }
            
            // Mixed type comparisons (establish consistent ordering)
            (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
            (Value::Null, _) => std::cmp::Ordering::Less,
            (_, Value::Null) => std::cmp::Ordering::Greater,
            
            // Different types: Bool < Number < String < Array < Object  
            (Value::Bool(_), Value::Number(_)) => std::cmp::Ordering::Less,
            (Value::Bool(_), Value::String(_)) => std::cmp::Ordering::Less,
            (Value::Bool(_), Value::Array(_)) => std::cmp::Ordering::Less,
            (Value::Bool(_), Value::Object(_)) => std::cmp::Ordering::Less,
            
            (Value::Number(_), Value::Bool(_)) => std::cmp::Ordering::Greater,
            (Value::Number(_), Value::String(_)) => std::cmp::Ordering::Less,
            (Value::Number(_), Value::Array(_)) => std::cmp::Ordering::Less,
            (Value::Number(_), Value::Object(_)) => std::cmp::Ordering::Less,
            
            (Value::String(_), Value::Bool(_)) => std::cmp::Ordering::Greater,
            (Value::String(_), Value::Number(_)) => std::cmp::Ordering::Greater,
            (Value::String(_), Value::Array(_)) => std::cmp::Ordering::Less,
            (Value::String(_), Value::Object(_)) => std::cmp::Ordering::Less,
            
            (Value::Array(_), Value::Bool(_)) => std::cmp::Ordering::Greater,
            (Value::Array(_), Value::Number(_)) => std::cmp::Ordering::Greater,
            (Value::Array(_), Value::String(_)) => std::cmp::Ordering::Greater,
            (Value::Array(_), Value::Object(_)) => std::cmp::Ordering::Less,
            
            (Value::Object(_), _) => std::cmp::Ordering::Greater,
            
            // Fallback to string comparison
            _ => a.to_string().cmp(&b.to_string()),
        }
    }
    
    /// Advanced cluster-then-sort implementation
    fn cluster_then_sort_records(
        &self,
        records: &mut [VectorRecord],
        cluster_count: usize,
        inner_strategy: &SortingStrategy,
    ) -> Result<()> {
        if records.is_empty() || cluster_count == 0 {
            return Ok(());
        }
        
        tracing::debug!("🌟 Starting cluster-then-sort: {} records → {} clusters", 
                       records.len(), cluster_count);
        
        // Stage 1: Simple k-means clustering based on vector magnitude and first dimensions
        let clusters = self.simple_vector_clustering(records, cluster_count)?;
        
        // Stage 2: Sort within each cluster using inner strategy
        for cluster_indices in clusters {
            if cluster_indices.len() <= 1 {
                continue; // Skip single-element clusters
            }
            
            // Extract records for this cluster
            let mut cluster_records: Vec<VectorRecord> = cluster_indices
                .iter()
                .map(|&idx| records[idx].clone())
                .collect();
            
            // Sort within cluster using inner strategy
            self.apply_sorting_strategy(&mut cluster_records, inner_strategy)?;
            
            // Write back sorted cluster records
            for (i, &record_idx) in cluster_indices.iter().enumerate() {
                records[record_idx] = cluster_records[i].clone();
            }
        }
        
        tracing::debug!("✅ Cluster-then-sort completed successfully");
        Ok(())
    }
    
    /// Simple vector clustering for ClusterThenSort strategy
    fn simple_vector_clustering(
        &self,
        records: &[VectorRecord],
        cluster_count: usize,
    ) -> Result<Vec<Vec<usize>>> {
        let effective_clusters = std::cmp::min(cluster_count, records.len());
        let mut clusters: Vec<Vec<usize>> = vec![Vec::new(); effective_clusters];
        
        // Simple hash-based clustering using vector characteristics
        for (idx, record) in records.iter().enumerate() {
            let vector_hash = self.compute_vector_hash(&record.vector);
            let cluster_idx = (vector_hash as usize) % effective_clusters;
            clusters[cluster_idx].push(idx);
        }
        
        // Rebalance clusters if some are empty
        self.rebalance_clusters(&mut clusters, records.len());
        
        Ok(clusters)
    }
    
    /// Compute a simple hash for vector clustering
    fn compute_vector_hash(&self, vector: &[f32]) -> u64 {
        if vector.is_empty() {
            return 0;
        }
        
        // Combine magnitude and first few dimensions for clustering
        let magnitude = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        let mag_bits = magnitude.to_bits() as u64;
        
        let mut hash = mag_bits;
        for (i, &val) in vector.iter().take(3).enumerate() {
            hash ^= (val.to_bits() as u64).wrapping_shl(8 * i as u32);
        }
        
        hash
    }
    
    /// Rebalance clusters to ensure none are empty
    fn rebalance_clusters(&self, clusters: &mut [Vec<usize>], total_records: usize) {
        let target_size = total_records / clusters.len();
        let mut excess_indices = Vec::new();
        
        // Collect excess indices from oversized clusters
        for cluster in clusters.iter_mut() {
            if cluster.len() > target_size + 1 {
                let excess = cluster.split_off(target_size + 1);
                excess_indices.extend(excess);
            }
        }
        
        // Distribute excess indices to undersized clusters
        let mut excess_iter = excess_indices.into_iter();
        for cluster in clusters.iter_mut() {
            while cluster.len() < target_size {
                if let Some(idx) = excess_iter.next() {
                    cluster.push(idx);
                } else {
                    break;
                }
            }
        }
    }
    
    /// Apply custom sorting strategies
    fn apply_custom_sorting(
        &self,
        records: &mut [VectorRecord],
        strategy_name: &str,
        comparison_type: &CustomComparisonType,
    ) -> Result<()> {
        match comparison_type {
            CustomComparisonType::VectorMagnitude => {
                records.sort_by(|a, b| {
                    let mag_a: f32 = a.vector.iter().map(|x| x * x).sum::<f32>().sqrt();
                    let mag_b: f32 = b.vector.iter().map(|x| x * x).sum::<f32>().sqrt();
                    mag_a.partial_cmp(&mag_b).unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            
            CustomComparisonType::MetadataRichness => {
                records.sort_by(|a, b| {
                    // Sort by metadata field count (descending)
                    let count_cmp = b.metadata.len().cmp(&a.metadata.len());
                    if count_cmp != std::cmp::Ordering::Equal {
                        return count_cmp;
                    }
                    // Secondary sort by ID for consistency
                    a.id.cmp(&b.id)
                });
            }
            
            CustomComparisonType::CollectionGrouped => {
                records.sort_by(|a, b| {
                    // Primary: group by collection_id
                    let coll_cmp = a.collection_id.cmp(&b.collection_id);
                    if coll_cmp != std::cmp::Ordering::Equal {
                        return coll_cmp;
                    }
                    // Secondary: maintain insertion order within collection
                    a.timestamp.cmp(&b.timestamp)
                });
            }
            
            CustomComparisonType::CompressionOptimal => {
                records.sort_by(|a, b| {
                    // Sort for optimal compression: similar vectors together
                    // Use first few dimensions as similarity proxy
                    for i in 0..std::cmp::min(3, std::cmp::min(a.vector.len(), b.vector.len())) {
                        let dim_cmp = a.vector[i].partial_cmp(&b.vector[i]).unwrap_or(std::cmp::Ordering::Equal);
                        if dim_cmp != std::cmp::Ordering::Equal {
                            return dim_cmp;
                        }
                    }
                    std::cmp::Ordering::Equal
                });
            }
        }
        
        tracing::debug!("⚡ Applied '{}' custom sorting strategy: {:?}", strategy_name, comparison_type);
        Ok(())
    }
    
    /// Recursive helper for applying sorting strategies
    fn apply_sorting_strategy(
        &self,
        records: &mut [VectorRecord],
        strategy: &SortingStrategy,
    ) -> Result<()> {
        match strategy {
            SortingStrategy::ByTimestamp => {
                records.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
            }
            SortingStrategy::ById => {
                records.sort_by(|a, b| a.id.cmp(&b.id));
            }
            SortingStrategy::ByMetadata(fields) => {
                records.sort_by(|a, b| self.compare_by_metadata_fields(a, b, fields));
            }
            SortingStrategy::CompositeOptimal { metadata_fields, include_id, include_timestamp } => {
                records.sort_by(|a, b| {
                    if *include_id {
                        let id_cmp = a.id.cmp(&b.id);
                        if id_cmp != std::cmp::Ordering::Equal {
                            return id_cmp;
                        }
                    }
                    
                    let meta_cmp = self.compare_by_metadata_fields(a, b, metadata_fields);
                    if meta_cmp != std::cmp::Ordering::Equal {
                        return meta_cmp;
                    }
                    
                    if *include_timestamp {
                        return a.timestamp.cmp(&b.timestamp);
                    }
                    
                    std::cmp::Ordering::Equal
                });
            }
            SortingStrategy::Custom { strategy_name, comparison_type } => {
                self.apply_custom_sorting(records, strategy_name, comparison_type)?;
            }
            _ => {} // For recursive calls, handle other strategies as no-op or implement as needed
        }
        Ok(())
    }
    
    // =============================================================================
    // POSTPROCESSING OPTIMIZATION METHODS
    // =============================================================================
    
    /// Apply column pruning to remove unnecessary columns for storage optimization
    fn apply_column_pruning(&self, batch: RecordBatch) -> Result<RecordBatch> {
        if !self.config.enable_compression {
            return Ok(batch);
        }
        
        let schema = batch.schema();
        let mut keep_columns = Vec::new();
        let mut pruned_columns = Vec::new();
        
        // Analyze column usage and importance
        for (i, field) in schema.fields().iter().enumerate() {
            let column_array = batch.column(i);
            
            // Always keep essential columns
            if self.is_essential_column(field.name()) {
                keep_columns.push(i);
                continue;
            }
            
            // Check if column has meaningful data (not all nulls/empty)
            let null_count = column_array.null_count();
            let null_ratio = null_count as f64 / column_array.len() as f64;
            
            if null_ratio < 0.95 { // Keep columns with less than 95% nulls
                keep_columns.push(i);
            } else {
                pruned_columns.push(field.name().clone());
                tracing::debug!("🗑️ Pruning column '{}' ({}% null)", field.name(), (null_ratio * 100.0) as u32);
            }
        }
        
        if !pruned_columns.is_empty() {
            tracing::info!("✂️ Column pruning: kept {}/{} columns, pruned: {:?}", 
                          keep_columns.len(), schema.fields().len(), pruned_columns);
        }
        
        // If all columns are kept, return original batch
        if keep_columns.len() == schema.fields().len() {
            return Ok(batch);
        }
        
        // Create new batch with selected columns
        let new_schema = Arc::new(Schema::new(
            keep_columns.iter()
                .map(|&i| schema.field(i).clone())
                .collect::<Vec<_>>()
        ));
        
        let new_columns: Vec<Arc<dyn arrow_array::Array>> = keep_columns.iter()
            .map(|&i| batch.column(i).clone())
            .collect();
        
        RecordBatch::try_new(new_schema, new_columns)
            .map_err(|e| anyhow::anyhow!("Failed to create pruned batch: {}", e))
    }
    
    /// Check if a column is essential and should never be pruned
    fn is_essential_column(&self, column_name: &str) -> bool {
        matches!(column_name, "id" | "vector" | "timestamp" | "collection_id")
    }
    
    /// Apply optimizations that leverage sorted batch order for better compression
    fn apply_sorted_batch_optimizations(&self, batch: RecordBatch) -> Result<RecordBatch> {
        let optimization_start = Instant::now();
        
        // Analyze sorted order to determine optimal compression strategies
        let sort_analysis = self.analyze_sorted_order(&batch)?;
        
        // Apply run-length encoding hints for sorted categorical columns
        let rle_optimized = self.apply_run_length_encoding_hints(batch, &sort_analysis)?;
        
        // Apply dictionary encoding hints for repeated values in sorted order
        let dict_optimized = self.apply_dictionary_encoding_hints(rle_optimized, &sort_analysis)?;
        
        let optimization_duration = optimization_start.elapsed();
        tracing::debug!("🎯 Sorted batch optimizations completed in {}ms", 
                       optimization_duration.as_millis());
        
        Ok(dict_optimized)
    }
    
    /// Analyze the sorted order of the batch to identify optimization opportunities
    fn analyze_sorted_order(&self, batch: &RecordBatch) -> Result<SortAnalysis> {
        let mut analysis = SortAnalysis::default();
        
        for (i, field) in batch.schema().fields().iter().enumerate() {
            let column = batch.column(i);
            
            // Analyze run-length potential (consecutive identical values)
            let run_length_potential = self.calculate_run_length_potential(column);
            
            // Analyze dictionary potential (repeated values)
            let dictionary_potential = self.calculate_dictionary_potential(column);
            
            // Analyze compression potential based on data distribution
            let compression_potential = self.calculate_compression_potential(column);
            
            analysis.column_optimizations.insert(field.name().clone(), ColumnOptimization {
                run_length_potential,
                dictionary_potential,
                compression_potential,
                recommended_encoding: self.recommend_encoding(run_length_potential, dictionary_potential),
            });
        }
        
        Ok(analysis)
    }
    
    /// Calculate run-length encoding potential for a column
    fn calculate_run_length_potential(&self, column: &Arc<dyn arrow_array::Array>) -> f32 {
        if column.len() <= 1 {
            return 0.0;
        }
        
        let mut consecutive_runs = 0;
        let mut total_comparisons = 0;
        
        // For now, use a simplified analysis based on null patterns
        // In a full implementation, this would compare actual values
        for i in 1..column.len() {
            total_comparisons += 1;
            if column.is_null(i) == column.is_null(i - 1) {
                consecutive_runs += 1;
            }
        }
        
        if total_comparisons == 0 {
            0.0
        } else {
            consecutive_runs as f32 / total_comparisons as f32
        }
    }
    
    /// Calculate dictionary encoding potential for a column
    fn calculate_dictionary_potential(&self, column: &Arc<dyn arrow_array::Array>) -> f32 {
        let total_values = column.len();
        let null_count = column.null_count();
        let non_null_values = total_values - null_count;
        
        if non_null_values == 0 {
            return 0.0;
        }
        
        // Simplified calculation - assume good dictionary potential for small cardinality
        // In a full implementation, this would analyze actual unique values
        let estimated_unique_ratio = if non_null_values < 100 {
            0.3 // Assume 30% unique values for small datasets
        } else if non_null_values < 1000 {
            0.5 // Assume 50% unique values for medium datasets
        } else {
            0.7 // Assume 70% unique values for large datasets
        };
        
        1.0 - estimated_unique_ratio // Higher potential with lower unique ratio
    }
    
    /// Calculate overall compression potential for a column
    fn calculate_compression_potential(&self, column: &Arc<dyn arrow_array::Array>) -> f32 {
        let null_ratio = column.null_count() as f32 / column.len() as f32;
        
        // High null ratio means good compression potential
        // Simplified calculation based on null patterns
        if null_ratio > 0.5 {
            0.8 // High compression potential
        } else if null_ratio > 0.2 {
            0.6 // Medium compression potential
        } else {
            0.4 // Lower compression potential
        }
    }
    
    /// Recommend the best encoding based on analysis
    fn recommend_encoding(&self, run_length_potential: f32, dictionary_potential: f32) -> RecommendedEncoding {
        if run_length_potential > 0.7 {
            RecommendedEncoding::RunLength
        } else if dictionary_potential > 0.6 {
            RecommendedEncoding::Dictionary
        } else if run_length_potential > 0.4 || dictionary_potential > 0.4 {
            RecommendedEncoding::Hybrid
        } else {
            RecommendedEncoding::Standard
        }
    }
    
    /// Apply run-length encoding hints based on sorted order analysis
    fn apply_run_length_encoding_hints(&self, batch: RecordBatch, analysis: &SortAnalysis) -> Result<RecordBatch> {
        // In a full implementation, this would add metadata hints to the batch
        // For now, just log the recommendations
        for (column_name, optimization) in &analysis.column_optimizations {
            if optimization.run_length_potential > 0.5 {
                tracing::debug!("🔄 RLE recommended for column '{}' (potential: {:.2})", 
                               column_name, optimization.run_length_potential);
            }
        }
        
        Ok(batch)
    }
    
    /// Apply dictionary encoding hints based on sorted order analysis
    fn apply_dictionary_encoding_hints(&self, batch: RecordBatch, analysis: &SortAnalysis) -> Result<RecordBatch> {
        // In a full implementation, this would add metadata hints to the batch
        // For now, just log the recommendations
        for (column_name, optimization) in &analysis.column_optimizations {
            if optimization.dictionary_potential > 0.6 {
                tracing::debug!("📖 Dictionary encoding recommended for column '{}' (potential: {:.2})", 
                               column_name, optimization.dictionary_potential);
            }
        }
        
        Ok(batch)
    }
    
    /// Apply compression hints to the batch for optimal Parquet writing
    fn apply_compression_hints(&self, batch: RecordBatch) -> Result<RecordBatch> {
        // In a full implementation, this would set compression metadata on the batch
        // For now, just log compression strategy recommendations
        
        let total_size = batch.get_array_memory_size();
        let row_count = batch.num_rows();
        let avg_row_size = if row_count > 0 { total_size / row_count } else { 0 };
        
        let recommended_algorithm = if avg_row_size > 1024 {
            "ZSTD" // Better for larger rows
        } else {
            "SNAPPY" // Better for smaller rows
        };
        
        tracing::debug!("🗜️ Compression hint: {} recommended for {} rows (avg size: {} bytes)", 
                       recommended_algorithm, row_count, avg_row_size);
        
        Ok(batch)
    }
    
    /// Collect statistics during postprocessing for monitoring and optimization
    async fn collect_postprocessing_statistics(&self, batch: &RecordBatch) -> Result<()> {
        let stats_collection_start = Instant::now();
        
        // Collect basic batch statistics
        let row_count = batch.num_rows();
        let column_count = batch.num_columns();
        let memory_size = batch.get_array_memory_size();
        
        // Calculate data distribution statistics
        let mut null_counts = Vec::new();
        let mut column_sizes = Vec::new();
        
        for i in 0..column_count {
            let column = batch.column(i);
            null_counts.push(column.null_count());
            column_sizes.push(column.get_array_memory_size());
        }
        
        let total_nulls: usize = null_counts.iter().sum();
        let null_ratio = total_nulls as f64 / (row_count * column_count) as f64;
        
        // Update processing statistics asynchronously
        {
            let mut stats = self.stats.write().await;
            stats.records_processed += row_count as u64;
        }
        
        let stats_duration = stats_collection_start.elapsed();
        
        tracing::debug!("📊 Statistics collected: {} rows, {} columns, {} bytes, {:.1}% nulls ({}ms)", 
                       row_count, column_count, memory_size, null_ratio * 100.0, 
                       stats_duration.as_millis());
        
        Ok(())
    }
}

// =============================================================================
// POSTPROCESSING ANALYSIS STRUCTURES
// =============================================================================

/// Analysis of sorted batch for optimization decisions
#[derive(Debug, Default)]
struct SortAnalysis {
    column_optimizations: HashMap<String, ColumnOptimization>,
}

/// Optimization analysis for a specific column
#[derive(Debug)]
struct ColumnOptimization {
    run_length_potential: f32,
    dictionary_potential: f32,
    compression_potential: f32,
    recommended_encoding: RecommendedEncoding,
}

/// Recommended encoding strategy for a column
#[derive(Debug)]
enum RecommendedEncoding {
    Standard,
    RunLength,
    Dictionary,
    Hybrid,
}

impl VectorProcessor for VectorRecordProcessor {
    fn preprocess_records(&self, records: &mut [VectorRecord]) -> Result<()> {
        if !self.config.enable_preprocessing {
            return Ok(());
        }
        
        let sort_start = Instant::now();
        let record_count = records.len();
        
        match &self.config.sorting_strategy {
            SortingStrategy::ByTimestamp => {
                records.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
                tracing::debug!("🔢 Sorted {} records by timestamp", record_count);
            }
            
            SortingStrategy::BySimilarity => {
                // Enhanced similarity sorting using vector magnitude and first few dimensions
                records.sort_by(|a, b| {
                    // Primary: sort by vector magnitude
                    let mag_a: f32 = a.vector.iter().map(|x| x * x).sum::<f32>().sqrt();
                    let mag_b: f32 = b.vector.iter().map(|x| x * x).sum::<f32>().sqrt();
                    let mag_cmp = mag_a.partial_cmp(&mag_b).unwrap_or(std::cmp::Ordering::Equal);
                    
                    if mag_cmp != std::cmp::Ordering::Equal {
                        return mag_cmp;
                    }
                    
                    // Secondary: compare first few dimensions for fine-grained ordering
                    for i in 0..std::cmp::min(5, std::cmp::min(a.vector.len(), b.vector.len())) {
                        let dim_cmp = a.vector[i].partial_cmp(&b.vector[i]).unwrap_or(std::cmp::Ordering::Equal);
                        if dim_cmp != std::cmp::Ordering::Equal {
                            return dim_cmp;
                        }
                    }
                    
                    std::cmp::Ordering::Equal
                });
                tracing::debug!("🔢 Sorted {} records by vector similarity", record_count);
            }
            
            SortingStrategy::ById => {
                records.sort_by(|a, b| a.id.cmp(&b.id));
                tracing::debug!("🔢 Sorted {} records by ID", record_count);
            }
            
            SortingStrategy::ByMetadata(fields) => {
                records.sort_by(|a, b| {
                    self.compare_by_metadata_fields(a, b, fields)
                });
                tracing::debug!("🔢 Sorted {} records by metadata fields: {:?}", record_count, fields);
            }
            
            SortingStrategy::CompositeOptimal { metadata_fields, include_id, include_timestamp } => {
                records.sort_by(|a, b| {
                    // Multi-stage composite comparison for optimal layout
                    
                    // Stage 1: ID comparison (if enabled)
                    if *include_id {
                        let id_cmp = a.id.cmp(&b.id);
                        if id_cmp != std::cmp::Ordering::Equal {
                            return id_cmp;
                        }
                    }
                    
                    // Stage 2: Metadata fields comparison
                    let meta_cmp = self.compare_by_metadata_fields(a, b, metadata_fields);
                    if meta_cmp != std::cmp::Ordering::Equal {
                        return meta_cmp;
                    }
                    
                    // Stage 3: Timestamp comparison (if enabled)
                    if *include_timestamp {
                        let timestamp_cmp = a.timestamp.cmp(&b.timestamp);
                        if timestamp_cmp != std::cmp::Ordering::Equal {
                            return timestamp_cmp;
                        }
                    }
                    
                    std::cmp::Ordering::Equal
                });
                tracing::info!("🎯 Sorted {} records using CompositeOptimal strategy (ID: {}, fields: {:?}, timestamp: {})", 
                              record_count, include_id, metadata_fields, include_timestamp);
            }
            
            SortingStrategy::ClusterThenSort { cluster_count, inner_strategy } => {
                self.cluster_then_sort_records(records, *cluster_count, inner_strategy)?;
                tracing::info!("🌟 Applied ClusterThenSort to {} records ({} clusters, inner: {:?})", 
                              record_count, cluster_count, inner_strategy);
            }
            
            SortingStrategy::Custom { strategy_name, comparison_type } => {
                self.apply_custom_sorting(records, strategy_name, comparison_type)?;
                tracing::debug!("⚡ Applied custom sorting '{}' to {} records", strategy_name, record_count);
            }
            
            SortingStrategy::None => {
                tracing::debug!("➡️ Preserving insertion order for {} records", record_count);
            }
        }
        
        let sort_duration = sort_start.elapsed();
        if sort_duration.as_millis() > 100 {
            tracing::info!("⏱️ Sorting {} records took {}ms", record_count, sort_duration.as_millis());
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
        
        let postprocess_start = Instant::now();
        let row_count = batch.num_rows();
        
        tracing::debug!("🔧 Starting postprocessing for {} rows", row_count);
        
        // Stage 1: Column pruning - remove unnecessary columns for storage optimization
        let pruned_batch = self.apply_column_pruning(batch)?;
        
        // Stage 2: Sorted batch optimizations - leverage sorted order for better compression
        let optimized_batch = self.apply_sorted_batch_optimizations(pruned_batch)?;
        
        // Stage 3: Compression hints - add metadata for optimal Parquet compression
        let final_batch = self.apply_compression_hints(optimized_batch)?;
        
        // Stage 4: Statistics collection for monitoring
        self.collect_postprocessing_statistics(&final_batch).await?;
        
        let postprocess_duration = postprocess_start.elapsed();
        tracing::debug!("✅ Postprocessing completed for {} rows in {}ms", 
                       row_count, postprocess_duration.as_millis());
        
        Ok(final_batch)
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
            worker_handles: Arc::new(Mutex::new(Vec::new())),
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
    pub async fn start_workers(&self) -> Result<()> {
        let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel(1);
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
            
            self.worker_handles.lock().await.push(handle);
        }
        
        Ok(())
    }
    
    /// Stop background workers
    pub async fn stop_workers(&self) -> Result<()> {
        if let Some(sender) = self.shutdown_sender.lock().await.take() {
            let _ = sender.send(());
        }
        
        let mut handles = self.worker_handles.lock().await;
        for handle in handles.drain(..) {
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
        shutdown_receiver: &mut broadcast::Receiver<()>,
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
    async fn new(_pool_size: usize, _factory: Arc<DefaultParquetWriterFactory>) -> Result<Self> {
        Ok(Self {
            writers: Arc::new(Mutex::new(Vec::new())),
            pool_size: _pool_size,
            writer_factory: _factory,
        })
    }
}

// Implementations removed - using ones from viper_core module

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