//! Cache configuration for production deployment

use anyhow::Result;

/// Complete cache configuration for production deployment
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct CacheConfig {
    /// Global cache settings
    pub global: GlobalCacheConfig,

    /// Per-cache-type configurations
    pub vector_data: VectorCacheConfig,
    pub query_result: QueryCacheConfig,
    pub filter_bitmap: FilterCacheConfig,
    pub index_structure: IndexCacheConfig,
    pub metadata: MetadataStoreConfig,

    /// Cross-cache coordination settings
    pub coordination: CoordinationConfig,

    /// Monitoring and observability
    pub monitoring: MonitoringConfig,
}

/// Global cache configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GlobalCacheConfig {
    /// Total memory budget in MB
    pub total_memory_mb: usize,

    /// Enable cache system
    pub enabled: bool,

    /// Default TTL for cache entries in seconds
    pub default_ttl_seconds: u64,

    /// Default eviction policy
    pub default_eviction_policy: EvictionPolicy,

    /// Enable tiered storage (L1, L2, L3)
    pub enable_tiered_storage: bool,

    /// L2 storage path (NVMe/SSD)
    pub l2_storage_path: Option<String>,

    /// L3 storage endpoint (Network/Cloud)
    pub l3_storage_endpoint: Option<String>,

    /// Compression for cached data
    pub compression: bool,

    /// Cache warming on startup
    pub enable_warming: bool,

    /// Warming data source
    pub warming_source: Option<String>,
}

/// Eviction policy options
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum EvictionPolicy {
    LRU,
    LFU,
    ARC,
    FIFO,
    TTL,
    Adaptive,
}

/// Vector data cache configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VectorCacheConfig {
    /// Enable vector cache
    pub enabled: bool,

    /// Memory allocation percentage
    pub memory_percentage: u8,

    /// Maximum entry size in bytes
    pub max_entry_size_bytes: usize,

    /// Enable similarity-based prefetching
    pub enable_similarity_prefetch: bool,

    /// Prefetch radius for similarity
    pub similarity_prefetch_radius: f32,

    /// Batch size for prefetching
    pub prefetch_batch_size: usize,

    /// Vector compression in cache
    pub compression: bool,

    /// Compression algorithm
    pub compression_algorithm: Option<String>,
}

/// Query result cache configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QueryCacheConfig {
    /// Enable query cache
    pub enabled: bool,

    /// Memory allocation percentage
    pub memory_percentage: u8,

    /// Maximum result set size to cache
    pub max_result_size: usize,

    /// Query normalization for better hit rate
    pub enable_normalization: bool,

    /// Similarity threshold for approximate matching
    pub similarity_threshold: f32,

    /// TTL for query results in seconds
    pub ttl_seconds: u64,

    /// Enable subquery caching
    pub enable_subquery_cache: bool,
}

/// Filter bitmap cache configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FilterCacheConfig {
    /// Enable filter cache
    pub enabled: bool,

    /// Memory allocation percentage
    pub memory_percentage: u8,

    /// Enable filter decomposition
    pub enable_decomposition: bool,

    /// Maximum filter complexity to cache
    pub max_filter_complexity: usize,

    /// Enable incremental updates
    pub enable_incremental_updates: bool,

    /// Bitmap compression
    pub enable_bitmap_compression: bool,
}

/// Index structure cache configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IndexCacheConfig {
    /// Enable index cache
    pub enabled: bool,

    /// Memory allocation percentage
    pub memory_percentage: u8,

    /// Cache hot index nodes
    pub cache_hot_nodes: bool,

    /// Hot node threshold (access count)
    pub hot_node_threshold: usize,

    /// Prefetch depth for index traversal
    pub prefetch_depth: usize,
}

/// Metadata cache configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MetadataStoreConfig {
    /// Enable metadata cache
    pub enabled: bool,

    /// Memory allocation percentage
    pub memory_percentage: u8,

    /// Cache all metadata
    pub cache_all: bool,

    /// Metadata types to cache
    pub cache_types: Vec<String>,

    /// TTL for metadata in seconds
    pub ttl_seconds: u64,
}

/// Cross-cache coordination configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CoordinationConfig {
    /// Enable cross-cache coordination
    pub enabled: bool,

    /// Pattern analysis for prefetching
    pub enable_pattern_analysis: bool,

    /// Pattern history size
    pub pattern_history_size: usize,

    /// Correlation threshold for prefetching
    pub correlation_threshold: f32,

    /// Enable memory rebalancing
    pub enable_memory_rebalancing: bool,

    /// Rebalance interval in seconds
    pub rebalance_interval_seconds: u64,

    /// Enable cascade invalidation
    pub enable_cascade_invalidation: bool,

    /// Maximum cascade depth
    pub max_cascade_depth: usize,

    /// Prefetch queue size
    pub prefetch_queue_size: usize,

    /// Prefetch worker threads
    pub prefetch_workers: usize,
}

/// Monitoring and observability configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MonitoringConfig {
    /// Enable metrics collection
    pub enable_metrics: bool,

    /// Metrics collection interval in seconds
    pub metrics_interval_seconds: u64,

    /// Enable detailed tracing
    pub enable_tracing: bool,

    /// Trace sampling rate (0.0 - 1.0)
    pub trace_sampling_rate: f64,

    /// Export metrics endpoint
    pub metrics_endpoint: Option<String>,

    /// Enable performance profiling
    pub enable_profiling: bool,

    /// Profile output path
    pub profile_output_path: Option<String>,

    /// Alert thresholds
    pub alert_thresholds: AlertThresholds,
}

