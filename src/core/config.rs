use crate::network::NetworkConfig;
use crate::security::SecurityConfig;
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
    /// Unified security configuration (optional)
    pub security: Option<SecurityConfig>,
    /// Global cache runtime configuration (optional)
    pub cache: Option<CacheRuntimeConfig>,
    /// Graph runtime configuration (optional)
    pub graph: Option<GraphRuntimeConfig>,
    /// Hybrid query runtime configuration (optional)
    pub hybrid: Option<HybridRuntimeConfig>,
    /// Query processing configuration (including RL planner)
    #[serde(default)]
    pub query: Option<QueryConfig>,
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
            security: None,
            cache: None,
            graph: Some(GraphRuntimeConfig::default()),
            hybrid: Some(HybridRuntimeConfig::default()),
            query: None, // Uses default RL planner settings when None
        }
    }
}

/// Runtime cache configuration for the unified Cross-Cache Orchestrator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheRuntimeConfig {
    /// Total memory budget for orchestrator-managed caches (in MB)
    pub total_memory_mb: u64,

    /// Enable cache warming service
    pub enable_warming: bool,

    /// Memory rebalancing configuration
    pub rebalancing: CacheRebalancingConfig,

    /// Eviction configuration
    pub eviction: CacheEvictionConfig,

    /// Per-cache-type configurations
    pub types: CacheTypesConfig,

    /// Cache warming configuration
    pub warming: CacheWarmingConfig,
}

fn default_orchestrator_budget_mb() -> u64 {
    512
}

