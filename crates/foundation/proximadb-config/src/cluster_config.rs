//! Canonical cluster configuration shapes (convergence seam S4 / TD-107).
//!
//! These pure config structs/enums were consolidated here from `src/cluster/*.rs` so the
//! cluster runtime depends on the canonical config crate rather than defining config inline.
//! The origin modules re-export these names (`pub use proximadb_config::cluster_config::…`)
//! so every `crate::cluster::…` import path keeps resolving unchanged.
//!
//! NOTE: kept in this submodule (NOT re-exported at the crate root) because the crate root
//! already defines a different `ConsensusConfig` (bootstrap/peer-discovery); the cluster
//! `ConsensusConfig` here is Raft tuning.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

// ── Raft consensus tuning (from src/cluster/consensus.rs) ───────────────────

/// Configuration for the Raft consensus module
#[derive(Debug, Clone)]
pub struct ConsensusConfig {
    /// Election timeout range (min, max) in milliseconds
    pub election_timeout_ms: (u64, u64),
    /// Heartbeat interval in milliseconds
    pub heartbeat_interval_ms: u64,
    /// Maximum entries per append entries RPC
    pub max_entries_per_request: usize,
    /// Snapshot threshold (number of log entries before snapshot)
    pub snapshot_threshold: u64,
    /// Enable pre-vote to prevent disruptions from partitioned nodes
    pub enable_pre_vote: bool,
}

impl Default for ConsensusConfig {
    fn default() -> Self {
        Self {
            election_timeout_ms: (150, 300),
            heartbeat_interval_ms: 50,
            max_entries_per_request: 100,
            snapshot_threshold: 10000,
            enable_pre_vote: true,
        }
    }
}

// ── Routing + load-balancing strategy (from src/cluster/routing.rs) ───────

/// Configuration for the routing service
#[derive(Debug, Clone)]
pub struct RoutingConfig {
    /// Enable read replicas for load distribution
    pub enable_read_replicas: bool,
    /// Maximum number of retries for failed requests
    pub max_retries: u32,
    /// Timeout for routing decisions in milliseconds
    pub routing_timeout_ms: u64,
    /// Enable sticky sessions for consistency
    pub sticky_sessions: bool,
    /// Load balancing strategy
    pub load_balancing: LoadBalancingStrategy,
    /// Enable locality-aware routing
    pub locality_aware: bool,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            enable_read_replicas: true,
            max_retries: 3,
            routing_timeout_ms: 100,
            sticky_sessions: false,
            load_balancing: LoadBalancingStrategy::RoundRobin,
            locality_aware: true,
        }
    }
}

/// Load balancing strategies
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoadBalancingStrategy {
    /// Round-robin across available nodes
    RoundRobin,
    /// Route to node with lowest load
    LeastLoaded,
    /// Route to node with lowest latency
    LeastLatency,
    /// Random node selection
    Random,
    /// Weighted round-robin based on node capacity
    WeightedRoundRobin,
}

// ── Shard management (from src/cluster/shard.rs) ────────────────────────

/// Configuration for shard management
#[derive(Debug, Clone)]
pub struct ShardConfig {
    /// Default number of shards per collection
    pub default_shard_count: u32,
    /// Default replication factor
    pub default_replication_factor: u32,
    /// Minimum shards per collection
    pub min_shards: u32,
    /// Maximum shards per collection
    pub max_shards: u32,
    /// Enable automatic shard rebalancing
    pub auto_rebalance: bool,
    /// Rebalance threshold (load imbalance percentage)
    pub rebalance_threshold: f32,
    /// Maximum concurrent rebalance operations
    pub max_concurrent_rebalance: u32,
}

impl Default for ShardConfig {
    fn default() -> Self {
        Self {
            default_shard_count: 3,
            default_replication_factor: 2,
            min_shards: 1,
            max_shards: 256,
            auto_rebalance: true,
            rebalance_threshold: 0.2,
            max_concurrent_rebalance: 2,
        }
    }
}

// ── Partition strategy (from src/cluster/shard.rs) ──────────────────────

/// Partition strategy for a collection
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum PartitionStrategy {
    /// Hash-based partitioning on record ID (default)
    #[default]
    HashId,
    /// Hash-based partitioning on specified metadata field(s)
    HashMetadata {
        /// Fields to hash for partition assignment
        fields: Vec<String>,
    },
    /// Range-based partitioning on a field
    Range {
        /// Field to partition by
        field: String,
        /// Range boundaries
        boundaries: Vec<serde_json::Value>,
    },
    /// Tenant-based partitioning (co-locate all data for a tenant)
    Tenant,
    /// Domain-based partitioning (co-locate all data for a domain)
    Domain,
    /// Composite: tenant + hash for scalability within tenants
    TenantHash {
        /// Number of sub-shards per tenant
        shards_per_tenant: u32,
    },
}

