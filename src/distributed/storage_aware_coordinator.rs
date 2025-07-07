use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
// Note: Some distributed features are temporarily disabled for single-node optimization
// use crate::core::foundation::generic_types::{GenericMetadata, GenericStats};
// use crate::core::routing::{RoutingContext, SmartRouter};
// use crate::core::global_coordination::GlobalCoordinationConfig;
// use crate::services::assignment::AssignmentService;
// use crate::services::vector_service::VectorService;

/// Storage-aware distributed coordinator that handles different deployment models
/// 
/// Key architectural principles:
/// 1. **Local deployments**: File:// URLs require data replication across nodes
/// 2. **Cloud deployments**: Object stores (S3/GCS/ADLS) provide built-in replication
/// 3. **Stateless compute**: Cloud nodes rebuild state at startup from object storage
/// 4. **WAL affinity**: Unflushed WAL data must be served from the originating node
/// 5. **High availability**: Rack/AZ awareness for read distribution and write replication
#[derive(Debug, Clone)]
pub struct StorageAwareDistributedCoordinator {
    /// Deployment configuration determines replication strategy
    deployment_config: DeploymentConfig,
    /// Smart router from existing ProximaDB routing system
    router: Arc<SmartRouter>,
    /// Global coordination config for multi-region deployments
    global_config: GlobalCoordinationConfig,
    /// Node registry with rack/AZ awareness
    node_registry: Arc<RwLock<StorageAwareNodeRegistry>>,
    /// Collection to storage URL mapping
    collection_storage_map: Arc<RwLock<CollectionStorageMap>>,
}

