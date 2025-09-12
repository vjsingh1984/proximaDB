use crate::network::NetworkConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub storage: StorageConfig,
    pub consensus: ConsensusConfig,
    pub api: ApiConfig,
    pub monitoring: MonitoringConfig,
    pub network: Option<NetworkConfig>,
    pub tls: Option<TlsConfig>,
    pub hardware: Option<HardwareConfig>,
    pub sks: Option<SksConfig>,
    /// Global cache runtime configuration (optional)
    pub cache: Option<CacheRuntimeConfig>,
    /// Graph runtime configuration (optional)
    pub graph: Option<GraphRuntimeConfig>,
    /// Hybrid query runtime configuration (optional)
    pub hybrid: Option<HybridRuntimeConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    pub cert_file: Option<String>,
    pub key_file: Option<String>,
    pub enabled: bool,
    pub bind_interface: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareConfig {
    /// Enable automatic hardware detection (default: true)
    pub enable_detection: bool,

    /// Enable GPU acceleration if detected (default: true)
    pub enable_gpu_acceleration: bool,

    /// Enable SIMD acceleration if detected (default: true)
    pub enable_simd: bool,

    /// Enable AVX-512 if available (default: true)
    pub enable_avx512: bool,

    /// Enable GPU for SQL parsing (default: true)
    pub enable_gpu_parsing: bool,

    /// Enable GPU for distance calculations (default: true)
    pub enable_gpu_similarity: bool,

    /// Minimum vector size to use GPU (default: 64)
    pub gpu_min_vector_size: usize,

    /// Minimum batch size to use GPU (default: 100)
    pub gpu_min_batch_size: usize,
}

fn default_true() -> bool {
    true
}

fn default_gpu_min_vector_size() -> usize {
    64
}

fn default_gpu_min_batch_size() -> usize {
    100
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

/// Semantic Knowledge Store (SKS) configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SksConfig {
    /// Enable SKS features (default: false)
    pub enabled: bool,

    /// Enable entity storage (default: true when SKS enabled)
    pub enable_entities: bool,

    /// Enable graph relationships (default: true when SKS enabled)
    pub enable_relations: bool,

    /// Enable provenance tracking (default: true when SKS enabled)
    pub enable_provenance: bool,

    /// Enable temporal versioning (default: false)
    pub enable_temporal: bool,

    /// Enable SQL extensions (SIMILAR, FOLLOW, ASSEMBLE)
    pub enable_sql_extensions: bool,

    /// Maximum embedding versions per entity (default: 10)
    pub max_embedding_versions: usize,

    /// Maximum graph traversal depth (default: 5)
    pub max_traversal_depth: usize,

    /// Cache size for entity store in MB (default: 256)
    pub entity_cache_mb: usize,

    /// Cache size for relations in MB (default: 128)
    pub relations_cache_mb: usize,

    /// Default embedding model for text-to-vector conversion
    pub default_embedding_model: String,

    /// Storage backend for SKS data ("memory", "sst", "viper")
    pub storage_backend: String,
}

fn default_false_sks() -> bool {
    false
}

fn default_max_embedding_versions() -> usize {
    10
}

fn default_max_traversal_depth() -> usize {
    5
}

fn default_entity_cache_mb() -> usize {
    256
}

fn default_relations_cache_mb() -> usize {
    128
}

fn default_embedding_model() -> String {
    "openai/text-embedding-3-large".to_string()
}

fn default_sks_backend() -> String {
    "sst".to_string()
}

impl Default for SksConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            enable_entities: true,
            enable_relations: true,
            enable_provenance: true,
            enable_temporal: false,
            enable_sql_extensions: true,
            max_embedding_versions: 10,
            max_traversal_depth: 5,
            entity_cache_mb: 256,
            relations_cache_mb: 128,
            default_embedding_model: "openai/text-embedding-3-large".to_string(),
            storage_backend: "sst".to_string(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            storage: StorageConfig::default(),
            consensus: ConsensusConfig::default(),
            api: ApiConfig::default(),
            monitoring: MonitoringConfig::default(),
            network: None,
            tls: None,
            hardware: Some(HardwareConfig::default()),
            sks: None, // SKS disabled by default
            cache: None,
            graph: Some(GraphRuntimeConfig::default()),
            hybrid: Some(HybridRuntimeConfig::default()),
        }
    }
}

