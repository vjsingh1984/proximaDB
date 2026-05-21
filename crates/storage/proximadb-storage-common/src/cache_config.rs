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
    pub enabled: bool,
    pub memory_percentage: u8,
    pub max_entry_size_bytes: usize,
    pub enable_similarity_prefetch: bool,
    pub similarity_prefetch_radius: f32,
    pub prefetch_batch_size: usize,
    pub compression: bool,
    pub compression_algorithm: Option<String>,
}

/// Query result cache configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QueryCacheConfig {
    pub enabled: bool,
    pub memory_percentage: u8,
    pub max_result_size: usize,
    pub enable_normalization: bool,
    pub similarity_threshold: f32,
    pub ttl_seconds: u64,
    pub enable_subquery_cache: bool,
}

/// Filter bitmap cache configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FilterCacheConfig {
    pub enabled: bool,
    pub memory_percentage: u8,
    pub enable_decomposition: bool,
    pub max_filter_complexity: usize,
    pub enable_incremental_updates: bool,
    pub enable_bitmap_compression: bool,
}

/// Index structure cache configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IndexCacheConfig {
    pub enabled: bool,
    pub memory_percentage: u8,
    pub cache_hot_nodes: bool,
    pub hot_node_threshold: usize,
    pub prefetch_depth: usize,
}

/// Metadata cache configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MetadataStoreConfig {
    pub enabled: bool,
    pub memory_percentage: u8,
    pub cache_all: bool,
    pub cache_types: Vec<String>,
    pub ttl_seconds: u64,
}

/// Cross-cache coordination configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CoordinationConfig {
    pub enabled: bool,
    pub enable_pattern_analysis: bool,
    pub pattern_history_size: usize,
    pub correlation_threshold: f32,
    pub enable_memory_rebalancing: bool,
    pub rebalance_interval_seconds: u64,
    pub enable_cascade_invalidation: bool,
    pub max_cascade_depth: usize,
    pub prefetch_queue_size: usize,
    pub prefetch_workers: usize,
}

/// Monitoring and observability configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MonitoringConfig {
    pub enable_metrics: bool,
    pub metrics_interval_seconds: u64,
    pub enable_tracing: bool,
    pub trace_sampling_rate: f64,
    pub metrics_endpoint: Option<String>,
    pub enable_profiling: bool,
    pub profile_output_path: Option<String>,
    pub alert_thresholds: AlertThresholds,
}

/// Alert thresholds for monitoring
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AlertThresholds {
    pub min_hit_rate: f64,
    pub max_memory_usage: f64,
    pub max_eviction_rate: f64,
    pub max_cascade_size: usize,
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
            max_entry_size_bytes: 1024 * 1024,
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
    pub fn from_file(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: CacheConfig = toml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    pub fn to_file(&self, path: &str) -> Result<()> {
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
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

    pub fn total_memory_bytes(&self) -> usize {
        self.global.total_memory_mb * 1024 * 1024
    }

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

    pub fn create_example_config(path: &str) -> Result<()> {
        let config = CacheConfig::default();
        config.to_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_cache_config_is_valid_and_budgeted_by_cache_type() {
        let config = CacheConfig::default();

        config.validate().unwrap();
        assert!(config.global.enabled);
        assert_eq!(config.total_memory_bytes(), 1024 * 1024 * 1024);
        let total = config.total_memory_bytes();
        assert_eq!(
            config.get_cache_memory_bytes("vector_data"),
            total * 40 / 100
        );
        assert_eq!(
            config.get_cache_memory_bytes("query_result"),
            total * 30 / 100
        );
        assert_eq!(
            config.get_cache_memory_bytes("filter_bitmap"),
            total * 15 / 100
        );
        assert_eq!(
            config.get_cache_memory_bytes("index_structure"),
            total * 10 / 100
        );
        assert_eq!(
            config.get_cache_memory_bytes("metadata_info"),
            total * 5 / 100
        );
        assert_eq!(config.get_cache_memory_bytes("unknown"), 0);
    }

    #[test]
    fn default_cache_subconfigs_capture_expected_production_policy() {
        let config = CacheConfig::default();

        assert!(matches!(
            config.global.default_eviction_policy,
            EvictionPolicy::ARC
        ));
        assert!(config.global.enable_tiered_storage);
        assert!(config.vector_data.enable_similarity_prefetch);
        assert_eq!(config.vector_data.similarity_prefetch_radius, 0.9);
        assert!(config.query_result.enable_normalization);
        assert!(config.query_result.enable_subquery_cache);
        assert!(config.filter_bitmap.enable_decomposition);
        assert!(config.index_structure.cache_hot_nodes);
        assert_eq!(
            config.metadata.cache_types,
            vec!["collection".to_string(), "schema".to_string()]
        );
        assert!(config.coordination.enable_memory_rebalancing);
        assert!(config.monitoring.enable_metrics);
        assert_eq!(config.monitoring.alert_thresholds.max_cascade_size, 1000);
    }

    #[test]
    fn validation_rejects_invalid_memory_budget_and_thresholds() {
        let mut config = CacheConfig::default();
        config.metadata.memory_percentage = 6;
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("Memory percentages must sum to 100, got 101")
        );

        let mut config = CacheConfig::default();
        config.monitoring.alert_thresholds.min_hit_rate = -0.01;
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("Invalid min_hit_rate threshold")
        );

        let mut config = CacheConfig::default();
        config.monitoring.alert_thresholds.min_hit_rate = 1.01;
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("Invalid min_hit_rate threshold")
        );

        let mut config = CacheConfig::default();
        config.monitoring.trace_sampling_rate = -0.01;
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("Invalid trace_sampling_rate")
        );

        let mut config = CacheConfig::default();
        config.monitoring.trace_sampling_rate = 1.01;
        assert!(
            config
                .validate()
                .unwrap_err()
                .to_string()
                .contains("Invalid trace_sampling_rate")
        );
    }

    #[test]
    fn cache_config_round_trips_through_toml_file() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let path = file.path().to_str().unwrap();

        let mut config = CacheConfig::default();
        config.global.total_memory_mb = 2048;
        config.global.l2_storage_path = Some("/var/lib/proximadb/cache-l2".to_string());
        config.monitoring.metrics_endpoint = Some("http://127.0.0.1:9090/metrics".to_string());

        config.to_file(path).unwrap();
        let restored = CacheConfig::from_file(path).unwrap();

        assert_eq!(restored.global.total_memory_mb, 2048);
        assert_eq!(
            restored.global.l2_storage_path.as_deref(),
            Some("/var/lib/proximadb/cache-l2")
        );
        assert_eq!(
            restored.monitoring.metrics_endpoint.as_deref(),
            Some("http://127.0.0.1:9090/metrics")
        );
        assert!(matches!(
            restored.global.default_eviction_policy,
            EvictionPolicy::ARC
        ));
    }

    #[test]
    fn example_config_writer_creates_a_valid_default_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.toml");
        let path = path.to_str().unwrap();

        CacheConfig::create_example_config(path).unwrap();
        let restored = CacheConfig::from_file(path).unwrap();

        restored.validate().unwrap();
        assert_eq!(restored.global.total_memory_mb, 1024);
    }
}