/// Deployment configuration determines replication needs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentConfig {
    pub deployment_type: DeploymentType,
    pub high_availability: HighAvailabilityConfig,
    pub storage_config: StorageConfig,
    pub compute_config: ComputeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DeploymentType {
    /// Local deployment with file:// storage - requires active replication
    Local {
        replication_factor: u32,
        consistency_level: LocalConsistencyLevel,
    },
    /// Cloud deployment with object storage - stateless compute nodes
    Cloud {
        object_store_type: ObjectStoreType,
        region_strategy: CloudRegionStrategy,
        compute_scaling: ComputeScalingStrategy,
    },
    /// Hybrid deployment combining local and cloud storage
    Hybrid {
        local_config: Box<DeploymentType>,
        cloud_config: Box<DeploymentType>,
        data_tier_strategy: DataTierStrategy,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LocalConsistencyLevel {
    /// Strong consistency - all replicas must acknowledge writes
    Strong,
    /// Quorum consistency - majority of replicas must acknowledge
    Quorum,
    /// Eventual consistency - asynchronous replication
    Eventual { max_lag_ms: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ObjectStoreType {
    S3 { region: String, bucket: String },
    GCS { region: String, bucket: String },
    ADLS { region: String, container: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CloudRegionStrategy {
    /// Single region deployment
    SingleRegion { primary_region: String },
    /// Multi-region with primary-secondary
    MultiRegion {
        primary_region: String,
        secondary_regions: Vec<String>,
        failover_strategy: CloudFailoverStrategy,
    },
    /// Global deployment with intelligent routing
    Global {
        regions: Vec<String>,
        routing_strategy: GlobalRoutingStrategy,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CloudFailoverStrategy {
    Manual,
    Automatic { health_check_interval_ms: u64 },
    GradualTrafficShift { shift_percentage_per_minute: f32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GlobalRoutingStrategy {
    LatencyBased,
    CostOptimized,
    DataLocalityFirst,
    HybridWeighted { latency_weight: f32, cost_weight: f32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ComputeScalingStrategy {
    /// Fixed number of nodes
    Static { node_count: u32 },
    /// Auto-scaling based on metrics
    AutoScale {
        min_nodes: u32,
        max_nodes: u32,
        scale_up_threshold: f32,
        scale_down_threshold: f32,
    },
    /// Serverless scaling
    Serverless {
        cold_start_optimization: bool,
        max_concurrent_instances: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DataTierStrategy {
    /// Hot data in local storage, cold in cloud
    TemperatureBased {
        hot_threshold_days: u32,
        warm_threshold_days: u32,
    },
    /// Frequently accessed collections local, others cloud
    AccessPatternBased {
        local_access_threshold: f32,
        migration_policy: MigrationPolicy,
    },
    /// Size-based tiering
    SizeBased {
        local_max_size_gb: u64,
        cloud_overflow_policy: OverflowPolicy,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MigrationPolicy {
    Immediate,
    Scheduled { schedule_cron: String },
    Manual,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OverflowPolicy {
    MoveOldest,
    MoveLargest,
    MoveLeastAccessed,
}

/// High availability configuration with rack/AZ awareness
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HighAvailabilityConfig {
    pub rack_awareness: bool,
    pub availability_zone_awareness: bool,
    pub cross_az_replication: bool,
    pub read_preference: ReadPreferenceStrategy,
    pub write_replication: WriteReplicationStrategy,
    pub failure_detection: FailureDetectionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ReadPreferenceStrategy {
    /// Always read from primary node
    Primary,
    /// Prefer primary, fallback to replicas
    PrimaryPreferred,
    /// Read from nearest replica (latency-optimized)
    Nearest,
    /// Load balance across all healthy replicas
    LoadBalanced,
    /// Read from replicas in same AZ first
    SameAZ,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WriteReplicationStrategy {
    /// Synchronous replication to all replicas
    Synchronous,
    /// Quorum-based writes (majority acknowledgment)
    Quorum,
    /// Primary writes, asynchronous replication
    PrimaryAsync,
    /// Chain replication for ordered writes
    ChainReplication,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureDetectionConfig {
    pub heartbeat_interval_ms: u64,
    pub failure_threshold_count: u32,
    pub network_partition_detection: bool,
    pub split_brain_prevention: bool,
}

/// Storage configuration aware of different storage backends
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub wal_strategy: WalDistributionStrategy,
    pub viper_strategy: ViperDistributionStrategy,
    pub lsm_strategy: LsmDistributionStrategy,
    pub metadata_strategy: MetadataDistributionStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WalDistributionStrategy {
    /// WAL stays on originating node (for unflushed data)
    NodeLocal {
        replication_factor: u32,
        sync_mode: WalSyncMode,
    },
    /// WAL replicated across availability zones
    AzReplicated {
        replicas_per_az: u32,
        cross_az_sync: bool,
    },
    /// WAL stored in shared storage (cloud only)
    SharedStorage {
        storage_url: String,
        consistency_level: SharedStorageConsistency,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WalSyncMode {
    Immediate,
    PerBatch,
    Periodic { interval_ms: u64 },
    Never,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SharedStorageConsistency {
    ReadAfterWrite,
    EventualConsistency,
    StrongConsistency,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ViperDistributionStrategy {
    /// Parquet files in shared object storage
    ObjectStore {
        replication_built_in: bool,
        cross_region_replication: bool,
    },
    /// Parquet files replicated across local nodes
    LocalReplicated {
        replication_factor: u32,
        placement_strategy: ParquetPlacementStrategy,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParquetPlacementStrategy {
    RackAware,
    RandomReplicas,
    LatencyOptimized,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LsmDistributionStrategy {
    /// LSM SSTables distributed across nodes
    Distributed {
        sharding_strategy: LsmShardingStrategy,
        compaction_coordination: CompactionCoordination,
    },
    /// LSM in shared storage (cloud)
    SharedStorage {
        compaction_offload: bool,
        read_optimization: LsmReadOptimization,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LsmShardingStrategy {
    HashBased { shard_count: u32 },
    RangeBased { key_ranges: Vec<String> },
    SizeBased { max_size_per_shard_gb: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompactionCoordination {
    Centralized { coordinator_node: String },
    Distributed { leader_election: bool },
    OffloadToCloud { compute_service: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LsmReadOptimization {
    LocalCaching { cache_size_gb: u64 },
    PrefetchHotSSTables,
    BloomFilterCaching,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MetadataDistributionStrategy {
    /// Metadata replicated to all nodes (small size)
    FullReplication,
    /// Metadata sharded by collection
    Sharded { shard_count: u32 },
    /// Metadata in consensus-based storage (Raft/Paxos)
    ConsensusReplicated { consensus_nodes: u32 },
    /// Metadata in external system (etcd, Consul)
    External { external_system: String },
}

/// Compute configuration for stateless vs stateful deployments
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComputeConfig {
    pub node_type: ComputeNodeType,
    pub state_management: StateManagementStrategy,
    pub resource_allocation: ResourceAllocationStrategy,
    pub startup_optimization: StartupOptimizationConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ComputeNodeType {
    /// Stateful nodes with local storage
    Stateful {
        local_storage_gb: u64,
        persistent_volumes: bool,
    },
    /// Stateless nodes rebuilding state from storage
    Stateless {
        state_rebuild_timeout_ms: u64,
        in_memory_cache_gb: u64,
    },
    /// Hybrid nodes with local cache but durable storage elsewhere
    Hybrid {
        cache_size_gb: u64,
        cache_policy: CachePolicy,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StateManagementStrategy {
    /// Rebuild from storage on startup
    RebuildOnStartup {
        parallel_load_threads: u32,
        incremental_loading: bool,
    },
    /// Streaming state synchronization
    StreamingSync {
        sync_batch_size: u32,
        sync_interval_ms: u64,
    },
    /// Persistent state with checkpointing
    PersistentWithCheckpoints {
        checkpoint_interval_ms: u64,
        checkpoint_compression: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CachePolicy {
    LRU,
    LFU,
    TTL { ttl_seconds: u64 },
    Adaptive { ml_driven: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResourceAllocationStrategy {
    /// Static resource allocation
    Static {
        cpu_cores: u32,
        memory_gb: u64,
        disk_gb: u64,
    },
    /// Dynamic allocation based on workload
    Dynamic {
        min_resources: ResourceSpec,
        max_resources: ResourceSpec,
        scaling_triggers: Vec<ScalingTrigger>,
    },
    /// Shared resource pool
    Shared {
        resource_pool_id: String,
        priority_class: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceSpec {
    pub cpu_cores: u32,
    pub memory_gb: u64,
    pub disk_gb: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScalingTrigger {
    CpuUtilization { threshold_percent: f32 },
    MemoryUtilization { threshold_percent: f32 },
    QueueDepth { threshold: u32 },
    ResponseTime { threshold_ms: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupOptimizationConfig {
    pub warm_start_enabled: bool,
    pub metadata_preload: bool,
    pub index_preload_strategy: IndexPreloadStrategy,
    pub connection_pool_warmup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexPreloadStrategy {
    None,
    MostRecent { count: u32 },
    MostAccessed { access_threshold: f32 },
    PredictiveML { model_name: String },
}

/// Node registry with rack and AZ awareness
pub struct StorageAwareNodeRegistry {
    nodes: Vec<StorageAwareNode>,
    rack_topology: RackTopology,
    az_topology: AvailabilityZoneTopology,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageAwareNode {
    pub node_id: String,
    pub address: String,
    pub rack_id: String,
    pub availability_zone: String,
    pub node_type: ComputeNodeType,
    pub storage_capabilities: StorageCapabilities,
    pub current_load: NodeLoad,
    pub health_status: NodeHealthStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageCapabilities {
    pub supported_storage_types: Vec<String>, // ["file", "s3", "gcs", "adls"]
    pub local_storage_gb: u64,
    pub memory_gb: u64,
    pub cpu_cores: u32,
    pub network_bandwidth_gbps: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeLoad {
    pub cpu_utilization: f32,
    pub memory_utilization: f32,
    pub disk_utilization: f32,
    pub network_utilization: f32,
    pub active_connections: u32,
    pub queries_per_second: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NodeHealthStatus {
    Healthy,
    Degraded { reason: String },
    Unhealthy { reason: String },
    Draining,
    Maintenance,
}

/// Rack topology for rack-aware placement
pub struct RackTopology {
    racks: std::collections::HashMap<String, Vec<String>>, // rack_id -> node_ids
    cross_rack_latency: std::collections::HashMap<(String, String), u32>, // (rack1, rack2) -> latency_ms
}

/// AZ topology for AZ-aware placement  
pub struct AvailabilityZoneTopology {
    azs: std::collections::HashMap<String, Vec<String>>, // az_id -> node_ids
    cross_az_latency: std::collections::HashMap<(String, String), u32>, // (az1, az2) -> latency_ms
    az_capacity: std::collections::HashMap<String, AzCapacity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AzCapacity {
    pub max_nodes: u32,
    pub current_nodes: u32,
    pub total_storage_gb: u64,
    pub available_storage_gb: u64,
}

/// Collection to storage URL mapping with replication info
pub struct CollectionStorageMap {
    mappings: std::collections::HashMap<String, CollectionStorageInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionStorageInfo {
    pub collection_id: String,
    pub primary_storage_url: String,
    pub replica_storage_urls: Vec<String>,
    pub storage_type: StorageType,
    pub replication_requirements: ReplicationRequirements,
    pub wal_affinity_node: Option<String>, // Node handling unflushed WAL
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageType {
    Local { requires_replication: bool },
    Cloud { built_in_replication: bool },
    Hybrid { local_tier: String, cloud_tier: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicationRequirements {
    pub min_replicas: u32,
    pub rack_diversity: bool,
    pub az_diversity: bool,
    pub cross_region_replicas: u32,
}

impl StorageAwareDistributedCoordinator {
    pub fn new(
        deployment_config: DeploymentConfig,
        router: Arc<SmartRouter>,
        global_config: GlobalCoordinationConfig,
    ) -> Self {
        Self {
            deployment_config,
            router,
            global_config,
            node_registry: Arc::new(RwLock::new(StorageAwareNodeRegistry {
                nodes: Vec::new(),
                rack_topology: RackTopology {
                    racks: std::collections::HashMap::new(),
                    cross_rack_latency: std::collections::HashMap::new(),
                },
                az_topology: AvailabilityZoneTopology {
                    azs: std::collections::HashMap::new(),
                    cross_az_latency: std::collections::HashMap::new(),
                    az_capacity: std::collections::HashMap::new(),
                },
            })),
            collection_storage_map: Arc::new(RwLock::new(CollectionStorageMap {
                mappings: std::collections::HashMap::new(),
            })),
        }
    }
    
    /// Route a request based on storage type and WAL affinity
    pub async fn route_request(
        &self,
        collection_id: &str,
        operation_type: OperationType,
        routing_context: &RoutingContext,
    ) -> Result<DistributedRoutingDecision> {
        let storage_info = {
            let storage_map = self.collection_storage_map.read().await;
            storage_map.mappings.get(collection_id).cloned()
        };
        
        match storage_info {
            Some(info) => {
                match operation_type {
                    OperationType::Insert | OperationType::Update => {
                        self.route_write_operation(&info, routing_context).await
                    }
                    OperationType::Search => {
                        self.route_search_operation(&info, routing_context).await
                    }
                    OperationType::WalRead => {
                        self.route_wal_read_operation(&info, routing_context).await
                    }
                }
            }
            None => {
                // Collection not found, route to any healthy node for creation
                self.route_collection_creation(collection_id, routing_context).await
            }
        }
    }
    
    /// Route write operations considering WAL affinity and replication
    async fn route_write_operation(
        &self,
        storage_info: &CollectionStorageInfo,
        routing_context: &RoutingContext,
    ) -> Result<DistributedRoutingDecision> {
        match &storage_info.storage_type {
            StorageType::Local { requires_replication } => {
                if *requires_replication {
                    self.route_local_replicated_write(storage_info, routing_context).await
                } else {
                    self.route_single_node_write(storage_info, routing_context).await
                }
            }
            StorageType::Cloud { .. } => {
                // Cloud storage doesn't need replication, route to any healthy node
                self.route_cloud_write(storage_info, routing_context).await
            }
            StorageType::Hybrid { .. } => {
                self.route_hybrid_write(storage_info, routing_context).await
            }
        }
    }
    
    /// Route search operations with multi-tier awareness
    async fn route_search_operation(
        &self,
        storage_info: &CollectionStorageInfo,
        routing_context: &RoutingContext,
    ) -> Result<DistributedRoutingDecision> {
        // For search, we need to coordinate across multiple tiers:
        // 1. WAL data (unflushed) - must go to WAL affinity node
        // 2. VIPER data (flushed Parquet) - can read from any replica
        // 3. LSM data (compacted) - can read from any replica
        
        let mut target_nodes = Vec::new();
        
        // WAL affinity node for unflushed data
        if let Some(wal_node) = &storage_info.wal_affinity_node {
            target_nodes.push(wal_node.clone());
        }
        
        // Additional nodes for flushed data based on read preference
        let additional_nodes = self.select_read_replicas(storage_info, routing_context).await?;
        target_nodes.extend(additional_nodes);
        
        Ok(DistributedRoutingDecision {
            operation_type: OperationType::Search,
            primary_node: target_nodes.first().cloned().unwrap_or_default(),
            replica_nodes: target_nodes.into_iter().skip(1).collect(),
            requires_aggregation: true,
            consistency_level: ConsistencyLevel::EventualConsistency,
            routing_metadata: std::collections::HashMap::new(),
        })
    }
    
    /// Route WAL read operations to WAL affinity node
    async fn route_wal_read_operation(
        &self,
        storage_info: &CollectionStorageInfo,
        _routing_context: &RoutingContext,
    ) -> Result<DistributedRoutingDecision> {
        if let Some(wal_node) = &storage_info.wal_affinity_node {
            Ok(DistributedRoutingDecision {
                operation_type: OperationType::WalRead,
                primary_node: wal_node.clone(),
                replica_nodes: Vec::new(),
                requires_aggregation: false,
                consistency_level: ConsistencyLevel::StrongConsistency,
                routing_metadata: std::collections::HashMap::new(),
            })
        } else {
            Err(anyhow::anyhow!("No WAL affinity node found for collection"))
        }
    }
    
    async fn route_local_replicated_write(
        &self,
        storage_info: &CollectionStorageInfo,
        _routing_context: &RoutingContext,
    ) -> Result<DistributedRoutingDecision> {
        // Select replicas considering rack/AZ diversity
        let replicas = self.select_replicas_with_diversity(storage_info).await?;
        
        Ok(DistributedRoutingDecision {
            operation_type: OperationType::Insert,
            primary_node: replicas.first().cloned().unwrap_or_default(),
            replica_nodes: replicas.into_iter().skip(1).collect(),
            requires_aggregation: false,
            consistency_level: ConsistencyLevel::QuorumConsistency,
            routing_metadata: std::collections::HashMap::new(),
        })
    }
    
    async fn route_single_node_write(
        &self,
        storage_info: &CollectionStorageInfo,
        _routing_context: &RoutingContext,
    ) -> Result<DistributedRoutingDecision> {
        // For single node writes, use primary storage node
        Ok(DistributedRoutingDecision {
            operation_type: OperationType::Insert,
            primary_node: "primary_node".to_string(), // TODO: Extract from storage_info
            replica_nodes: Vec::new(),
            requires_aggregation: false,
            consistency_level: ConsistencyLevel::StrongConsistency,
            routing_metadata: std::collections::HashMap::new(),
        })
    }
    
    async fn route_cloud_write(
        &self,
        _storage_info: &CollectionStorageInfo,
        routing_context: &RoutingContext,
    ) -> Result<DistributedRoutingDecision> {
        // Use existing smart router for cloud deployments
        let routing_decision = self.router.route_request(routing_context, "write");
        
        Ok(DistributedRoutingDecision {
            operation_type: OperationType::Insert,
            primary_node: routing_decision.target_cluster,
            replica_nodes: routing_decision.fallback_clusters,
            requires_aggregation: false,
            consistency_level: ConsistencyLevel::EventualConsistency,
            routing_metadata: std::collections::HashMap::new(),
        })
    }
    
    async fn route_hybrid_write(
        &self,
        _storage_info: &CollectionStorageInfo,
        _routing_context: &RoutingContext,
    ) -> Result<DistributedRoutingDecision> {
        // TODO: Implement hybrid routing based on data tier strategy
        todo!("Implement hybrid write routing")
    }
    
    async fn route_collection_creation(
        &self,
        _collection_id: &str,
        routing_context: &RoutingContext,
    ) -> Result<DistributedRoutingDecision> {
        // Use smart router for new collection placement
        let routing_decision = self.router.route_request(routing_context, "create");
        
        Ok(DistributedRoutingDecision {
            operation_type: OperationType::Insert,
            primary_node: routing_decision.target_cluster,
            replica_nodes: routing_decision.fallback_clusters,
            requires_aggregation: false,
            consistency_level: ConsistencyLevel::StrongConsistency,
            routing_metadata: std::collections::HashMap::new(),
        })
    }
    
    async fn select_read_replicas(
        &self,
        storage_info: &CollectionStorageInfo,
        _routing_context: &RoutingContext,
    ) -> Result<Vec<String>> {
        // Select read replicas based on read preference strategy
        match &self.deployment_config.high_availability.read_preference {
            ReadPreferenceStrategy::Primary => {
                Ok(vec![]) // Only read from primary (WAL affinity node)
            }
            ReadPreferenceStrategy::Nearest => {
                // TODO: Implement nearest replica selection based on latency
                Ok(storage_info.replica_storage_urls.iter()
                   .take(2)
                   .map(|url| format!("node_for_{}", url))
                   .collect())
            }
            ReadPreferenceStrategy::SameAZ => {
                // TODO: Implement same-AZ replica selection
                Ok(storage_info.replica_storage_urls.iter()
                   .take(1)
                   .map(|url| format!("node_for_{}", url))
                   .collect())
            }
            _ => {
                Ok(storage_info.replica_storage_urls.iter()
                   .map(|url| format!("node_for_{}", url))
                   .collect())
            }
        }
    }
    
    async fn select_replicas_with_diversity(
        &self,
        storage_info: &CollectionStorageInfo,
    ) -> Result<Vec<String>> {
        // TODO: Implement rack/AZ diversity-aware replica selection
        Ok(storage_info.replica_storage_urls.iter()
           .map(|url| format!("node_for_{}", url))
           .collect())
    }
    
    /// Add a collection to storage mapping (for testing)
    pub async fn add_collection_storage_info(&self, info: CollectionStorageInfo) -> Result<()> {
        let mut storage_map = self.collection_storage_map.write().await;
        storage_map.mappings.insert(info.collection_id.clone(), info);
        Ok(())
    }
    
    /// Get deployment configuration (for testing)
    pub fn get_deployment_config(&self) -> &DeploymentConfig {
        &self.deployment_config
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OperationType {
    Insert,
    Update,
    Search,
    WalRead,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributedRoutingDecision {
    pub operation_type: OperationType,
    pub primary_node: String,
    pub replica_nodes: Vec<String>,
    pub requires_aggregation: bool,
    pub consistency_level: ConsistencyLevel,
    pub routing_metadata: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConsistencyLevel {
    StrongConsistency,
    QuorumConsistency,
    EventualConsistency,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::routing::{RoutingConfig, TenantRoutingConfig, TenantExtractionConfig, TenantMappingStrategy, CircuitBreakerConfig};
    use std::collections::HashMap;
    
    // Mock implementations for testing
    fn create_mock_smart_router() -> Arc<SmartRouter> {
        let config = RoutingConfig {
            strategy: crate::core::routing::RoutingStrategy::TenantBased {
                tenant_key: "tenant_id".to_string(),
                shard_count: 4,
                consistent_hashing: true,
            },
            load_balancing: crate::core::routing::LoadBalancingStrategy::RoundRobin,
            tenant_routing: TenantRoutingConfig {
                tenant_extraction: TenantExtractionConfig {
                    headers: vec!["x-tenant-id".to_string()],
                    jwt_claims: vec!["tenant_id".to_string()],
                    url_patterns: vec!["/tenant/{tenant_id}".to_string()],
                    default_tenant: "default".to_string(),
                },
                tenant_mapping: TenantMappingStrategy::Shared,
                isolation_tiers: HashMap::new(),
                rate_limiting: HashMap::new(),
            },
            geographic_routing: None,
            circuit_breaker: CircuitBreakerConfig {
                enabled: true,
                failure_threshold: 5,
                recovery_timeout_seconds: 60,
                half_open_max_calls: 3,
            },
        };
        Arc::new(SmartRouter::new(config))
    }
    
    fn create_mock_routing_context() -> RoutingContext {
        RoutingContext {
            tenant_id: "test_tenant".to_string(),
            customer_segment: crate::core::routing::CustomerSegment::Enterprise,
            account_tier: crate::core::routing::AccountTier::Professional,
            geographic_region: Some("us-west-2".to_string()),
            workload_type: crate::core::routing::WorkloadType::OLTP,
            collection_metadata: None,
            request_metadata: HashMap::new(),
        }
    }
    
    fn create_local_deployment_config() -> DeploymentConfig {
        DeploymentConfig {
            deployment_type: DeploymentType::Local {
                replication_factor: 3,
                consistency_level: LocalConsistencyLevel::Quorum,
            },
            high_availability: HighAvailabilityConfig {
                rack_awareness: true,
                availability_zone_awareness: true,
                cross_az_replication: true,
                read_preference: ReadPreferenceStrategy::Nearest,
                write_replication: WriteReplicationStrategy::Quorum,
                failure_detection: FailureDetectionConfig {
                    heartbeat_interval_ms: 30000,
                    failure_threshold_count: 3,
                    network_partition_detection: true,
                    split_brain_prevention: true,
                },
            },
            storage_config: StorageConfig {
                wal_strategy: WalDistributionStrategy::NodeLocal {
                    replication_factor: 2,
                    sync_mode: WalSyncMode::PerBatch,
                },
                viper_strategy: ViperDistributionStrategy::LocalReplicated {
                    replication_factor: 2,
                    placement_strategy: ParquetPlacementStrategy::RackAware,
                },
                lsm_strategy: LsmDistributionStrategy::Distributed {
                    sharding_strategy: LsmShardingStrategy::HashBased { shard_count: 16 },
                    compaction_coordination: CompactionCoordination::Distributed { leader_election: true },
                },
                metadata_strategy: MetadataDistributionStrategy::FullReplication,
            },
            compute_config: ComputeConfig {
                node_type: ComputeNodeType::Stateful {
                    local_storage_gb: 1000,
                    persistent_volumes: true,
                },
                state_management: StateManagementStrategy::PersistentWithCheckpoints {
                    checkpoint_interval_ms: 300000,
                    checkpoint_compression: true,
                },
                resource_allocation: ResourceAllocationStrategy::Static {
                    cpu_cores: 16,
                    memory_gb: 64,
                    disk_gb: 1000,
                },
                startup_optimization: StartupOptimizationConfig {
                    warm_start_enabled: true,
                    metadata_preload: true,
                    index_preload_strategy: IndexPreloadStrategy::MostAccessed { access_threshold: 0.8 },
                    connection_pool_warmup: true,
                },
            },
        }
    }
    
    fn create_cloud_deployment_config() -> DeploymentConfig {
        DeploymentConfig {
            deployment_type: DeploymentType::Cloud {
                object_store_type: ObjectStoreType::S3 {
                    region: "us-west-2".to_string(),
                    bucket: "proximadb-data".to_string(),
                },
                region_strategy: CloudRegionStrategy::SingleRegion {
                    primary_region: "us-west-2".to_string(),
                },
                compute_scaling: ComputeScalingStrategy::AutoScale {
                    min_nodes: 2,
                    max_nodes: 10,
                    scale_up_threshold: 0.7,
                    scale_down_threshold: 0.3,
                },
            },
            high_availability: HighAvailabilityConfig {
                rack_awareness: false, // Not relevant for cloud
                availability_zone_awareness: true,
                cross_az_replication: false, // Handled by object storage
                read_preference: ReadPreferenceStrategy::LoadBalanced,
                write_replication: WriteReplicationStrategy::PrimaryAsync,
                failure_detection: FailureDetectionConfig {
                    heartbeat_interval_ms: 15000,
                    failure_threshold_count: 2,
                    network_partition_detection: false,
                    split_brain_prevention: false,
                },
            },
            storage_config: StorageConfig {
                wal_strategy: WalDistributionStrategy::SharedStorage {
                    storage_url: "s3://proximadb-wal/".to_string(),
                    consistency_level: SharedStorageConsistency::ReadAfterWrite,
                },
                viper_strategy: ViperDistributionStrategy::ObjectStore {
                    replication_built_in: true,
                    cross_region_replication: false,
                },
                lsm_strategy: LsmDistributionStrategy::SharedStorage {
                    compaction_offload: true,
                    read_optimization: LsmReadOptimization::LocalCaching { cache_size_gb: 100 },
                },
                metadata_strategy: MetadataDistributionStrategy::ConsensusReplicated { consensus_nodes: 3 },
            },
            compute_config: ComputeConfig {
                node_type: ComputeNodeType::Stateless {
                    state_rebuild_timeout_ms: 30000,
                    in_memory_cache_gb: 32,
                },
                state_management: StateManagementStrategy::RebuildOnStartup {
                    parallel_load_threads: 8,
                    incremental_loading: true,
                },
                resource_allocation: ResourceAllocationStrategy::Dynamic {
                    min_resources: ResourceSpec {
                        cpu_cores: 4,
                        memory_gb: 16,
                        disk_gb: 100,
                    },
                    max_resources: ResourceSpec {
                        cpu_cores: 32,
                        memory_gb: 128,
                        disk_gb: 1000,
                    },
                    scaling_triggers: vec![
                        ScalingTrigger::CpuUtilization { threshold_percent: 70.0 },
                        ScalingTrigger::MemoryUtilization { threshold_percent: 80.0 },
                    ],
                },
                startup_optimization: StartupOptimizationConfig {
                    warm_start_enabled: true,
                    metadata_preload: true,
                    index_preload_strategy: IndexPreloadStrategy::PredictiveML { 
                        model_name: "index_predictor_v1".to_string() 
                    },
                    connection_pool_warmup: true,
                },
            },
        }
    }
    
    #[test]
    fn test_deployment_config_serialization() {
        let config = create_local_deployment_config();
        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: DeploymentConfig = serde_json::from_str(&serialized).unwrap();
        
        // Basic check that serialization round-trip works
        assert!(matches!(deserialized.deployment_type, DeploymentType::Local { .. }));
    }
    
    #[test]
    fn test_cloud_deployment_config() {
        let config = create_cloud_deployment_config();
        
        // Verify cloud-specific settings
        match config.deployment_type {
            DeploymentType::Cloud { object_store_type, .. } => {
                match object_store_type {
                    ObjectStoreType::S3 { region, bucket } => {
                        assert_eq!(region, "us-west-2");
                        assert_eq!(bucket, "proximadb-data");
                    }
                    _ => panic!("Expected S3 object store type"),
                }
            }
            _ => panic!("Expected cloud deployment type"),
        }
        
        // Verify stateless compute config
        match config.compute_config.node_type {
            ComputeNodeType::Stateless { state_rebuild_timeout_ms, .. } => {
                assert_eq!(state_rebuild_timeout_ms, 30000);
            }
            _ => panic!("Expected stateless node type for cloud deployment"),
        }
    }
    
    #[tokio::test]
    async fn test_storage_aware_coordinator_creation() {
        let deployment_config = create_local_deployment_config();
        let router = create_mock_smart_router();
        let global_config = GlobalCoordinationConfig::default();
        
        let coordinator = StorageAwareDistributedCoordinator::new(
            deployment_config,
            router,
            global_config,
        );
        
        // Verify coordinator is created successfully
        assert!(matches!(coordinator.get_deployment_config().deployment_type, DeploymentType::Local { .. }));
    }
    
    #[tokio::test]
    async fn test_local_storage_collection_routing() {
        let deployment_config = create_local_deployment_config();
        let router = create_mock_smart_router();
        let global_config = GlobalCoordinationConfig::default();
        let routing_context = create_mock_routing_context();
        
        let coordinator = StorageAwareDistributedCoordinator::new(
            deployment_config,
            router,
            global_config,
        );
        
        // Add a test collection with local storage
        let collection_info = CollectionStorageInfo {
            collection_id: "test_collection".to_string(),
            primary_storage_url: "file:///data/test_collection".to_string(),
            replica_storage_urls: vec![
                "file:///replica1/test_collection".to_string(),
                "file:///replica2/test_collection".to_string(),
            ],
            storage_type: StorageType::Local { requires_replication: true },
            replication_requirements: ReplicationRequirements {
                min_replicas: 2,
                rack_diversity: true,
                az_diversity: true,
                cross_region_replicas: 0,
            },
            wal_affinity_node: Some("node_1".to_string()),
        };
        
        coordinator.add_collection_storage_info(collection_info).await.unwrap();
        
        // Test write operation routing
        let decision = coordinator.route_request(
            "test_collection",
            OperationType::Insert,
            &routing_context,
        ).await.unwrap();
        
        assert_eq!(decision.operation_type, OperationType::Insert);
        assert_eq!(decision.consistency_level, ConsistencyLevel::QuorumConsistency);
        assert!(!decision.primary_node.is_empty());
    }
    
    #[tokio::test]
    async fn test_wal_affinity_routing() {
        let deployment_config = create_local_deployment_config();
        let router = create_mock_smart_router();
        let global_config = GlobalCoordinationConfig::default();
        let routing_context = create_mock_routing_context();
        
        let coordinator = StorageAwareDistributedCoordinator::new(
            deployment_config,
            router,
            global_config,
        );
        
        // Add a test collection with WAL affinity
        let collection_info = CollectionStorageInfo {
            collection_id: "wal_test_collection".to_string(),
            primary_storage_url: "file:///data/wal_test".to_string(),
            replica_storage_urls: vec![],
            storage_type: StorageType::Local { requires_replication: false },
            replication_requirements: ReplicationRequirements {
                min_replicas: 1,
                rack_diversity: false,
                az_diversity: false,
                cross_region_replicas: 0,
            },
            wal_affinity_node: Some("wal_node_1".to_string()),
        };
        
        coordinator.add_collection_storage_info(collection_info).await.unwrap();
        
        // Test WAL read operation routing
        let decision = coordinator.route_request(
            "wal_test_collection",
            OperationType::WalRead,
            &routing_context,
        ).await.unwrap();
        
        assert_eq!(decision.operation_type, OperationType::WalRead);
        assert_eq!(decision.primary_node, "wal_node_1");
        assert_eq!(decision.consistency_level, ConsistencyLevel::StrongConsistency);
        assert!(!decision.requires_aggregation);
    }
    
    #[tokio::test]
    async fn test_cloud_storage_routing() {
        let deployment_config = create_cloud_deployment_config();
        let router = create_mock_smart_router();
        let global_config = GlobalCoordinationConfig::default();
        let routing_context = create_mock_routing_context();
        
        let coordinator = StorageAwareDistributedCoordinator::new(
            deployment_config,
            router,
            global_config,
        );
        
        // Add a test collection with cloud storage
        let collection_info = CollectionStorageInfo {
            collection_id: "cloud_collection".to_string(),
            primary_storage_url: "s3://proximadb-data/cloud_collection".to_string(),
            replica_storage_urls: vec![],
            storage_type: StorageType::Cloud { built_in_replication: true },
            replication_requirements: ReplicationRequirements {
                min_replicas: 0, // Not needed for cloud storage
                rack_diversity: false,
                az_diversity: false,
                cross_region_replicas: 0,
            },
            wal_affinity_node: Some("compute_node_1".to_string()),
        };
        
        coordinator.add_collection_storage_info(collection_info).await.unwrap();
        
        // Test search operation routing
        let decision = coordinator.route_request(
            "cloud_collection",
            OperationType::Search,
            &routing_context,
        ).await.unwrap();
        
        assert_eq!(decision.operation_type, OperationType::Search);
        assert_eq!(decision.consistency_level, ConsistencyLevel::EventualConsistency);
        assert!(decision.requires_aggregation); // Multi-tier search requires aggregation
    }
    
    #[tokio::test]
    async fn test_collection_not_found_routing() {
        let deployment_config = create_local_deployment_config();
        let router = create_mock_smart_router();
        let global_config = GlobalCoordinationConfig::default();
        let routing_context = create_mock_routing_context();
        
        let coordinator = StorageAwareDistributedCoordinator::new(
            deployment_config,
            router,
            global_config,
        );
        
        // Test routing for non-existent collection
        let decision = coordinator.route_request(
            "non_existent_collection",
            OperationType::Insert,
            &routing_context,
        ).await.unwrap();
        
        // Should route to cluster for collection creation
        assert_eq!(decision.operation_type, OperationType::Insert);
        assert_eq!(decision.consistency_level, ConsistencyLevel::StrongConsistency);
    }
    
    #[test]
    fn test_storage_type_classification() {
        // Test local storage type
        let local_storage = StorageType::Local { requires_replication: true };
        match local_storage {
            StorageType::Local { requires_replication } => {
                assert!(requires_replication);
            }
            _ => panic!("Expected local storage type"),
        }
        
        // Test cloud storage type
        let cloud_storage = StorageType::Cloud { built_in_replication: true };
        match cloud_storage {
            StorageType::Cloud { built_in_replication } => {
                assert!(built_in_replication);
            }
            _ => panic!("Expected cloud storage type"),
        }
    }
    
    #[test]
    fn test_consistency_level_configuration() {
        // Test strong consistency for local storage
        let local_config = create_local_deployment_config();
        match local_config.deployment_type {
            DeploymentType::Local { consistency_level, .. } => {
                assert!(matches!(consistency_level, LocalConsistencyLevel::Quorum));
            }
            _ => panic!("Expected local deployment type"),
        }
        
        // Test eventual consistency appropriate for cloud
        let cloud_config = create_cloud_deployment_config();
        assert!(matches!(cloud_config.high_availability.write_replication, WriteReplicationStrategy::PrimaryAsync));
    }
}