//! Typed configuration contracts shared across ProximaDB workspace crates.
//!
//! Keep this crate limited to serializable configuration shapes. Runtime conversion and service
//! bootstrap stay in platform/root layers until those boundaries are independently extracted.

use serde::{Deserialize, Serialize};

/// TLS transport security configuration shared by protocol listeners.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    /// Path to the PEM-encoded TLS certificate file.
    pub cert_file: Option<String>,

    /// Path to the PEM-encoded TLS private key file.
    pub key_file: Option<String>,

    /// Whether TLS is enabled.
    pub enabled: bool,

    /// Network interface to bind the TLS listener to.
    pub bind_interface: Option<String>,
}

impl Default for TlsConfig {
    fn default() -> Self {
        Self {
            cert_file: None,
            key_file: None,
            enabled: false,
            bind_interface: None,
        }
    }
}

/// Hardware acceleration configuration controlling SIMD and GPU features.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareConfig {
    /// Enable automatic hardware detection.
    pub enable_detection: bool,

    /// Enable GPU acceleration if detected.
    pub enable_gpu_acceleration: bool,

    /// Enable SIMD acceleration if detected.
    pub enable_simd: bool,

    /// Enable AVX-512 if available.
    pub enable_avx512: bool,

    /// Enable GPU for SQL parsing.
    pub enable_gpu_parsing: bool,

    /// Enable GPU for distance calculations.
    pub enable_gpu_similarity: bool,

    /// Minimum vector size to use GPU.
    pub gpu_min_vector_size: usize,

    /// Minimum batch size to use GPU.
    pub gpu_min_batch_size: usize,
}

impl Default for HardwareConfig {
    fn default() -> Self {
        Self {
            enable_detection: true,
            enable_gpu_acceleration: true,
            enable_simd: true,
            enable_avx512: true,
            enable_gpu_parsing: true,
            enable_gpu_similarity: true,
            gpu_min_vector_size: 64,
            gpu_min_batch_size: 100,
        }
    }
}

/// Metadata backend configuration for cloud and local storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataBackendConfig {
    /// Backend type (filestore, memory).
    pub backend_type: String,

    /// Storage URL (file://, s3://, adls://, gcs://).
    pub storage_url: String,

    /// Cloud-specific configuration.
    pub cloud_config: Option<CloudStorageConfig>,

    /// In-memory cache size in megabytes for metadata.
    pub cache_size_mb: Option<u64>,

    /// Interval in seconds between metadata flush operations.
    pub flush_interval_secs: Option<u64>,
}

impl Default for MetadataBackendConfig {
    fn default() -> Self {
        Self {
            backend_type: "filestore".to_string(),
            storage_url: "file://./metadata".to_string(),
            cloud_config: None,
            cache_size_mb: Some(256),
            flush_interval_secs: Some(60),
        }
    }
}

/// Cloud storage configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudStorageConfig {
    /// AWS S3 configuration.
    pub s3_config: Option<S3Config>,

    /// Azure Blob Storage configuration.
    pub azure_config: Option<AzureConfig>,

    /// Google Cloud Storage configuration.
    pub gcs_config: Option<GcsConfig>,
}

/// AWS S3 configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Config {
    /// AWS region (e.g., "us-east-1").
    pub region: String,
    /// S3 bucket name.
    pub bucket: String,
    /// AWS access key ID (optional if using IAM role).
    pub access_key_id: Option<String>,
    /// AWS secret access key (optional if using IAM role).
    pub secret_access_key: Option<String>,
    /// Use IAM role-based authentication instead of static keys.
    pub use_iam_role: bool,
    /// Custom S3-compatible endpoint URL (e.g., MinIO).
    pub endpoint: Option<String>,
}

/// Azure Blob Storage configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AzureConfig {
    /// Azure Storage account name.
    pub account_name: String,
    /// Blob container name.
    pub container: String,
    /// Storage account access key (optional if using managed identity).
    pub access_key: Option<String>,
    /// Shared Access Signature token (optional).
    pub sas_token: Option<String>,
    /// Use Azure Managed Identity for authentication.
    pub use_managed_identity: bool,
}

/// Google Cloud Storage configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcsConfig {
    /// Google Cloud project ID.
    pub project_id: String,
    /// GCS bucket name.
    pub bucket: String,
    /// Path to the service account JSON key file.
    pub service_account_path: Option<String>,
    /// Use GKE Workload Identity for authentication.
    pub use_workload_identity: bool,
}