/// Alert thresholds for monitoring
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AlertThresholds {
    /// Minimum hit rate before alerting
    pub min_hit_rate: f64,

    /// Maximum memory usage percentage
    pub max_memory_usage: f64,

    /// Maximum eviction rate per second
    pub max_eviction_rate: f64,

    /// Maximum invalidation cascade size
    pub max_cascade_size: usize,

    /// Maximum prefetch queue depth
    pub max_prefetch_queue: usize,
}

impl Default for GlobalCacheConfig {
    fn default() -> Self {
        Self {
            total_memory_mb: 1024,
            enabled: true,
            default_ttl_seconds: 3600,
            default_eviction_policy: EvictionPolicy::ARC,
            enable_tiered_storage: true,
            l2_storage_path: None,
            l3_storage_endpoint: None,
            compression: true,
            enable_warming: false,
            warming_source: None,
        }
    }
}

impl Default for VectorCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            memory_percentage: 40,
            max_entry_size_bytes: 1024 * 1024, // 1MB
            enable_similarity_prefetch: true,
            similarity_prefetch_radius: 0.9,
            prefetch_batch_size: 100,
            compression: false,
            compression_algorithm: None,
        }
    }
}

impl Default for QueryCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            memory_percentage: 30,
            max_result_size: 10000,
            enable_normalization: true,
            similarity_threshold: 0.95,
            ttl_seconds: 300,
            enable_subquery_cache: true,
        }
    }
}

impl Default for FilterCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            memory_percentage: 15,
            enable_decomposition: true,
            max_filter_complexity: 100,
            enable_incremental_updates: true,
            enable_bitmap_compression: true,
        }
    }
}

impl Default for IndexCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            memory_percentage: 10,
            cache_hot_nodes: true,
            hot_node_threshold: 100,
            prefetch_depth: 3,
        }
    }
}

impl Default for MetadataStoreConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            memory_percentage: 5,
            cache_all: false,
            cache_types: vec!["collection".to_string(), "schema".to_string()],
            ttl_seconds: 3600,
        }
    }
}

impl Default for CoordinationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            enable_pattern_analysis: true,
            pattern_history_size: 10000,
            correlation_threshold: 0.3,
            enable_memory_rebalancing: true,
            rebalance_interval_seconds: 300,
            enable_cascade_invalidation: true,
            max_cascade_depth: 10,
            prefetch_queue_size: 1000,
            prefetch_workers: 4,
        }
    }
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            enable_metrics: true,
            metrics_interval_seconds: 60,
            enable_tracing: false,
            trace_sampling_rate: 0.01,
            metrics_endpoint: None,
            enable_profiling: false,
            profile_output_path: None,
            alert_thresholds: AlertThresholds::default(),
        }
    }
}

impl Default for AlertThresholds {
    fn default() -> Self {
        Self {
            min_hit_rate: 0.5,
            max_memory_usage: 0.95,
            max_eviction_rate: 1000.0,
            max_cascade_size: 1000,
            max_prefetch_queue: 5000,
        }
    }
}

impl CacheConfig {
    /// Load configuration from TOML file
    pub fn from_file(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: CacheConfig = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    /// Save configuration to TOML file
    pub fn to_file(&self, path: &str) -> Result<()> {
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<()> {
        // Check memory percentages sum to 100
        let total_percentage = self.vector_data.memory_percentage as u16
            + self.query_result.memory_percentage as u16
            + self.filter_bitmap.memory_percentage as u16
            + self.index_structure.memory_percentage as u16
            + self.metadata.memory_percentage as u16;

        if total_percentage != 100 {
            return Err(anyhow::anyhow!(
                "Memory percentages must sum to 100, got {}",
                total_percentage
            ));
        }

        // Validate thresholds
        if self.monitoring.alert_thresholds.min_hit_rate < 0.0
            || self.monitoring.alert_thresholds.min_hit_rate > 1.0
        {
            return Err(anyhow::anyhow!("Invalid min_hit_rate threshold"));
        }

        if self.monitoring.trace_sampling_rate < 0.0 || self.monitoring.trace_sampling_rate > 1.0 {
            return Err(anyhow::anyhow!("Invalid trace_sampling_rate"));
        }

        Ok(())
    }

    /// Get total memory budget in bytes
    pub fn total_memory_bytes(&self) -> usize {
        self.global.total_memory_mb * 1024 * 1024
    }

    /// Get memory allocation for a specific cache type in bytes
    pub fn get_cache_memory_bytes(&self, cache_type: &str) -> usize {
        let percentage = match cache_type {
            "vector_data" => self.vector_data.memory_percentage,
            "query_result" => self.query_result.memory_percentage,
            "filter_bitmap" => self.filter_bitmap.memory_percentage,
            "index_structure" => self.index_structure.memory_percentage,
            "metadata_info" => self.metadata.memory_percentage,
            _ => 0,
        };

        self.total_memory_bytes() * percentage as usize / 100
    }

    /// Create example configuration file
    pub fn create_example_config(path: &str) -> Result<()> {
        let config = CacheConfig::default();
        config.to_file(path)
    }
}
