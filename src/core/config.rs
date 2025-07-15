use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub storage: StorageConfig,
    pub consensus: ConsensusConfig,
    pub api: ApiConfig,
    pub monitoring: MonitoringConfig,
    pub tls: Option<TlsConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    pub cert_file: Option<String>,
    pub key_file: Option<String>,
    pub enabled: bool,
    pub bind_interface: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub node_id: String,
    pub bind_address: String,
    pub port: u16,
    pub data_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Legacy fields (deprecated, use storage_layout instead)
    pub data_dirs: Vec<PathBuf>,
    pub wal_dir: PathBuf,

    /// Modern WAL configuration with multi-disk support
    #[serde(default)]
    pub wal_config: WalStorageConfig,

    /// ProximaDB hierarchical storage layout configuration
    pub storage_layout: crate::core::storage_layout::StorageLayoutConfig,

    /// Storage engine configuration
    pub mmap_enabled: bool,
    pub lsm_config: LsmConfig,
    pub cache_size_mb: u64,
    pub bloom_filter_bits: u32,
    pub bloom_filter_config: Option<BloomFilterConfig>,

    /// Filesystem optimization settings
    pub filesystem_config: FilesystemConfig,

    /// Metadata backend configuration
    pub metadata_backend: Option<MetadataBackendConfig>,
}

/// Metadata backend configuration for cloud and local storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataBackendConfig {
    /// Backend type (filestore, memory)
    pub backend_type: String,

    /// Storage URL (file://, s3://, adls://, gcs://)
    pub storage_url: String,

    /// Cloud-specific configuration
    pub cloud_config: Option<CloudStorageConfig>,

    /// Performance settings
    pub cache_size_mb: Option<u64>,
    pub flush_interval_secs: Option<u64>,
}

/// Cloud storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudStorageConfig {
    /// AWS S3 configuration
    pub s3_config: Option<S3Config>,

    /// Azure Blob Storage configuration
    pub azure_config: Option<AzureConfig>,

    /// Google Cloud Storage configuration
    pub gcs_config: Option<GcsConfig>,
}

/// AWS S3 configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct S3Config {
    pub region: String,
    pub bucket: String,
    pub access_key_id: Option<String>,
    pub secret_access_key: Option<String>,
    pub use_iam_role: bool,
    pub endpoint: Option<String>, // For S3-compatible stores
}

/// Azure Blob Storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AzureConfig {
    pub account_name: String,
    pub container: String,
    pub access_key: Option<String>,
    pub sas_token: Option<String>,
    pub use_managed_identity: bool,
}

/// Google Cloud Storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GcsConfig {
    pub project_id: String,
    pub bucket: String,
    pub service_account_path: Option<String>,
    pub use_workload_identity: bool,
}

/// Filesystem configuration for performance optimization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemConfig {
    /// Enable write strategy caching
    pub enable_write_strategy_cache: bool,

    /// Temp directory configuration
    pub temp_strategy: TempStrategy,

    /// Atomic operations configuration
    pub atomic_config: AtomicOperationsConfig,
}

/// Temp strategy configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TempStrategy {
    /// Same directory temp (recommended for local filesystem)
    SameDirectory,

    /// Configured temp directory
    ConfiguredTemp { temp_dir: String },

    /// System temp directory (fallback)
    SystemTemp,
}

/// Atomic operations configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AtomicOperationsConfig {
    /// Enable atomic writes for local filesystem
    pub enable_local_atomic: bool,

    /// Enable write-temp-rename for object stores
    pub enable_object_store_atomic: bool,

    /// Cleanup temp files on startup
    pub cleanup_temp_on_startup: bool,
}

impl Default for FilesystemConfig {
    fn default() -> Self {
        Self {
            enable_write_strategy_cache: true,
            temp_strategy: TempStrategy::SameDirectory,
            atomic_config: AtomicOperationsConfig::default(),
        }
    }
}