// ── Partition config (from src/cluster/shard.rs) ────────────────────────

/// Configuration for collection partitioning
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PartitionConfig {
    /// Partitioning strategy
    pub strategy: PartitionStrategy,
    /// Fields to extract partition key from (for HashMetadata strategy)
    pub partition_key_fields: Vec<String>,
    /// Whether to update shard metadata bounds on writes
    pub track_metadata_bounds: bool,
}

impl PartitionConfig {
    /// Extract partition key from record metadata
    pub fn extract_partition_key(
        &self,
        metadata: &HashMap<String, serde_json::Value>,
    ) -> Option<String> {
        match &self.strategy {
            PartitionStrategy::HashId => None,
            PartitionStrategy::HashMetadata { fields } => {
                let key_parts: Vec<String> = fields
                    .iter()
                    .filter_map(|f| {
                        metadata.get(f).map(|v| match v {
                            serde_json::Value::String(s) => s.clone(),
                            _ => v.to_string(),
                        })
                    })
                    .collect();
                if key_parts.is_empty() {
                    None
                } else {
                    Some(key_parts.join(":"))
                }
            }
            PartitionStrategy::Range {
                field,
                boundaries: _,
            } => metadata.get(field).map(|v| match v {
                serde_json::Value::String(s) => s.clone(),
                _ => v.to_string(),
            }),
            PartitionStrategy::Tenant => metadata.get("tenant_id").and_then(|v| {
                if let serde_json::Value::String(s) = v {
                    Some(s.clone())
                } else {
                    None
                }
            }),
            PartitionStrategy::Domain => metadata.get("domain_id").and_then(|v| {
                if let serde_json::Value::String(s) = v {
                    Some(s.clone())
                } else {
                    None
                }
            }),
            PartitionStrategy::TenantHash { .. } => metadata.get("tenant_id").and_then(|v| {
                if let serde_json::Value::String(s) = v {
                    Some(s.clone())
                } else {
                    None
                }
            }),
        }
    }
}

// ── Replication (from src/cluster/replication.rs) ─────────────────────────────

/// Configuration for replication
#[derive(Debug, Clone)]
pub struct ClusterReplicationConfig {
    /// Maximum replication lag allowed in milliseconds
    pub max_lag_ms: u64,
    /// Replication timeout in milliseconds
    pub replication_timeout_ms: u64,
    /// Batch size for replication
    pub batch_size: usize,
    /// Enable async replication (vs sync)
    pub async_replication: bool,
    /// Buffer size for replication queue
    pub queue_buffer_size: usize,
    /// Enable compression for replication
    pub enable_compression: bool,
    /// Retry configuration
    pub retry_config: ReplicationRetryConfig,
}

impl Default for ClusterReplicationConfig {
    fn default() -> Self {
        Self {
            max_lag_ms: 1000,
            replication_timeout_ms: 5000,
            batch_size: 100,
            async_replication: false,
            queue_buffer_size: 10000,
            enable_compression: true,
            retry_config: ReplicationRetryConfig::default(),
        }
    }
}

/// Retry configuration for failed replications
#[derive(Debug, Clone)]
pub struct ReplicationRetryConfig {
    /// Maximum retry attempts
    pub max_retries: u32,
    /// Initial backoff in milliseconds
    pub initial_backoff_ms: u64,
    /// Maximum backoff in milliseconds
    pub max_backoff_ms: u64,
    /// Backoff multiplier
    pub backoff_multiplier: f64,
}

impl Default for ReplicationRetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff_ms: 50,
            max_backoff_ms: 2000,
            backoff_multiplier: 2.0,
        }
    }
}

// ── Node registry (from src/cluster/node_registry.rs) ───────────────────────────

/// Configuration for the node registry
#[derive(Debug, Clone)]
pub struct NodeRegistryConfig {
    /// Interval between health checks in milliseconds
    pub health_check_interval_ms: u64,
    /// Timeout for health check responses in milliseconds
    pub health_check_timeout_ms: u64,
    /// Number of failed health checks before marking node unhealthy
    pub unhealthy_threshold: u32,
    /// Number of successful health checks before marking node healthy
    pub healthy_threshold: u32,
    /// Time after which an unresponsive node is considered dead
    pub dead_node_timeout_secs: u64,
}