/// Runtime cache configuration for the unified Cross-Cache Orchestrator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheRuntimeConfig {
    /// Total memory budget for orchestrator-managed caches (in MB)
    pub total_memory_mb: u64,
}

fn default_orchestrator_budget_mb() -> u64 { 512 }

impl Default for CacheRuntimeConfig {
    fn default() -> Self {
        Self { total_memory_mb: default_orchestrator_budget_mb() }
    }
}

/// Graph runtime configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphRuntimeConfig {
    /// Enable bounded prefetch hints during traversals
    pub enable_prefetch: bool,
    /// Per-node/iteration adjacency prefetch budget
    pub prefetch_budget: usize,
    /// Select graph engine ("ORION"|"PULSAR"|"QUASAR")
    pub engine: String,
}

impl Default for GraphRuntimeConfig {
    fn default() -> Self {
        Self { enable_prefetch: true, prefetch_budget: 8, engine: default_graph_engine() }
    }
}

fn default_graph_engine() -> String { "ORION".to_string() }

/// Hybrid query runtime configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridRuntimeConfig {
    /// Default seeding strategy ("AVERAGE"|"PER_SEED"|"NONE")
    pub seeding_strategy: String,
    /// Fusion weights for [vector, graph]
    pub fusion_weights: Option<Vec<f64>>,
}

impl Default for HybridRuntimeConfig {
    fn default() -> Self {
        Self { seeding_strategy: "AVERAGE".to_string(), fusion_weights: Some(vec![0.6, 0.4]) }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            node_id: "node-1".to_string(),
            bind_address: "127.0.0.1".to_string(),
            port: 5678,
            grpc_port: None,
            data_dir: PathBuf::from("./data"),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            storage_locations: vec![StorageLocation::default()],
            metadata_url: "file://./metadata".to_string(),
            assignment_config: AssignmentConfig::default(),
            wal_config: WriteBufferUserConfig::default(),
            mmap_enabled: true,
            sst_config: Some(SstConfig::default()),
            viper_config: Some(ViperConfig::default()),
            cache_size_mb: 512,
            bloom_filter_config: Some(BloomFilterConfig::default()),
            compaction_config: CompactionConfig::default(),
            filesystem_config: FilesystemOptimizationConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub node_id: String,
    pub bind_address: String,
    pub port: u16,
    /// Optional gRPC port for convenience; if not set, ApiConfig.grpc_port is used
    pub grpc_port: Option<u16>,
    pub data_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Storage locations - each can host WriteBuffer, data, and indexes
    pub storage_locations: Vec<StorageLocation>,

    /// Single metadata URL for consistency (e.g., "file:///fast-ssd/proximadb/metadata_info")
    pub metadata_url: String,

    /// Assignment configuration
    pub assignment_config: AssignmentConfig,

    /// Write buffer configuration (global memtable settings)
    pub wal_config: WriteBufferUserConfig,

    /// Storage engine configurations
    pub mmap_enabled: bool,
    pub sst_config: Option<SstConfig>,
    pub viper_config: Option<ViperConfig>,
    pub cache_size_mb: u64,
    // bloom_filter_bits removed - use bloom_filter_config instead
    pub bloom_filter_config: Option<BloomFilterConfig>,

    /// Common compaction configuration (can be overridden per engine)
    pub compaction_config: CompactionConfig,

    /// Filesystem optimization settings
    pub filesystem_config: FilesystemOptimizationConfig,
}

/// Storage location configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageLocation {
    /// Storage URL (e.g., "file:///nvme1/proximadb", "s3://bucket/proximadb")
    pub url: String,

    /// Weight for weighted distribution (default: 1)
    pub weight: u32,

    /// Tags for filtering (e.g., ["fast", "local"], ["cloud", "archive"])
    pub tags: Vec<String>,
}