/// Filesystem configuration for performance optimization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemOptimizationConfig {
    /// Enable write strategy caching.
    pub enable_write_strategy_cache: bool,

    /// Temp directory configuration.
    pub temp_strategy: TempStrategy,

    /// Atomic operations configuration.
    pub atomic_config: TransactionalOperationsConfig,
}

/// Temp strategy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TempStrategy {
    /// Same directory temp (recommended for local filesystem).
    SameDirectory,

    /// Configured temp directory.
    ConfiguredTemp {
        /// Path to the custom temporary directory.
        temp_dir: String,
    },

    /// System temp directory (fallback).
    SystemTemp,
}

/// Atomic operations configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionalOperationsConfig {
    /// Enable atomic writes for local filesystem.
    pub enable_local_atomic: bool,

    /// Enable write-temp-rename for object stores.
    pub enable_object_store_atomic: bool,

    /// Cleanup temp files on startup.
    pub cleanup_temp_on_startup: bool,
}

impl Default for FilesystemOptimizationConfig {
    fn default() -> Self {
        Self {
            enable_write_strategy_cache: true,
            temp_strategy: TempStrategy::SameDirectory,
            atomic_config: TransactionalOperationsConfig::default(),
        }
    }
}

impl Default for TransactionalOperationsConfig {
    fn default() -> Self {
        Self {
            enable_local_atomic: true,
            enable_object_store_atomic: true,
            cleanup_temp_on_startup: true,
        }
    }
}

/// Observability and monitoring configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitoringConfig {
    /// Whether Prometheus metrics collection is enabled.
    pub metrics_enabled: bool,

    /// Default tracing log level (e.g., "info", "debug", "trace").
    pub log_level: String,

    /// Dashboard refresh interval in seconds.
    #[serde(default = "default_dashboard_refresh_interval")]
    pub dashboard_refresh_interval_seconds: u64,
}

fn default_dashboard_refresh_interval() -> u64 {
    60
}

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            metrics_enabled: true,
            log_level: "info".to_string(),
            dashboard_refresh_interval_seconds: 60,
        }
    }
}

impl MonitoringConfig {
    /// Get dashboard refresh interval, ensuring it is at least 15 seconds.
    pub fn dashboard_refresh_interval(&self) -> u64 {
        self.dashboard_refresh_interval_seconds.max(15)
    }
}

/// Storage location configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageLocation {
    /// Storage URL (e.g., "file:///nvme1/proximadb", "s3://bucket/proximadb").
    pub url: String,

    /// Weight for weighted distribution.
    pub weight: u32,

    /// Tags for filtering (e.g., ["fast", "local"], ["cloud", "archive"]).
    pub tags: Vec<String>,
}

impl Default for StorageLocation {
    fn default() -> Self {
        Self {
            url: "file://./data".to_string(),
            weight: 1,
            tags: vec!["local".to_string()],
        }
    }
}

/// Assignment configuration for placing collection data across storage locations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentConfig {
    /// Assignment strategy: "hash", "round-robin", "weighted".
    pub strategy: String,

    /// Keep all collection data together (WAL, data, index on same location).
    pub affinity: bool,
}

impl Default for AssignmentConfig {
    fn default() -> Self {
        Self {
            strategy: "hash".to_string(),
            affinity: true,
        }
    }
}

/// Common compaction configuration shared across storage engines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    /// L0 file count threshold for compaction.
    pub l0_file_threshold: usize,

    /// L0 size threshold in MB for compaction.
    pub l0_size_threshold_mb: usize,

    /// Multiplier for higher level thresholds.
    pub level_multiplier: f64,

    /// Maximum number of levels.
    pub max_levels: u8,

    /// Compaction strategy: "count", "size", or "hybrid".
    pub strategy: String,

    /// Target output file size in MB for size-based compaction.
    pub target_file_size_mb: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            l0_file_threshold: 5,
            l0_size_threshold_mb: 256,
            level_multiplier: 2.0,
            max_levels: 7,
            strategy: "hybrid".to_string(),
            target_file_size_mb: 128,
        }
    }
}

