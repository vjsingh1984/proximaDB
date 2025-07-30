use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{info, warn};

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
    /// Storage locations - each can host WriteBuffer, data, and indexes
    pub storage_locations: Vec<StorageLocation>,
    
    /// Single metadata URL for consistency (e.g., "file:///fast-ssd/proximadb/metadata")
    pub metadata_url: String,

    /// Assignment configuration
    #[serde(default)]
    pub assignment_config: AssignmentConfig,

    /// Storage engine configuration
    pub mmap_enabled: bool,
    pub sst_config: SstConfig,
    pub cache_size_mb: u64,
    // bloom_filter_bits removed - use bloom_filter_config instead
    pub bloom_filter_config: Option<BloomFilterConfig>,

    /// Filesystem optimization settings
    pub filesystem_config: FilesystemOptimizationConfig,
}

/// Storage location configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageLocation {
    /// Storage URL (e.g., "file:///nvme1/proximadb", "s3://bucket/proximadb")
    pub url: String,
    
    /// Weight for weighted distribution (default: 1)
    #[serde(default = "default_weight")]
    pub weight: u32,
    
    /// Tags for filtering (e.g., ["fast", "local"], ["cloud", "archive"])
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_weight() -> u32 {
    1
}

/// Assignment configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentConfig {
    /// Assignment strategy: "hash", "round-robin", "weighted"
    #[serde(default = "default_assignment_strategy")]
    pub strategy: String,
    
    /// Keep all collection data together (WAL, data, index on same location)
    #[serde(default = "default_affinity")]
    pub affinity: bool,
}

fn default_assignment_strategy() -> String {
    "hash".to_string()
}

fn default_affinity() -> bool {
    true
}

impl Default for AssignmentConfig {
    fn default() -> Self {
        Self {
            strategy: default_assignment_strategy(),
            affinity: default_affinity(),
        }
    }
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
pub struct FilesystemOptimizationConfig {
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

impl Default for FilesystemOptimizationConfig {
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

impl StorageConfig {
    /// Get storage URLs from locations
    pub fn get_storage_urls(&self) -> Vec<String> {
        self.storage_locations.iter().map(|loc| loc.url.clone()).collect()
    }
    
    /// Get WAL URLs derived from storage URLs
    pub fn get_write_buffer_urls(&self) -> Vec<String> {
        self.storage_locations.iter()
            .map(|loc| format!("{}/wal", loc.url.trim_end_matches('/')))
            .collect()
    }
    
    /// Get data URLs derived from storage URLs
    pub fn get_data_urls(&self) -> Vec<String> {
        self.storage_locations.iter()
            .map(|loc| format!("{}/data", loc.url.trim_end_matches('/')))
            .collect()
    }
    