fn default_weight() -> u32 {
    1
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

/// Assignment configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentConfig {
    /// Assignment strategy: "hash", "round-robin", "weighted"
    pub strategy: String,
    /// Keep all collection data together (WAL, data, index on same location)
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
    pub atomic_config: TransactionalOperationsConfig,
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
pub struct TransactionalOperationsConfig {
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

impl StorageConfig {
    /// Get storage URLs from locations
    pub fn storage_urls(&self) -> Vec<String> {
        self.storage_locations
            .iter()
            .map(|loc| loc.url.clone())
            .collect()
    }

    /// Get WAL URLs derived from storage URLs
    pub fn write_buffer_urls(&self) -> Vec<String> {
        self.storage_locations
            .iter()
            .map(|loc| format!("{}/wal", loc.url.trim_end_matches('/')))
            .collect()
    }

    /// Get data URLs derived from storage URLs
    pub fn data_urls(&self) -> Vec<String> {
        self.storage_locations
            .iter()
            .map(|loc| format!("{}/data", loc.url.trim_end_matches('/')))
            .collect()
    }

    /// Get index URLs derived from storage URLs
    pub fn index_urls(&self) -> Vec<String> {
        self.storage_locations
            .iter()
            .map(|loc| format!("{}/index", loc.url.trim_end_matches('/')))
            .collect()
    }
}

// Default implementation moved to line 115

/// User-facing write buffer configuration (from TOML files)
/// This is the simple configuration that users specify in their config files.
/// It gets converted to the internal WALConfig for the engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteBufferUserConfig {
    /// Total write buffer size across all collections in MB
    pub write_buffer_size_mb: u64,
    /// Threshold in bytes to trigger flush when a collection's total unflushed data exceeds this
    pub memory_flush_size_bytes: usize,
    /// Threshold in vector count to trigger flush when a collection's vector count exceeds this
    pub vector_count_threshold: usize,
    /// Memtable implementation type (BTree, SkipList)
    pub memtable_type: String,
    /// Sync mode for durability (PerBatch, Periodic, None)
    pub sync_mode: String,
    /// Directory for write-ahead log files
    pub write_buffer_directory: String,
    /// Enable write-ahead logging
    pub enable_wal: bool,
}

impl Default for WriteBufferUserConfig {
    fn default() -> Self {
        Self {
            write_buffer_size_mb: 8192, // 8GB total across all collections
            memory_flush_size_bytes: 16 * 1024 * 1024, // 16MB per collection (aggregate)
            vector_count_threshold: 100_000, // 100k vectors per collection
            memtable_type: "BTree".to_string(),
            sync_mode: "PerBatch".to_string(),
            write_buffer_directory: "./data/write_buffer".to_string(),
            enable_wal: true,
        }
    }
}

impl WriteBufferUserConfig {
    /// Convert user configuration to internal engine configuration
    pub fn to_engine_config(&self) -> crate::storage::persistence::write_ahead_log::WALConfig {
        use crate::storage::persistence::write_ahead_log::{
            WALConfig, WriteBufferStrategyType,
            config::{CompressionConfig, MemTableConfig, MultiDiskConfig, PerformanceConfig},
        };

        WALConfig {
            strategy_type: WriteBufferStrategyType::default(),
            memtable: MemTableConfig::default(),
            multi_disk: MultiDiskConfig::default(),
            compression: CompressionConfig::default(),
            performance: PerformanceConfig::default(),
            enable_mvcc: true,
            enable_ttl: true,
            enable_background_compaction: true,
            collection_overrides: std::collections::HashMap::new(),
            enable_optimized_writer: false,
            optimized_writer_batch_size: None,
            optimized_writer_batch_timeout_ms: None,
            optimized_writer_threads: None,
            optimized_writer_enable_combining: None,
        }
    }
}

/// Common compaction configuration shared across engines
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    /// L0 file count threshold for compaction (default: 5)
    pub l0_file_threshold: usize,

    /// L0 size threshold in MB for compaction (default: 256MB)
    pub l0_size_threshold_mb: usize,

    /// Multiplier for higher level thresholds (default: 2.0)
    pub level_multiplier: f64,

    /// Maximum number of levels (default: 7)
    pub max_levels: u8,

    /// Compaction strategy: "count", "size", or "hybrid" (default: "hybrid")
    pub strategy: String,

    /// Target output file size in MB for size-based compaction (default: 128MB)
    pub target_file_size_mb: usize,
}

