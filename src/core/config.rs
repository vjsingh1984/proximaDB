use crate::ai::LLMConfig;
use crate::network::NetworkConfig;
use crate::query::unified::RerankConfig;
use crate::security::SecurityConfig;
use serde::{Deserialize, Serialize};
use tracing::info;

/// Top-level application configuration loaded from `config.toml`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Server identity and bind settings
    pub server: ServerConfig,
    /// Storage engine and data layout settings
    pub storage: StorageConfig,
    /// Raft consensus cluster settings
    pub consensus: ConsensusConfig,
    /// REST/gRPC API settings
    pub api: ApiConfig,
    /// Metrics and logging settings
    pub monitoring: MonitoringConfig,
    /// Network transport configuration (optional)
    pub network: Option<NetworkConfig>,
    /// TLS certificate and key paths (optional)
    pub tls: Option<TlsConfig>,
    /// Hardware detection overrides (optional)
    #[allow(dead_code)]
    pub hardware: Option<HardwareConfig>,
    /// Semantic Knowledge Store configuration (optional)
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
    /// LLM integration configuration (optional)
    pub llm: Option<LLMConfig>,
    /// Async-ingest queue runtime configuration (optional).
    ///
    /// When present, ProximaDB opens the queue subsystem at startup
    /// and async ingest (`/v3/documents?mode=async`) routes through
    /// `producer.send` → background drainer → bulk-load. When absent,
    /// async ingest degrades to inline embed.
    ///
    /// Override precedence (highest wins) — see
    /// [`QueueRuntimeConfig::resolve`] for the implementation:
    ///   1. CLI flag (none today; reserved)
    ///   2. Environment variable (`PROXIMADB_QUEUE_ROOT`,
    ///      `PROXIMADB_QUEUE_OBJECT_ARCHIVE`, `PROXIMADB_QUEUE_SYNC_MODE`,
    ///      `PROXIMADB_EMBED_DRAINER_PARTITIONS`)
    ///   3. TOML `[queue]` section (this field) — the canonical artifact
    ///   4. Built-in defaults
    ///
    /// The TOML is intentionally the authoritative declarative source
    /// (downloadable from object store, version-controlled, identical
    /// across replicas). Env vars exist for per-pod emergency tuning
    /// without rebuilding the artifact.
    pub queue: Option<QueueRuntimeConfig>,
}

pub use proximadb_config::{HardwareConfig, SksConfig, TlsConfig};

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
            llm: None,
            queue: None,
        }
    }
}

/// Queue subsystem runtime configuration. Lives at the `[queue]` TOML
/// section. See `Config.queue` for the precedence rules — short version:
/// the TOML is the canonical declarative source; env vars exist for
/// per-pod emergency overrides; defaults fill in the gaps.
///
/// Wire-format example:
///
/// ```toml
/// [queue]
/// root = "file:///var/lib/proximadb/queue"
/// object_archive = "adls://anvaiops/queue-cold"   # optional
/// sync_mode = "strict"                             # "strict" or "lazy"
/// drainer_partitions = "0..16"                    # this replica's range
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueueRuntimeConfig {
    /// Filesystem URL where queue segments live (e.g.
    /// `file:///var/lib/proximadb/queue`, `adls://...`, `s3://...`).
    /// Required to enable async ingest.
    pub root: Option<String>,
    /// Optional second-tier archive URL. Sealed segments are mirrored
    /// here so the queue survives ECS pod loss / k8s reschedule onto a
    /// fresh-disk node.
    pub object_archive: Option<String>,
    /// `"strict"` (default, fsync-before-ack) or `"lazy"`.
    pub sync_mode: Option<String>,
    /// Partition range this replica drains, e.g. `"0..16"` or
    /// `"0,3..6,9"`. Single-replica deployments leave it unset; multi-
    /// replica deployments pass disjoint ranges per pod (the cross-
    /// process leases enforce ownership).
    pub drainer_partitions: Option<String>,
}

