use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalCoordinationConfig {
    pub deployment_topology: DeploymentTopology,
    pub consistency_model: ConsistencyModel,
    pub replication_strategy: ReplicationStrategy,
    pub conflict_resolution: ConflictResolution,
    pub disaster_recovery: DisasterRecoveryConfig,
    pub metadata_sharding: MetadataShardingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeploymentTopology {
    /// Single region for MVP
    SingleRegion {
        region: String,
        availability_zones: Vec<String>,
        cross_az_replication: bool,
    },
    /// Multi-region with primary-secondary
    MultiRegionActive {
        primary_region: String,
        secondary_regions: Vec<String>,
        failover_strategy: FailoverStrategy,
    },
    /// Multi-region active-active
    MultiRegionActiveActive {
        regions: HashMap<String, RegionConfig>,
        global_load_balancing: GlobalLoadBalancingConfig,
        data_locality_rules: DataLocalityConfig,
    },
    /// Global mesh for maximum availability
    GlobalMesh {
        regions: HashMap<String, RegionConfig>,
        edge_locations: Vec<EdgeLocationConfig>,
        intelligent_routing: IntelligentRoutingConfig,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionConfig {
    pub region_id: String,
    pub availability_zones: Vec<String>,
    pub capacity_weight: f32,
    pub latency_zone: LatencyZone,
    pub data_residency_constraints: Vec<String>,
    pub disaster_recovery_tier: DRTier,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LatencyZone {
    UltraLow, // < 10ms
    Low,      // < 50ms
    Medium,   // < 100ms
    High,     // < 500ms
    Best,     // Best effort
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DRTier {
    Primary, // Main serving region
    Hot,     // Hot standby (real-time sync)
    Warm,    // Warm standby (near real-time sync)
    Cold,    // Cold backup (periodic sync)
    Archive, // Long-term archive
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeLocationConfig {
    pub location_id: String,
    pub parent_region: String,
    pub caching_enabled: bool,
    pub read_only: bool,
    pub cache_ttl_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConsistencyModel {
    /// Immediate consistency across all regions (high latency)
    Strong,
    /// Eventual consistency with bounded staleness  
    EventualBounded { max_staleness_seconds: u64 },
    /// Session consistency per tenant/user
    Session,
    /// Causal consistency maintaining order
    Causal,
    /// Per-operation consistency configuration
    Configurable { default: Box<ConsistencyModel> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReplicationStrategy {
    /// Synchronous replication (strong consistency, high latency)
    Synchronous { min_replicas: u32, timeout_ms: u32 },
    /// Asynchronous replication (eventual consistency, low latency)
    Asynchronous {
        replication_lag_target_ms: u32,
        batch_size: u32,
    },
    /// Hybrid: sync within region, async cross-region
    Hybrid {
        intra_region: Box<ReplicationStrategy>,
        inter_region: Box<ReplicationStrategy>,
    },
    /// Quorum-based replication
    Quorum {
        read_quorum: u32,
        write_quorum: u32,
        total_replicas: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConflictResolution {
    /// Last writer wins (simple, may lose data)
    LastWriterWins,
    /// Timestamp-based with vector clocks
    VectorClock,
    /// Application-defined conflict resolution
    ApplicationDefined { handler: String },
    /// Multi-version with user choice
    MultiVersion { max_versions: u32 },
    /// CRDT (Conflict-free Replicated Data Types)
    CRDT { crdt_type: CRDTType },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CRDTType {
    GCounter,    // Grow-only counter
    PNCounter,   // Increment/decrement counter
    GSet,        // Grow-only set
    ORSet,       // Observed-remove set
    LWWRegister, // Last-writer-wins register
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisasterRecoveryConfig {
    pub enabled: bool,
    pub rto_seconds: u32, // Recovery Time Objective
    pub rpo_seconds: u32, // Recovery Point Objective
    pub backup_strategy: BackupStrategy,
    pub failover_automation: FailoverAutomation,
    pub testing_schedule: DRTestingSchedule,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackupStrategy {
    /// Continuous backup with point-in-time recovery
    Continuous { retention_days: u32 },
    /// Scheduled snapshots
    Snapshot {
        frequency_hours: u32,
        retention_days: u32,
        cross_region_copy: bool,
    },
    /// Hybrid approach
    Hybrid {
        continuous_retention_hours: u32,
        snapshot_frequency_hours: u32,
        long_term_retention_days: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailoverAutomation {
    pub enabled: bool,
    pub health_check_interval_seconds: u32,
    pub failure_threshold_count: u32,
    pub automatic_failback: bool,
    pub notification_channels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DRTestingSchedule {
    pub enabled: bool,
    pub frequency_days: u32,
    pub test_type: DRTestType,
    pub rollback_testing: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DRTestType {
    NonDisruptive, // Test without affecting production
    Failover,      // Actual failover test
    FullDR,        // Complete disaster recovery simulation
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataShardingConfig {
    pub sharding_strategy: MetadataShardingStrategy,
    pub shard_count: u32,
    pub rebalancing: ShardRebalancingConfig,
    pub hot_shard_detection: HotShardConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MetadataShardingStrategy {
    /// Hash-based sharding by tenant ID
    HashByTenant,
    /// Range-based sharding by tenant ID
    RangeByTenant { ranges: Vec<TenantRange> },
    /// Geographic sharding
    Geographic {
        region_assignments: HashMap<String, Vec<String>>,
    },
    /// Workload-based sharding
    WorkloadBased {
        oltp_shards: Vec<u32>,
        olap_shards: Vec<u32>,
        ml_shards: Vec<u32>,
    },
    /// Hierarchical sharding (tenant -> collection -> data)
    Hierarchical {
        tenant_shards: u32,
        collection_shards_per_tenant: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantRange {
    pub start: String,
    pub end: String,
    pub shard_id: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardRebalancingConfig {
    pub enabled: bool,
    pub trigger_threshold: f32, // Imbalance threshold (0.1 = 10%)
    pub rebalancing_strategy: RebalancingStrategy,
    pub migration_rate_limit: u32, // MB/s
    pub maintenance_window: Option<MaintenanceWindow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RebalancingStrategy {
    Immediate,
    Scheduled {
        schedule: String,
    }, // Cron-like schedule
    LoadThreshold {
        cpu_threshold: f32,
        memory_threshold: f32,
    },
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenanceWindow {
    pub timezone: String,
    pub start_hour: u8,
    pub duration_hours: u8,
    pub days_of_week: Vec<u8>, // 0=Sunday, 1=Monday, etc.
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotShardConfig {
    pub detection_enabled: bool,
    pub cpu_threshold: f32,
    pub memory_threshold: f32,
    pub request_rate_threshold: f32,
    pub mitigation_strategies: Vec<HotShardMitigation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HotShardMitigation {
    /// Add read replicas for hot shards
    ScaleOut { max_replicas: u32 },
    /// Move hot tenants to dedicated infrastructure
    TenantIsolation,
    /// Cache frequently accessed data
    AggressiveCaching { cache_ttl_seconds: u64 },
    /// Split hot shards
    ShardSplitting { split_threshold: f32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalLoadBalancingConfig {
    pub strategy: GlobalLBStrategy,
    pub health_checks: GlobalHealthCheckConfig,
    pub traffic_distribution: TrafficDistributionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GlobalLBStrategy {
    /// Route to nearest region by latency
    LatencyBased,
    /// Route based on region capacity
    CapacityBased,
    /// Route based on cost optimization
    CostOptimized,
    /// Hybrid strategy with weights
    Weighted {
        latency_weight: f32,
        capacity_weight: f32,
        cost_weight: f32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalHealthCheckConfig {
    pub interval_seconds: u32,
    pub timeout_seconds: u32,
    pub failure_threshold: u32,
    pub success_threshold: u32,
    pub health_check_endpoints: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrafficDistributionConfig {
    pub default_weights: HashMap<String, f32>, // region -> weight
    pub tenant_overrides: HashMap<String, HashMap<String, f32>>, // tenant -> region -> weight
    pub canary_traffic_percent: f32,
    pub circuit_breaker_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataLocalityConfig {
    /// Data residency requirements per tenant/region
    pub residency_rules: HashMap<String, DataResidencyRule>,
    /// Cross-border data transfer restrictions
    pub transfer_restrictions: Vec<TransferRestriction>,
    /// Data sovereignty compliance
    pub sovereignty_compliance: HashMap<String, ComplianceFramework>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataResidencyRule {
    pub tenant_pattern: String, // Regex pattern for tenant matching
    pub allowed_regions: Vec<String>,
    pub prohibited_regions: Vec<String>,
    pub encryption_required: bool,
    pub audit_logging: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferRestriction {
    pub from_region: String,
    pub to_region: String,
    pub allowed: bool,
    pub encryption_required: bool,
    pub approval_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ComplianceFramework {
    GDPR,   // European Union
    CCPA,   // California
    PIPEDA, // Canada
    LGPD,   // Brazil
    Custom {
        name: String,
        requirements: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntelligentRoutingConfig {
    pub ml_enabled: bool,
    pub learning_period_days: u32,
    pub routing_features: Vec<RoutingFeature>,
    pub feedback_loop_enabled: bool,
    pub model_update_frequency_hours: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RoutingFeature {
    Latency,
    Throughput,
    ErrorRate,
    Cost,
    UserLocation,
    TimeOfDay,
    DayOfWeek,
    WorkloadType,
    TenantTier,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FailoverStrategy {
    /// Manual failover with human approval
    Manual,
    /// Automatic failover based on health checks
    Automatic {
        health_check_failures: u32,
        failover_timeout_seconds: u32,
    },
    /// Gradual traffic shifting
    Gradual {
        traffic_shift_percent_per_minute: f32,
        max_shift_duration_minutes: u32,
    },
}

/// Global metadata coordinator
pub struct GlobalMetadataCoordinator {
    config: GlobalCoordinationConfig,
    region_states: HashMap<String, RegionState>,
    global_metadata_cache: GlobalMetadataCache,
    coordination_protocol: CoordinationProtocol,
}

#[derive(Debug, Clone)]
pub struct RegionState {
    pub region_id: String,
    pub status: RegionStatus,
    pub last_heartbeat: DateTime<Utc>,
    pub lag_seconds: u32,
    pub pending_operations: u32,
    pub health_score: f32,
}

#[derive(Debug, Clone)]
pub enum RegionStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Maintenance,
    Recovering,
}

/// Cache for globally distributed metadata
pub struct GlobalMetadataCache {
    /// Tenant metadata distributed across regions
    tenant_metadata: HashMap<String, TenantMetadata>,
    /// Collection metadata with location affinity
    collection_metadata: HashMap<String, CollectionMetadata>,
    /// Schema definitions cached globally
    schema_cache: HashMap<String, SchemaMetadata>,
    /// Routing table for efficient request routing
    routing_table: RoutingTable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantMetadata {
    pub tenant_id: String,
    pub account_tier: String,
    pub home_region: String,
    pub allowed_regions: Vec<String>,
    pub resource_quotas: HashMap<String, u64>,
    pub data_residency_requirements: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub last_updated: DateTime<Utc>,
    pub version: u64, // For conflict resolution
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionMetadata {
    pub collection_id: String,
    pub tenant_id: String,
    pub schema_id: String,
    pub primary_region: String,
    pub replica_regions: Vec<String>,
    pub sharding_strategy: String,
    pub size_bytes: u64,
    pub vector_count: u64,
    pub last_accessed: DateTime<Utc>,
    pub access_pattern: AccessPattern,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AccessPattern {
    Hot,     // Frequently accessed
    Warm,    // Occasionally accessed
    Cold,    // Rarely accessed
    Archive, // Long-term storage
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaMetadata {
    pub schema_id: String,
    pub schema_type: String,
    pub version: u32,
    pub fields: HashMap<String, FieldMetadata>,
    pub indexes: Vec<IndexMetadata>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldMetadata {
    pub field_name: String,
    pub field_type: String,
    pub nullable: bool,
    pub indexed: bool,
    pub compression: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMetadata {
    pub index_name: String,
    pub index_type: String,
    pub fields: Vec<String>,
    pub unique: bool,
    pub partial: bool,
}

/// Routing table for efficient request routing
#[derive(Debug, Clone)]
pub struct RoutingTable {
    pub tenant_routes: HashMap<String, TenantRoute>,
    pub collection_routes: HashMap<String, CollectionRoute>,
    pub region_topology: RegionTopology,
    pub last_updated: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct TenantRoute {
    pub tenant_id: String,
    pub primary_region: String,
    pub replica_regions: Vec<String>,
    pub load_balancing_weights: HashMap<String, f32>,
    pub preferred_az: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CollectionRoute {
    pub collection_id: String,
    pub shard_locations: HashMap<u32, String>, // shard_id -> region
    pub read_preferences: ReadPreference,
    pub write_preference: WritePreference,
}

#[derive(Debug, Clone)]
pub enum ReadPreference {
    Primary,
    PrimaryPreferred,
    Secondary,
    SecondaryPreferred,
    Nearest,
}

#[derive(Debug, Clone)]
pub enum WritePreference {
    Primary,
    Majority,
    Acknowledged,
}

#[derive(Debug, Clone)]
pub struct RegionTopology {
    pub regions: HashMap<String, RegionInfo>,
    pub latency_matrix: HashMap<(String, String), u32>, // (from, to) -> latency_ms
    pub bandwidth_matrix: HashMap<(String, String), u32>, // (from, to) -> bandwidth_mbps
}

#[derive(Debug, Clone)]
pub struct RegionInfo {
    pub region_id: String,
    pub availability_zones: Vec<String>,
    pub coordinates: Option<(f32, f32)>, // lat, lon for distance calculations
    pub tier: RegionTier,
    pub capacity: RegionCapacity,
}

#[derive(Debug, Clone)]
pub enum RegionTier {
    Tier1, // Primary regions with full capabilities
    Tier2, // Secondary regions with most capabilities
    Edge,  // Edge locations with caching only
}

#[derive(Debug, Clone)]
pub struct RegionCapacity {
    pub max_tenants: u32,
    pub max_collections: u32,
    pub max_vectors: u64,
    pub current_utilization: f32,
}

/// Coordination protocol for global consensus
pub enum CoordinationProtocol {
    /// Raft consensus (strong consistency, limited scale)
    Raft {
        leader_region: String,
        follower_regions: Vec<String>,
    },
    /// Multi-Paxos (better for geo-distributed)
    MultiPaxos {
        acceptors: Vec<String>,
        proposers: Vec<String>,
    },
    /// Eventually consistent with vector clocks
    EventualConsistency {
        conflict_resolution: ConflictResolution,
        gossip_interval_seconds: u32,
    },
    /// Hybrid: strong within region, eventual across regions
    Hybrid {
        intra_region_protocol: Box<CoordinationProtocol>,
        inter_region_protocol: Box<CoordinationProtocol>,
    },
}

impl Default for GlobalCoordinationConfig {
    fn default() -> Self {
        Self {
            deployment_topology: DeploymentTopology::SingleRegion {
                region: "us-east-1".to_string(),
                availability_zones: vec!["us-east-1a".to_string(), "us-east-1b".to_string()],
                cross_az_replication: true,
            },
            consistency_model: ConsistencyModel::EventualBounded {
                max_staleness_seconds: 60,
            },
            replication_strategy: ReplicationStrategy::Hybrid {
                intra_region: Box::new(ReplicationStrategy::Synchronous {
                    min_replicas: 2,
                    timeout_ms: 5000,
                }),
                inter_region: Box::new(ReplicationStrategy::Asynchronous {
                    replication_lag_target_ms: 30000,
                    batch_size: 1000,
                }),
            },
            conflict_resolution: ConflictResolution::VectorClock,
            disaster_recovery: DisasterRecoveryConfig {
                enabled: true,
                rto_seconds: 300, // 5 minutes
                rpo_seconds: 60,  // 1 minute
                backup_strategy: BackupStrategy::Continuous { retention_days: 30 },
                failover_automation: FailoverAutomation {
                    enabled: true,
                    health_check_interval_seconds: 30,
                    failure_threshold_count: 3,
                    automatic_failback: false,
                    notification_channels: vec!["slack".to_string(), "email".to_string()],
                },
                testing_schedule: DRTestingSchedule {
                    enabled: true,
                    frequency_days: 90,
                    test_type: DRTestType::NonDisruptive,
                    rollback_testing: true,
                },
            },
            metadata_sharding: MetadataShardingConfig {
                sharding_strategy: MetadataShardingStrategy::HashByTenant,
                shard_count: 16,
                rebalancing: ShardRebalancingConfig {
                    enabled: true,
                    trigger_threshold: 0.2,
                    rebalancing_strategy: RebalancingStrategy::Scheduled {
                        schedule: "0 2 * * SUN".to_string(), // Sunday 2 AM
                    },
                    migration_rate_limit: 100, // 100 MB/s
                    maintenance_window: Some(MaintenanceWindow {
                        timezone: "UTC".to_string(),
                        start_hour: 2,
                        duration_hours: 4,
                        days_of_week: vec![0, 6], // Sunday, Saturday
                    }),
                },
                hot_shard_detection: HotShardConfig {
                    detection_enabled: true,
                    cpu_threshold: 0.8,
                    memory_threshold: 0.8,
                    request_rate_threshold: 1000.0,
                    mitigation_strategies: vec![
                        HotShardMitigation::ScaleOut { max_replicas: 5 },
                        HotShardMitigation::AggressiveCaching {
                            cache_ttl_seconds: 300,
                        },
                    ],
                },
            },
        }
    }
}