fn default_l0_file_threshold() -> usize {
    5
}
fn default_l0_size_threshold_mb() -> usize {
    256
}
fn default_level_multiplier() -> f64 {
    2.0
}
fn default_max_levels() -> u8 {
    7
}
fn default_compaction_strategy() -> String {
    "hybrid".to_string()
}
fn default_target_file_size_mb() -> usize {
    128
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            l0_file_threshold: default_l0_file_threshold(),
            l0_size_threshold_mb: default_l0_size_threshold_mb(),
            level_multiplier: default_level_multiplier(),
            max_levels: default_max_levels(),
            strategy: default_compaction_strategy(),
            target_file_size_mb: default_target_file_size_mb(),
        }
    }
}

/// SST (Sorted String Table) engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SstConfig {
    /// Number of levels in the SST tree
    pub level_count: u8,
    /// Minimum files before compaction triggers (DEPRECATED - use compaction_config)
    pub compaction_threshold: u32,

    /// Compaction configuration (overrides common config if specified)
    pub compaction_config: Option<CompactionConfig>,
    /// SSTable block size in KB. Configurable from TOML, defaults to 1MB.
    ///
    /// **Performance Guidelines:**
    /// - **256-512KB**: Good for memory-constrained environments
    /// - **1MB**: Optimal for EC2 GP2/GP3 and modern SSDs (default)
    /// - **2-4MB**: Best for high-throughput workloads with ample memory
    ///
    /// **Cloud-Optimized Block Size (MB):**
    /// - 3MB: Universal optimization for AWS EBS gp3/st1, Azure Premium SSD, GCS Standard
    /// - 2MB: Memory-constrained environments
    /// - 4MB: Very large sparse vector deployments
    ///
    /// Block size in KB (256-16384 KB range, default 2048 KB = 2MB)
    /// **Backward Compatibility:**
    /// Changing this value during restarts is safe. Each SSTable block stores its own
    /// length prefix [block_len:4][block_data], so existing files continue to work.
    /// Mixed block sizes within the same system are fully supported.
    pub block_size_kb: u32,
    /// Compaction strategy (leveled, tiered, unified)
    pub compaction_strategy: String,
    /// Compression algorithm (snappy, lz4, zstd, none)
    pub compression: String,
    /// Compression level (1-22 for ZSTD, ignored for other algorithms)
    pub compression_level: i32,
    /// Bloom filter configuration for SST files
    pub bloom_filter_config: Option<BloomFilterConfig>,
    /// Cache size for SST blocks in MB
    pub cache_size_mb: u64,
    /// Maximum files per level
    pub max_files_per_level: u32,
    /// Size multiplier between levels
    pub level_size_multiplier: f64,
    /// Maximum number of levels
    pub max_levels: u8,
    /// Number of background compaction threads
    pub background_thread_count: u32,
    /// Data directory for SST files
    pub data_directory: String,
    /// Enable memory-mapped I/O for SST files
    pub mmap_enabled: bool,
    /// Enable prefetching for sequential reads
    pub prefetch_enabled: bool,
    /// Prefetch size in KB
    pub prefetch_size_kb: u32,
    /// Decompression cache configuration
    pub decompression_cache_config:
        Option<crate::storage::engines::impls::sst::decompression_cache::CacheConfig>,
}

/// VIPER (columnar storage) engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViperConfig {
    /// Parquet file configuration
    pub row_group_size: usize,
    /// Compression for Parquet files (snappy, gzip, lz4, zstd)
    pub compression: String,
    /// Compression level (1-22 for ZSTD, ignored for other algorithms)
    pub compression_level: i32,
    /// Enable statistics in Parquet files
    pub enable_statistics: bool,
    /// Data directory for VIPER files
    pub data_directory: String,
    /// Cache size for columnar data in MB
    pub cache_size_mb: u64,

    /// Compaction configuration (overrides common config if specified)
    pub compaction_config: Option<CompactionConfig>,
}