impl Default for NodeRegistryConfig {
    fn default() -> Self {
        Self {
            health_check_interval_ms: 5000,
            health_check_timeout_ms: 2000,
            unhealthy_threshold: 3,
            healthy_threshold: 2,
            dead_node_timeout_secs: 30,
        }
    }
}

// ── Metadata service (from src/cluster/metadata_service.rs) ────────────────────────

/// Configuration for the metadata service
#[derive(Debug, Clone)]
pub struct MetadataServiceConfig {
    /// Maximum entries in metadata cache
    pub cache_size: usize,
    /// TTL for cached entries in seconds
    pub cache_ttl_secs: u64,
    /// Enable persistent storage of metadata
    pub persistent: bool,
    /// Storage path for persistent metadata
    pub storage_path: Option<String>,
}

impl Default for MetadataServiceConfig {
    fn default() -> Self {
        Self {
            cache_size: 10000,
            cache_ttl_secs: 300,
            persistent: true,
            storage_path: None,
        }
    }
}

// ── Cluster-wide configuration (from src/cluster/metadata_service.rs) ──────────────

/// Cluster-wide configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterConfiguration {
    /// Default replication factor for new collections
    pub default_replication_factor: u32,
    /// Default shard count for new collections
    pub default_shard_count: u32,
    /// Enable automatic rebalancing
    pub auto_rebalance: bool,
    /// Rebalance threshold (load difference percentage)
    pub rebalance_threshold: f32,
}

impl Default for ClusterConfiguration {
    fn default() -> Self {
        Self {
            default_replication_factor: 1,
            default_shard_count: 1,
            auto_rebalance: true,
            rebalance_threshold: 0.2,
        }
    }
}

// ── Distributed ops + consistency (from src/cluster/distributed_ops.rs) ───────────

/// Configuration for distributed operations
#[derive(Debug, Clone)]
pub struct DistributedOpsConfig {
    /// Timeout for distributed operations in milliseconds
    pub operation_timeout_ms: u64,
    /// Maximum concurrent shard operations
    pub max_concurrent_ops: usize,
    /// Enable parallel shard queries
    pub parallel_queries: bool,
    /// Consistency level for writes
    pub write_consistency: ConsistencyLevel,
    /// Consistency level for reads
    pub read_consistency: ConsistencyLevel,
    /// Retry configuration
    pub retry_config: DistributedRetryConfig,
}

impl Default for DistributedOpsConfig {
    fn default() -> Self {
        Self {
            operation_timeout_ms: 30000,
            max_concurrent_ops: 16,
            parallel_queries: true,
            write_consistency: ConsistencyLevel::Quorum,
            read_consistency: ConsistencyLevel::One,
            retry_config: DistributedRetryConfig::default(),
        }
    }
}

/// Backwards-compat alias for [`DistributedRetryConfig`].
pub type RetryConfig = DistributedRetryConfig;

/// Retry configuration for failed operations
#[derive(Debug, Clone)]
pub struct DistributedRetryConfig {
    /// Maximum number of retries
    pub max_retries: u32,
    /// Initial backoff in milliseconds
    pub initial_backoff_ms: u64,
    /// Maximum backoff in milliseconds
    pub max_backoff_ms: u64,
    /// Backoff multiplier
    pub backoff_multiplier: f64,
}

impl Default for DistributedRetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff_ms: 100,
            max_backoff_ms: 5000,
            backoff_multiplier: 2.0,
        }
    }
}

/// Consistency levels for distributed operations
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ConsistencyLevel {
    /// Only one node needs to acknowledge
    One,
    /// Majority of nodes must acknowledge
    Quorum,
    /// All nodes must acknowledge
    All,
    /// Local datacenter quorum
    LocalQuorum,
}

// ── Connection pool (from src/cluster/rpc/connection.rs) ─────────────────────────

/// Configuration for connection pool behavior
#[derive(Debug, Clone)]
pub struct ConnectionPoolConfig {
    /// Maximum number of connections per node
    pub max_connections_per_node: usize,

    /// Idle timeout for connections (after which they may be closed)
    pub idle_timeout: Duration,

    /// Connection timeout for establishing new connections
    pub connect_timeout: Duration,

    /// Request timeout for individual RPC calls
    pub request_timeout: Duration,

    /// Health check interval
    pub health_check_interval: Duration,

    /// Health cache TTL (how long to cache health status)
    pub health_cache_ttl: Duration,

    /// Whether to use TLS for connections
    pub use_tls: bool,

    /// TCP keepalive interval
    pub tcp_keepalive: Option<Duration>,

