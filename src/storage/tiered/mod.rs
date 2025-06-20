use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieredStorageConfig {
    pub deployment_mode: DeploymentMode,
    pub storage_tiers: Vec<StorageTier>,
    pub tiering_policy: TieringPolicy,
    pub background_migration: BackgroundMigrationConfig,
    pub cache_configuration: CacheConfiguration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeploymentMode {
    /// Container-based deployment (ECS/Fargate/Cloud Run)
    Container {
        ephemeral_storage_gb: u32,
        persistent_volumes: Vec<PersistentVolumeConfig>,
        cloud_storage_backend: CloudStorageBackend,
    },
    /// EC2/VM-based deployment with local disks
    VirtualMachine {
        local_storage_paths: Vec<LocalStorageConfig>,
        cloud_storage_backend: CloudStorageBackend,
        os_optimization: OSOptimizationConfig,
    },
    /// Serverless deployment (Lambda/Functions)
    Serverless {
        tmp_storage_mb: u32,
        cloud_storage_backend: CloudStorageBackend,
        cold_start_optimization: ColdStartOptimization,
    },
    /// Kubernetes deployment
    Kubernetes {
        storage_classes: HashMap<String, String>,
        persistent_volume_claims: Vec<PVCConfig>,
        cloud_storage_backend: CloudStorageBackend,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentVolumeConfig {
    pub name: String,
    pub size_gb: u32,
    pub storage_class: String,
    pub mount_path: PathBuf,
    pub performance_tier: PerformanceTier,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalStorageConfig {
    pub path: PathBuf,
    pub storage_type: LocalStorageType,
    pub size_gb: Option<u32>,
    pub performance_tier: PerformanceTier,
    pub raid_config: Option<RaidConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LocalStorageType {
    /// NVMe SSD for ultra-hot data
    NVMe { iops: u32, throughput_mbps: u32 },
    /// SATA SSD for hot data
    SATASSD { iops: u32 },
    /// Spinning disk for warm data
    HDD { rpm: u32 },
    /// Memory-mapped files
    Memory { max_size_gb: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaidConfig {
    pub raid_level: RaidLevel,
    pub devices: Vec<String>,
    pub stripe_size_kb: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RaidLevel {
    RAID0,  // Striping for performance
    RAID1,  // Mirroring for reliability
    RAID5,  // Parity for balance
    RAID10, // Stripe + Mirror
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CloudStorageBackend {
    AWS {
        s3_bucket: String,
        storage_classes: HashMap<String, S3StorageClass>,
        intelligent_tiering: bool,
        cross_region_replication: Option<String>,
    },
    Azure {
        storage_account: String,
        container_name: String,
        access_tiers: HashMap<String, AzureAccessTier>,
        lifecycle_management: bool,
    },
    GCP {
        bucket_name: String,
        storage_classes: HashMap<String, GCPStorageClass>,
        autoclass_enabled: bool,
    },
    MultiCloud {
        primary: Box<CloudStorageBackend>,
        secondary: Box<CloudStorageBackend>,
        sync_strategy: CloudSyncStrategy,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum S3StorageClass {
    Standard,
    StandardIA, // Infrequent Access
    OneZoneIA,
    Glacier,
    GlacierDeepArchive,
    IntelligentTiering,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AzureAccessTier {
    Hot,
    Cool,
    Archive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GCPStorageClass {
    Standard,
    Nearline,
    Coldline,
    Archive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CloudSyncStrategy {
    ActiveActive,  // Sync to both clouds
    ActivePassive, // Primary with backup
    Geographic,    // Route by user location
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OSOptimizationConfig {
    pub mmap_enabled: bool,
    pub page_cache_optimization: PageCacheConfig,
    pub io_scheduler: IOScheduler,
    pub numa_awareness: bool,
    pub huge_pages_enabled: bool,
    pub direct_io_threshold_mb: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageCacheConfig {
    pub enabled: bool,
    pub drop_caches_on_memory_pressure: bool,
    pub readahead_kb: u32,
    pub dirty_ratio: u8,
    pub dirty_background_ratio: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IOScheduler {
    CFQ,      // Completely Fair Queuing
    NOOP,     // No Operation (for SSDs)
    Deadline, // Deadline scheduler
    MQ,       // Multi-queue block layer
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColdStartOptimization {
    pub warm_cache_size_mb: u32,
    pub prefetch_strategies: Vec<PrefetchStrategy>,
    pub lazy_loading_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PrefetchStrategy {
    RecentlyAccessed,
    PopularCollections,
    TenantSpecific,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PVCConfig {
    pub name: String,
    pub storage_class: String,
    pub size: String,
    pub access_modes: Vec<String>,
    pub mount_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageTier {
    pub tier_name: String,
    pub tier_level: TierLevel,
    pub storage_backend: StorageBackend,
    pub access_pattern: AccessPattern,
    pub retention_policy: RetentionPolicy,
    pub performance_characteristics: PerformanceCharacteristics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TierLevel {
    UltraHot, // L1 cache equivalent - memory/NVMe
    Hot,      // L2 cache equivalent - local SSD
    Warm,     // L3 cache equivalent - local HDD or network storage
    Cold,     // Remote storage - S3 Standard
    Archive,  // Long-term storage - S3 Glacier
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageBackend {
    /// Memory-mapped files with OS page cache
    MMAP {
        file_path: PathBuf,
        advise_random: bool,
        advise_sequential: bool,
        populate_pages: bool,
    },
    /// Local file system
    LocalFS {
        base_path: PathBuf,
        use_direct_io: bool,
        sync_strategy: SyncStrategy,
    },
    /// Cloud object storage
    CloudObjectStore {
        backend: CloudStorageBackend,
        local_cache_path: Option<PathBuf>,
        multipart_threshold_mb: u32,
    },
    /// Distributed block storage
    BlockStorage {
        volume_type: BlockVolumeType,
        mount_point: PathBuf,
        filesystem: FilesystemType,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncStrategy {
    WriteThrough, // Sync on every write
    WriteBack,    // Batch sync
    Async,        // Fire and forget
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BlockVolumeType {
    EBS {
        volume_type: String,
        iops: Option<u32>,
    },
    AzureDisk {
        disk_type: String,
    },
    GCEPersistentDisk {
        disk_type: String,
    },
    NFS {
        server: String,
        export_path: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FilesystemType {
    EXT4,
    XFS,
    ZFS,
    BTRFS,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AccessPattern {
    /// Frequently accessed data - keep in fastest tier
    Sequential {
        prefetch_size_mb: u32,
    },
    Random {
        cache_size_mb: u32,
    },
    WriteOnce {
        compression_enabled: bool,
    },
    ReadMostly {
        replication_factor: u32,
    },
    Hybrid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub hot_retention_hours: u32,
    pub warm_retention_days: u32,
    pub cold_retention_months: u32,
    pub archive_retention_years: Option<u32>,
    pub auto_deletion_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceCharacteristics {
    pub read_latency_ms: LatencyRange,
    pub write_latency_ms: LatencyRange,
    pub throughput_mbps: ThroughputRange,
    pub iops: IOPSRange,
    pub consistency_level: ConsistencyLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LatencyRange {
    pub p50: f32,
    pub p95: f32,
    pub p99: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ThroughputRange {
    pub read_mbps: u32,
    pub write_mbps: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IOPSRange {
    pub read_iops: u32,
    pub write_iops: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConsistencyLevel {
    Immediate,
    Eventual { max_delay_ms: u32 },
    Session,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PerformanceTier {
    UltraHigh, // NVMe, memory
    High,      // SSD
    Medium,    // Fast HDD
    Low,       // Standard HDD
    Archive,   // Tape, cold storage
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TieringPolicy {
    pub auto_tiering_enabled: bool,
    pub promotion_rules: Vec<PromotionRule>,
    pub demotion_rules: Vec<DemotionRule>,
    pub migration_schedule: MigrationSchedule,
    pub cost_optimization: CostOptimizationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromotionRule {
    pub name: String,
    pub condition: TieringCondition,
    pub from_tier: TierLevel,
    pub to_tier: TierLevel,
    pub priority: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DemotionRule {
    pub name: String,
    pub condition: TieringCondition,
    pub from_tier: TierLevel,
    pub to_tier: TierLevel,
    pub priority: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TieringCondition {
    AccessFrequency {
        min_accesses_per_hour: u32,
    },
    LastAccessTime {
        hours_since_access: u32,
    },
    DataAge {
        days_old: u32,
    },
    CostThreshold {
        cost_per_gb_per_month: f32,
    },
    PerformanceRequirement {
        max_latency_ms: u32,
    },
    TenantTier {
        tier: String,
    },
    DataSize {
        min_size_gb: u32,
        max_size_gb: u32,
    },
    Composite {
        conditions: Vec<TieringCondition>,
        operator: LogicalOperator,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogicalOperator {
    AND,
    OR,
    NOT,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationSchedule {
    pub enabled: bool,
    pub migration_windows: Vec<MigrationWindow>,
    pub bandwidth_limit_mbps: u32,
    pub concurrent_migrations: u32,
    pub failure_retry_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationWindow {
    pub name: String,
    pub cron_schedule: String,
    pub duration_hours: u32,
    pub allowed_migrations: Vec<MigrationType>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigrationType {
    Promotion,
    Demotion,
    Rebalancing,
    Compression,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostOptimizationConfig {
    pub enabled: bool,
    pub target_cost_per_gb_per_month: f32,
    pub cost_monitoring_interval_hours: u32,
    pub automatic_adjustments: bool,
    pub cost_alerts: Vec<CostAlert>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostAlert {
    pub threshold_percent: f32,
    pub notification_channels: Vec<String>,
    pub escalation_levels: Vec<EscalationLevel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationLevel {
    pub level: u32,
    pub delay_minutes: u32,
    pub actions: Vec<AutomatedAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AutomatedAction {
    DemoteToLowerTier,
    EnableCompression,
    ReduceReplicationFactor,
    NotifyAdmins,
    DisableAutoScaling,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundMigrationConfig {
    pub enabled: bool,
    pub worker_threads: u32,
    pub migration_batch_size: u32,
    pub throttling: ThrottlingConfig,
    pub monitoring: MigrationMonitoringConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThrottlingConfig {
    pub max_bandwidth_mbps: u32,
    pub max_iops: u32,
    pub backoff_on_error: bool,
    pub priority_queue_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationMonitoringConfig {
    pub progress_reporting_interval_seconds: u32,
    pub success_rate_threshold: f32,
    pub retry_failed_migrations: bool,
    pub dead_letter_queue_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfiguration {
    pub l1_cache: L1CacheConfig, // Memory cache
    pub l2_cache: L2CacheConfig, // Local SSD cache
    pub distributed_cache: Option<DistributedCacheConfig>,
    pub cache_eviction: CacheEvictionPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L1CacheConfig {
    pub enabled: bool,
    pub max_size_mb: u32,
    pub ttl_seconds: u32,
    pub cache_type: L1CacheType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum L1CacheType {
    LRU,  // Least Recently Used
    LFU,  // Least Frequently Used
    ARC,  // Adaptive Replacement Cache
    TwoQ, // Two Queue algorithm
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L2CacheConfig {
    pub enabled: bool,
    pub cache_path: PathBuf,
    pub max_size_gb: u32,
    pub block_size_kb: u32,
    pub write_through: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributedCacheConfig {
    pub cache_type: DistributedCacheType,
    pub nodes: Vec<String>,
    pub replication_factor: u32,
    pub consistency_level: CacheConsistencyLevel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DistributedCacheType {
    Redis { cluster_mode: bool },
    Memcached,
    Hazelcast,
    Custom { endpoint: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CacheConsistencyLevel {
    Strong,
    Eventual,
    Session,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEvictionPolicy {
    pub memory_pressure_threshold: f32,
    pub eviction_strategy: EvictionStrategy,
    pub writeback_on_eviction: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvictionStrategy {
    LRU,
    LFU,
    Random,
    TTL,
    CostBased { cost_function: String },
}

/// Main tiered storage manager
pub struct TieredStorageManager {
    config: TieredStorageConfig,
    storage_backends: HashMap<TierLevel, Box<dyn StorageBackendTrait>>,
    migration_engine: MigrationEngine,
    cache_manager: CacheManager,
    metrics_collector: MetricsCollector,
}

/// Trait for different storage backends
#[async_trait]
pub trait StorageBackendTrait: Send + Sync {
    async fn read(&self, key: &str) -> crate::Result<Option<Vec<u8>>>;
    async fn write(&self, key: &str, data: &[u8]) -> crate::Result<()>;
    async fn delete(&self, key: &str) -> crate::Result<()>;
    async fn list(&self, prefix: &str) -> crate::Result<Vec<String>>;
    async fn metadata(&self, key: &str) -> crate::Result<Option<ObjectMetadata>>;
    fn get_performance_characteristics(&self) -> &PerformanceCharacteristics;
    fn get_tier_level(&self) -> TierLevel;
}

#[derive(Debug, Clone)]
pub struct ObjectMetadata {
    pub size_bytes: u64,
    pub last_modified: chrono::DateTime<chrono::Utc>,
    pub access_count: u64,
    pub last_accessed: chrono::DateTime<chrono::Utc>,
    pub content_type: String,
    pub compression: Option<String>,
    pub encryption: Option<String>,
}

/// Background migration engine
pub struct MigrationEngine {
    config: BackgroundMigrationConfig,
    migration_queue: tokio::sync::mpsc::Receiver<MigrationTask>,
    active_migrations: HashMap<String, MigrationStatus>,
}

#[derive(Debug, Clone)]
pub struct MigrationTask {
    pub task_id: String,
    pub object_key: String,
    pub from_tier: TierLevel,
    pub to_tier: TierLevel,
    pub priority: MigrationPriority,
    pub estimated_size_bytes: u64,
}

#[derive(Debug, Clone)]
pub enum MigrationPriority {
    Critical,
    High,
    Medium,
    Low,
    Background,
}

#[derive(Debug, Clone)]
pub struct MigrationStatus {
    pub task_id: String,
    pub status: MigrationState,
    pub progress_percent: f32,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub estimated_completion: Option<chrono::DateTime<chrono::Utc>>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone)]
pub enum MigrationState {
    Queued,
    InProgress,
    Completed,
    Failed,
    Cancelled,
}

/// Cache manager for multi-level caching
pub struct CacheManager {
    l1_cache: Option<Box<dyn CacheTrait>>,
    l2_cache: Option<Box<dyn CacheTrait>>,
    distributed_cache: Option<Box<dyn CacheTrait>>,
    cache_stats: CacheStats,
}

#[async_trait]
pub trait CacheTrait: Send + Sync {
    async fn get(&self, key: &str) -> crate::Result<Option<Vec<u8>>>;
    async fn put(&self, key: &str, data: &[u8], ttl: Option<u64>) -> crate::Result<()>;
    async fn evict(&self, key: &str) -> crate::Result<()>;
    async fn clear(&self) -> crate::Result<()>;
    fn get_stats(&self) -> CacheStats;
}

#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub size_bytes: u64,
    pub item_count: u64,
}

/// Metrics collection for observability
pub struct MetricsCollector {
    storage_metrics: HashMap<TierLevel, StorageMetrics>,
    migration_metrics: MigrationMetrics,
    cache_metrics: HashMap<String, CacheStats>,
}

#[derive(Debug, Clone, Default)]
pub struct StorageMetrics {
    pub reads_per_second: f32,
    pub writes_per_second: f32,
    pub latency_percentiles: LatencyRange,
    pub throughput_mbps: f32,
    pub error_rate: f32,
    pub utilization_percent: f32,
}

#[derive(Debug, Clone, Default)]
pub struct MigrationMetrics {
    pub active_migrations: u32,
    pub completed_migrations: u64,
    pub failed_migrations: u64,
    pub average_migration_time_seconds: f32,
    pub data_migrated_gb: f64,
}

impl Default for TieredStorageConfig {
    fn default() -> Self {
        Self {
            deployment_mode: DeploymentMode::Container {
                ephemeral_storage_gb: 20,
                persistent_volumes: vec![PersistentVolumeConfig {
                    name: "hot-data".to_string(),
                    size_gb: 100,
                    storage_class: "fast-ssd".to_string(),
                    mount_path: PathBuf::from("/data/hot"),
                    performance_tier: PerformanceTier::High,
                }],
                cloud_storage_backend: CloudStorageBackend::AWS {
                    s3_bucket: "vectordb-storage".to_string(),
                    storage_classes: HashMap::from([
                        ("hot".to_string(), S3StorageClass::Standard),
                        ("warm".to_string(), S3StorageClass::StandardIA),
                        ("cold".to_string(), S3StorageClass::Glacier),
                    ]),
                    intelligent_tiering: true,
                    cross_region_replication: None,
                },
            },
            storage_tiers: vec![StorageTier {
                tier_name: "ultra-hot".to_string(),
                tier_level: TierLevel::UltraHot,
                storage_backend: StorageBackend::MMAP {
                    file_path: PathBuf::from("/data/hot/mmap"),
                    advise_random: true,
                    advise_sequential: false,
                    populate_pages: true,
                },
                access_pattern: AccessPattern::Random {
                    cache_size_mb: 1024,
                },
                retention_policy: RetentionPolicy {
                    hot_retention_hours: 24,
                    warm_retention_days: 7,
                    cold_retention_months: 12,
                    archive_retention_years: Some(7),
                    auto_deletion_enabled: false,
                },
                performance_characteristics: PerformanceCharacteristics {
                    read_latency_ms: LatencyRange {
                        p50: 0.1,
                        p95: 0.5,
                        p99: 1.0,
                    },
                    write_latency_ms: LatencyRange {
                        p50: 0.2,
                        p95: 1.0,
                        p99: 2.0,
                    },
                    throughput_mbps: ThroughputRange {
                        read_mbps: 5000,
                        write_mbps: 3000,
                    },
                    iops: IOPSRange {
                        read_iops: 100000,
                        write_iops: 50000,
                    },
                    consistency_level: ConsistencyLevel::Immediate,
                },
            }],
            tiering_policy: TieringPolicy {
                auto_tiering_enabled: true,
                promotion_rules: vec![PromotionRule {
                    name: "frequent-access".to_string(),
                    condition: TieringCondition::AccessFrequency {
                        min_accesses_per_hour: 10,
                    },
                    from_tier: TierLevel::Warm,
                    to_tier: TierLevel::Hot,
                    priority: 1,
                }],
                demotion_rules: vec![DemotionRule {
                    name: "infrequent-access".to_string(),
                    condition: TieringCondition::LastAccessTime {
                        hours_since_access: 72,
                    },
                    from_tier: TierLevel::Hot,
                    to_tier: TierLevel::Warm,
                    priority: 2,
                }],
                migration_schedule: MigrationSchedule {
                    enabled: true,
                    migration_windows: vec![MigrationWindow {
                        name: "nightly".to_string(),
                        cron_schedule: "0 2 * * *".to_string(),
                        duration_hours: 4,
                        allowed_migrations: vec![
                            MigrationType::Demotion,
                            MigrationType::Compression,
                        ],
                    }],
                    bandwidth_limit_mbps: 100,
                    concurrent_migrations: 4,
                    failure_retry_count: 3,
                },
                cost_optimization: CostOptimizationConfig {
                    enabled: true,
                    target_cost_per_gb_per_month: 0.10,
                    cost_monitoring_interval_hours: 24,
                    automatic_adjustments: false,
                    cost_alerts: vec![],
                },
            },
            background_migration: BackgroundMigrationConfig {
                enabled: true,
                worker_threads: 4,
                migration_batch_size: 100,
                throttling: ThrottlingConfig {
                    max_bandwidth_mbps: 100,
                    max_iops: 1000,
                    backoff_on_error: true,
                    priority_queue_enabled: true,
                },
                monitoring: MigrationMonitoringConfig {
                    progress_reporting_interval_seconds: 60,
                    success_rate_threshold: 0.95,
                    retry_failed_migrations: true,
                    dead_letter_queue_enabled: true,
                },
            },
            cache_configuration: CacheConfiguration {
                l1_cache: L1CacheConfig {
                    enabled: true,
                    max_size_mb: 512,
                    ttl_seconds: 3600,
                    cache_type: L1CacheType::ARC,
                },
                l2_cache: L2CacheConfig {
                    enabled: true,
                    cache_path: PathBuf::from("/data/cache"),
                    max_size_gb: 10,
                    block_size_kb: 64,
                    write_through: false,
                },
                distributed_cache: None,
                cache_eviction: CacheEvictionPolicy {
                    memory_pressure_threshold: 0.8,
                    eviction_strategy: EvictionStrategy::LRU,
                    writeback_on_eviction: true,
                },
            },
        }
    }
}