impl Default for ViperConfig {
    fn default() -> Self {
        Self {
            row_group_size: 100_000,
            compression: "zstd".to_string(), // ZSTD for better compression
            compression_level: 3,            // Balanced speed/compression
            enable_statistics: true,
            data_directory: "./data/viper_data".to_string(),
            cache_size_mb: 512,
            compaction_config: None, // Use common config by default
        }
    }
}
fn default_compression_level() -> i32 {
    3 // Balanced compression level
}

// BloomFilterConfig moved to core::bloom module for polymorphic design
// Re-export for backward compatibility
pub use crate::core::bloom::BloomFilterConfig;

impl Default for SstConfig {
    fn default() -> Self {
        Self {
            level_count: 7,
            compaction_threshold: 5,
            compaction_config: None, // Use common config by default
            block_size_kb: 2048, // 2MB default (2048 KB) - optimal balance for disk IOPS and cloud storage
            compaction_strategy: "leveled".to_string(),
            compression: "none".to_string(), // No compression for server default
            compression_level: 0,            // No compression level
            bloom_filter_config: Some(BloomFilterConfig::default()),
            cache_size_mb: 128,
            max_files_per_level: 10,
            level_size_multiplier: 10.0,
            max_levels: 7,
            background_thread_count: 4,
            data_directory: "./sst_data".to_string(),
            mmap_enabled: true,
            prefetch_enabled: true,
            prefetch_size_kb: 64,
            decompression_cache_config: Some(
                crate::storage::engines::impls::sst::decompression_cache::CacheConfig::default(),
            ),
        }
    }
}