    /// Get index URLs derived from storage URLs
    pub fn get_index_urls(&self) -> Vec<String> {
        self.storage_locations.iter()
            .map(|loc| format!("{}/index", loc.url.trim_end_matches('/')))
            .collect()
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            storage_locations: vec![
                StorageLocation {
                    url: "file:///data/proximadb/disk1".to_string(),
                    weight: 1,
                    tags: vec!["local".to_string()],
                },
                StorageLocation {
                    url: "file:///data/proximadb/disk2".to_string(),
                    weight: 1,
                    tags: vec!["local".to_string()],
                },
            ],
            metadata_url: "file:///data/proximadb/disk1/metadata".to_string(),
            assignment_config: AssignmentConfig::default(),
            mmap_enabled: true,
            sst_config: SstConfig::default(),
            cache_size_mb: 2048,
            // Use unified bloom filter config
            bloom_filter_config: Some(BloomFilterConfig::default()),
            filesystem_config: FilesystemOptimizationConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SstConfig {
    pub memtable_size_mb: u64,
    pub level_count: u8,
    pub compaction_threshold: u32,
    /// SSTable block size in KB. Configurable from TOML, defaults to 1MB.
    /// 
    /// **Performance Guidelines:**
    /// - **256-512KB**: Good for memory-constrained environments
    /// - **1MB**: Optimal for EC2 GP2/GP3 and modern SSDs (default)
    /// - **2-4MB**: Best for high-throughput workloads with ample memory
    /// 
    /// **Backward Compatibility:** 
    /// Changing this value during restarts is safe. Each SSTable block stores its own
    /// length prefix [block_len:4][block_data], so existing files continue to work.
    /// Mixed block sizes within the same system are fully supported.
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
    pub enable_write_buffer: bool,
    pub write_buffer_directory: String,
    pub data_directory: String,
    pub mmap_enabled: bool,
    pub prefetch_enabled: bool,
    pub prefetch_size_kb: u32,
}

// BloomFilterConfig moved to core::bloom module for polymorphic design
// Re-export for backward compatibility
pub use crate::core::bloom::BloomFilterConfig;

impl Default for SstConfig {
    fn default() -> Self {
        Self {
            memtable_size_mb: 64,
            level_count: 7,
            compaction_threshold: 4,
            block_size_kb: 1024, // 1MB blocks optimized for EC2 GP2/GP3 IOPS and modern storage
            memory_flush_size_bytes: 64 * 1024 * 1024, // 64MB
            memtable_type: "skiplist".to_string(),
            compaction_strategy: "leveled".to_string(),
            compression: "snappy".to_string(),
            bloom_filter_config: Some(BloomFilterConfig::default()),
            cache_size_mb: 128,
            write_buffer_size_mb: 64,
            max_files_per_level: 10,
            level_size_multiplier: 10.0,
            max_levels: 7,
            background_thread_count: 4,
            sync_mode: "sync".to_string(),
            enable_write_buffer: true,
            write_buffer_directory: "./sst_wal".to_string(),
            data_directory: "./sst_data".to_string(),
            mmap_enabled: true,
            prefetch_enabled: true,
            prefetch_size_kb: 64,
        }
    }
}

impl SstConfig {
    /// Validate SST configuration parameters for optimal performance and correctness
    /// 
    /// Note: block_size_kb changes are backward compatible since each SSTable block
    /// stores its own length prefix [block_len:4][block_data], allowing mixed block sizes.
    pub fn validate(&self) -> Result<(), String> {
        if self.memtable_size_mb == 0 {
            return Err("memtable_size_mb must be greater than 0".to_string());
        }
        if self.level_count == 0 {
            return Err("level_count must be greater than 0".to_string());
        }
        if self.compaction_threshold == 0 {
            return Err("compaction_threshold must be greater than 0".to_string());
        }
        
        // Validate block size for optimal performance and storage compatibility
        if self.block_size_kb < 4 {
            return Err("block_size_kb must be at least 4KB for reasonable I/O performance".to_string());
        }
        if self.block_size_kb > 16 * 1024 {
            return Err("block_size_kb should not exceed 16MB to avoid excessive memory usage per block".to_string());
        }
        
        // Performance recommendations for common deployment scenarios
        match self.block_size_kb {
            1024 => {
                // 1MB - Optimal for EC2 GP2/GP3 and modern SSDs
                info!("block_size_kb=1MB - Optimized for EC2 GP2/GP3 and modern storage IOPS");
            }
            256 | 512 => {
                // Good for memory-constrained environments
                info!("block_size_kb={}KB - Good for memory-constrained deployments", self.block_size_kb);
            }
            2048 | 4096 => {
                // Good for high-throughput scenarios
                info!("block_size_kb={}KB - Optimized for high-throughput workloads", self.block_size_kb);
            }
            _ if self.block_size_kb < 256 => {
                warn!("block_size_kb={}KB - Consider 256KB+ for better I/O efficiency", self.block_size_kb);
            }
            _ if self.block_size_kb > 4096 => {
                warn!("block_size_kb={}KB - Very large blocks may increase memory pressure", self.block_size_kb);
            }
            _ => {
                // Any other size is fine
            }
        }
        
        Ok(())
    }
    
    /// Get block size in bytes for internal use
    pub fn block_size_bytes(&self) -> usize {
        (self.block_size_kb as usize) * 1024
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
    pub write_buffer_urls: Vec<String>,

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
            write_buffer_urls: vec!["file://./data/wal".to_string()],
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
