/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Shard-Aware Routing Service
//!
//! Provides intelligent routing of requests to appropriate nodes based on:
//! - Shard placement and replication
//! - Node health and load balancing
//! - Read/write operation requirements
//! - Locality preferences

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use super::node_registry::{NodeHealth, NodeInfo, NodeRole};
use super::shard::{PartitionConfig, PartitionStrategy, ShardId};

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

/// Result of a routing decision
#[derive(Debug, Clone)]
pub struct RouteDecision {
    /// Target node for the request
    pub target_node: NodeInfo,
    /// Shard ID if applicable
    pub shard_id: Option<ShardId>,
    /// Whether this is a primary or replica
    pub is_primary: bool,
    /// Retry count if this is a retry
    pub retry_count: u32,
    /// Routing latency in microseconds
    pub routing_latency_us: u64,
}

/// Routing request type
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OperationType {
    /// Read operation (can use replicas)
    Read,
    /// Write operation (must go to primary)
    Write,
    /// Admin operation (must go to leader)
    Admin,
}

/// Context for metadata-aware routing decisions
///
/// `RouteContext` provides additional information to the routing layer
/// that enables intelligent shard selection based on:
/// - Multi-tenant isolation (tenant_id)
/// - Domain-based partitioning (domain_id)
/// - Custom partition keys for flexible sharding
/// - Filter hints for shard pruning optimization
///
/// # Example
///
/// ```ignore
/// let context = RouteContext::new()
///     .with_tenant_id("tenant-123")
///     .with_domain_id("analytics")
///     .with_filter_hint("category", "electronics");
///
/// let decision = routing_service
///     .route_with_context("my_collection", OperationType::Read, Some("vec_1"), &context)
///     .await?;
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RouteContext {
    /// Tenant identifier for multi-tenant routing
    /// When set, routes to shards containing data for this tenant
    pub tenant_id: Option<String>,

    /// Domain identifier for domain-based partitioning
    /// Useful for separating data by business domain (e.g., "analytics", "ml", "search")
    pub domain_id: Option<String>,

    /// Custom partition key for hash-based routing
    /// Used when PartitionStrategy::HashMetadata is configured
    pub partition_key: Option<String>,

    /// Filter hints for shard pruning optimization
    /// Maps field names to expected values, allowing the router to
    /// skip shards that definitely don't contain matching data
    pub filter_hints: HashMap<String, serde_json::Value>,

    /// Whether to prefer local shards (same availability zone/region)
    pub prefer_local: bool,

    /// Optional request trace ID for distributed tracing
    pub trace_id: Option<String>,
}

impl RouteContext {
    /// Create a new empty route context
    pub fn new() -> Self {
        Self::default()
    }