impl SstConfig {
    /// Validate SST configuration parameters for optimal performance and correctness
    ///
    /// Note: block_size_kb changes are backward compatible since each SSTable block
    /// stores its own length prefix [block_len:4][block_data], allowing mixed block sizes.
    pub fn validate(&self) -> Result<(), String> {
        if self.level_count == 0 {
            return Err("level_count must be greater than 0".to_string());
        }
        if self.compaction_threshold == 0 {
            return Err("compaction_threshold must be greater than 0".to_string());
        }

        // Validate block size for optimal performance and storage compatibility
        if self.block_size_kb < 256 {
            return Err(
                "block_size_kb must be at least 256KB for reasonable I/O performance".to_string(),
            );
        }
        if self.block_size_kb > 16384 {
            return Err("block_size_kb should not exceed 16384KB (16MB) to avoid excessive memory usage per block".to_string());
        }

        // Performance recommendations for common deployment scenarios
        match self.block_size_kb {
            256..=512 => {
                // 256-512KB - Good for disk IOPS optimization and memory-constrained environments
                info!(
                    "block_size_kb={}KB - Optimized for disk IOPS and memory-constrained deployments",
                    self.block_size_kb
                );
            }
            1024 => {
                // 1MB - Good for standard disk deployments
                info!(
                    "block_size_kb=1024KB (1MB) - Good for standard disk deployments with moderate mem"
                );
            }
            2048 => {
                // 2MB - Optimal balance for both disk and cloud
                info!(
                    "block_size_kb=2048KB (2MB) - Optimal balance for disk IOPS and cloud storage patterns"
                );
            }
            3072 => {
                // 3MB - Optimal for all cloud providers (AWS EBS gp3/st1, Azure Premium SSD, GCS Standard)
                info!(
                    "block_size_kb=3072KB (3MB) - Optimal for cloud storage IOPS patterns (AWS/Azure/GCS)"
                );
            }
            4096..=8192 => {
                // 4-8MB - Good for high-throughput cloud scenarios
                info!(
                    "block_size_kb={}KB ({}MB) - Optimized for high-throughput cloud workloads",
                    self.block_size_kb,
                    self.block_size_kb / 1024
                );
            }
            8193..=16384 => {
                // Large blocks for very high-throughput workloads
                info!(
                    "block_size_kb={}KB ({}MB) - Large blocks for very high-throughput workloads",
                    self.block_size_kb,
                    self.block_size_kb / 1024
                );
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

    /// Create test-specific SST configuration with smaller block sizes for quantization testing
    /// This helps demonstrate quantization clustering with smaller blocks while keeping
    /// server default at 2048KB for production performance
    pub fn test_config_256kb() -> Self {
        let mut config = Self::default();
        config.block_size_kb = 256; // Small blocks for quantization clustering tests
        config.compression = "zstd".to_string(); // Zstd compression for tests
        config.compression_level = 3; // Balanced compression level
        config
    }

    /// Create test-specific SST configuration with 512KB blocks
    pub fn test_config_512kb() -> Self {
        let mut config = Self::default();
        config.block_size_kb = 512;
        config.compression = "zstd".to_string(); // Zstd compression for tests
        config.compression_level = 3; // Balanced compression level
        config
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

impl Default for ConsensusConfig {
    fn default() -> Self {
        Self {
            node_id: None,
            cluster_peers: Vec::new(),
            election_timeout_ms: 5000,
            heartbeat_interval_ms: 1000,
            snapshot_threshold: 10000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub grpc_port: u16,
    pub rest_port: u16,
    pub max_request_size_mb: u64,
    pub timeout_seconds: u64,
    pub enable_tls: Option<bool>,
    /// Interval for background TTL sweeper in seconds (default: 900 = 15 minutes)
    pub ttl_sweep_interval_seconds: u64,

    /// Enable REST API compression (default: false)
    pub rest_compression: bool,

    /// Enable gRPC compression (default: false)
    pub grpc_compression: bool,

    /// Compression algorithm: "gzip", "deflate", "br" (default: "gzip")
    pub compression_algorithm: String,

    /// Compression level 1-9 for gzip, 1-11 for brotli (default: 6)
    pub compression_level: i32,
}

fn default_false() -> bool {
    false
}

fn default_compression_algorithm() -> String {
    "gzip".to_string()
}

fn default_compression_level_api() -> i32 {
    6
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            grpc_port: 5679,
            rest_port: 5678,
            max_request_size_mb: 100,
            timeout_seconds: 60,
            enable_tls: Some(false),
            rest_compression: false,
            grpc_compression: false,
            compression_algorithm: "gzip".to_string(),
            compression_level: 6,
            ttl_sweep_interval_seconds: 900,
        }
    }
}

fn default_ttl_sweep_interval() -> u64 {
    900
}

/// WAL storage configuration supporting multiple directories and cloud storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalStorageConfig {
    /// WAL storage URLs - supports file://, s3://, adls://, gcs://
    /// Multiple URLs enable multi-disk performance scaling
    pub write_buffer_urls: Vec<String>,

    /// Distribution strategy for collections across WAL directories
    pub distribution_strategy: WalDistributionStrategy,

    /// Whether to keep each collection on a single WAL directory
    pub collection_affinity: bool,

    /// Memory flush threshold per collection (bytes)
    pub memory_flush_size_bytes: usize,

    /// Global WAL size threshold for forced flush (bytes)
    pub global_flush_threshold: usize,

    /// WAL strategy type (Avro vs Bincode)
    pub strategy_type: Option<String>,

    /// Memtable type for memory structure
    pub memtable_type: Option<String>,

    /// Sync mode for durability vs performance tradeoff
    pub sync_mode: Option<String>,

    /// Batch threshold for operations
    pub batch_threshold: Option<usize>,

    /// Write buffer size in MB
    pub write_buffer_size_mb: Option<usize>,

    /// Maximum concurrent flush operations
    pub concurrent_flushes: Option<usize>,

    /// Shrink factor for global threshold management (percentage)
    /// When global threshold is exceeded, flush collections until memory usage drops to this percentage
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
            memory_flush_size_bytes: 10 * 1024 * 1024, // 10MB - recommended for collection-level flush
            global_flush_threshold: 4 * 1024 * 1024 * 1024, // 4GB - recommended for global memory threshold
            strategy_type: None,                            // Use WAL defaults
            memtable_type: None,                            // Use WAL defaults
            sync_mode: None,                                // Use WAL defaults
            batch_threshold: None,                          // Use WAL defaults
            write_buffer_size_mb: None,                     // Use WAL defaults
            concurrent_flushes: None,                       // Use WAL defaults
            global_shrink_factor: Some(0.4),                // 40% shrink factor - recommended
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

impl Default for MonitoringConfig {
    fn default() -> Self {
        Self {
            metrics_enabled: true,
            log_level: "info".to_string(),
        }
    }
}