impl QueueRuntimeConfig {
    /// Build the runtime queue settings by layering env over TOML over
    /// defaults. Returns `None` when none of the layers supplied a
    /// `root` URL — that's the explicit signal that async ingest is
    /// disabled.
    ///
    /// Precedence (highest wins):
    /// 1. **Environment variable** (`PROXIMADB_QUEUE_*`) — per-pod
    ///    emergency override. Set in the k8s/ECS task spec.
    /// 2. **TOML `[queue]` section** — canonical declarative artifact.
    ///    Downloadable from object store; identical across replicas.
    /// 3. **Compiled-in defaults** — `sync_mode = "strict"`,
    ///    `drainer_partitions = "0..16"`.
    ///
    /// A future CLI-flag layer (e.g. `--queue-root <url>`) would slot
    /// above env without changing the return type.
    pub fn resolve(toml_section: Option<&QueueRuntimeConfig>) -> Option<ResolvedQueueConfig> {
        let from_toml = toml_section.cloned().unwrap_or_default();
        let root = std::env::var("PROXIMADB_QUEUE_ROOT")
            .ok()
            .or(from_toml.root.clone());
        // No root anywhere → async ingest stays off entirely.
        let root = root?;

        let object_archive = std::env::var("PROXIMADB_QUEUE_OBJECT_ARCHIVE")
            .ok()
            .or(from_toml.object_archive.clone());
        let sync_mode = std::env::var("PROXIMADB_QUEUE_SYNC_MODE")
            .ok()
            .or(from_toml.sync_mode.clone())
            .unwrap_or_else(|| "strict".to_string());
        let drainer_partitions = std::env::var("PROXIMADB_EMBED_DRAINER_PARTITIONS")
            .ok()
            .or(from_toml.drainer_partitions.clone())
            .unwrap_or_else(|| "0..16".to_string());

        Some(ResolvedQueueConfig {
            root,
            object_archive,
            sync_mode,
            drainer_partitions,
        })
    }
}

/// Resolved queue configuration after the precedence layers are
/// folded. All fields are populated (no Options) so callers can
/// consume directly without per-field defaulting.
#[derive(Debug, Clone)]
pub struct ResolvedQueueConfig {
    pub root: String,
    pub object_archive: Option<String>,
    pub sync_mode: String,
    pub drainer_partitions: String,
}

#[cfg(test)]
mod queue_config_tests {
    use super::*;

