//! Typed configuration contracts shared across ProximaDB workspace crates.
//!
//! Keep this crate limited to serializable configuration shapes. Runtime conversion and service
//! bootstrap stay in platform/root layers until those boundaries are independently extracted.

use serde::{Deserialize, Serialize};

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
}
