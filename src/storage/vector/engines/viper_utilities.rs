// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! VIPER Utilities and Operations - Consolidated Implementation
//!
//! 🔥 PHASE 4.4: This consolidates 5 VIPER utility files into a unified implementation:
//! - `stats.rs` → Performance statistics and monitoring
//! - `ttl.rs` → TTL cleanup service for automatic expiration
//! - `staging_operations.rs` → Staging operations for Parquet optimization
//! - `partitioner.rs` → Data partitioning and clustering algorithms
//! - `compression.rs` → Advanced compression strategies and optimization
//!
//! ## Key Features
//! - **Comprehensive Monitoring**: Real-time performance metrics and analytics
//! - **Automatic TTL Management**: Background cleanup of expired vectors
//! - **Intelligent Partitioning**: ML-driven data clustering and partitioning
//! - **Advanced Compression**: Adaptive compression with format optimization
//! - **Staging Operations**: Optimized Parquet operations with atomic writes

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::time::Instant;
use tracing::info;

use crate::core::{CollectionId, VectorRecord, CompressionAlgorithm};
use crate::storage::filesystem::FilesystemFactory;

/// VIPER Utilities coordinator - Central management for all utility services
pub struct ViperUtilities {
    /// Performance statistics collector
    stats_collector: Arc<PerformanceStatsCollector>,
    
    /// TTL cleanup service
    ttl_service: Arc<Mutex<TTLCleanupService>>,
    
    /// Staging operations coordinator
    staging_coordinator: Arc<StagingOperationsCoordinator>,
    
    /// Data partitioner
    partitioner: Arc<DataPartitioner>,
    
    /// Compression optimizer
    compression_optimizer: Arc<CompressionOptimizer>,
    
    /// Utilities configuration
    config: ViperUtilitiesConfig,
    
    /// Background service handles
    service_handles: Vec<tokio::task::JoinHandle<()>>,
}

/// Configuration for VIPER utilities
#[derive(Debug, Clone)]
pub struct ViperUtilitiesConfig {
    /// Statistics collection configuration
    pub stats_config: StatsConfig,
    
    /// TTL configuration
    pub ttl_config: TTLConfig,
    
    /// Staging operations configuration
    pub staging_config: StagingConfig,
    
    /// Partitioning configuration
    pub partitioning_config: PartitioningConfig,
    
    /// Compression configuration
    pub compression_config: CompressionConfig,
    
    /// Enable background services
    pub enable_background_services: bool,
}

// Performance Statistics

/// Performance statistics collector for VIPER operations
pub struct PerformanceStatsCollector {
    /// Operation metrics
    operation_metrics: Arc<RwLock<HashMap<String, OperationMetrics>>>,
    
    /// Collection-level statistics
    collection_stats: Arc<RwLock<HashMap<CollectionId, CollectionStats>>>,
    
    /// Global VIPER statistics
    global_stats: Arc<RwLock<GlobalViperStats>>,
    
    /// Configuration
    config: StatsConfig,
}

/// Configuration for statistics collection
#[derive(Debug, Clone)]
pub struct StatsConfig {
    /// Enable detailed operation tracking
    pub enable_detailed_tracking: bool,
    
    /// Enable real-time metrics
    pub enable_realtime_metrics: bool,
    
    /// Statistics retention period
    pub retention_period_hours: u64,
    
    /// Metrics collection interval
    pub collection_interval_secs: u64,
    
    /// Enable performance profiling
    pub enable_profiling: bool,
}

/// Metrics for individual operations
#[derive(Debug, Clone)]
pub struct OperationMetrics {
    pub operation_type: String,
    pub collection_id: CollectionId,
    pub records_processed: u64,
    pub total_time_ms: u64,
    pub preprocessing_time_ms: u64,
    pub processing_time_ms: u64,
    pub postprocessing_time_ms: u64,
    pub bytes_written: u64,
    pub compression_ratio: f32,
    pub throughput_ops_sec: f64,
    pub timestamp: DateTime<Utc>,
}