    /// HTTP/2 keep-alive interval
    pub http2_keepalive_interval: Option<Duration>,

    /// HTTP/2 keep-alive timeout
    pub http2_keepalive_timeout: Option<Duration>,
}

impl Default for ConnectionPoolConfig {
    fn default() -> Self {
        Self {
            max_connections_per_node: 10,
            idle_timeout: Duration::from_secs(300), // 5 minutes
            connect_timeout: Duration::from_secs(5), // 5 seconds
            request_timeout: Duration::from_secs(30), // 30 seconds
            health_check_interval: Duration::from_secs(10), // 10 seconds
            health_cache_ttl: Duration::from_secs(5), // 5 seconds
            use_tls: false,
            tcp_keepalive: Some(Duration::from_secs(60)),
            http2_keepalive_interval: Some(Duration::from_secs(30)),
            http2_keepalive_timeout: Some(Duration::from_secs(10)),
        }
    }
}

impl ConnectionPoolConfig {
    /// Create a new configuration with custom settings
    pub fn new() -> Self {
        Self::default()
    }

    /// Set maximum connections per node
    pub fn with_max_connections(mut self, max: usize) -> Self {
        self.max_connections_per_node = max;
        self
    }

    /// Set idle timeout
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    /// Set connection timeout
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// Set request timeout
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Enable TLS
    pub fn with_tls(mut self, use_tls: bool) -> Self {
        self.use_tls = use_tls;
        self
    }
}

// ── Cluster aggregator config (from src/cluster/mod.rs) ───────────────

/// Cluster configuration
#[derive(Debug, Clone)]
pub struct ClusterConfig {
    /// Unique cluster identifier
    pub cluster_id: String,
    /// This node's identifier
    pub node_id: String,
    /// This node's advertised address
    pub advertise_addr: String,
    /// List of seed nodes for discovery
    pub seed_nodes: Vec<String>,
    /// Metadata service configuration
    pub metadata: MetadataServiceConfig,
    /// Node registry configuration
    pub node_registry: NodeRegistryConfig,
    /// Consensus configuration
    pub consensus: ConsensusConfig,
    /// Routing configuration
    pub routing: RoutingConfig,
    /// Shard configuration
    pub shard: ShardConfig,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            cluster_id: "proximadb-cluster".to_string(),
            node_id: uuid::Uuid::new_v4().to_string(),
            advertise_addr: "127.0.0.1:5679".to_string(),
            seed_nodes: vec![],
            metadata: MetadataServiceConfig::default(),
            node_registry: NodeRegistryConfig::default(),
            consensus: ConsensusConfig::default(),
            routing: RoutingConfig::default(),
            shard: ShardConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    //! TD-107 consolidation guards. These cluster config types moved here from
    //! `src/cluster/*.rs`; the tests pin the behavior that travelled with them
    //! (Default wiring + `extract_partition_key`) so the move stays lossless and
    //! the canonical home is exercised independently of the `cluster` feature.
    use super::*;

    #[test]
    fn cluster_config_default_wires_subconfigs() {
        let c = ClusterConfig::default();
        assert_eq!(c.cluster_id, "proximadb-cluster");
        assert!(!c.node_id.is_empty(), "node_id is a generated uuid");
        // Sub-config Defaults moved intact alongside the aggregator.
        assert_eq!(
            c.consensus.election_timeout_ms,
            ConsensusConfig::default().election_timeout_ms
        );
    }

    #[test]
    fn partition_key_hash_metadata_joins_fields() {
        let cfg = PartitionConfig {
            strategy: PartitionStrategy::HashMetadata {
                fields: vec!["region".to_string(), "tier".to_string()],
            },
            ..Default::default()
        };
        let mut md = HashMap::new();
        md.insert("region".to_string(), serde_json::json!("us-east"));
        md.insert("tier".to_string(), serde_json::json!(3));
        assert_eq!(
            cfg.extract_partition_key(&md),
            Some("us-east:3".to_string())
        );
    }

    #[test]
    fn partition_key_tenant_reads_tenant_id() {
        let cfg = PartitionConfig {
            strategy: PartitionStrategy::Tenant,
            ..Default::default()
        };
        let mut md = HashMap::new();
        md.insert("tenant_id".to_string(), serde_json::json!("acme"));
        assert_eq!(cfg.extract_partition_key(&md), Some("acme".to_string()));
    }

    #[test]
    fn partition_key_hash_id_is_none() {
        let cfg = PartitionConfig::default(); // PartitionStrategy::HashId
        let md = HashMap::new();
        assert_eq!(cfg.extract_partition_key(&md), None);
    }
}