/// Performance optimization configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationConfig {
    /// Enable memory-mapped I/O for large files.
    #[serde(default = "default_enable_mmap")]
    pub enable_mmap: bool,

    /// Enable zone map pruning to skip irrelevant blocks.
    #[serde(default = "default_enable_zone_map_pruning")]
    pub enable_zone_map_pruning: bool,

    /// Enable AXIS indexes for approximate nearest neighbor search.
    #[serde(default = "default_enable_axis_indexes")]
    pub enable_axis_indexes: bool,

    /// Default index type for new collections: flat, hnsw, ivf, lsh.
    #[serde(default = "default_index_type")]
    pub default_index_type: String,

    /// Enable progressive quantization search (Binary -> INT8 -> FP32).
    #[serde(default = "default_enable_progressive_search")]
    pub enable_progressive_search: bool,

    /// Enable block-level bloom filters for metadata filtering.
    #[serde(default = "default_enable_bloom_filters")]
    pub enable_bloom_filters: bool,
}

fn default_enable_mmap() -> bool {
    true
}

fn default_enable_zone_map_pruning() -> bool {
    true
}

fn default_enable_axis_indexes() -> bool {
    true
}

fn default_index_type() -> String {
    "hnsw".to_string()
}

fn default_enable_progressive_search() -> bool {
    true
}

fn default_enable_bloom_filters() -> bool {
    true
}

impl Default for OptimizationConfig {
    fn default() -> Self {
        Self {
            enable_mmap: default_enable_mmap(),
            enable_zone_map_pruning: default_enable_zone_map_pruning(),
            enable_axis_indexes: default_enable_axis_indexes(),
            default_index_type: default_index_type(),
            enable_progressive_search: default_enable_progressive_search(),
            enable_bloom_filters: default_enable_bloom_filters(),
        }
    }
}

/// Configuration for search pruning, allowing for simple or advanced setup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum PruneModeConfig {
    /// Simple pruning mode specified by a single strategy name.
    Simple(String),

    /// Advanced pruning mode with fine-grained control.
    Advanced(AdvancedPruneConfig),
}

/// Advanced configuration for search pruning.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdvancedPruneConfig {
    /// Pruning algorithm type (e.g., "sqrt", "log").
    #[serde(default = "default_prune_type")]
    pub r#type: String,

    /// Minimum number of candidates to keep after pruning.
    pub min_keep: Option<usize>,

    /// Maximum number of candidates to keep after pruning.
    pub max_keep: Option<usize>,

    /// Pruning ratio controlling aggressiveness (0.0 to 1.0).
    pub ratio: Option<f32>,
}

fn default_prune_type() -> String {
    "sqrt".to_string()
}

impl Default for AdvancedPruneConfig {
    fn default() -> Self {
        Self {
            r#type: default_prune_type(),
            min_keep: None,
            max_keep: None,
            ratio: None,
        }
    }
}

/// WAL storage configuration supporting multiple directories and cloud storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalStorageConfig {
    /// Distribution strategy for collections across storage locations.
    pub distribution_strategy: WalDistributionStrategy,

    /// Whether to keep each collection on a single WAL directory.
    pub collection_affinity: bool,

    /// Memory flush threshold per collection (bytes).
    pub memory_flush_size_bytes: usize,

    /// Global WAL size threshold for forced flush (bytes).
    pub global_flush_threshold: usize,

    /// WAL strategy type (Avro vs Bincode).
    pub strategy_type: Option<String>,

    /// Memtable type for memory structure.
    pub memtable_type: Option<String>,

    /// Sync mode for durability vs performance tradeoff.
    pub sync_mode: Option<String>,

    /// Batch threshold for operations.
    pub batch_threshold: Option<usize>,

    /// Write buffer size in MB.
    pub write_buffer_size_mb: Option<usize>,

    /// Maximum concurrent flush operations.
    pub concurrent_flushes: Option<usize>,

    /// Shrink factor for global threshold management (percentage).
    pub global_shrink_factor: Option<f64>,

    /// Global manifest location (optional - explicit configuration).
    pub global_manifest_url: Option<String>,
}

/// Strategy for distributing WAL segments across multiple storage directories.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum WalDistributionStrategy {
    /// Round-robin across WAL directories.
    RoundRobin,
    /// Hash-based distribution (consistent).
    Hash,
    /// Load-balanced distribution (dynamic).
    #[default]
    LoadBalanced,
}