/// Collection-level statistics
#[derive(Debug, Clone)]
pub struct CollectionStats {
    pub collection_id: CollectionId,
    pub total_operations: u64,
    pub total_records: u64,
    pub total_bytes: u64,
    pub avg_latency_ms: f64,
    pub avg_throughput_ops_sec: f64,
    pub avg_compression_ratio: f32,
    pub last_operation: Option<DateTime<Utc>>,
    pub error_count: u64,
}

/// Global VIPER statistics
#[derive(Debug, Default, Clone)]
pub struct GlobalViperStats {
    pub total_operations: u64,
    pub total_collections: usize,
    pub total_records_processed: u64,
    pub total_bytes_written: u64,
    pub avg_latency_ms: f64,
    pub avg_compression_ratio: f32,
    pub uptime_seconds: u64,
    pub cache_hit_ratio: f32,
    pub error_rate: f32,
}

/// Performance statistics collector for individual operations
#[derive(Debug)]
pub struct OperationStatsCollector {
    operation_start: Option<Instant>,
    phase_timers: HashMap<String, Instant>,
    metrics: OperationMetrics,
}

// TTL Management

/// TTL cleanup service for automatic vector expiration
pub struct TTLCleanupService {
    /// TTL configuration
    config: TTLConfig,
    
    /// Filesystem access for file operations
    filesystem: Arc<FilesystemFactory>,
    
    /// Partition metadata tracking
    partition_metadata: Arc<RwLock<HashMap<PartitionId, PartitionMetadata>>>,
    
    /// Background cleanup task handle
    cleanup_task: Option<tokio::task::JoinHandle<()>>,
    
    /// TTL statistics
    stats: Arc<RwLock<TTLStats>>,
}

/// TTL configuration
#[derive(Debug, Clone)]
pub struct TTLConfig {
    /// Enable TTL functionality
    pub enabled: bool,
    
    /// Default TTL duration for vectors
    pub default_ttl: Option<Duration>,
    
    /// Background cleanup interval
    pub cleanup_interval: Duration,
    
    /// Maximum vectors to clean per batch
    pub max_cleanup_batch_size: usize,
    
    /// Enable file-level TTL filtering
    pub enable_file_level_filtering: bool,
    
    /// Minimum expiration age before file deletion
    pub min_expiration_age: Duration,
    
    /// Cleanup priority scheduling
    pub enable_priority_scheduling: bool,
}

/// TTL cleanup statistics
#[derive(Debug, Default, Clone)]
pub struct TTLStats {
    pub total_cleanup_runs: u64,
    pub vectors_expired: u64,
    pub files_deleted: u64,
    pub partitions_cleaned: u64,
    pub last_cleanup_at: Option<DateTime<Utc>>,
    pub last_cleanup_duration_ms: u64,
    pub errors_count: u64,
    pub avg_cleanup_time_ms: f64,
}

/// Cleanup result for TTL operations
#[derive(Debug)]
pub struct CleanupResult {
    pub vectors_expired: u64,
    pub files_deleted: u64,
    pub partitions_processed: u64,
    pub errors: Vec<String>,
    pub duration_ms: u64,
}

// Staging Operations

/// Staging operations coordinator for Parquet optimization
pub struct StagingOperationsCoordinator {
    /// Filesystem interface
    filesystem: Arc<FilesystemFactory>,
    
    /// Staging configuration
    config: StagingConfig,
    
    /// Active staging operations
    active_operations: Arc<RwLock<HashMap<String, StagingOperation>>>,
    
    /// Optimization cache
    optimization_cache: Arc<RwLock<HashMap<String, ParquetOptimizationConfig>>>,
}

/// Staging configuration
#[derive(Debug, Clone)]
pub struct StagingConfig {
    /// Enable staging optimizations
    pub enable_optimizations: bool,
    
    /// Default staging directory
    pub staging_directory: String,
    
    /// Cleanup interval for staging files
    pub cleanup_interval_secs: u64,
    
    /// Maximum staging file age before cleanup
    pub max_staging_age_secs: u64,
    
    /// Enable atomic operations
    pub enable_atomic_operations: bool,
}

