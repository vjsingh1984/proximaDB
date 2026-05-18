// Configuration system for Zero-Copy I/O System
// Provides workload-specific presets and fine-tuned configuration options

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{MetadataSerializer, ZeroCopyIOSystem};
use crate::storage::persistence::filesystem::FilesystemFactory;
use proximadb_kernel::error::ProximaDBError;

/// Workload types for configuration presets
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum WorkloadType {
    /// High-performance setup (minimize latency)
    HighPerformance,
    /// High-throughput batch processing
    HighThroughput,
    /// Analytics workloads with complex queries
    Analytics,
    /// Real-time latency-sensitive operations
    RealTime,
    /// Mixed workload with balanced settings
    Balanced,
    /// Cost-optimized workload (minimize bandwidth/cost)
    CostOptimized,
}

/// Complete system configuration
#[derive(Debug, Clone, Default)]
pub struct ZeroCopyIOConfig {
    /// Metadata cache configuration
    pub metadata_cache: MetadataCacheConfig,

    /// Download optimizer configuration
    pub download_optimizer: DownloadOptimizerConfig,

    /// Integration settings
    pub integration: IntegrationConfig,

    /// Performance tuning
    pub performance: PerformanceConfig,
}

/// Metadata cache configuration
#[derive(Debug, Clone)]
pub struct MetadataCacheConfig {
    /// Cache directory path
    pub cache_dir: PathBuf,

    /// Maximum memory for metadata cache (MB)
    pub max_memory_mb: usize,

    /// Enable compression for variable-size metadata
    pub enable_compression: bool,

    /// Sync metadata writes to disk
    pub sync_to_disk: bool,

    /// Cache eviction policy
    pub eviction_policy: EvictionPolicy,

    /// Maximum number of cached entries
    pub max_entries: usize,

    /// Enable memory pressure monitoring
    pub enable_pressure_eviction: bool,

    /// Metadata validity duration
    pub validity_duration: Duration,
}

/// Cache eviction policies
#[derive(Debug, Clone)]
pub enum EvictionPolicy {
    /// Least Recently Used
    Lru,
    /// Least Frequently Used
    Lfu,
    /// Adaptive Replacement Cache
    Arc,
    /// Time-based expiration
    Ttl,
    /// Size-based with priority
    SizeBased,
}

/// Download optimizer configuration
#[derive(Debug, Clone)]
pub struct DownloadOptimizerConfig {
    /// Base threshold percentage for selective downloads
    pub base_threshold_percent: f32,

    /// File size-based thresholds
    pub size_thresholds: SizeBasedThresholds,

    /// Network condition adjustments
    pub network_adjustments: NetworkAdjustments,

    /// Access prediction settings
    pub access_prediction: AccessPredictionConfig,

    /// Cost optimization settings
    pub cost_optimization: CostOptimizationConfig,

    /// Range request optimization
    pub range_optimization: RangeOptimizationConfig,
}

/// Size-based threshold configuration
#[derive(Debug, Clone)]
pub struct SizeBasedThresholds {
    /// Small files threshold and download percentage
    pub small_file_threshold: u64,
    pub small_file_download_percent: f32,

    /// Medium files threshold and download percentage
    pub medium_file_threshold: u64,
    pub medium_file_download_percent: f32,

    /// Large files threshold and download percentage
    pub large_file_threshold: u64,
    pub large_file_download_percent: f32,

    /// Huge files download percentage
    pub huge_file_download_percent: f32,
}

/// Network condition adjustments
#[derive(Debug, Clone)]
pub struct NetworkAdjustments {
    /// High latency threshold (ms)
    pub high_latency_threshold: f32,

    /// High latency adjustment (increase threshold)
    pub high_latency_adjustment: f32,

    /// Low bandwidth threshold (Mbps)
    pub low_bandwidth_threshold: f32,

    /// Low bandwidth adjustment (decrease threshold)
    pub low_bandwidth_adjustment: f32,