impl Default for AtomicOperationsConfig {
    fn default() -> Self {
        Self {
            enable_local_atomic: true,
            enable_object_store_atomic: true,
            cleanup_temp_on_startup: true,
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            data_dirs: vec![
                PathBuf::from("/data/proximadb/1"),
                PathBuf::from("/data/proximadb/2"),
            ],
            wal_dir: PathBuf::from("/data/proximadb/1/wal"),
            wal_config: WalStorageConfig::default(),
            storage_layout: crate::core::storage_layout::StorageLayoutConfig::default_2_disk(),
            mmap_enabled: true,
            lsm_config: LsmConfig::default(),
            cache_size_mb: 2048,
            bloom_filter_bits: 12,
            bloom_filter_config: Some(BloomFilterConfig {
                bits_per_key: 10,
                enabled: true,
            }),
            filesystem_config: FilesystemConfig::default(),
            metadata_backend: None, // Use default filestore backend
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LsmConfig {
    pub memtable_size_mb: u64,
    pub level_count: u8,
    pub compaction_threshold: u32,
    pub block_size_kb: u32,
    pub memory_flush_size_bytes: usize,
    pub memtable_type: String,
    pub compaction_strategy: String,
    pub compression: String,
    pub bloom_filter_config: Option<BloomFilterConfig>,
    pub cache_size_mb: u64,
    pub write_buffer_size_mb: u64,
    pub max_files_per_level: u32,
    pub level_size_multiplier: f64,
    pub max_levels: u8,
    pub background_thread_count: u32,
    pub sync_mode: String,
    pub enable_wal: bool,
    pub wal_directory: String,
    pub data_directory: String,
    pub mmap_enabled: bool,
    pub prefetch_enabled: bool,
    pub prefetch_size_kb: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BloomFilterConfig {
    pub bits_per_key: u32,
    pub enabled: bool,
}

impl Default for LsmConfig {
    fn default() -> Self {
        Self {
            memtable_size_mb: 64,
            level_count: 7,
            compaction_threshold: 4,
            block_size_kb: 64,
            memory_flush_size_bytes: 64 * 1024 * 1024, // 64MB
            memtable_type: "skiplist".to_string(),
            compaction_strategy: "leveled".to_string(),
            compression: "snappy".to_string(),
            bloom_filter_config: Some(BloomFilterConfig {
                bits_per_key: 10,
                enabled: true,
            }),
            cache_size_mb: 128,
            write_buffer_size_mb: 64,
            max_files_per_level: 10,
            level_size_multiplier: 10.0,
            max_levels: 7,
            background_thread_count: 4,
            sync_mode: "sync".to_string(),
            enable_wal: true,
            wal_directory: "./lsm_wal".to_string(),
            data_directory: "./lsm_data".to_string(),
            mmap_enabled: true,
            prefetch_enabled: true,
            prefetch_size_kb: 64,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusConfig {
    pub node_id: Option<u64>,
    pub cluster_peers: Vec<String>,
    pub election_timeout_ms: u64,
    pub heartbeat_interval_ms: u64,
    pub snapshot_threshold: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub grpc_port: u16,
    pub rest_port: u16,
    pub max_request_size_mb: u64,
    pub timeout_seconds: u64,
    pub enable_tls: Option<bool>,
}

/// WAL storage configuration supporting multiple directories and cloud storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalStorageConfig {
    /// WAL storage URLs - supports file://, s3://, adls://, gcs://
    /// Multiple URLs enable multi-disk performance scaling
    pub wal_urls: Vec<String>,

    /// Distribution strategy for collections across WAL directories
    #[serde(default)]
    pub distribution_strategy: WalDistributionStrategy,

    /// Whether to keep each collection on a single WAL directory
    #[serde(default = "default_collection_affinity")]
    pub collection_affinity: bool,

    /// Memory flush threshold per collection (bytes)
    #[serde(default = "default_memory_flush_size")]
    pub memory_flush_size_bytes: usize,

    /// Global WAL size threshold for forced flush (bytes)
    #[serde(default = "default_global_flush_threshold")]
    pub global_flush_threshold: usize,

    /// WAL strategy type (Avro vs Bincode)
    #[serde(default = "default_strategy_type")]
    pub strategy_type: Option<String>,

    /// Memtable type for memory structure
    #[serde(default = "default_memtable_type")]
    pub memtable_type: Option<String>,

    /// Sync mode for durability vs performance tradeoff
    #[serde(default = "default_sync_mode")]
    pub sync_mode: Option<String>,

    /// Batch threshold for operations
    #[serde(default = "default_batch_threshold")]
    pub batch_threshold: Option<usize>,

    /// Write buffer size in MB
    #[serde(default = "default_write_buffer_size_mb")]
    pub write_buffer_size_mb: Option<usize>,

    /// Maximum concurrent flush operations
    #[serde(default = "default_concurrent_flushes")]
    pub concurrent_flushes: Option<usize>,

    /// Shrink factor for global threshold management (percentage)
    /// When global threshold is exceeded, flush collections until memory usage drops to this percentage
    #[serde(default = "default_global_shrink_factor")]
    pub global_shrink_factor: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WalDistributionStrategy {
    /// Round-robin across WAL directories
    RoundRobin,
    /// Hash-based distribution (consistent)
    Hash,
    /// Load-balanced distribution (dynamic)
    LoadBalanced,
}

impl Default for WalDistributionStrategy {
    fn default() -> Self {
        Self::LoadBalanced
    }
}

impl Default for WalStorageConfig {
    fn default() -> Self {
        Self {
            wal_urls: vec!["file:///workspace/data/wal".to_string()],
            distribution_strategy: WalDistributionStrategy::LoadBalanced,
            collection_affinity: true,
            memory_flush_size_bytes: 10 * 1024 * 1024,  // 10MB - recommended for collection-level flush
            global_flush_threshold: 4 * 1024 * 1024 * 1024, // 4GB - recommended for global memory threshold
            strategy_type: None,                       // Use WAL defaults
            memtable_type: None,                       // Use WAL defaults
            sync_mode: None,                           // Use WAL defaults
            batch_threshold: None,                     // Use WAL defaults
            write_buffer_size_mb: None,                // Use WAL defaults
            concurrent_flushes: None,                  // Use WAL defaults
            global_shrink_factor: Some(0.4),           // 40% shrink factor - recommended
        }
    }
}

// Helper functions for serde defaults
fn default_collection_affinity() -> bool {
    true
}
fn default_memory_flush_size() -> usize {
    2 * 1024 * 1024 // 2MB - reduced for faster recovery as per CLAUDE.md
}
fn default_global_flush_threshold() -> usize {
    4 * 1024 * 1024 * 1024 // 4GB - recommended for global memory threshold
}
fn default_strategy_type() -> Option<String> {
    None
}
fn default_memtable_type() -> Option<String> {
    None
}
fn default_sync_mode() -> Option<String> {
    None
}
fn default_batch_threshold() -> Option<usize> {
    None
}
fn default_write_buffer_size_mb() -> Option<usize> {
    None
}
fn default_concurrent_flushes() -> Option<usize> {
    None
}
fn default_global_shrink_factor() -> Option<f64> {
    Some(0.4) // 40% shrink factor - recommended for global threshold management
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitoringConfig {
    pub metrics_enabled: bool,
    pub log_level: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                node_id: "proximadb-node-1".to_string(),
                bind_address: "0.0.0.0".to_string(),
                port: 5678,
                data_dir: PathBuf::from("/data/proximadb/1"),
            },
            storage: StorageConfig::default(),
            consensus: ConsensusConfig {
                node_id: Some(1),
                cluster_peers: vec![],
                election_timeout_ms: 5000,
                heartbeat_interval_ms: 1000,
                snapshot_threshold: 1000,
            },
            api: ApiConfig {
                grpc_port: 5679,
                rest_port: 5678,
                max_request_size_mb: 64,
                timeout_seconds: 30,
                enable_tls: Some(false),
            },
            monitoring: MonitoringConfig {
                metrics_enabled: true,
                log_level: "info".to_string(),
            },
            tls: None,
        }
    }
}