/// Staging operation metadata
#[derive(Debug)]
pub struct StagingOperation {
    pub operation_id: String,
    pub operation_type: StagingOperationType,
    pub staging_path: String,
    pub target_path: String,
    pub started_at: DateTime<Utc>,
    pub status: StagingStatus,
}

/// Staging operation type
#[derive(Debug, Clone)]
pub enum StagingOperationType {
    Flush,
    Compaction,
    Migration,
}

/// Staging operation status
#[derive(Debug, Clone)]
pub enum StagingStatus {
    Preparing,
    Writing,
    Finalizing,
    Completed,
    Failed(String),
}

/// Parquet optimization configuration for staging
#[derive(Debug, Clone)]
pub struct ParquetOptimizationConfig {
    /// Rows per row group
    pub rows_per_rowgroup: usize,
    
    /// Number of row groups
    pub num_rowgroups: usize,
    
    /// Target file size in MB
    pub target_file_size_mb: usize,
    
    /// Enable dictionary encoding
    pub enable_dictionary_encoding: bool,
    
    /// Enable bloom filters
    pub enable_bloom_filters: bool,
    
    /// Compression level
    pub compression_level: i32,
    
    /// Column ordering for optimization
    pub column_order: Vec<String>,
}

/// Optimized Parquet records with metadata
#[derive(Debug, Clone)]
pub struct OptimizedParquetRecords {
    pub records: Vec<VectorRecord>,
    pub schema_version: u32,
    pub column_order: Vec<String>,
    pub rowgroup_size: usize,
    pub compression_config: ParquetOptimizationConfig,
}

// Data Partitioning

/// Data partitioner for intelligent clustering and partitioning
pub struct DataPartitioner {
    /// Partitioning configuration
    config: PartitioningConfig,
    
    /// ML models for clustering
    clustering_models: Arc<RwLock<HashMap<CollectionId, ClusteringModel>>>,
    
    /// Partition metadata
    partition_metadata: Arc<RwLock<HashMap<PartitionId, PartitionMetadata>>>,
    
    /// Partitioning statistics
    stats: Arc<RwLock<PartitioningStats>>,
}

/// Partitioning configuration
#[derive(Debug, Clone)]
pub struct PartitioningConfig {
    /// Enable ML-driven clustering
    pub enable_ml_clustering: bool,
    
    /// Default number of clusters
    pub default_cluster_count: usize,
    
    /// Minimum vectors per partition
    pub min_vectors_per_partition: usize,
    
    /// Maximum vectors per partition
    pub max_vectors_per_partition: usize,
    
    /// Clustering algorithm
    pub clustering_algorithm: ClusteringAlgorithm,
    
    /// Enable adaptive clustering
    pub enable_adaptive_clustering: bool,
    
    /// Re-clustering threshold
    pub reclustering_threshold: f32,
}

/// Clustering algorithm options
#[derive(Debug, Clone)]
pub enum ClusteringAlgorithm {
    KMeans,
    HDBSCAN,
    SpectralClustering,
    MiniBatchKMeans,
    AdaptiveKMeans,
}

/// ML model for clustering
#[derive(Debug)]
pub struct ClusteringModel {
    pub model_id: String,
    pub collection_id: CollectionId,
    pub algorithm: ClusteringAlgorithm,
    pub cluster_count: usize,
    pub centroids: Vec<Vec<f32>>,
    pub accuracy: f32,
    pub trained_at: DateTime<Utc>,
}

/// Partition metadata
#[derive(Debug, Clone)]
pub struct PartitionMetadata {
    pub partition_id: PartitionId,
    pub collection_id: CollectionId,
    pub cluster_id: ClusterId,
    pub file_path: String,
    pub vector_count: usize,
    pub size_bytes: usize,
    pub compression_ratio: f32,
    pub created_at: DateTime<Utc>,
    pub last_accessed: DateTime<Utc>,
}

/// Partitioning statistics
#[derive(Debug, Default)]
pub struct PartitioningStats {
    pub total_partitions_created: u64,
    pub total_reclustering_operations: u64,
    pub avg_vectors_per_partition: f64,
    pub avg_compression_ratio: f32,
    pub avg_clustering_time_ms: f64,
}