    /// Request cost per KB (for pricing optimization)
    pub request_cost_per_kb: f32,
}

/// Access prediction configuration
#[derive(Debug, Clone)]
pub struct AccessPredictionConfig {
    /// Enable access pattern learning
    pub enable_learning: bool,

    /// History window for pattern analysis
    pub history_window: Duration,

    /// Minimum accesses before prediction
    pub min_accesses_for_prediction: u32,

    /// Prediction confidence threshold
    pub confidence_threshold: f32,

    /// Future access window for predictions
    pub prediction_window: Duration,
}

/// Cost optimization configuration
#[derive(Debug, Clone)]
pub struct CostOptimizationConfig {
    /// Minimum savings required for selective downloads (bytes)
    pub min_savings_for_selective: u64,

    /// Maximum number of range requests per operation
    pub max_range_requests: usize,

    /// Request cost weight in decision making
    pub request_cost_weight: f32,

    /// Bandwidth cost per GB
    pub bandwidth_cost_per_gb: f64,

    /// Storage cost per GB-month
    pub storage_cost_per_gb_month: f64,
}

/// Range request optimization configuration
#[derive(Debug, Clone)]
pub struct RangeOptimizationConfig {
    /// Maximum gap to merge ranges (bytes)
    pub max_merge_gap: u64,

    /// Minimum range size for splitting
    pub min_range_size: u64,

    /// Enable parallel range downloads
    pub enable_parallel_downloads: bool,

    /// Maximum concurrent range requests
    pub max_concurrent_requests: usize,

    /// Range request timeout
    pub request_timeout: Duration,
}

/// Integration settings
#[derive(Debug, Clone)]
pub struct IntegrationConfig {
    /// Enable batch optimization across files
    pub enable_batch_optimization: bool,

    /// Batch processing window
    pub batch_window: Duration,

    /// Maximum batch size
    pub max_batch_size: usize,

    /// Enable cross-collection optimization
    pub enable_cross_collection_optimization: bool,

    /// Collection isolation level
    pub collection_isolation: CollectionIsolation,
}

/// Collection isolation levels
#[derive(Debug, Clone)]
pub enum CollectionIsolation {
    /// Complete isolation per collection
    Strict,
    /// Shared metadata cache, isolated decisions
    SharedCache,
    /// Full sharing across collections
    Shared,
}

/// Performance tuning configuration
#[derive(Debug, Clone)]
pub struct PerformanceConfig {
    /// Enable performance monitoring
    pub enable_monitoring: bool,

    /// Metrics collection interval
    pub metrics_interval: Duration,

    /// Enable adaptive threshold adjustment
    pub enable_adaptive_thresholds: bool,

    /// Threshold adjustment sensitivity
    pub threshold_adjustment_rate: f32,

    /// Enable circuit breaker for failures
    pub enable_circuit_breaker: bool,

    /// Circuit breaker failure threshold
    pub circuit_breaker_threshold: f32,

    /// Background task intervals
    pub background_tasks: BackgroundTaskConfig,
}

/// Background task configuration
#[derive(Debug, Clone)]
pub struct BackgroundTaskConfig {
    /// Cache cleanup interval
    pub cache_cleanup_interval: Duration,

    /// Metrics aggregation interval
    pub metrics_aggregation_interval: Duration,

    /// Access pattern analysis interval
    pub pattern_analysis_interval: Duration,

    /// Threshold optimization interval
    pub threshold_optimization_interval: Duration,
}

/// Builder for creating Zero-Copy I/O System
pub struct ZeroCopyIOSystemBuilder {
    config: ZeroCopyIOConfig,
    filesystem: Option<Arc<FilesystemFactory>>,
    serializers: Vec<Box<dyn MetadataSerializer>>,
    custom_cache_dir: Option<PathBuf>,
}