    /// Restoring env vars after a test mutates them is fragile (other
    /// tests run in the same process and see the changes). Each test
    /// scopes its env reads via this guard, which restores the value
    /// on drop.
    struct EnvGuard {
        key: String,
        original: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &str, value: Option<&str>) -> Self {
            let original = std::env::var(key).ok();
            match value {
                Some(v) => unsafe { std::env::set_var(key, v) },
                None => unsafe { std::env::remove_var(key) },
            }
            EnvGuard {
                key: key.to_string(),
                original,
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.original.take() {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    /// Without any TOML section AND without env vars, async ingest is
    /// disabled — resolve returns None. This is the "no behavior
    /// change for unaware deployments" guarantee.
    #[test]
    fn resolve_returns_none_when_no_toml_and_no_env() {
        let _g_root = EnvGuard::set("PROXIMADB_QUEUE_ROOT", None);
        let _g_arc = EnvGuard::set("PROXIMADB_QUEUE_OBJECT_ARCHIVE", None);
        let _g_sync = EnvGuard::set("PROXIMADB_QUEUE_SYNC_MODE", None);
        let _g_part = EnvGuard::set("PROXIMADB_EMBED_DRAINER_PARTITIONS", None);
        assert!(QueueRuntimeConfig::resolve(None).is_none());
        assert!(QueueRuntimeConfig::resolve(Some(&QueueRuntimeConfig::default())).is_none());
    }

    /// TOML-only flow: a TOML `[queue]` section with `root` set
    /// activates the queue. Defaults fill in everything else.
    #[test]
    fn resolve_uses_toml_when_env_unset() {
        let _g_root = EnvGuard::set("PROXIMADB_QUEUE_ROOT", None);
        let _g_arc = EnvGuard::set("PROXIMADB_QUEUE_OBJECT_ARCHIVE", None);
        let _g_sync = EnvGuard::set("PROXIMADB_QUEUE_SYNC_MODE", None);
        let _g_part = EnvGuard::set("PROXIMADB_EMBED_DRAINER_PARTITIONS", None);
        let toml = QueueRuntimeConfig {
            root: Some("file:///srv/queue".to_string()),
            object_archive: Some("adls://anvaiops/archive".to_string()),
            sync_mode: Some("lazy".to_string()),
            drainer_partitions: Some("0..4".to_string()),
        };
        let resolved = QueueRuntimeConfig::resolve(Some(&toml)).expect("resolved");
        assert_eq!(resolved.root, "file:///srv/queue");
        assert_eq!(resolved.object_archive.as_deref(), Some("adls://anvaiops/archive"));
        assert_eq!(resolved.sync_mode, "lazy");
        assert_eq!(resolved.drainer_partitions, "0..4");
    }

    /// Env beats TOML — the documented precedence in `Config.queue`.
    /// All four fields swap when env is set even when TOML provided a
    /// different value.
    #[test]
    fn resolve_env_overrides_toml() {
        let _g_root = EnvGuard::set("PROXIMADB_QUEUE_ROOT", Some("file:///emergency"));
        let _g_arc = EnvGuard::set("PROXIMADB_QUEUE_OBJECT_ARCHIVE", Some("s3://hotfix"));
        let _g_sync = EnvGuard::set("PROXIMADB_QUEUE_SYNC_MODE", Some("strict"));
        let _g_part = EnvGuard::set("PROXIMADB_EMBED_DRAINER_PARTITIONS", Some("8..16"));
        let toml = QueueRuntimeConfig {
            root: Some("file:///canonical".to_string()),
            object_archive: Some("adls://canonical".to_string()),
            sync_mode: Some("lazy".to_string()),
            drainer_partitions: Some("0..16".to_string()),
        };
        let resolved = QueueRuntimeConfig::resolve(Some(&toml)).expect("resolved");
        assert_eq!(resolved.root, "file:///emergency");
        assert_eq!(resolved.object_archive.as_deref(), Some("s3://hotfix"));
        assert_eq!(resolved.sync_mode, "strict");
        assert_eq!(resolved.drainer_partitions, "8..16");
    }

    /// Env-only flow: when there's no TOML section at all, env vars
    /// can still activate the queue (useful for ad-hoc local runs).
    #[test]
    fn resolve_env_alone_activates_queue() {
        let _g_root = EnvGuard::set("PROXIMADB_QUEUE_ROOT", Some("file:///env-only"));
        let _g_arc = EnvGuard::set("PROXIMADB_QUEUE_OBJECT_ARCHIVE", None);
        let _g_sync = EnvGuard::set("PROXIMADB_QUEUE_SYNC_MODE", None);
        let _g_part = EnvGuard::set("PROXIMADB_EMBED_DRAINER_PARTITIONS", None);
        let resolved = QueueRuntimeConfig::resolve(None).expect("resolved");
        assert_eq!(resolved.root, "file:///env-only");
        assert!(resolved.object_archive.is_none());
        assert_eq!(resolved.sync_mode, "strict", "default fills in");
        assert_eq!(resolved.drainer_partitions, "0..16", "default fills in");
    }

    /// Per-field precedence: env can override SOME fields while TOML
    /// supplies others. This is the "partial emergency tuning" case —
    /// e.g., a hot pod gets a different partition range without
    /// touching the canonical TOML.
    #[test]
    fn resolve_per_field_override() {
        let _g_root = EnvGuard::set("PROXIMADB_QUEUE_ROOT", None);
        let _g_arc = EnvGuard::set("PROXIMADB_QUEUE_OBJECT_ARCHIVE", None);
        let _g_sync = EnvGuard::set("PROXIMADB_QUEUE_SYNC_MODE", None);
        let _g_part =
            EnvGuard::set("PROXIMADB_EMBED_DRAINER_PARTITIONS", Some("12..16"));
        let toml = QueueRuntimeConfig {
            root: Some("file:///canonical".to_string()),
            object_archive: Some("adls://canonical".to_string()),
            sync_mode: Some("strict".to_string()),
            drainer_partitions: Some("0..16".to_string()),
        };
        let resolved = QueueRuntimeConfig::resolve(Some(&toml)).expect("resolved");
        // TOML wins for the unmasked fields...
        assert_eq!(resolved.root, "file:///canonical");
        assert_eq!(resolved.object_archive.as_deref(), Some("adls://canonical"));
        assert_eq!(resolved.sync_mode, "strict");
        // ...env wins for the one field that's set.
        assert_eq!(resolved.drainer_partitions, "12..16");
    }

    /// Round-trip the [queue] section through serde + TOML to lock the
    /// wire format. Future renames will fail this test loudly.
    #[test]
    fn queue_section_round_trips_through_toml() {
        let original = QueueRuntimeConfig {
            root: Some("file:///canonical".to_string()),
            object_archive: Some("adls://anvaiops/archive".to_string()),
            sync_mode: Some("strict".to_string()),
            drainer_partitions: Some("0..16".to_string()),
        };
        let serialized = toml::to_string(&original).expect("ser");
        // Lock the literal field names — these are the public TOML
        // surface and changing them is a breaking config-file change.
        assert!(serialized.contains("root ="));
        assert!(serialized.contains("object_archive ="));
        assert!(serialized.contains("sync_mode ="));
        assert!(serialized.contains("drainer_partitions ="));
        let restored: QueueRuntimeConfig = toml::from_str(&serialized).expect("de");
        assert_eq!(restored.root, original.root);
        assert_eq!(restored.object_archive, original.object_archive);
        assert_eq!(restored.sync_mode, original.sync_mode);
        assert_eq!(restored.drainer_partitions, original.drainer_partitions);
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

/// Configuration for periodic cache rebalancing across cache tiers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheRebalancingConfig {
    /// Whether automatic rebalancing is enabled
    pub enabled: bool,
    /// Seconds between rebalancing passes
    pub interval_seconds: u64,
    /// Minimum hit-rate below which a cache tier is considered cold
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

/// Configuration for cache eviction policies and thresholds
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEvictionConfig {
    /// Whether automatic eviction is enabled
    pub enabled: bool,
    /// Seconds between eviction checks
    pub check_interval_seconds: u64,
    /// Number of entries to evict per batch
    pub batch_size: usize,
    /// Memory usage percentage that triggers eviction
    pub memory_threshold_percent: u8,
    /// Ordered list of eviction policies to apply
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

/// Configuration for a single eviction policy (LRU, TTL, etc.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvictionPolicyConfig {
    /// Policy type identifier (e.g., "lru", "ttl")
    #[serde(rename = "type")]
    pub policy_type: String,
    /// Maximum number of cached items before eviction starts
    pub max_items: Option<usize>,
    /// Number of entries to evict at once
    pub batch_size: Option<usize>,
    /// Maximum age in seconds before an entry expires (TTL policy)
    pub max_age_seconds: Option<u64>,
    /// Seconds between TTL cleanup sweeps
    pub cleanup_interval_seconds: Option<u64>,
}

/// Per-cache-type memory allocation configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheTypesConfig {
    /// Allocation for the vector data cache
    pub vector: CacheTypeConfig,
    /// Allocation for the query result cache
    pub query: CacheTypeConfig,
    /// Allocation for the metadata cache
    pub metadata: CacheTypeConfig,
    /// Allocation for the index node cache
    pub index: CacheTypeConfig,
    /// Allocation for the bitmap filter cache
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

/// Memory allocation bounds for a single cache type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheTypeConfig {
    /// Initial memory allocation in megabytes
    pub initial_allocation_mb: u64,
    /// Minimum memory allocation in megabytes
    pub min_allocation_mb: u64,
    /// Maximum memory allocation in megabytes
    pub max_allocation_mb: u64,
}

/// Configuration for proactive cache warming strategies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheWarmingConfig {
    /// Warming strategy names (e.g., "popularity", "time_based")
    pub strategies: Vec<String>,
    /// Whether to warm the cache immediately on server startup
    pub warm_on_startup: bool,
    /// Number of entries to warm per batch
    pub warm_batch_size: usize,
    /// Minimum access count for an entry to qualify for popularity warming
    pub popularity_threshold: u32,
    /// Lookback window in hours for time-based warming
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

pub use proximadb_config::{GraphRuntimeConfig, HybridRuntimeConfig};

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

pub use proximadb_config::ServerConfig;

/// Storage engine layout, WAL, compaction, and filesystem optimization settings
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

    /// Enable memory-mapped I/O for storage engines
    pub mmap_enabled: bool,
    /// SST (Sorted String Table) engine configuration
    pub sst_config: Option<SstConfig>,
    /// VIPER columnar engine configuration
    pub viper_config: Option<ViperConfig>,
    /// Global read cache size in megabytes
    pub cache_size_mb: u64,
    /// Bloom filter settings for metadata filtering
    pub bloom_filter_config: Option<BloomFilterConfig>,

    /// Common compaction configuration (can be overridden per engine)
    pub compaction_config: CompactionConfig,

    /// Filesystem optimization settings
    pub filesystem_config: FilesystemOptimizationConfig,

    /// Performance optimization settings
    #[serde(default)]
    pub optimization: OptimizationConfig,
}

pub use proximadb_config::{
    AdvancedPruneConfig, AssignmentConfig, AzureConfig, CloudStorageConfig, CompactionConfig,
    ConsensusConfig, FilesystemOptimizationConfig, GcsConfig, MetadataBackendConfig,
    MonitoringConfig, OptimizationConfig, PruneModeConfig, S3Config, StorageLocation, TempStrategy,
    TransactionalOperationsConfig,
};

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
            encryption: Default::default(), // TD-016: Encryption disabled by default
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
        Option<crate::storage::engines::sst::decompression_cache::CacheConfig>,

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

    /// Block storage format: Controls how blocks are serialized to disk
    ///
    /// # Available Formats:
    ///
    /// * `"ProximaBlocks"` (DEFAULT) - ProximaDB's native block format
    ///   - Optimized for vector workloads with cache-line alignment
    ///   - B+ tree index for O(log n) ID lookups
    ///   - Supports quantization and compression
    ///   - Best for: Production vector databases
    ///
    /// * `"ArrowBlock"` - Arrow IPC based storage format
    ///   - Standard Arrow IPC files (compatible with PyArrow, DuckDB, Polars)
    ///   - Zero-copy reads via memory mapping
    ///   - Sidecar B+ tree index file (.idx)
    ///   - Best for: Interoperability with Arrow ecosystem
    ///
    /// # Configuration Example:
    ///
    /// ```toml
    /// [storage.sst_config]
    /// # Use Arrow IPC format for interoperability
    /// block_format = "ArrowBlock"
    ///
    /// # Use ProximaBlocks for production (default)
    /// block_format = "ProximaBlocks"
    /// ```
    ///
    /// Default: ProximaBlocks
    #[serde(default = "default_block_format")]
    pub block_format: String,
}

fn default_block_format() -> String {
    "ProximaBlocks".to_string()
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
            row_group_size: 65536,           // ~32MB row groups for 128D vectors
            compression: "zstd".to_string(), // ZSTD for better compression
            compression_level: 3,            // Balanced speed/compression
            enable_statistics: true,
            data_directory: "./data/viper_data".to_string(),
            cache_size_mb: 512,
            compaction_config: None, // Use common config by default
        }
    }
}
#[allow(dead_code)]
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
                crate::storage::engines::sst::decompression_cache::CacheConfig::default(),
            ),
            vector_encoding_strategy: default_vector_encoding_strategy(),
            block_format: default_block_format(),
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
        Self {
            block_size_kb: 256,              // Small blocks for quantization clustering tests
            compression: "zstd".to_string(), // Zstd compression for tests
            compression_level: 3,            // Balanced compression level
            ..Default::default()
        }
    }

    /// Create test-specific SST configuration with 512KB blocks
    pub fn test_config_512kb() -> Self {
        Self {
            block_size_kb: 512,
            compression: "zstd".to_string(), // Zstd compression for tests
            compression_level: 3,            // Balanced compression level
            ..Default::default()
        }
    }
}