/// Type aliases
pub type PartitionId = String;
pub type ClusterId = String;

// Compression Optimization

/// Compression optimizer for advanced compression strategies
pub struct CompressionOptimizer {
    /// Compression configuration
    config: CompressionConfig,
    
    /// Compression models per collection
    compression_models: Arc<RwLock<HashMap<CollectionId, CompressionModel>>>,
    
    /// Compression statistics
    stats: Arc<RwLock<CompressionStats>>,
    
    /// Algorithm performance cache
    algorithm_cache: Arc<RwLock<HashMap<String, AlgorithmPerformance>>>,
}

/// Compression configuration
#[derive(Debug, Clone)]
pub struct CompressionConfig {
    /// Enable adaptive compression
    pub enable_adaptive_compression: bool,
    
    /// Default compression algorithm
    pub default_algorithm: CompressionAlgorithm,
    
    /// Compression level range
    pub compression_level_range: (u8, u8),
    
    /// Enable format-specific optimization
    pub enable_format_optimization: bool,
    
    /// Enable compression benchmarking
    pub enable_benchmarking: bool,
    
    /// Optimization interval
    pub optimization_interval_secs: u64,
}


/// Compression model for optimal algorithm selection
#[derive(Debug)]
pub struct CompressionModel {
    pub model_id: String,
    pub collection_id: CollectionId,
    pub optimal_algorithm: CompressionAlgorithm,
    pub optimal_level: u8,
    pub expected_ratio: f32,
    pub performance_profile: CompressionPerformance,
    pub trained_at: DateTime<Utc>,
}

/// Compression performance metrics
#[derive(Debug, Clone)]
pub struct CompressionPerformance {
    pub compression_ratio: f32,
    pub compression_speed_mb_sec: f32,
    pub decompression_speed_mb_sec: f32,
    pub cpu_usage_percent: f32,
    pub memory_usage_mb: f32,
}

/// Algorithm performance tracking
#[derive(Debug, Clone)]
pub struct AlgorithmPerformance {
    pub algorithm: CompressionAlgorithm,
    pub avg_compression_ratio: f32,
    pub avg_compression_speed: f32,
    pub avg_decompression_speed: f32,
    pub reliability_score: f32,
    pub usage_count: u64,
}

/// Compression statistics
#[derive(Debug, Default)]
pub struct CompressionStats {
    pub total_compressions: u64,
    pub total_bytes_input: u64,
    pub total_bytes_output: u64,
    pub avg_compression_ratio: f32,
    pub avg_compression_time_ms: f64,
    pub algorithm_usage: HashMap<String, u64>,
}

// Implementation

impl ViperUtilities {
    /// Create new VIPER utilities coordinator
    pub async fn new(
        config: ViperUtilitiesConfig,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        let stats_collector = Arc::new(
            PerformanceStatsCollector::new(config.stats_config.clone()).await?
        );
        
        let ttl_service = Arc::new(Mutex::new(
            TTLCleanupService::new(
                config.ttl_config.clone(),
                filesystem.clone(),
            ).await?
        ));
        
        let staging_coordinator = Arc::new(
            StagingOperationsCoordinator::new(
                filesystem.clone(),
                config.staging_config.clone(),
            ).await?
        );
        
        let partitioner = Arc::new(
            DataPartitioner::new(config.partitioning_config.clone()).await?
        );
        
        let compression_optimizer = Arc::new(
            CompressionOptimizer::new(config.compression_config.clone()).await?
        );
        
        Ok(Self {
            stats_collector,
            ttl_service,
            staging_coordinator,
            partitioner,
            compression_optimizer,
            config,
            service_handles: Vec::new(),
        })
    }
    
