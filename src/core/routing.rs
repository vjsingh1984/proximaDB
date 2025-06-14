use serde::{Deserialize, Serialize};
use std::collections::HashMap;
// Routing module for smart traffic distribution

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingConfig {
    pub strategy: RoutingStrategy,
    pub load_balancing: LoadBalancingStrategy,
    pub tenant_routing: TenantRoutingConfig,
    pub geographic_routing: Option<GeographicRoutingConfig>,
    pub circuit_breaker: CircuitBreakerConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RoutingStrategy {
    /// Route based on tenant metadata (customer_id, account_id, etc)
    TenantBased {
        tenant_key: String,
        shard_count: u32,
        consistent_hashing: bool,
    },
    /// Route based on data characteristics (collection size, query patterns)
    DataBased {
        size_thresholds: Vec<SizeThreshold>,
        query_pattern_routing: bool,
    },
    /// Route based on workload type (OLTP vs OLAP)
    WorkloadBased {
        read_replicas: Vec<String>,
        write_primaries: Vec<String>,
        analytics_clusters: Vec<String>,
    },
    /// Hybrid routing combining multiple strategies
    Hybrid {
        primary_strategy: Box<RoutingStrategy>,
        fallback_strategy: Box<RoutingStrategy>,
        decision_metadata: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SizeThreshold {
    pub max_vectors: u64,
    pub target_clusters: Vec<String>,
    pub scaling_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LoadBalancingStrategy {
    RoundRobin,
    LeastConnections,
    WeightedRoundRobin { weights: HashMap<String, u32> },
    ConsistentHashing { hash_ring_size: u32 },
    LatencyBased { health_check_interval_ms: u32 },
    ResourceBased { 
        cpu_weight: f32, 
        memory_weight: f32, 
        queue_depth_weight: f32 
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantRoutingConfig {
    /// Extract tenant info from request metadata
    pub tenant_extraction: TenantExtractionConfig,
    /// Map tenants to specific infrastructure
    pub tenant_mapping: TenantMappingStrategy,
    /// Resource isolation levels per tenant tier
    pub isolation_tiers: HashMap<String, IsolationTier>,
    /// Traffic shaping per tenant
    pub rate_limiting: HashMap<String, RateLimitConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantExtractionConfig {
    /// HTTP headers to check for tenant info
    pub headers: Vec<String>,
    /// JWT token claims to extract tenant from
    pub jwt_claims: Vec<String>,
    /// URL path patterns to extract tenant from
    pub url_patterns: Vec<String>,
    /// Default tenant for requests without tenant info
    pub default_tenant: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TenantMappingStrategy {
    /// All tenants share infrastructure with logical separation
    Shared,
    /// Dedicated clusters per enterprise customer
    Dedicated { 
        enterprise_tenants: HashMap<String, String> // tenant_id -> cluster_id
    },
    /// Automatic tiering based on tenant characteristics
    TierBased {
        starter_tier: Vec<String>,     // Shared infrastructure
        professional_tier: Vec<String>, // Dedicated containers  
        enterprise_tier: Vec<String>,  // Dedicated clusters
    },
    /// Hybrid approach with overflow routing
    Hybrid {
        primary_assignments: HashMap<String, String>,
        overflow_clusters: Vec<String>,
        overflow_threshold: f32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IsolationTier {
    pub name: String,
    pub resource_limits: ResourceLimits,
    pub scaling_policy: ScalingPolicy,
    pub storage_class: StorageClass,
    pub sla_guarantees: SlaGuarantees,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub max_cpu_cores: Option<f32>,
    pub max_memory_gb: Option<f32>,
    pub max_storage_gb: Option<f32>,
    pub max_requests_per_second: Option<f32>,
    pub max_concurrent_queries: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScalingPolicy {
    pub auto_scaling_enabled: bool,
    pub min_instances: u32,
    pub max_instances: u32,
    pub scale_up_threshold: f32,
    pub scale_down_threshold: f32,
    pub cooldown_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StorageClass {
    /// High-performance SSD for enterprise
    Premium { iops: u32, throughput_mbps: u32 },
    /// Standard SSD for professional
    Standard { iops: u32 },
    /// Cost-optimized for starter tiers
    Economy { backup_frequency_hours: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlaGuarantees {
    pub availability_percent: f32,
    pub response_time_p99_ms: u32,
    pub throughput_qps: u32,
    pub data_durability_nines: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    pub requests_per_second: f32,
    pub burst_capacity: u32,
    pub queue_timeout_ms: u32,
    pub backoff_strategy: BackoffStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackoffStrategy {
    Exponential { base_ms: u32, max_ms: u32 },
    Linear { increment_ms: u32, max_ms: u32 },
    Immediate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeographicRoutingConfig {
    pub enabled: bool,
    pub regions: HashMap<String, RegionConfig>,
    pub latency_routing: bool,
    pub data_residency_rules: HashMap<String, Vec<String>>, // tenant -> allowed_regions
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegionConfig {
    pub clusters: Vec<String>,
    pub primary: bool,
    pub latency_weight: f32,
    pub capacity_weight: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    pub enabled: bool,
    pub failure_threshold: u32,
    pub recovery_timeout_seconds: u32,
    pub half_open_max_calls: u32,
}

/// Request routing context extracted from HTTP headers, JWT, URL, etc.
#[derive(Debug, Clone)]
pub struct RoutingContext {
    pub tenant_id: String,
    pub customer_segment: CustomerSegment,
    pub account_tier: AccountTier,
    pub geographic_region: Option<String>,
    pub workload_type: WorkloadType,
    pub collection_metadata: Option<CollectionMetadata>,
    pub request_metadata: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CustomerSegment {
    Startup,
    SMB,        // Small/Medium Business
    Enterprise,
    Government,
    NonProfit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AccountTier {
    Free,
    Starter,
    Professional,
    Enterprise,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkloadType {
    /// Real-time queries, low latency requirements
    OLTP,
    /// Analytics, high throughput batch processing  
    OLAP,
    /// Machine learning inference
    ML,
    /// Bulk data ingestion
    Ingestion,
    /// Mixed workload
    Hybrid,
}

#[derive(Debug, Clone)]
pub struct CollectionMetadata {
    pub size: u64,
    pub query_patterns: Vec<QueryPattern>,
    pub access_frequency: AccessFrequency,
    pub data_sensitivity: DataSensitivity,
}

#[derive(Debug, Clone)]
pub enum QueryPattern {
    PointQuery,      // Get specific vectors
    SimilaritySearch, // Vector similarity queries
    RangeQuery,      // Filtered searches
    Aggregation,     // Analytics queries
}

#[derive(Debug, Clone)]
pub enum AccessFrequency {
    Hot,    // Accessed frequently
    Warm,   // Accessed occasionally  
    Cold,   // Rarely accessed
    Archive, // Long-term storage
}

#[derive(Debug, Clone)]
pub enum DataSensitivity {
    Public,
    Internal,
    Confidential,
    Restricted,
}

/// Smart router that determines optimal service instance for requests
pub struct SmartRouter {
    config: RoutingConfig,
    tenant_cache: HashMap<String, RoutingContext>,
    cluster_health: HashMap<String, ClusterHealth>,
}

#[derive(Debug, Clone)]
pub struct ClusterHealth {
    pub cluster_id: String,
    pub cpu_utilization: f32,
    pub memory_utilization: f32,
    pub active_connections: u32,
    pub queue_depth: u32,
    pub response_time_p99: u32,
    pub error_rate: f32,
    pub last_updated: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct RoutingDecision {
    pub target_cluster: String,
    pub target_instance: Option<String>,
    pub load_balancing_key: String,
    pub priority: RoutingPriority,
    pub fallback_clusters: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub enum RoutingPriority {
    High,    // Enterprise SLA
    Medium,  // Professional tier
    Low,     // Best effort
    Batch,   // Background processing
}

impl SmartRouter {
    pub fn new(config: RoutingConfig) -> Self {
        Self {
            config,
            tenant_cache: HashMap::new(),
            cluster_health: HashMap::new(),
        }
    }

    /// Extract routing context from request metadata
    pub fn extract_routing_context(&self, headers: &HashMap<String, String>) -> RoutingContext {
        let tenant_id = self.extract_tenant_id(headers);
        let customer_segment = self.determine_customer_segment(&tenant_id);
        let account_tier = self.determine_account_tier(&tenant_id);
        
        RoutingContext {
            tenant_id,
            customer_segment,
            account_tier,
            geographic_region: headers.get("x-region").cloned(),
            workload_type: self.determine_workload_type(headers),
            collection_metadata: None, // Will be populated during query processing
            request_metadata: headers.clone(),
        }
    }

    /// Make routing decision based on context and cluster health
    pub fn route_request(&self, context: &RoutingContext, operation_type: &str) -> RoutingDecision {
        match &self.config.strategy {
            RoutingStrategy::TenantBased { tenant_key: _, shard_count, consistent_hashing } => {
                self.route_by_tenant(context, *shard_count, *consistent_hashing)
            }
            RoutingStrategy::WorkloadBased { read_replicas, write_primaries, analytics_clusters } => {
                self.route_by_workload(context, operation_type, read_replicas, write_primaries, analytics_clusters)
            }
            _ => {
                // Default routing logic
                RoutingDecision {
                    target_cluster: "default".to_string(),
                    target_instance: None,
                    load_balancing_key: context.tenant_id.clone(),
                    priority: RoutingPriority::Medium,
                    fallback_clusters: vec!["fallback".to_string()],
                    reason: "Default routing".to_string(),
                }
            }
        }
    }

    fn extract_tenant_id(&self, headers: &HashMap<String, String>) -> String {
        for header in &self.config.tenant_routing.tenant_extraction.headers {
            if let Some(value) = headers.get(header) {
                return value.clone();
            }
        }
        self.config.tenant_routing.tenant_extraction.default_tenant.clone()
    }

    fn determine_customer_segment(&self, tenant_id: &str) -> CustomerSegment {
        // TODO: Implement tenant -> segment mapping
        // This could query a database or use naming conventions
        if tenant_id.starts_with("ent_") {
            CustomerSegment::Enterprise
        } else if tenant_id.starts_with("smb_") {
            CustomerSegment::SMB
        } else {
            CustomerSegment::Startup
        }
    }

    fn determine_account_tier(&self, _tenant_id: &str) -> AccountTier {
        // TODO: Implement tenant -> tier mapping
        // This would typically query a subscription/billing database
        AccountTier::Professional // Default
    }

    fn determine_workload_type(&self, headers: &HashMap<String, String>) -> WorkloadType {
        if let Some(workload) = headers.get("x-workload-type") {
            match workload.as_str() {
                "oltp" => WorkloadType::OLTP,
                "olap" => WorkloadType::OLAP,
                "ml" => WorkloadType::ML,
                "ingestion" => WorkloadType::Ingestion,
                _ => WorkloadType::Hybrid,
            }
        } else {
            WorkloadType::OLTP // Default to real-time
        }
    }

    fn route_by_tenant(&self, context: &RoutingContext, shard_count: u32, _consistent_hashing: bool) -> RoutingDecision {
        // Use consistent hashing to determine shard
        let hash = self.hash_tenant_id(&context.tenant_id);
        let shard = hash % shard_count;
        let cluster_id = format!("shard-{}", shard);

        RoutingDecision {
            target_cluster: cluster_id,
            target_instance: None,
            load_balancing_key: context.tenant_id.clone(),
            priority: self.get_priority_for_tier(&context.account_tier),
            fallback_clusters: vec!["shard-0".to_string()], // Always fall back to shard 0
            reason: format!("Tenant-based routing: tenant {} -> shard {}", context.tenant_id, shard),
        }
    }

    fn route_by_workload(&self, context: &RoutingContext, operation_type: &str, 
                        read_replicas: &[String], write_primaries: &[String], 
                        analytics_clusters: &[String]) -> RoutingDecision {
        let target_cluster = match (operation_type, &context.workload_type) {
            ("read" | "search", WorkloadType::OLAP) => {
                analytics_clusters.first().unwrap_or(&read_replicas[0]).clone()
            }
            ("read" | "search", _) => {
                read_replicas[0].clone() // TODO: Implement load balancing
            }
            ("write" | "insert" | "update" | "delete", _) => {
                write_primaries[0].clone() // TODO: Implement load balancing
            }
            _ => read_replicas[0].clone(),
        };

        RoutingDecision {
            target_cluster,
            target_instance: None,
            load_balancing_key: context.tenant_id.clone(),
            priority: self.get_priority_for_tier(&context.account_tier),
            fallback_clusters: read_replicas.to_vec(),
            reason: format!("Workload-based routing: {} operation for {:?} workload", operation_type, context.workload_type),
        }
    }

    fn hash_tenant_id(&self, tenant_id: &str) -> u32 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        tenant_id.hash(&mut hasher);
        hasher.finish() as u32
    }

    fn get_priority_for_tier(&self, tier: &AccountTier) -> RoutingPriority {
        match tier {
            AccountTier::Enterprise | AccountTier::Custom => RoutingPriority::High,
            AccountTier::Professional => RoutingPriority::Medium,
            AccountTier::Starter => RoutingPriority::Low,
            AccountTier::Free => RoutingPriority::Batch,
        }
    }
}