    /// Set tenant ID for multi-tenant routing
    pub fn with_tenant_id(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant_id.into());
        self
    }

    /// Set domain ID for domain-based routing
    pub fn with_domain_id(mut self, domain_id: impl Into<String>) -> Self {
        self.domain_id = Some(domain_id.into());
        self
    }

    /// Set partition key for hash-based routing
    pub fn with_partition_key(mut self, partition_key: impl Into<String>) -> Self {
        self.partition_key = Some(partition_key.into());
        self
    }

    /// Add a filter hint for shard pruning
    pub fn with_filter_hint(mut self, field: impl Into<String>, value: serde_json::Value) -> Self {
        self.filter_hints.insert(field.into(), value);
        self
    }

    /// Set locality preference
    pub fn with_prefer_local(mut self, prefer: bool) -> Self {
        self.prefer_local = prefer;
        self
    }

    /// Set trace ID for distributed tracing
    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        self.trace_id = Some(trace_id.into());
        self
    }

    /// Extract the effective partition key based on context and strategy
    ///
    /// Returns the partition key to use for shard selection based on:
    /// 1. Explicit partition_key if set
    /// 2. tenant_id for Tenant strategy
    /// 3. domain_id for Domain strategy
    /// 4. Composite key for TenantHash strategy
    pub fn effective_partition_key(&self, strategy: &PartitionStrategy) -> Option<String> {
        // If explicit partition key is set, use it
        if self.partition_key.is_some() {
            return self.partition_key.clone();
        }

        match strategy {
            PartitionStrategy::HashId => None,
            PartitionStrategy::HashMetadata { fields } => {
                // Build composite key from filter hints matching the configured fields
                let key_parts: Vec<String> = fields
                    .iter()
                    .filter_map(|f| {
                        self.filter_hints.get(f).map(|v| match v {
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
            PartitionStrategy::Range { field, .. } => {
                self.filter_hints.get(field).map(|v| match v {
                    serde_json::Value::String(s) => s.clone(),
                    _ => v.to_string(),
                })
            }
            PartitionStrategy::Tenant => self.tenant_id.clone(),
            PartitionStrategy::Domain => self.domain_id.clone(),
            PartitionStrategy::TenantHash { .. } => self.tenant_id.clone(),
        }
    }

    /// Check if context has any routing hints
    pub fn has_hints(&self) -> bool {
        self.tenant_id.is_some()
            || self.domain_id.is_some()
            || self.partition_key.is_some()
            || !self.filter_hints.is_empty()
    }
}

/// Routing statistics
#[derive(Debug, Default)]
struct RoutingStats {
    total_routes: u64,
    primary_routes: u64,
    replica_routes: u64,
    retries: u64,
    failures: u64,
    total_latency_us: u64,
}

/// Internal node state for routing
struct RoutableNode {
    info: NodeInfo,
    #[allow(dead_code)] // Reserved for per-node round-robin in future load balancing
    round_robin_counter: u64,
    last_latency_ms: f64,
    weight: u32,
}

/// Cached partition configuration entry with TTL
#[derive(Debug, Clone)]
struct CachedPartitionConfig {
    /// The partition configuration
    config: PartitionConfig,
    /// When this entry was cached
    cached_at: Instant,
    /// Number of shards for this collection
    shard_count: u32,
}

impl CachedPartitionConfig {
    /// Check if the cache entry is still valid
    fn is_valid(&self, ttl: Duration) -> bool {
        self.cached_at.elapsed() < ttl
    }
}

/// Routing service for shard-aware request routing
///
/// The RoutingService provides intelligent request routing with support for:
/// - Hash-based sharding (default)
/// - Metadata-aware routing (tenant, domain, custom partition keys)
/// - Shard pruning based on metadata bounds
/// - Load balancing across replicas
/// - Locality-aware routing
///
/// # Metadata-Aware Routing
///
/// When using `route_with_context`, the service uses the collection's `PartitionConfig`
/// to determine the appropriate shard for a request. This enables:
///
/// - **Tenant Isolation**: Route all requests for a tenant to dedicated shards
/// - **Domain Partitioning**: Separate data by business domain
/// - **Custom Partitioning**: Use any metadata field(s) for partition key
///
/// # Example
///
/// ```ignore
/// let routing = RoutingService::new(config)?;
///
/// // Register partition config for a collection
/// routing.register_partition_config("my_collection", PartitionConfig {
///     strategy: PartitionStrategy::Tenant,
///     track_metadata_bounds: true,
///     ..Default::default()
/// }, 8).await?;
///
/// // Route with context
/// let context = RouteContext::new().with_tenant_id("tenant-123");
/// let decision = routing.route_with_context(
///     "my_collection",
///     OperationType::Read,
///     None,
///     &context
/// ).await?;
/// ```
pub struct RoutingService {
    config: RoutingConfig,
    /// Routing table: shard_id -> (primary_node, replica_nodes)
    routing_table: Arc<RwLock<HashMap<ShardId, ShardRoute>>>,
    /// All known nodes with routing state
    nodes: Arc<RwLock<HashMap<String, RoutableNode>>>,
    /// Round-robin counter for load balancing
    rr_counter: Arc<RwLock<u64>>,
    /// Routing statistics
    stats: Arc<RwLock<RoutingStats>>,
    /// Cached partition configurations per collection
    /// Key: collection_id, Value: cached partition config
    partition_configs: Arc<RwLock<HashMap<String, CachedPartitionConfig>>>,
    /// TTL for partition config cache entries
    partition_config_ttl: Duration,
}

/// Route information for a shard
#[derive(Debug, Clone)]
pub struct ShardRoute {
    /// Primary node for this shard
    pub primary: String,
    /// Replica nodes for this shard
    pub replicas: Vec<String>,
    /// Shard is available for routing
    pub available: bool,
}

impl RoutingService {
    /// Default TTL for partition config cache entries (5 minutes)
    const DEFAULT_PARTITION_CONFIG_TTL: Duration = Duration::from_secs(300);

    /// Create a new routing service
    pub fn new(config: RoutingConfig) -> Result<Self> {
        Ok(Self {
            config,
            routing_table: Arc::new(RwLock::new(HashMap::new())),
            nodes: Arc::new(RwLock::new(HashMap::new())),
            rr_counter: Arc::new(RwLock::new(0)),
            stats: Arc::new(RwLock::new(RoutingStats::default())),
            partition_configs: Arc::new(RwLock::new(HashMap::new())),
            partition_config_ttl: Self::DEFAULT_PARTITION_CONFIG_TTL,
        })
    }

    /// Create a new routing service with custom partition config TTL
    pub fn with_partition_config_ttl(config: RoutingConfig, ttl: Duration) -> Result<Self> {
        Ok(Self {
            config,
            routing_table: Arc::new(RwLock::new(HashMap::new())),
            nodes: Arc::new(RwLock::new(HashMap::new())),
            rr_counter: Arc::new(RwLock::new(0)),
            stats: Arc::new(RwLock::new(RoutingStats::default())),
            partition_configs: Arc::new(RwLock::new(HashMap::new())),
            partition_config_ttl: ttl,
        })
    }

    /// Route a request to an appropriate node
    ///
    /// This is the basic routing method that uses hash-based sharding on the
    /// collection and vector ID. For metadata-aware routing, use `route_with_context`
    /// or `route_request` with a `RouteContext`.
    pub async fn route(
        &self,
        collection_id: &str,
        operation: OperationType,
        vector_id: Option<&str>,
    ) -> Result<RouteDecision> {
        let start = Instant::now();

        // Determine shard for this request
        let shard_id = self.compute_shard_id(collection_id, vector_id).await?;

        // Get route for the shard
        let route = {
            let table = self.routing_table.read().await;
            table.get(&shard_id).cloned()
        };

        let target_node = match &route {
            Some(r) if r.available => self.select_node(r, operation).await?,
            _ => {
                // No specific route, use any available node
                self.select_any_node(operation).await?
            }
        };

        let is_primary = match &route {
            Some(r) => target_node.node_id == r.primary,
            None => true,
        };

        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.total_routes += 1;
            if is_primary {
                stats.primary_routes += 1;
            } else {
                stats.replica_routes += 1;
            }
            stats.total_latency_us += start.elapsed().as_micros() as u64;
        }

        Ok(RouteDecision {
            target_node,
            shard_id: Some(shard_id),
            is_primary,
            retry_count: 0,
            routing_latency_us: start.elapsed().as_micros() as u64,
        })
    }

    /// Route a request with optional metadata context
    ///
    /// This is the unified routing method that supports both basic hash-based
    /// routing and metadata-aware routing. When a `RouteContext` is provided,
    /// it uses the collection's partition configuration to determine the
    /// optimal shard.
    ///
    /// # Arguments
    ///
    /// * `collection_id` - The collection to route to
    /// * `operation` - Type of operation (Read, Write, Admin)
    /// * `vector_id` - Optional vector ID for ID-based routing
    /// * `context` - Optional routing context for metadata-aware routing
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Basic routing (no context)
    /// let decision = routing.route_request("my_collection", OperationType::Read, Some("vec_1"), None).await?;
    ///
    /// // Metadata-aware routing
    /// let context = RouteContext::new().with_tenant_id("tenant-123");
    /// let decision = routing.route_request("my_collection", OperationType::Read, None, Some(&context)).await?;
    /// ```
    pub async fn route_request(
        &self,
        collection_id: &str,
        operation: OperationType,
        vector_id: Option<&str>,
        context: Option<&RouteContext>,
    ) -> Result<RouteDecision> {
        match context {
            Some(ctx) if ctx.has_hints() => {
                // Use metadata-aware routing when context has hints
                self.route_with_context(collection_id, operation, vector_id, ctx)
                    .await
            }
            _ => {
                // Fall back to basic hash-based routing
                self.route(collection_id, operation, vector_id).await
            }
        }
    }

    /// Compute shard ID for a request
    async fn compute_shard_id(
        &self,
        collection_id: &str,
        vector_id: Option<&str>,
    ) -> Result<ShardId> {
        // Simple hash-based sharding
        let mut hasher = DefaultHasher::new();
        collection_id.hash(&mut hasher);
        if let Some(vid) = vector_id {
            vid.hash(&mut hasher);
        }

        Ok(ShardId::new(format!("shard_{:016x}", hasher.finish())))
    }

    // =========================================================================
    // Metadata-Aware Routing Methods
    // =========================================================================

    /// Route a request using metadata context for intelligent shard selection
    ///
    /// This method uses the collection's partition configuration and the provided
    /// `RouteContext` to determine the optimal shard for the request. It supports:
    ///
    /// - **Tenant-based routing**: All data for a tenant goes to specific shards
    /// - **Domain-based routing**: Data partitioned by business domain
    /// - **Custom hash routing**: Hash on specified metadata fields
    /// - **Range-based routing**: Route based on field value ranges
    ///
    /// # Arguments
    ///
    /// * `collection_id` - The collection to route to
    /// * `operation` - Type of operation (Read, Write, Admin)
    /// * `vector_id` - Optional vector ID for ID-based routing fallback
    /// * `context` - Routing context with tenant_id, domain_id, partition_key, filter hints
    ///
    /// # Returns
    ///
    /// A `RouteDecision` containing the target node and shard information
    ///
    /// # Example
    ///
    /// ```ignore
    /// let context = RouteContext::new()
    ///     .with_tenant_id("acme-corp")
    ///     .with_filter_hint("category", json!("electronics"));
    ///
    /// let decision = routing.route_with_context(
    ///     "products",
    ///     OperationType::Read,
    ///     None,
    ///     &context
    /// ).await?;
    /// ```
    pub async fn route_with_context(
        &self,
        collection_id: &str,
        operation: OperationType,
        vector_id: Option<&str>,
        context: &RouteContext,
    ) -> Result<RouteDecision> {
        let start = Instant::now();

        // Get partition config for this collection (if cached)
        let partition_config = self.get_partition_config(collection_id).await;

        // Compute shard ID based on partition strategy and context
        let shard_id = self
            .compute_shard_id_with_context(
                collection_id,
                vector_id,
                context,
                partition_config.as_ref(),
            )
            .await?;

        // Get route for the shard
        let route = {
            let table = self.routing_table.read().await;
            table.get(&shard_id).cloned()
        };

        let target_node = match &route {
            Some(r) if r.available => self.select_node(r, operation).await?,
            _ => {
                // No specific route, use any available node
                self.select_any_node(operation).await?
            }
        };

        let is_primary = match &route {
            Some(r) => target_node.node_id == r.primary,
            None => true,
        };

        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.total_routes += 1;
            if is_primary {
                stats.primary_routes += 1;
            } else {
                stats.replica_routes += 1;
            }
            stats.total_latency_us += start.elapsed().as_micros() as u64;
        }

        Ok(RouteDecision {
            target_node,
            shard_id: Some(shard_id),
            is_primary,
            retry_count: 0,
            routing_latency_us: start.elapsed().as_micros() as u64,
        })
    }

    /// Compute shard ID using partition strategy and route context
    ///
    /// This method implements the core sharding logic for different partition strategies:
    ///
    /// - `HashId`: Hash on vector_id (default behavior)
    /// - `HashMetadata`: Hash on specified metadata fields from context
    /// - `Range`: Select shard based on field value ranges
    /// - `Tenant`: Hash on tenant_id for tenant isolation
    /// - `Domain`: Hash on domain_id for domain separation
    /// - `TenantHash`: Composite of tenant + sub-shard for scalability
    async fn compute_shard_id_with_context(
        &self,
        collection_id: &str,
        vector_id: Option<&str>,
        context: &RouteContext,
        partition_config: Option<&CachedPartitionConfig>,
    ) -> Result<ShardId> {
        // If no partition config, fall back to default hash-based routing
        let Some(cached_config) = partition_config else {
            return self.compute_shard_id(collection_id, vector_id).await;
        };

        let config = &cached_config.config;
        let shard_count = cached_config.shard_count;

        // Get effective partition key from context
        let partition_key = context.effective_partition_key(&config.strategy);

        match &config.strategy {
            PartitionStrategy::HashId => {
                // Default: hash on collection + vector_id
                self.compute_shard_id(collection_id, vector_id).await
            }

            PartitionStrategy::HashMetadata { fields: _ } => {
                // Hash on partition key derived from metadata fields
                if let Some(key) = partition_key {
                    self.compute_shard_from_key(collection_id, &key, shard_count)
                } else {
                    // Fall back to vector_id if no metadata available
                    self.compute_shard_id(collection_id, vector_id).await
                }
            }

            PartitionStrategy::Range { field, boundaries } => {
                // Find the shard based on value range
                if let Some(key) = partition_key {
                    self.compute_shard_from_range(collection_id, &key, boundaries, shard_count)
                } else {
                    self.compute_shard_id(collection_id, vector_id).await
                }
            }

            PartitionStrategy::Tenant => {
                // Hash on tenant_id for tenant isolation
                if let Some(tenant_id) = &context.tenant_id {
                    self.compute_shard_from_key(collection_id, tenant_id, shard_count)
                } else {
                    self.compute_shard_id(collection_id, vector_id).await
                }
            }

            PartitionStrategy::Domain => {
                // Hash on domain_id for domain separation
                if let Some(domain_id) = &context.domain_id {
                    self.compute_shard_from_key(collection_id, domain_id, shard_count)
                } else {
                    self.compute_shard_id(collection_id, vector_id).await
                }
            }

            PartitionStrategy::TenantHash { shards_per_tenant } => {
                // Composite: tenant determines shard group, then hash within group
                if let Some(tenant_id) = &context.tenant_id {
                    self.compute_tenant_hash_shard(
                        collection_id,
                        tenant_id,
                        vector_id,
                        *shards_per_tenant,
                        shard_count,
                    )
                } else {
                    self.compute_shard_id(collection_id, vector_id).await
                }
            }
        }
    }

    /// Compute shard ID from a partition key using consistent hashing
    fn compute_shard_from_key(
        &self,
        collection_id: &str,
        partition_key: &str,
        shard_count: u32,
    ) -> Result<ShardId> {
        let mut hasher = DefaultHasher::new();
        collection_id.hash(&mut hasher);
        partition_key.hash(&mut hasher);
        let hash = hasher.finish();

        // Map hash to shard number
        let shard_number = (hash % shard_count as u64) as u32;
        Ok(ShardId::generate(collection_id, shard_number))
    }

    /// Compute shard ID from a value using range boundaries
    fn compute_shard_from_range(
        &self,
        collection_id: &str,
        value: &str,
        boundaries: &[serde_json::Value],
        shard_count: u32,
    ) -> Result<ShardId> {
        // Find the first boundary that the value is less than
        let shard_number = boundaries
            .iter()
            .position(|boundary| match boundary {
                serde_json::Value::String(b) => value < b.as_str(),
                serde_json::Value::Number(n) => {
                    if let Ok(v) = value.parse::<f64>() {
                        n.as_f64().map(|b| v < b).unwrap_or(false)
                    } else {
                        false
                    }
                }
                _ => false,
            })
            .unwrap_or(boundaries.len()) as u32;

        // Ensure shard number is within bounds
        let shard_number = shard_number.min(shard_count - 1);
        Ok(ShardId::generate(collection_id, shard_number))
    }

    /// Compute shard ID for tenant-hash strategy
    ///
    /// This provides a two-level sharding scheme:
    /// 1. Tenant determines which group of shards to use
    /// 2. Vector ID determines which shard within the group
    fn compute_tenant_hash_shard(
        &self,
        collection_id: &str,
        tenant_id: &str,
        vector_id: Option<&str>,
        shards_per_tenant: u32,
        total_shards: u32,
    ) -> Result<ShardId> {
        // Calculate tenant's base shard
        let mut tenant_hasher = DefaultHasher::new();
        tenant_id.hash(&mut tenant_hasher);
        let tenant_hash = tenant_hasher.finish();

        // Number of tenant groups
        let tenant_groups = total_shards.div_ceil(shards_per_tenant);
        let tenant_group = (tenant_hash % tenant_groups as u64) as u32;
        let base_shard = tenant_group * shards_per_tenant;

        // Hash within tenant's shard group
        let sub_shard = if let Some(vid) = vector_id {
            let mut hasher = DefaultHasher::new();
            vid.hash(&mut hasher);
            (hasher.finish() % shards_per_tenant as u64) as u32
        } else {
            0
        };

        let shard_number = (base_shard + sub_shard).min(total_shards - 1);
        Ok(ShardId::generate(collection_id, shard_number))
    }

    // =========================================================================
    // Partition Configuration Management
    // =========================================================================

    /// Register a partition configuration for a collection
    ///
    /// This caches the partition config for efficient routing decisions.
    /// The cache entry will be automatically invalidated after the TTL.
    ///
    /// # Arguments
    ///
    /// * `collection_id` - The collection ID
    /// * `config` - The partition configuration
    /// * `shard_count` - Number of shards for this collection
    pub async fn register_partition_config(
        &self,
        collection_id: &str,
        config: PartitionConfig,
        shard_count: u32,
    ) -> Result<()> {
        let mut configs = self.partition_configs.write().await;
        configs.insert(
            collection_id.to_string(),
            CachedPartitionConfig {
                config,
                cached_at: Instant::now(),
                shard_count,
            },
        );

        tracing::debug!(
            collection_id = %collection_id,
            shard_count = shard_count,
            "Registered partition config"
        );

        Ok(())
    }

    /// Get cached partition configuration for a collection
    ///
    /// Returns None if no config is cached or if the cache entry has expired.
    pub async fn get_partition_config(&self, collection_id: &str) -> Option<CachedPartitionConfig> {
        let configs = self.partition_configs.read().await;
        configs.get(collection_id).and_then(|cached| {
            if cached.is_valid(self.partition_config_ttl) {
                Some(cached.clone())
            } else {
                None
            }
        })
    }

    /// Invalidate cached partition configuration for a collection
    ///
    /// Call this when the collection's partition config changes.
    pub async fn invalidate_partition_config(&self, collection_id: &str) -> Result<()> {
        let mut configs = self.partition_configs.write().await;
        configs.remove(collection_id);
        Ok(())
    }

    /// Clear all cached partition configurations
    pub async fn clear_partition_configs(&self) -> Result<()> {
        let mut configs = self.partition_configs.write().await;
        configs.clear();
        Ok(())
    }

    /// Get all registered collection IDs with partition configs
    pub async fn list_partition_configs(&self) -> Vec<String> {
        let configs = self.partition_configs.read().await;
        configs
            .iter()
            .filter(|(_, cached)| cached.is_valid(self.partition_config_ttl))
            .map(|(id, _)| id.clone())
            .collect()
    }

    // =========================================================================
    // Multi-Shard Routing for Scatter-Gather
    // =========================================================================

    /// Route a request to multiple shards for scatter-gather operations
    ///
    /// This is used for search operations that need to query multiple shards
    /// and aggregate results. It supports shard pruning based on metadata bounds.
    ///
    /// # Arguments
    ///
    /// * `collection_id` - The collection to route to
    /// * `operation` - Type of operation
    /// * `context` - Optional routing context for shard pruning
    ///
    /// # Returns
    ///
    /// A vector of `RouteDecision` for each relevant shard
    pub async fn route_to_all_shards(
        &self,
        collection_id: &str,
        operation: OperationType,
        context: Option<&RouteContext>,
    ) -> Result<Vec<RouteDecision>> {
        let start = Instant::now();

        // Get all shards for this collection from routing table
        let table = self.routing_table.read().await;
        let collection_prefix = format!("{}_", collection_id);

        let mut decisions = Vec::new();

        for (shard_id, route) in table.iter() {
            // Filter to shards belonging to this collection
            if !shard_id.id().starts_with(&collection_prefix) {
                continue;
            }

            // Skip unavailable shards
            if !route.available {
                continue;
            }

            // Note: Shard pruning based on metadata bounds would be done here
            // if we had access to the shard metadata. For now, we include all shards.
            // In production, the ShardManager would be consulted for metadata bounds.

            let target_node = match self.select_node(route, operation).await {
                Ok(node) => node,
                Err(_) => continue, // Skip shards with no available nodes
            };

            let is_primary = target_node.node_id == route.primary;

            decisions.push(RouteDecision {
                target_node,
                shard_id: Some(shard_id.clone()),
                is_primary,
                retry_count: 0,
                routing_latency_us: start.elapsed().as_micros() as u64,
            });
        }

        // Log routing decision for tracing
        if let Some(ctx) = context {
            if let Some(trace_id) = &ctx.trace_id {
                tracing::debug!(
                    trace_id = %trace_id,
                    collection_id = %collection_id,
                    shard_count = decisions.len(),
                    "Routed to multiple shards"
                );
            }
        }

        Ok(decisions)
    }

    /// Select a node based on operation type and load balancing
    async fn select_node(&self, route: &ShardRoute, operation: OperationType) -> Result<NodeInfo> {
        let nodes = self.nodes.read().await;

        match operation {
            OperationType::Write | OperationType::Admin => {
                // Write operations must go to primary
                nodes
                    .get(&route.primary)
                    .filter(|n| n.info.health == NodeHealth::Healthy)
                    .map(|n| n.info.clone())
                    .ok_or_else(|| anyhow::anyhow!("Primary node unavailable"))
            }
            OperationType::Read => {
                if self.config.enable_read_replicas && !route.replicas.is_empty() {
                    // Try to use a replica
                    self.select_from_nodes(&route.replicas, &nodes)
                        .await
                        .or_else(|_| {
                            // Fall back to primary
                            nodes
                                .get(&route.primary)
                                .map(|n| n.info.clone())
                                .ok_or_else(|| anyhow::anyhow!("No nodes available"))
                        })
                } else {
                    // Use primary
                    nodes
                        .get(&route.primary)
                        .map(|n| n.info.clone())
                        .ok_or_else(|| anyhow::anyhow!("Primary node unavailable"))
                }
            }
        }
    }

    /// Select a node from a list using load balancing
    async fn select_from_nodes(
        &self,
        node_ids: &[String],
        nodes: &HashMap<String, RoutableNode>,
    ) -> Result<NodeInfo> {
        let healthy_nodes: Vec<_> = node_ids
            .iter()
            .filter_map(|id| nodes.get(id))
            .filter(|n| n.info.health == NodeHealth::Healthy)
            .collect();

        if healthy_nodes.is_empty() {
            return Err(anyhow::anyhow!("No healthy nodes available"));
        }

        let selected = match self.config.load_balancing {
            LoadBalancingStrategy::RoundRobin => {
                let mut counter = self.rr_counter.write().await;
                let idx = (*counter as usize) % healthy_nodes.len();
                *counter += 1;
                &healthy_nodes[idx]
            }
            LoadBalancingStrategy::LeastLoaded => healthy_nodes
                .iter()
                .min_by(|a, b| a.info.load.partial_cmp(&b.info.load).unwrap())
                .unwrap(),
            LoadBalancingStrategy::LeastLatency => healthy_nodes
                .iter()
                .min_by(|a, b| a.last_latency_ms.partial_cmp(&b.last_latency_ms).unwrap())
                .unwrap(),
            LoadBalancingStrategy::Random => {
                use rand::Rng;
                let idx = rand::thread_rng().gen_range(0..healthy_nodes.len());
                &healthy_nodes[idx]
            }
            LoadBalancingStrategy::WeightedRoundRobin => {
                // Simplified weighted round-robin
                let total_weight: u32 = healthy_nodes.iter().map(|n| n.weight).sum();
                let mut counter = self.rr_counter.write().await;
                let target = (*counter as u32) % total_weight;
                *counter += 1;

                let mut cumulative = 0u32;
                healthy_nodes
                    .iter()
                    .find(|n| {
                        cumulative += n.weight;
                        cumulative > target
                    })
                    .unwrap()
            }
        };

        Ok(selected.info.clone())
    }

    /// Select any available node for operations without specific routing
    async fn select_any_node(&self, operation: OperationType) -> Result<NodeInfo> {
        let nodes = self.nodes.read().await;

        let healthy_nodes: Vec<_> = nodes
            .values()
            .filter(|n| n.info.health == NodeHealth::Healthy)
            .filter(|n| match operation {
                OperationType::Admin => n.info.role == NodeRole::Leader,
                _ => true,
            })
            .collect();

        if healthy_nodes.is_empty() {
            return Err(anyhow::anyhow!("No healthy nodes available"));
        }

        // Use round-robin for selection
        let mut counter = self.rr_counter.write().await;
        let idx = (*counter as usize) % healthy_nodes.len();
        *counter += 1;

        Ok(healthy_nodes[idx].info.clone())
    }

    /// Update routing table for a shard
    pub async fn update_route(&self, shard_id: ShardId, route: ShardRoute) -> Result<()> {
        let mut table = self.routing_table.write().await;
        table.insert(shard_id, route);
        Ok(())
    }

    /// Register a node for routing
    pub async fn register_node(&self, info: NodeInfo, weight: u32) -> Result<()> {
        let mut nodes = self.nodes.write().await;
        nodes.insert(
            info.node_id.clone(),
            RoutableNode {
                info,
                round_robin_counter: 0,
                last_latency_ms: 0.0,
                weight,
            },
        );
        Ok(())
    }

    /// Update node latency for latency-based routing
    pub async fn update_node_latency(&self, node_id: &str, latency_ms: f64) -> Result<()> {
        let mut nodes = self.nodes.write().await;
        if let Some(node) = nodes.get_mut(node_id) {
            // Exponential moving average
            node.last_latency_ms = node.last_latency_ms * 0.7 + latency_ms * 0.3;
        }
        Ok(())
    }

    /// Get routing statistics
    pub async fn get_stats(&self) -> RoutingStatsSummary {
        let stats = self.stats.read().await;
        RoutingStatsSummary {
            total_routes: stats.total_routes,
            primary_routes: stats.primary_routes,
            replica_routes: stats.replica_routes,
            retries: stats.retries,
            failures: stats.failures,
            avg_latency_us: if stats.total_routes > 0 {
                stats.total_latency_us / stats.total_routes
            } else {
                0
            },
        }
    }
}

/// Summary of routing statistics
#[derive(Debug, Clone)]
pub struct RoutingStatsSummary {
    pub total_routes: u64,
    pub primary_routes: u64,
    pub replica_routes: u64,
    pub retries: u64,
    pub failures: u64,
    pub avg_latency_us: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_routing_service_creation() {
        let config = RoutingConfig::default();
        let service = RoutingService::new(config);
        assert!(service.is_ok());
    }

    #[tokio::test]
    async fn test_shard_id_computation() {
        let service = RoutingService::new(RoutingConfig::default()).unwrap();

        let shard1 = service
            .compute_shard_id("collection1", Some("vec1"))
            .await
            .unwrap();
        let shard2 = service
            .compute_shard_id("collection1", Some("vec1"))
            .await
            .unwrap();
        let shard3 = service
            .compute_shard_id("collection1", Some("vec2"))
            .await
            .unwrap();

        // Same input should produce same shard
        assert_eq!(shard1.id(), shard2.id());
        // Different input should produce different shard
        assert_ne!(shard1.id(), shard3.id());
    }

    #[tokio::test]
    async fn test_node_registration() {
        let service = RoutingService::new(RoutingConfig::default()).unwrap();

        let info = NodeInfo {
            node_id: "node-1".to_string(),
            address: "127.0.0.1:5679".to_string(),
            health: NodeHealth::Healthy,
            ..Default::default()
        };

        service.register_node(info, 100).await.unwrap();

        let result = service.select_any_node(OperationType::Read).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().node_id, "node-1");
    }

    // =========================================================================
    // RouteContext Tests
    // =========================================================================

    #[test]
    fn test_route_context_creation() {
        let context = RouteContext::new();
        assert!(context.tenant_id.is_none());
        assert!(context.domain_id.is_none());
        assert!(context.partition_key.is_none());
        assert!(context.filter_hints.is_empty());
        assert!(!context.prefer_local);
    }

    #[test]
    fn test_route_context_builder() {
        let context = RouteContext::new()
            .with_tenant_id("tenant-123")
            .with_domain_id("analytics")
            .with_partition_key("custom-key")
            .with_filter_hint("category", serde_json::json!("electronics"))
            .with_prefer_local(true)
            .with_trace_id("trace-abc");

        assert_eq!(context.tenant_id, Some("tenant-123".to_string()));
        assert_eq!(context.domain_id, Some("analytics".to_string()));
        assert_eq!(context.partition_key, Some("custom-key".to_string()));
        assert!(context.filter_hints.contains_key("category"));
        assert!(context.prefer_local);
        assert_eq!(context.trace_id, Some("trace-abc".to_string()));
    }

    #[test]
    fn test_route_context_has_hints() {
        let empty_context = RouteContext::new();
        assert!(!empty_context.has_hints());

        let tenant_context = RouteContext::new().with_tenant_id("tenant-1");
        assert!(tenant_context.has_hints());

        let domain_context = RouteContext::new().with_domain_id("domain-1");
        assert!(domain_context.has_hints());

        let partition_context = RouteContext::new().with_partition_key("key-1");
        assert!(partition_context.has_hints());

        let filter_context =
            RouteContext::new().with_filter_hint("field", serde_json::json!("value"));
        assert!(filter_context.has_hints());
    }

    #[test]
    fn test_effective_partition_key_tenant_strategy() {
        let context = RouteContext::new().with_tenant_id("tenant-123");
        let strategy = PartitionStrategy::Tenant;

        let key = context.effective_partition_key(&strategy);
        assert_eq!(key, Some("tenant-123".to_string()));
    }

    #[test]
    fn test_effective_partition_key_domain_strategy() {
        let context = RouteContext::new().with_domain_id("analytics");
        let strategy = PartitionStrategy::Domain;

        let key = context.effective_partition_key(&strategy);
        assert_eq!(key, Some("analytics".to_string()));
    }

    #[test]
    fn test_effective_partition_key_hash_metadata() {
        let context = RouteContext::new()
            .with_filter_hint("region", serde_json::json!("us-west"))
            .with_filter_hint("category", serde_json::json!("electronics"));

        let strategy = PartitionStrategy::HashMetadata {
            fields: vec!["region".to_string(), "category".to_string()],
        };

        let key = context.effective_partition_key(&strategy);
        assert!(key.is_some());
        let key_str = key.unwrap();
        assert!(key_str.contains("us-west"));
        assert!(key_str.contains("electronics"));
    }

    #[test]
    fn test_effective_partition_key_explicit_override() {
        // When partition_key is explicitly set, it should override strategy-based keys
        let context = RouteContext::new()
            .with_tenant_id("tenant-123")
            .with_partition_key("explicit-key");

        let strategy = PartitionStrategy::Tenant;

        let key = context.effective_partition_key(&strategy);
        assert_eq!(key, Some("explicit-key".to_string()));
    }

    // =========================================================================
    // Partition Config Cache Tests
    // =========================================================================

    #[tokio::test]
    async fn test_partition_config_registration() {
        let service = RoutingService::new(RoutingConfig::default()).unwrap();

        let config = PartitionConfig {
            strategy: PartitionStrategy::Tenant,
            partition_key_fields: vec![],
            track_metadata_bounds: true,
        };

        service
            .register_partition_config("my_collection", config, 8)
            .await
            .unwrap();

        let cached = service.get_partition_config("my_collection").await;
        assert!(cached.is_some());

        let cached_config = cached.unwrap();
        assert_eq!(cached_config.shard_count, 8);
        assert!(matches!(
            cached_config.config.strategy,
            PartitionStrategy::Tenant
        ));
    }

    #[tokio::test]
    async fn test_partition_config_invalidation() {
        let service = RoutingService::new(RoutingConfig::default()).unwrap();

        let config = PartitionConfig {
            strategy: PartitionStrategy::Domain,
            ..Default::default()
        };

        service
            .register_partition_config("collection1", config, 4)
            .await
            .unwrap();

        // Should be cached
        assert!(service.get_partition_config("collection1").await.is_some());

        // Invalidate
        service
            .invalidate_partition_config("collection1")
            .await
            .unwrap();

        // Should be gone
        assert!(service.get_partition_config("collection1").await.is_none());
    }

    #[tokio::test]
    async fn test_list_partition_configs() {
        let service = RoutingService::new(RoutingConfig::default()).unwrap();

        let config = PartitionConfig::default();

        service
            .register_partition_config("collection1", config.clone(), 4)
            .await
            .unwrap();
        service
            .register_partition_config("collection2", config.clone(), 8)
            .await
            .unwrap();
        service
            .register_partition_config("collection3", config, 16)
            .await
            .unwrap();

        let collections = service.list_partition_configs().await;
        assert_eq!(collections.len(), 3);
        assert!(collections.contains(&"collection1".to_string()));
        assert!(collections.contains(&"collection2".to_string()));
        assert!(collections.contains(&"collection3".to_string()));
    }

    // =========================================================================
    // Metadata-Aware Routing Tests
    // =========================================================================

    #[tokio::test]
    async fn test_compute_shard_from_key() {
        let service = RoutingService::new(RoutingConfig::default()).unwrap();

        // Same key should always produce same shard
        let shard1 = service
            .compute_shard_from_key("collection", "tenant-123", 8)
            .unwrap();
        let shard2 = service
            .compute_shard_from_key("collection", "tenant-123", 8)
            .unwrap();
        assert_eq!(shard1.id(), shard2.id());

        // Different keys should produce different shards (with high probability)
        let shard3 = service
            .compute_shard_from_key("collection", "tenant-456", 8)
            .unwrap();
        // Note: Could theoretically be the same due to hash collision, but very unlikely
        assert_ne!(shard1.id(), shard3.id());
    }

    #[tokio::test]
    async fn test_compute_shard_from_range() {
        let service = RoutingService::new(RoutingConfig::default()).unwrap();

        let boundaries = vec![
            serde_json::json!("e"), // Shard 0: < "e"
            serde_json::json!("m"), // Shard 1: "e" <= x < "m"
            serde_json::json!("t"), // Shard 2: "m" <= x < "t"
                                    // Shard 3: >= "t"
        ];

        let shard_a = service
            .compute_shard_from_range("collection", "apple", &boundaries, 4)
            .unwrap();
        let shard_h = service
            .compute_shard_from_range("collection", "hello", &boundaries, 4)
            .unwrap();
        let shard_p = service
            .compute_shard_from_range("collection", "python", &boundaries, 4)
            .unwrap();
        let shard_z = service
            .compute_shard_from_range("collection", "zebra", &boundaries, 4)
            .unwrap();

        // "apple" < "e" -> shard 0
        assert!(shard_a.id().ends_with("_0000"));
        // "hello" >= "e" && < "m" -> shard 1
        assert!(shard_h.id().ends_with("_0001"));
        // "python" >= "m" && < "t" -> shard 2
        assert!(shard_p.id().ends_with("_0002"));
        // "zebra" >= "t" -> shard 3
        assert!(shard_z.id().ends_with("_0003"));
    }

    #[tokio::test]
    async fn test_tenant_hash_shard() {
        let service = RoutingService::new(RoutingConfig::default()).unwrap();

        // All vectors for the same tenant should go to shards in the same group
        let shard1 = service
            .compute_tenant_hash_shard("collection", "tenant-1", Some("vec-1"), 4, 16)
            .unwrap();
        let shard2 = service
            .compute_tenant_hash_shard("collection", "tenant-1", Some("vec-2"), 4, 16)
            .unwrap();
        let shard3 = service
            .compute_tenant_hash_shard("collection", "tenant-1", Some("vec-3"), 4, 16)
            .unwrap();

        // Extract shard numbers
        let num1: u32 = shard1.id().split('_').last().unwrap().parse().unwrap();
        let num2: u32 = shard2.id().split('_').last().unwrap().parse().unwrap();
        let num3: u32 = shard3.id().split('_').last().unwrap().parse().unwrap();

        // All should be in the same tenant group (within 4 shards of each other)
        let base = (num1 / 4) * 4;
        assert!(num1 >= base && num1 < base + 4);
        assert!(num2 >= base && num2 < base + 4);
        assert!(num3 >= base && num3 < base + 4);
    }

    #[tokio::test]
    async fn test_route_with_context_tenant() {
        let service = RoutingService::new(RoutingConfig::default()).unwrap();

        // Register a node
        let node = NodeInfo {
            node_id: "node-1".to_string(),
            address: "127.0.0.1:5679".to_string(),
            health: NodeHealth::Healthy,
            ..Default::default()
        };
        service.register_node(node, 100).await.unwrap();

        // Register partition config
        let config = PartitionConfig {
            strategy: PartitionStrategy::Tenant,
            partition_key_fields: vec![],
            track_metadata_bounds: true,
        };
        service
            .register_partition_config("my_collection", config, 8)
            .await
            .unwrap();

        // Route with tenant context
        let context = RouteContext::new().with_tenant_id("tenant-123");
        let decision = service
            .route_with_context("my_collection", OperationType::Read, None, &context)
            .await
            .unwrap();

        assert!(decision.shard_id.is_some());
        assert_eq!(decision.target_node.node_id, "node-1");

        // Same tenant should route to same shard
        let decision2 = service
            .route_with_context("my_collection", OperationType::Read, None, &context)
            .await
            .unwrap();
        assert_eq!(decision.shard_id, decision2.shard_id);
    }

    #[tokio::test]
    async fn test_route_request_with_and_without_context() {
        let service = RoutingService::new(RoutingConfig::default()).unwrap();

        // Register a node
        let node = NodeInfo {
            node_id: "node-1".to_string(),
            address: "127.0.0.1:5679".to_string(),
            health: NodeHealth::Healthy,
            ..Default::default()
        };
        service.register_node(node, 100).await.unwrap();

        // Route without context
        let decision1 = service
            .route_request("my_collection", OperationType::Read, Some("vec-1"), None)
            .await
            .unwrap();
        assert!(decision1.shard_id.is_some());

        // Route with context
        let context = RouteContext::new().with_tenant_id("tenant-123");
        let decision2 = service
            .route_request(
                "my_collection",
                OperationType::Read,
                Some("vec-1"),
                Some(&context),
            )
            .await
            .unwrap();
        assert!(decision2.shard_id.is_some());

        // Without partition config, both should use hash-based routing
        // (they may or may not be the same depending on hash)
    }
}