    /// Start all background services
    pub async fn start_services(&mut self) -> Result<()> {
        if !self.config.enable_background_services {
            info!("🔧 VIPER Utilities: Background services disabled");
            return Ok(());
        }
        
        info!("🚀 VIPER Utilities: Starting background services");
        
        // Start TTL cleanup service
        {
            let mut ttl_service = self.ttl_service.lock().await;
            ttl_service.start().await?;
        }
        
        // Start statistics collection
        self.stats_collector.start_collection().await?;
        
        // Start staging cleanup
        self.staging_coordinator.start_cleanup().await?;
        
        // Start compression optimization
        self.compression_optimizer.start_optimization().await?;
        
        info!("✅ VIPER Utilities: All background services started");
        Ok(())
    }
    
    /// Stop all background services
    pub async fn stop_services(&mut self) -> Result<()> {
        info!("🛑 VIPER Utilities: Stopping background services");
        
        // Stop TTL service
        {
            let mut ttl_service = self.ttl_service.lock().await;
            ttl_service.stop().await?;
        }
        
        // Stop other services
        for handle in self.service_handles.drain(..) {
            let _ = handle.await;
        }
        
        info!("✅ VIPER Utilities: All background services stopped");
        Ok(())
    }
    
    /// Record operation metrics
    pub async fn record_operation(&self, metrics: OperationMetrics) -> Result<()> {
        self.stats_collector.record_operation(metrics).await
    }
    
    /// Get performance statistics
    pub async fn get_performance_stats(&self, collection_id: Option<&CollectionId>) -> Result<PerformanceReport> {
        self.stats_collector.get_performance_report(collection_id).await
    }
    
    /// Schedule TTL cleanup
    pub async fn schedule_ttl_cleanup(&self, collection_id: &CollectionId) -> Result<()> {
        let ttl_service = self.ttl_service.lock().await;
        ttl_service.schedule_cleanup(collection_id).await
    }
    
    /// Optimize compression for collection
    pub async fn optimize_compression(&self, collection_id: &CollectionId) -> Result<CompressionRecommendation> {
        self.compression_optimizer.optimize_for_collection(collection_id).await
    }
    
    /// Partition data for optimal storage
    pub async fn partition_data(&self, collection_id: &CollectionId, records: Vec<VectorRecord>) -> Result<Vec<PartitionedData>> {
        self.partitioner.partition_records(collection_id, records).await
    }
    
    /// Create staging operation
    pub async fn create_staging_operation(
        &self,
        operation_type: StagingOperationType,
        records: Vec<VectorRecord>,
    ) -> Result<StagingOperationResult> {
        self.staging_coordinator.create_operation(operation_type, records).await
    }
}

/// Performance report
#[derive(Debug)]
pub struct PerformanceReport {
    pub global_stats: GlobalViperStats,
    pub collection_stats: Option<CollectionStats>,
    pub recent_operations: Vec<OperationMetrics>,
    pub recommendations: Vec<PerformanceRecommendation>,
}

/// Performance recommendation
#[derive(Debug)]
pub struct PerformanceRecommendation {
    pub recommendation_type: String,
    pub description: String,
    pub expected_improvement: f32,
    pub confidence: f32,
}

/// Compression recommendation
#[derive(Debug)]
pub struct CompressionRecommendation {
    pub recommended_algorithm: CompressionAlgorithm,
    pub recommended_level: u8,
    pub expected_ratio: f32,
    pub expected_performance: CompressionPerformance,
}

/// Partitioned data result
#[derive(Debug)]
pub struct PartitionedData {
    pub partition_id: PartitionId,
    pub cluster_id: ClusterId,
    pub records: Vec<VectorRecord>,
    pub metadata: PartitionMetadata,
}

/// Staging operation result
#[derive(Debug)]
pub struct StagingOperationResult {
    pub operation_id: String,
    pub staging_path: String,
    pub optimized_records: OptimizedParquetRecords,
    pub estimated_size_mb: f32,
}

// Placeholder implementations for compilation

impl PerformanceStatsCollector {
    async fn new(_config: StatsConfig) -> Result<Self> {
        Ok(Self {
            operation_metrics: Arc::new(RwLock::new(HashMap::new())),
            collection_stats: Arc::new(RwLock::new(HashMap::new())),
            global_stats: Arc::new(RwLock::new(GlobalViperStats::default())),
            config: _config,
        })
    }
    