#[allow(dead_code)]
impl ZeroCopyIOSystemBuilder {
    /// Create a new builder with default configuration
    pub fn new() -> Self {
        Self {
            config: ZeroCopyIOConfig::default(),
            filesystem: None,
            serializers: Vec::new(),
            custom_cache_dir: None,
        }
    }

    /// Configure for specific workload type
    pub fn for_workload(mut self, workload: WorkloadType) -> Self {
        self.config = ZeroCopyIOConfig::for_workload(workload);
        self
    }

    /// Set custom metadata cache configuration
    pub fn with_metadata_cache_config(mut self, config: MetadataCacheConfig) -> Self {
        self.config.metadata_cache = config;
        self
    }

    /// Set custom download optimizer configuration
    pub fn with_download_config(mut self, config: DownloadOptimizerConfig) -> Self {
        self.config.download_optimizer = config;
        self
    }

    /// Set custom integration configuration
    pub fn with_integration_config(mut self, config: IntegrationConfig) -> Self {
        self.config.integration = config;
        self
    }

    /// Set custom performance configuration
    pub fn with_performance_config(mut self, config: PerformanceConfig) -> Self {
        self.config.performance = config;
        self
    }

    /// Register an engine-specific metadata serializer
    pub fn register_engine_serializer<S: MetadataSerializer + 'static>(
        mut self,
        serializer: S,
    ) -> Self {
        self.serializers.push(Box::new(serializer));
        self
    }

    /// Set filesystem factory
    pub fn with_filesystem(mut self, filesystem: Arc<FilesystemFactory>) -> Self {
        self.filesystem = Some(filesystem);
        self
    }

    /// Set custom cache directory
    pub fn with_cache_directory<P: Into<PathBuf>>(mut self, dir: P) -> Self {
        self.custom_cache_dir = Some(dir.into());
        self
    }

    /// Enable/disable specific features
    pub fn with_batch_optimization(mut self, enabled: bool) -> Self {
        self.config.integration.enable_batch_optimization = enabled;
        self
    }

    pub fn with_access_prediction(mut self, enabled: bool) -> Self {
        self.config
            .download_optimizer
            .access_prediction
            .enable_learning = enabled;
        self
    }

    pub fn with_performance_monitoring(mut self, enabled: bool) -> Self {
        self.config.performance.enable_monitoring = enabled;
        self
    }

    /// Build the final system
    pub async fn build(mut self) -> Result<ZeroCopyIOSystem, ProximaDBError> {
        // Validate configuration
        self.validate_config()?;

        // Apply custom cache directory if provided
        if let Some(cache_dir) = self.custom_cache_dir.take() {
            self.config.metadata_cache.cache_dir = cache_dir;
        }

        // Require filesystem
        let filesystem = self
            .filesystem
            .as_ref()
            .ok_or_else(|| ProximaDBError::Config("Filesystem is required".into()))?
            .clone();

        // Register default serializers if none provided
        if self.serializers.is_empty() {
            self.register_default_serializers(filesystem.clone())?;
        }

        // Create the system
        ZeroCopyIOSystem::new(self.config, filesystem, self.serializers).await
    }

    fn validate_config(&self) -> Result<(), ProximaDBError> {
        let cache_config = &self.config.metadata_cache;
        let download_config = &self.config.download_optimizer;

        // Validate cache configuration
        if cache_config.max_memory_mb == 0 {
            return Err(ProximaDBError::InvalidInput(
                "Max memory must be > 0".into(),
            ));
        }

        if cache_config.max_entries == 0 {
            return Err(ProximaDBError::InvalidInput(
                "Max entries must be > 0".into(),
            ));
        }

        // Validate download configuration
        if download_config.base_threshold_percent < 0.0
            || download_config.base_threshold_percent > 100.0
        {
            return Err(ProximaDBError::InvalidInput(
                "Base threshold must be 0-100%".into(),
            ));
        }

        if download_config.cost_optimization.max_range_requests == 0 {
            return Err(ProximaDBError::InvalidInput(
                "Max range requests must be > 0".into(),
            ));
        }

        Ok(())
    }

    fn register_default_serializers(
        &mut self,
        filesystem: Arc<FilesystemFactory>,
    ) -> Result<(), ProximaDBError> {
        // Register SST serializer
        let sst_serializer = crate::storage::engines::core::formats::proximablocks::sst_metadata::SstMetadataSerializer::new(filesystem.clone());
        self.serializers.push(Box::new(sst_serializer));

        // Register Parquet serializer
        let parquet_serializer = crate::storage::engines::core::formats::columnar::parquet_metadata::ParquetMetadataSerializer::new(filesystem.clone());
        self.serializers.push(Box::new(parquet_serializer));

        Ok(())
    }
}

impl Default for ZeroCopyIOSystemBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ZeroCopyIOConfig {
    /// Create configuration for specific workload type
    pub fn for_workload(workload: WorkloadType) -> Self {
        match workload {
            WorkloadType::HighPerformance => Self::high_performance(),
            WorkloadType::CostOptimized => Self::cost_optimized(),
            WorkloadType::Balanced => Self::balanced(),
            WorkloadType::Analytics => Self::analytics(),
            WorkloadType::HighThroughput => Self::high_performance(), // Map to high performance
            WorkloadType::RealTime => Self::high_performance(), // Map to high performance for low latency
        }
    }

    /// High-performance configuration (minimize latency)
    pub fn high_performance() -> Self {
        Self {
            metadata_cache: MetadataCacheConfig {
                max_memory_mb: 2048,       // 2GB cache
                enable_compression: false, // Skip compression for speed
                eviction_policy: EvictionPolicy::Lru,
                max_entries: 100000,
                ..Default::default()
            },
            download_optimizer: DownloadOptimizerConfig {
                base_threshold_percent: 30.0, // Aggressive selective downloads
                size_thresholds: SizeBasedThresholds {
                    small_file_download_percent: 20.0,
                    medium_file_download_percent: 35.0,
                    large_file_download_percent: 45.0,
                    huge_file_download_percent: 55.0,
                    ..Default::default()
                },
                range_optimization: RangeOptimizationConfig {
                    enable_parallel_downloads: true,
                    max_concurrent_requests: 10,
                    ..Default::default()
                },
                ..Default::default()
            },
            performance: PerformanceConfig {
                enable_adaptive_thresholds: true,
                threshold_adjustment_rate: 0.1,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Cost-optimized configuration (minimize bandwidth)
    pub fn cost_optimized() -> Self {
        Self {
            metadata_cache: MetadataCacheConfig {
                max_memory_mb: 512,       // Smaller cache
                enable_compression: true, // Use compression
                ..Default::default()
            },
            download_optimizer: DownloadOptimizerConfig {
                base_threshold_percent: 60.0, // Very selective
                size_thresholds: SizeBasedThresholds {
                    small_file_download_percent: 40.0,
                    medium_file_download_percent: 60.0,
                    large_file_download_percent: 70.0,
                    huge_file_download_percent: 80.0,
                    ..Default::default()
                },
                cost_optimization: CostOptimizationConfig {
                    min_savings_for_selective: 100 * 1024 * 1024, // 100MB
                    max_range_requests: 5,                        // Minimize API calls
                    request_cost_weight: 2.0,                     // High cost sensitivity
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Balanced configuration (general purpose)
    pub fn balanced() -> Self {
        Self::default()
    }

    /// Analytics configuration (large scan workloads)
    pub fn analytics() -> Self {
        Self {
            download_optimizer: DownloadOptimizerConfig {
                base_threshold_percent: 70.0, // Prefer full downloads for scans
                access_prediction: AccessPredictionConfig {
                    enable_learning: true,
                    prediction_window: Duration::from_secs(1800), // 30 minutes
                    ..Default::default()
                },
                ..Default::default()
            },
            integration: IntegrationConfig {
                enable_batch_optimization: true,
                max_batch_size: 100,
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

// Default implementations

impl Default for MetadataCacheConfig {
    fn default() -> Self {
        Self {
            cache_dir: PathBuf::from("/tmp/proximadb_metadata_cache"),
            max_memory_mb: 512,
            enable_compression: true,
            sync_to_disk: true,
            eviction_policy: EvictionPolicy::Lru,
            max_entries: 50000,
            enable_pressure_eviction: true,
            validity_duration: Duration::from_secs(3600), // 1 hour
        }
    }
}

impl Default for DownloadOptimizerConfig {
    fn default() -> Self {
        Self {
            base_threshold_percent: 40.0,
            size_thresholds: SizeBasedThresholds::default(),
            network_adjustments: NetworkAdjustments::default(),
            access_prediction: AccessPredictionConfig::default(),
            cost_optimization: CostOptimizationConfig::default(),
            range_optimization: RangeOptimizationConfig::default(),
        }
    }
}

impl Default for SizeBasedThresholds {
    fn default() -> Self {
        Self {
            small_file_threshold: 10 * 1024 * 1024, // 10MB
            small_file_download_percent: 25.0,
            medium_file_threshold: 100 * 1024 * 1024, // 100MB
            medium_file_download_percent: 40.0,
            large_file_threshold: 1024 * 1024 * 1024, // 1GB
            large_file_download_percent: 50.0,
            huge_file_download_percent: 60.0,
        }
    }
}

impl Default for NetworkAdjustments {
    fn default() -> Self {
        Self {
            high_latency_threshold: 200.0, // 200ms
            high_latency_adjustment: 15.0,
            low_bandwidth_threshold: 10.0, // 10 Mbps
            low_bandwidth_adjustment: -10.0,
            request_cost_per_kb: 0.0001,
        }
    }
}

impl Default for AccessPredictionConfig {
    fn default() -> Self {
        Self {
            enable_learning: true,
            history_window: Duration::from_secs(7200), // 2 hours
            min_accesses_for_prediction: 5,
            confidence_threshold: 0.6,
            prediction_window: Duration::from_secs(300), // 5 minutes
        }
    }
}

impl Default for CostOptimizationConfig {
    fn default() -> Self {
        Self {
            min_savings_for_selective: 10 * 1024 * 1024, // 10MB
            max_range_requests: 20,
            request_cost_weight: 1.0,
            bandwidth_cost_per_gb: 0.09,      // $0.09/GB (typical S3)
            storage_cost_per_gb_month: 0.023, // $0.023/GB-month
        }
    }
}

impl Default for RangeOptimizationConfig {
    fn default() -> Self {
        Self {
            max_merge_gap: 64 * 1024, // 64KB
            min_range_size: 4 * 1024, // 4KB
            enable_parallel_downloads: false,
            max_concurrent_requests: 5,
            request_timeout: Duration::from_secs(30),
        }
    }
}

impl Default for IntegrationConfig {
    fn default() -> Self {
        Self {
            enable_batch_optimization: true,
            batch_window: Duration::from_millis(100),
            max_batch_size: 50,
            enable_cross_collection_optimization: false,
            collection_isolation: CollectionIsolation::SharedCache,
        }
    }
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            enable_monitoring: true,
            metrics_interval: Duration::from_secs(60),
            enable_adaptive_thresholds: false,
            threshold_adjustment_rate: 0.05,
            enable_circuit_breaker: true,
            circuit_breaker_threshold: 0.1, // 10% failure rate
            background_tasks: BackgroundTaskConfig::default(),
        }
    }
}

impl Default for BackgroundTaskConfig {
    fn default() -> Self {
        Self {
            cache_cleanup_interval: Duration::from_secs(300), // 5 minutes
            metrics_aggregation_interval: Duration::from_secs(60), // 1 minute
            pattern_analysis_interval: Duration::from_secs(600), // 10 minutes
            threshold_optimization_interval: Duration::from_secs(1800), // 30 minutes
        }
    }
}