impl Default for WalStorageConfig {
    fn default() -> Self {
        Self {
            global_manifest_url: None,
            distribution_strategy: WalDistributionStrategy::LoadBalanced,
            collection_affinity: true,
            memory_flush_size_bytes: 10 * 1024 * 1024,
            global_flush_threshold: 4 * 1024 * 1024 * 1024,
            strategy_type: None,
            memtable_type: None,
            sync_mode: None,
            batch_threshold: None,
            write_buffer_size_mb: None,
            concurrent_flushes: None,
            global_shrink_factor: Some(0.4),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wal_storage_defaults_match_root_runtime_expectations() {
        let config = WalStorageConfig::default();

        assert!(matches!(
            config.distribution_strategy,
            WalDistributionStrategy::LoadBalanced
        ));
        assert!(config.collection_affinity);
        assert_eq!(config.memory_flush_size_bytes, 10 * 1024 * 1024);
        assert_eq!(config.global_flush_threshold, 4 * 1024 * 1024 * 1024);
        assert_eq!(config.global_shrink_factor, Some(0.4));
    }

    #[test]
    fn hardware_defaults_match_root_runtime_expectations() {
        let config = HardwareConfig::default();

        assert!(config.enable_detection);
        assert!(config.enable_gpu_acceleration);
        assert!(config.enable_simd);
        assert!(config.enable_avx512);
        assert!(config.enable_gpu_parsing);
        assert!(config.enable_gpu_similarity);
        assert_eq!(config.gpu_min_vector_size, 64);
        assert_eq!(config.gpu_min_batch_size, 100);
    }

    #[test]
    fn metadata_backend_defaults_match_root_runtime_expectations() {
        let config = MetadataBackendConfig::default();

        assert_eq!(config.backend_type, "filestore");
        assert_eq!(config.storage_url, "file://./metadata");
        assert!(config.cloud_config.is_none());
        assert_eq!(config.cache_size_mb, Some(256));
        assert_eq!(config.flush_interval_secs, Some(60));
    }

    #[test]
    fn filesystem_optimization_defaults_match_root_runtime_expectations() {
        let config = FilesystemOptimizationConfig::default();

        assert!(config.enable_write_strategy_cache);
        assert!(matches!(config.temp_strategy, TempStrategy::SameDirectory));
        assert!(config.atomic_config.enable_local_atomic);
        assert!(config.atomic_config.enable_object_store_atomic);
        assert!(config.atomic_config.cleanup_temp_on_startup);
    }

    #[test]
    fn tls_defaults_match_root_runtime_expectations() {
        let config = TlsConfig::default();

        assert!(!config.enabled);
        assert!(config.cert_file.is_none());
        assert!(config.key_file.is_none());
        assert!(config.bind_interface.is_none());
    }

    #[test]
    fn monitoring_defaults_match_root_runtime_expectations() {
        let config = MonitoringConfig::default();

        assert!(config.metrics_enabled);
        assert_eq!(config.log_level, "info");
        assert_eq!(config.dashboard_refresh_interval_seconds, 60);
        assert_eq!(config.dashboard_refresh_interval(), 60);
    }

    #[test]
    fn monitoring_dashboard_refresh_interval_has_minimum() {
        let config = MonitoringConfig {
            dashboard_refresh_interval_seconds: 1,
            ..MonitoringConfig::default()
        };

        assert_eq!(config.dashboard_refresh_interval(), 15);
    }

    #[test]
    fn storage_location_defaults_match_root_runtime_expectations() {
        let config = StorageLocation::default();

        assert_eq!(config.url, "file://./data");
        assert_eq!(config.weight, 1);
        assert_eq!(config.tags, vec!["local"]);
    }

    #[test]
    fn assignment_defaults_match_root_runtime_expectations() {
        let config = AssignmentConfig::default();

        assert_eq!(config.strategy, "hash");
        assert!(config.affinity);
    }

    #[test]
    fn compaction_defaults_match_root_runtime_expectations() {
        let config = CompactionConfig::default();

        assert_eq!(config.l0_file_threshold, 5);
        assert_eq!(config.l0_size_threshold_mb, 256);
        assert_eq!(config.level_multiplier, 2.0);
        assert_eq!(config.max_levels, 7);
        assert_eq!(config.strategy, "hybrid");
        assert_eq!(config.target_file_size_mb, 128);
    }

    #[test]
    fn optimization_defaults_match_root_runtime_expectations() {
        let config = OptimizationConfig::default();

        assert!(config.enable_mmap);
        assert!(config.enable_zone_map_pruning);
        assert!(config.enable_axis_indexes);
        assert_eq!(config.default_index_type, "hnsw");
        assert!(config.enable_progressive_search);
        assert!(config.enable_bloom_filters);
    }

    #[test]
    fn advanced_prune_defaults_match_root_runtime_expectations() {
        let config = AdvancedPruneConfig::default();

        assert_eq!(config.r#type, "sqrt");
        assert!(config.min_keep.is_none());
        assert!(config.max_keep.is_none());
        assert!(config.ratio.is_none());
    }
}