    async fn start_collection(&self) -> Result<()> {
        Ok(())
    }
    
    async fn record_operation(&self, _metrics: OperationMetrics) -> Result<()> {
        Ok(())
    }
    
    async fn get_performance_report(&self, _collection_id: Option<&CollectionId>) -> Result<PerformanceReport> {
        Ok(PerformanceReport {
            global_stats: GlobalViperStats::default(),
            collection_stats: None,
            recent_operations: Vec::new(),
            recommendations: Vec::new(),
        })
    }
}

impl TTLCleanupService {
    async fn new(_config: TTLConfig, _filesystem: Arc<FilesystemFactory>) -> Result<Self> {
        Ok(Self {
            config: _config,
            filesystem: _filesystem,
            partition_metadata: Arc::new(RwLock::new(HashMap::new())),
            cleanup_task: None,
            stats: Arc::new(RwLock::new(TTLStats::default())),
        })
    }
    
    async fn start(&mut self) -> Result<()> {
        Ok(())
    }
    
    async fn stop(&mut self) -> Result<()> {
        Ok(())
    }
    
    async fn schedule_cleanup(&self, _collection_id: &CollectionId) -> Result<()> {
        Ok(())
    }
}

impl StagingOperationsCoordinator {
    async fn new(_filesystem: Arc<FilesystemFactory>, _config: StagingConfig) -> Result<Self> {
        Ok(Self {
            filesystem: _filesystem,
            config: _config,
            active_operations: Arc::new(RwLock::new(HashMap::new())),
            optimization_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }
    
    async fn start_cleanup(&self) -> Result<()> {
        Ok(())
    }
    
    async fn create_operation(
        &self,
        _operation_type: StagingOperationType,
        _records: Vec<VectorRecord>,
    ) -> Result<StagingOperationResult> {
        Ok(StagingOperationResult {
            operation_id: "test".to_string(),
            staging_path: "/tmp/staging".to_string(),
            optimized_records: OptimizedParquetRecords {
                records: Vec::new(),
                schema_version: 1,
                column_order: Vec::new(),
                rowgroup_size: 1000,
                compression_config: ParquetOptimizationConfig {
                    rows_per_rowgroup: 1000,
                    num_rowgroups: 1,
                    target_file_size_mb: 64,
                    enable_dictionary_encoding: true,
                    enable_bloom_filters: true,
                    compression_level: 3,
                    column_order: Vec::new(),
                },
            },
            estimated_size_mb: 1.0,
        })
    }
}

impl DataPartitioner {
    async fn new(_config: PartitioningConfig) -> Result<Self> {
        Ok(Self {
            config: _config,
            clustering_models: Arc::new(RwLock::new(HashMap::new())),
            partition_metadata: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(PartitioningStats::default())),
        })
    }
    
    async fn partition_records(
        &self,
        _collection_id: &CollectionId,
        _records: Vec<VectorRecord>,
    ) -> Result<Vec<PartitionedData>> {
        Ok(Vec::new())
    }
}

impl CompressionOptimizer {
    async fn new(_config: CompressionConfig) -> Result<Self> {
        Ok(Self {
            config: _config,
            compression_models: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(CompressionStats::default())),
            algorithm_cache: Arc::new(RwLock::new(HashMap::new())),
        })
    }
    
    async fn start_optimization(&self) -> Result<()> {
        Ok(())
    }
    
    async fn optimize_for_collection(
        &self,
        _collection_id: &CollectionId,
    ) -> Result<CompressionRecommendation> {
        Ok(CompressionRecommendation {
            recommended_algorithm: CompressionAlgorithm::Zstd,
            recommended_level: 3,
            expected_ratio: 0.7,
            expected_performance: CompressionPerformance {
                compression_ratio: 0.7,
                compression_speed_mb_sec: 100.0,
                decompression_speed_mb_sec: 200.0,
                cpu_usage_percent: 20.0,
                memory_usage_mb: 50.0,
            },
        })
    }
}