pub use proximadb_config::ApiConfig;

pub use proximadb_config::{WalDistributionStrategy, WalStorageConfig};

// Helper functions for serde defaults
#[allow(dead_code)]
fn default_collection_affinity() -> bool {
    true
}
#[allow(dead_code)]
fn default_memory_flush_size() -> usize {
    2 * 1024 * 1024 // 2MB - reduced for faster recovery as per CLAUDE.md
}
#[allow(dead_code)]
fn default_global_flush_threshold() -> usize {
    4 * 1024 * 1024 * 1024 // 4GB - recommended for global memory threshold
}
#[allow(dead_code)]
fn default_strategy_type() -> Option<String> {
    None
}
#[allow(dead_code)]
fn default_memtable_type() -> Option<String> {
    None
}
#[allow(dead_code)]
fn default_sync_mode() -> Option<String> {
    None
}
#[allow(dead_code)]
fn default_batch_threshold() -> Option<usize> {
    None
}
#[allow(dead_code)]
fn default_write_buffer_size_mb() -> Option<usize> {
    None
}
#[allow(dead_code)]
fn default_concurrent_flushes() -> Option<usize> {
    None
}
#[allow(dead_code)]
fn default_global_shrink_factor() -> Option<f64> {
    Some(0.4) // 40% shrink factor - recommended for global threshold management
}

/// Query processing configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QueryConfig {
    /// RL-based adaptive query planner configuration
    #[serde(default)]
    pub rl_planner: RLPlannerConfig,
    /// Cross-modal reranking policy. Defaults are neutral and disabled.
    #[serde(default)]
    pub reranking: RerankConfig,
}

impl QueryConfig {
    /// Validate query-scoped runtime policy before the server starts.
    pub fn validate(&self) -> anyhow::Result<()> {
        self.reranking.validate()
    }
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