impl Default for CacheRuntimeConfig {
    fn default() -> Self {
        Self {
            total_memory_mb: default_orchestrator_budget_mb(),
            enable_warming: false, // Disabled by default
            rebalancing: CacheRebalancingConfig::default(),
            eviction: CacheEvictionConfig::default(),
            types: CacheTypesConfig::default(),
            warming: CacheWarmingConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheRebalancingConfig {
    pub enabled: bool,
    pub interval_seconds: u64,
    pub min_hit_rate_threshold: f64,
}

impl Default for CacheRebalancingConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_seconds: 300,       // 5 minutes
            min_hit_rate_threshold: 0.1, // 10% minimum hit rate
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEvictionConfig {
    pub enabled: bool,
    pub check_interval_seconds: u64,
    pub batch_size: usize,
    pub memory_threshold_percent: u8,
    pub policies: Vec<EvictionPolicyConfig>,
}

impl Default for CacheEvictionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            check_interval_seconds: 60,
            batch_size: 100,
            memory_threshold_percent: 90,
            policies: vec![
                EvictionPolicyConfig {
                    policy_type: "lru".to_string(),
                    max_items: Some(10000),
                    batch_size: Some(100),
                    max_age_seconds: None,
                    cleanup_interval_seconds: None,
                },
                EvictionPolicyConfig {
                    policy_type: "ttl".to_string(),
                    max_items: None,
                    batch_size: None,
                    max_age_seconds: Some(3600),
                    cleanup_interval_seconds: Some(300),
                },
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvictionPolicyConfig {
    #[serde(rename = "type")]
    pub policy_type: String,
    pub max_items: Option<usize>,
    pub batch_size: Option<usize>,
    pub max_age_seconds: Option<u64>,
    pub cleanup_interval_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheTypesConfig {
    pub vector: CacheTypeConfig,
    pub query: CacheTypeConfig,
    pub metadata: CacheTypeConfig,
    pub index: CacheTypeConfig,
    pub filter: CacheTypeConfig,
}

impl Default for CacheTypesConfig {
    fn default() -> Self {
        Self {
            vector: CacheTypeConfig {
                initial_allocation_mb: 200,
                min_allocation_mb: 50,
                max_allocation_mb: 400,
            },
            query: CacheTypeConfig {
                initial_allocation_mb: 150,
                min_allocation_mb: 30,
                max_allocation_mb: 300,
            },
            metadata: CacheTypeConfig {
                initial_allocation_mb: 50,
                min_allocation_mb: 10,
                max_allocation_mb: 100,
            },
            index: CacheTypeConfig {
                initial_allocation_mb: 50,
                min_allocation_mb: 10,
                max_allocation_mb: 100,
            },
            filter: CacheTypeConfig {
                initial_allocation_mb: 50,
                min_allocation_mb: 10,
                max_allocation_mb: 100,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheTypeConfig {
    pub initial_allocation_mb: u64,
    pub min_allocation_mb: u64,
    pub max_allocation_mb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheWarmingConfig {
    pub strategies: Vec<String>,
    pub warm_on_startup: bool,
    pub warm_batch_size: usize,
    pub popularity_threshold: u32,
    pub time_window_hours: u64,
}

impl Default for CacheWarmingConfig {
    fn default() -> Self {
        Self {
            strategies: vec!["popularity".to_string(), "time_based".to_string()],
            warm_on_startup: false,
            warm_batch_size: 100,
            popularity_threshold: 10,
            time_window_hours: 24,
        }
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
    /// Embedding storage mode: "none" (default), "cold", "memory"
    /// - "none": No embeddings stored (pure graph, best performance)
    /// - "cold": Embeddings in vector engine (SST/HELIX/VIPER)
    /// - "memory": Embeddings cached in memory (for SKS-heavy workloads)
    #[serde(default = "default_embedding_mode")]
    pub embedding_mode: String,
    /// Vector engine for cold tier embeddings (only if embedding_mode = "cold")
    /// Options: "sst", "helix", "viper"
    #[serde(default = "default_embedding_engine")]
    pub embedding_engine: String,
    /// Memory cache size in MB for embeddings (only if embedding_mode = "memory")
    #[serde(default)]
    pub embedding_memory_cache_mb: Option<usize>,
}

impl Default for GraphRuntimeConfig {
    fn default() -> Self {
        Self {
            enable_prefetch: true,
            prefetch_budget: 8,
            engine: default_graph_engine(),
            embedding_mode: default_embedding_mode(),
            embedding_engine: default_embedding_engine(),
            embedding_memory_cache_mb: None,
        }
    }
}

fn default_graph_engine() -> String {
    "ORION".to_string()
}

fn default_embedding_mode() -> String {
    "none".to_string()
}

fn default_embedding_engine() -> String {
    "sst".to_string()
}

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
        Self {
            seeding_strategy: "AVERAGE".to_string(),
            fusion_weights: Some(vec![0.6, 0.4]),
        }
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
            prune_mode: None,
            mmap_enabled: true,
            sst_config: Some(SstConfig::default()),
            viper_config: Some(ViperConfig::default()),
            cache_size_mb: 512,
            bloom_filter_config: Some(BloomFilterConfig::default()),
            compaction_config: CompactionConfig::default(),
            filesystem_config: FilesystemOptimizationConfig::default(),
            optimization: OptimizationConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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

    /// Search pruning configuration
    #[serde(default)]
    pub prune_mode: Option<PruneModeConfig>,

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

    /// Performance optimization settings
    #[serde(default)]
    pub optimization: OptimizationConfig,
}

/// Configuration for search pruning, allowing for simple or advanced setup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum PruneModeConfig {
    Simple(String),
    Advanced(AdvancedPruneConfig),
}

/// Advanced configuration for search pruning.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct AdvancedPruneConfig {
    #[serde(default = "default_prune_type")]
    pub r#type: String,
    pub min_keep: Option<usize>,
    pub max_keep: Option<usize>,
    pub ratio: Option<f32>,
}

fn default_prune_type() -> String {
    "sqrt".to_string()
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

/// Performance optimization configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationConfig {
    /// Enable memory-mapped I/O for large files (40-60% faster reads)
    #[serde(default = "default_enable_mmap")]
    pub enable_mmap: bool,

    /// Enable zone map pruning to skip irrelevant blocks (30-50% faster search)
    #[serde(default = "default_enable_zone_map_pruning")]
    pub enable_zone_map_pruning: bool,

    /// Enable AXIS indexes for approximate nearest neighbor search
    #[serde(default = "default_enable_axis_indexes")]
    pub enable_axis_indexes: bool,

    /// Default index type for new collections: flat, hnsw, ivf, lsh
    #[serde(default = "default_index_type")]
    pub default_index_type: String,

    /// Enable progressive quantization search (Binary → INT8 → FP32)
    #[serde(default = "default_enable_progressive_search")]
    pub enable_progressive_search: bool,

    /// Enable block-level bloom filters for metadata filtering
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// Global manifest location (optional)
    pub global_manifest_url: Option<String>,
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
            global_manifest_url: None,
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
            global_manifest_url: self.global_manifest_url.clone(),
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

    /// Vector encoding strategy: Controls how vectors are encoded in blocks
    ///
    /// # Available Strategies (Data-Driven from 12-Pattern Benchmarks):
    ///
    /// * `"FullVector"` (DEFAULT) - Row-wise encoding, fastest decode
    ///   - Compression: 18-20%
    ///   - Performance: Fastest decode in 8/12 configs, wins 5/12 (42%)
    ///   - Best for: Vector databases, WORM workloads, RAG, semantic search
    ///   - Use when: Query speed is critical (embeddings queried thousands of times)
    ///
    /// * `"GroupedBlock"` - Block-grouped encoding, best balanced
    ///   - Compression: 18-21%
    ///   - Performance: Wins 6/12 configs (50% - HIGHEST), fastest encode
    ///   - Best for: Mixed workloads, ETL, data pipelines
    ///   - Use when: Encode and decode are equally important
    ///
    /// * `"GroupedField"` - Field-grouped encoding, maximum compression
    ///   - Compression: 19-22% (BEST)
    ///   - Performance: Wins 1/12 (8%), slower decode
    ///   - Best for: Storage-critical, cost optimization, cold storage
    ///   - Use when: Storage costs dominate, data read infrequently
    ///
    /// * `"TransposeBlock"` - Transpose block-grouped encoding
    ///   - Compression: 19-20%
    ///   - Performance: Good for large batches (>4096 vectors)
    ///   - Best for: Batch analytics, columnar processing
    ///   - Use when: Processing large batches with dimensional correlation
    ///
    /// * `"TransposeField"` - ⚠️ NOT RECOMMENDED (very slow)
    ///   - Compression: 17-19%
    ///   - Performance: Slowest encode/decode (14-121ms encode, 4-170ms decode)
    ///   - Only use if: You have very specific dimensional correlation requirements
    ///   - Warning: 10-40x slower than other strategies, use with caution
    ///
    /// * `"Auto"` - Automatic selection based on workload
    ///   - Currently resolves to FullVector for vector database workloads
    ///   - Safe default choice for production
    ///
    /// # Configuration Examples:
    ///
    /// ```toml
    /// # config.toml
    ///
    /// # Vector database (default - fastest decode)
    /// [storage.sst_config]
    /// vector_encoding_strategy = "FullVector"
    ///
    /// # Balanced workload
    /// [storage.sst_config]
    /// vector_encoding_strategy = "GroupedBlock"
    ///
    /// # Storage optimization
    /// [storage.sst_config]
    /// vector_encoding_strategy = "GroupedField"
    /// ```
    ///
    /// # Performance Data:
    ///
    /// Based on comprehensive 12-pattern benchmark (sparse, gaussian, quantized,
    /// sinusoidal, random, clustered, time-series, dense, high-freq, power-law,
    /// binary, exponential) representing production ML embeddings:
    ///
    /// - OpenAI 1536D: FullVector 19.6% comp, 0.75ms decode (FASTEST)
    /// - BERT 1024D:   FullVector 19.1% comp, 4.71ms decode (FASTEST)
    /// - BERT 768D:    FullVector 18.9% comp, 4.06ms decode (FASTEST)
    /// - MiniLM 384D:  FullVector 18.5% comp, 0.26ms decode (FASTEST)
    ///
    /// Default: FullVector (optimal for vector databases with WORM characteristics)
    ///
    /// See: docs/performance/encoding_strategies.adoc for detailed guide
    #[serde(default = "default_vector_encoding_strategy")]
    pub vector_encoding_strategy: String,
}

/// VIPER (columnar storage) engine configuration
///
/// **Note**: VIPER uses Parquet's native columnar format and does NOT use ProximaDataBlocks.
/// Therefore, it does not have `vector_encoding_strategy` (that's only for ProximaDataBlocks
/// which use columnar encoding within blocks, as used by SST engine).
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
            row_group_size: 65536,            // ~32MB row groups for 128D vectors
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

fn default_vector_encoding_strategy() -> String {
    "FullVector".to_string() // Default to FullVector (best for vector databases - WORM workloads)
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
            block_size_kb: 256, // 256KB default - optimized for low-latency random access on NVMe
            compaction_strategy: "leveled".to_string(),
            compression: "lz4".to_string(), // LZ4 default - 7% faster than no compression (measured)
            compression_level: 3,           // LZ4 compression level
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
            vector_encoding_strategy: default_vector_encoding_strategy(),
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
        if self.block_size_kb > 4096 {
            return Err("block_size_kb should not exceed 4096KB (4MB) to avoid excessive memory usage per block".to_string());
        }

        // Performance recommendations for common deployment scenarios
        match self.block_size_kb {
            256..=512 => {
                // 256-512KB - Optimized for random access and point queries
                info!(
                    "block_size_kb={}KB - Optimized for random access and NVMe alignment",
                    self.block_size_kb
                );
            }
            513..=1023 => {
                // 512KB-1MB range
                info!(
                    "block_size_kb={}KB - Balanced for mixed random/sequential access",
                    self.block_size_kb
                );
            }
            1024 => {
                // 1MB - New default for balanced workloads
                info!("block_size_kb=1024KB (1MB) - Default balanced configuration");
            }
            1025..=2047 => {
                // 1-2MB range
                info!(
                    "block_size_kb={}KB - Good for cloud storage patterns",
                    self.block_size_kb
                );
            }
            2048..=4096 => {
                // 2-4MB - Maximum allowed, optimized for sequential scans
                info!(
                    "block_size_kb={}KB ({}MB) - Optimized for sequential scans and large transfers",
                    self.block_size_kb,
                    self.block_size_kb / 1024
                );
            }
            _ => {
                // Should not reach here due to validation
                info!(
                    "block_size_kb={}KB - Out of recommended range",
                    self.block_size_kb
                );
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
    /// Distribution strategy for collections across storage locations
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

    /// Global manifest location (optional - explicit configuration)
    /// If not specified, defaults to {first write_buffer_url}/wal
    /// Examples:
    /// - "file:///data/wal-metadata" (dedicated fast SSD)
    /// - "file:///shared/nfs/wal" (shared storage for HA)
    /// - "s3://bucket/wal-global" (cloud-based)
    pub global_manifest_url: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
            global_manifest_url: None, // Defaults to {storage_locations[0]}/wal
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
    /// Dashboard refresh interval in seconds (default: 60, minimum: 15)
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
    /// Get dashboard refresh interval, ensuring it's at least 15 seconds
    pub fn dashboard_refresh_interval(&self) -> u64 {
        self.dashboard_refresh_interval_seconds.max(15)
    }
}

/// Query processing configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueryConfig {
    /// RL-based adaptive query planner configuration
    #[serde(default)]
    pub rl_planner: RLPlannerConfig,
}

/// RL-based Adaptive Query Planner Configuration
///
/// Controls how the reinforcement learning query planner learns and selects
/// optimal execution paths across storage engines, indexes, and quantization strategies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RLPlannerConfig {
    /// Enable RL-based planning (false = use static heuristics)
    #[serde(default = "default_rl_enabled")]
    pub enabled: bool,

    /// Use Thompson Sampling (true) or epsilon-greedy (false) for action selection
    #[serde(default = "default_thompson_sampling")]
    pub thompson_sampling: bool,

    /// Exploration rate for epsilon-greedy fallback (0.0 - 1.0)
    #[serde(default = "default_exploration_rate")]
    pub exploration_rate: f32,

    /// Size of experience replay buffer for batch learning
    #[serde(default = "default_experience_buffer_size")]
    pub experience_buffer_size: usize,

    /// Number of experiences before batch policy update
    #[serde(default = "default_batch_update_interval")]
    pub batch_update_interval: usize,

    /// Log all query executions to JSONL for offline analysis
    #[serde(default = "default_log_all_executions")]
    pub log_all_executions: bool,

    /// Path for execution logs (JSONL format) - None for no file logging
    #[serde(default)]
    pub log_path: Option<String>,

    /// Default optimization goal: MinLatency, MaxRecall, MaxThroughput, Balanced
    #[serde(default = "default_optimization_goal")]
    pub default_goal: String,
}

fn default_rl_enabled() -> bool {
    true
}

fn default_thompson_sampling() -> bool {
    true
}

fn default_exploration_rate() -> f32 {
    0.1
}

fn default_experience_buffer_size() -> usize {
    10_000
}

fn default_batch_update_interval() -> usize {
    100
}

fn default_log_all_executions() -> bool {
    true
}

fn default_optimization_goal() -> String {
    "Balanced".to_string()
}

impl Default for RLPlannerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            thompson_sampling: true,
            exploration_rate: 0.1,
            experience_buffer_size: 10_000,
            batch_update_interval: 100,
            log_all_executions: true,
            log_path: None,
            default_goal: "Balanced".to_string(),
        }
    }
}

impl RLPlannerConfig {
    /// Convert to the RL planner module's config type
    pub fn to_rl_planner_config(&self) -> crate::query::rl_planner::RLPlannerConfig {
        use crate::query::rl_planner::OptimizationGoal;

        let goal = match self.default_goal.to_lowercase().as_str() {
            "minlatency" | "min_latency" => OptimizationGoal::MinLatency,
            "maxrecall" | "max_recall" => OptimizationGoal::MaxRecall,
            "maxthroughput" | "max_throughput" => OptimizationGoal::MaxThroughput,
            _ => OptimizationGoal::Balanced,
        };

        crate::query::rl_planner::RLPlannerConfig {
            enabled: self.enabled,
            exploration_rate: self.exploration_rate,
            thompson_sampling: self.thompson_sampling,
            experience_buffer_size: self.experience_buffer_size,
            batch_update_interval: self.batch_update_interval,
            log_all_executions: self.log_all_executions,
            log_path: self.log_path.clone(),
            default_goal: goal,
        }
    }
}