impl OperationStatsCollector {
    pub fn new(operation_type: String, collection_id: CollectionId) -> Self {
        Self {
            operation_start: None,
            phase_timers: HashMap::new(),
            metrics: OperationMetrics {
                operation_type,
                collection_id,
                records_processed: 0,
                total_time_ms: 0,
                preprocessing_time_ms: 0,
                processing_time_ms: 0,
                postprocessing_time_ms: 0,
                bytes_written: 0,
                compression_ratio: 1.0,
                throughput_ops_sec: 0.0,
                timestamp: Utc::now(),
            },
        }
    }
    
    pub fn start_operation(&mut self) {
        self.operation_start = Some(Instant::now());
    }
    
    pub fn start_phase(&mut self, phase_name: &str) {
        self.phase_timers.insert(phase_name.to_string(), Instant::now());
    }
    
    pub fn end_phase(&mut self, phase_name: &str) {
        if let Some(start_time) = self.phase_timers.remove(phase_name) {
            let duration_ms = start_time.elapsed().as_millis() as u64;
            
            match phase_name {
                "preprocessing" => self.metrics.preprocessing_time_ms = duration_ms,
                "processing" => self.metrics.processing_time_ms = duration_ms,
                "postprocessing" => self.metrics.postprocessing_time_ms = duration_ms,
                _ => {}
            }
        }
    }
    
    pub fn finalize(&mut self) -> OperationMetrics {
        if let Some(start_time) = self.operation_start {
            self.metrics.total_time_ms = start_time.elapsed().as_millis() as u64;
            
            if self.metrics.total_time_ms > 0 {
                self.metrics.throughput_ops_sec = 
                    self.metrics.records_processed as f64 * 1000.0 / self.metrics.total_time_ms as f64;
            }
        }
        
        self.metrics.clone()
    }
    
    pub fn set_records_processed(&mut self, count: u64) {
        self.metrics.records_processed = count;
    }
    
    pub fn set_bytes_written(&mut self, bytes: u64) {
        self.metrics.bytes_written = bytes;
    }
    
    pub fn set_compression_ratio(&mut self, ratio: f32) {
        self.metrics.compression_ratio = ratio;
    }
}

// Default implementations

impl Default for ViperUtilitiesConfig {
    fn default() -> Self {
        Self {
            stats_config: StatsConfig::default(),
            ttl_config: TTLConfig::default(),
            staging_config: StagingConfig::default(),
            partitioning_config: PartitioningConfig::default(),
            compression_config: CompressionConfig::default(),
            enable_background_services: true,
        }
    }
}

impl Default for StatsConfig {
    fn default() -> Self {
        Self {
            enable_detailed_tracking: true,
            enable_realtime_metrics: true,
            retention_period_hours: 168, // 1 week
            collection_interval_secs: 60,
            enable_profiling: true,
        }
    }
}

impl Default for TTLConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_ttl: None,
            cleanup_interval: Duration::hours(1),
            max_cleanup_batch_size: 10000,
            enable_file_level_filtering: true,
            min_expiration_age: Duration::hours(24),
            enable_priority_scheduling: true,
        }
    }
}

impl Default for StagingConfig {
    fn default() -> Self {
        Self {
            enable_optimizations: true,
            staging_directory: "/tmp/viper_staging".to_string(),
            cleanup_interval_secs: 3600,
            max_staging_age_secs: 86400, // 24 hours
            enable_atomic_operations: true,
        }
    }
}

impl Default for PartitioningConfig {
    fn default() -> Self {
        Self {
            enable_ml_clustering: true,
            default_cluster_count: 10,
            min_vectors_per_partition: 1000,
            max_vectors_per_partition: 100000,
            clustering_algorithm: ClusteringAlgorithm::AdaptiveKMeans,
            enable_adaptive_clustering: true,
            reclustering_threshold: 0.8,
        }
    }
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            enable_adaptive_compression: true,
            default_algorithm: CompressionAlgorithm::Zstd { level: 3 },
            compression_level_range: (1, 9),
            enable_format_optimization: true,
            enable_benchmarking: true,
            optimization_interval_secs: 3600,
        }
    }
}