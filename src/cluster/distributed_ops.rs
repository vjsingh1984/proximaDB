//! Distributed Operations for Collections
//!
//! Provides distributed search and write operations by coordinating
//! the cluster infrastructure (sharding, routing) with collection services.
//!
//! This module implements the fan-out/fan-in pattern for distributed queries
//! and handles write replication to shard replicas.

use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::consensus::RaftConsensus;
use super::node_registry::{NodeInfo, NodeRegistry, NodeStatus};
use super::routing::RoutingService;
use super::rpc::{
    ForwardWriteRequest, NodeEndpoint, SearchFanout, SearchParams, ShardSearchRequest,
    WriteRecord as RpcWriteRecord,
};
use super::shard::{Shard, ShardId, ShardManager, ShardState};

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
    pub retry_config: RetryConfig,
}

impl Default for DistributedOpsConfig {
    fn default() -> Self {
        Self {
            operation_timeout_ms: 30000,
            max_concurrent_ops: 16,
            parallel_queries: true,
            write_consistency: ConsistencyLevel::Quorum,
            read_consistency: ConsistencyLevel::One,
            retry_config: RetryConfig::default(),
        }
    }
}

/// Retry configuration for failed operations
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retries
    pub max_retries: u32,
    /// Initial backoff in milliseconds
    pub initial_backoff_ms: u64,
    /// Maximum backoff in milliseconds
    pub max_backoff_ms: u64,
    /// Backoff multiplier
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
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

/// Result of a distributed search operation
#[derive(Debug, Clone)]
pub struct DistributedSearchResult {
    /// Merged results from all shards
    pub results: Vec<SearchResult>,
    /// Number of shards queried
    pub shards_queried: usize,
    /// Number of shards that succeeded
    pub shards_succeeded: usize,
    /// Total search time in milliseconds
    pub total_time_ms: u64,
    /// Per-shard timing information
    pub shard_timings: HashMap<String, u64>,
}

/// Individual search result
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Record ID
    pub id: String,
    /// Distance/similarity score
    pub distance: f32,
    /// Shard this result came from
    pub shard_id: String,
    /// Metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Result of a distributed write operation
#[derive(Debug, Clone)]
pub struct DistributedWriteResult {
    /// Number of records written
    pub records_written: usize,
    /// Shards that received writes
    pub shards_written: Vec<String>,
    /// Replicas that acknowledged
    pub replicas_acknowledged: usize,
    /// Total write time in milliseconds
    pub total_time_ms: u64,
}

/// Query context for metadata-aware shard pruning
#[derive(Debug, Clone, Default)]
pub struct QueryContext {
    /// Tenant ID for tenant-based filtering/pruning
    pub tenant_id: Option<String>,
    /// Domain ID for domain-based filtering/pruning
    pub domain_id: Option<String>,
    /// Partition key for partition-based routing
    pub partition_key: Option<String>,
    /// Additional field filters for shard pruning (field -> value)
    pub field_filters: HashMap<String, serde_json::Value>,
}

impl QueryContext {
    /// Create a new empty query context
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a query context with tenant ID
    pub fn with_tenant(tenant_id: impl Into<String>) -> Self {
        Self {
            tenant_id: Some(tenant_id.into()),
            ..Default::default()
        }
    }

    /// Create a query context with domain ID
    pub fn with_domain(domain_id: impl Into<String>) -> Self {
        Self {
            domain_id: Some(domain_id.into()),
            ..Default::default()
        }
    }

    /// Add tenant ID to the context
    pub fn tenant(mut self, tenant_id: impl Into<String>) -> Self {
        self.tenant_id = Some(tenant_id.into());
        self
    }

    /// Add domain ID to the context
    pub fn domain(mut self, domain_id: impl Into<String>) -> Self {
        self.domain_id = Some(domain_id.into());
        self
    }

    /// Add partition key to the context
    pub fn partition(mut self, partition_key: impl Into<String>) -> Self {
        self.partition_key = Some(partition_key.into());
        self
    }

    /// Add a field filter for shard pruning
    pub fn with_field_filter(mut self, field: impl Into<String>, value: serde_json::Value) -> Self {
        self.field_filters.insert(field.into(), value);
        self
    }

    /// Check if context has any filtering criteria
    pub fn has_filters(&self) -> bool {
        self.tenant_id.is_some()
            || self.domain_id.is_some()
            || self.partition_key.is_some()
            || !self.field_filters.is_empty()
    }
}

/// Distributed search request
#[derive(Debug, Clone)]
pub struct DistributedSearchRequest {
    /// Collection name
    pub collection: String,
    /// Query vector
    pub vector: Vec<f32>,
    /// Number of results to return
    pub top_k: usize,
    /// Optional metadata filter
    pub filter: Option<serde_json::Value>,
    /// Optional routing key for targeted search
    pub routing_key: Option<String>,
    /// Include specific shards only
    pub include_shards: Option<Vec<String>>,
    /// Exclude specific shards
    pub exclude_shards: Option<Vec<String>>,
    /// Query context for metadata-aware shard pruning
    pub query_context: Option<QueryContext>,
}

/// Distributed write request
#[derive(Debug, Clone)]
pub struct DistributedWriteRequest {
    /// Collection name
    pub collection: String,
    /// Records to write
    pub records: Vec<WriteRecord>,
    /// Optional routing key
    pub routing_key: Option<String>,
    /// Optional tenant ID for tenant-aware shard routing and metadata bounds updates
    pub tenant_id: Option<String>,
    /// Optional domain ID for domain-aware shard routing and metadata bounds updates
    pub domain_id: Option<String>,
}

/// Record for distributed write
#[derive(Debug, Clone)]
pub struct WriteRecord {
    /// Record ID
    pub id: String,
    /// Vector data
    pub vector: Vec<f32>,
    /// Metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Distributed operations coordinator
pub struct DistributedCollectionOps {
    config: DistributedOpsConfig,
    shard_manager: Arc<ShardManager>,
    routing_service: Arc<RoutingService>,
    node_registry: Arc<NodeRegistry>,
    consensus: Arc<RwLock<RaftConsensus>>,
    /// Local node ID
    local_node_id: String,
    /// Statistics
    stats: Arc<RwLock<DistributedOpsStats>>,
    /// RPC fanout for distributed search and write operations
    fanout: Option<Arc<dyn SearchFanout>>,
}

/// Statistics for distributed operations
#[derive(Debug, Default)]
struct DistributedOpsStats {
    total_searches: u64,
    total_writes: u64,
    failed_searches: u64,
    failed_writes: u64,
    total_search_time_ms: u64,
    total_write_time_ms: u64,
}

impl DistributedCollectionOps {
    /// Create a new distributed operations coordinator
    pub fn new(
        config: DistributedOpsConfig,
        shard_manager: Arc<ShardManager>,
        routing_service: Arc<RoutingService>,
        node_registry: Arc<NodeRegistry>,
        consensus: Arc<RwLock<RaftConsensus>>,
        local_node_id: String,
    ) -> Self {
        Self {
            config,
            shard_manager,
            routing_service,
            node_registry,
            consensus,
            local_node_id,
            stats: Arc::new(RwLock::new(DistributedOpsStats::default())),
            fanout: None,
        }
    }

    /// Create a new distributed operations coordinator with RPC fanout support
    ///
    /// This constructor enables actual distributed search and write operations
    /// by providing a SearchFanout implementation for RPC calls to remote nodes.
    pub fn with_fanout(
        config: DistributedOpsConfig,
        shard_manager: Arc<ShardManager>,
        routing_service: Arc<RoutingService>,
        node_registry: Arc<NodeRegistry>,
        consensus: Arc<RwLock<RaftConsensus>>,
        local_node_id: String,
        fanout: Arc<dyn SearchFanout>,
    ) -> Self {
        Self {
            config,
            shard_manager,
            routing_service,
            node_registry,
            consensus,
            local_node_id,
            stats: Arc::new(RwLock::new(DistributedOpsStats::default())),
            fanout: Some(fanout),
        }
    }

    /// Set the fanout implementation for RPC operations
    ///
    /// This allows setting or updating the fanout after creation, useful for
    /// dependency injection patterns where the fanout might not be available
    /// at construction time.
    pub fn set_fanout(&mut self, fanout: Arc<dyn SearchFanout>) {
        self.fanout = Some(fanout);
    }

    /// Get a reference to the current fanout implementation, if any
    pub fn fanout(&self) -> Option<&Arc<dyn SearchFanout>> {
        self.fanout.as_ref()
    }

    /// Execute a distributed search across shards
    pub async fn distributed_search(
        &self,
        request: DistributedSearchRequest,
    ) -> Result<DistributedSearchResult> {
        let start = Instant::now();

        // Get shards for the collection
        let mut shards = self
            .shard_manager
            .get_collection_shards(&request.collection)
            .await;
        let total_shards = shards.len();

        // Filter shards if specified
        if let Some(ref include) = request.include_shards {
            shards.retain(|s| include.contains(&s.id.id().to_string()));
        }
        if let Some(ref exclude) = request.exclude_shards {
            shards.retain(|s| !exclude.contains(&s.id.id().to_string()));
        }

        // Filter to active shards only
        shards.retain(|s| s.state == ShardState::Active);

        // Apply metadata-aware shard pruning based on query context
        let pruned_shards = self.prune_shards_by_metadata(&shards, &request.query_context);
        let pruned_count = shards.len() - pruned_shards.len();

        if pruned_count > 0 {
            info!(
                collection = %request.collection,
                total_shards = total_shards,
                active_shards = shards.len(),
                pruned_shards = pruned_count,
                remaining_shards = pruned_shards.len(),
                tenant_id = ?request.query_context.as_ref().and_then(|c| c.tenant_id.as_ref()),
                domain_id = ?request.query_context.as_ref().and_then(|c| c.domain_id.as_ref()),
                "Shard pruning applied based on metadata bounds"
            );
        }

        let shards = pruned_shards;

        if shards.is_empty() {
            return Err(anyhow!(
                "No active shards found for collection '{}'",
                request.collection
            ));
        }

        let shards_queried = shards.len();
        let mut shard_timings = HashMap::new();

        // Execute search on each shard
        let results = if self.config.parallel_queries {
            self.parallel_shard_search(&shards, &request, &mut shard_timings)
                .await?
        } else {
            self.sequential_shard_search(&shards, &request, &mut shard_timings)
                .await?
        };

        // Merge and sort results
        let merged = self.merge_search_results(results, request.top_k);

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_searches += 1;
            stats.total_search_time_ms += start.elapsed().as_millis() as u64;
        }

        Ok(DistributedSearchResult {
            results: merged,
            shards_queried,
            shards_succeeded: shard_timings.len(),
            total_time_ms: start.elapsed().as_millis() as u64,
            shard_timings,
        })
    }

    /// Execute parallel search across shards
    async fn parallel_shard_search(
        &self,
        shards: &[Shard],
        request: &DistributedSearchRequest,
        timings: &mut HashMap<String, u64>,
    ) -> Result<Vec<Vec<SearchResult>>> {
        use futures::future::join_all;

        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.config.max_concurrent_ops));
        let request = Arc::new(request.clone());
        let fanout = self.fanout.clone();
        let timeout_ms = self.config.operation_timeout_ms;
        let node_registry = self.node_registry.clone();

        let futures: Vec<_> = shards
            .iter()
            .map(|shard| {
                let shard = shard.clone();
                let request = request.clone();
                let semaphore = semaphore.clone();
                let local_node = self.local_node_id.clone();
                let fanout = fanout.clone();
                let node_registry = node_registry.clone();

                async move {
                    let _permit = semaphore.acquire().await.unwrap();
                    let shard_start = Instant::now();

                    let result = Self::search_single_shard(
                        &shard,
                        &request,
                        &local_node,
                        fanout.as_deref(),
                        &node_registry,
                        timeout_ms,
                    )
                    .await;

                    let elapsed = shard_start.elapsed().as_millis() as u64;
                    (shard.id.id().to_string(), elapsed, result)
                }
            })
            .collect();

        let results = join_all(futures).await;

        let mut search_results = Vec::new();
        for (shard_id, elapsed, result) in results {
            timings.insert(shard_id.clone(), elapsed);
            match result {
                Ok(r) => search_results.push(r),
                Err(e) => {
                    warn!("Shard {} search failed: {}", shard_id, e);
                }
            }
        }

        Ok(search_results)
    }

    /// Execute sequential search across shards
    async fn sequential_shard_search(
        &self,
        shards: &[Shard],
        request: &DistributedSearchRequest,
        timings: &mut HashMap<String, u64>,
    ) -> Result<Vec<Vec<SearchResult>>> {
        let mut results = Vec::new();

        for shard in shards {
            let shard_start = Instant::now();

            match Self::search_single_shard(
                shard,
                request,
                &self.local_node_id,
                self.fanout.as_deref(),
                &self.node_registry,
                self.config.operation_timeout_ms,
            )
            .await
            {
                Ok(r) => {
                    results.push(r);
                    timings.insert(
                        shard.id.id().to_string(),
                        shard_start.elapsed().as_millis() as u64,
                    );
                }
                Err(e) => {
                    warn!("Shard {} search failed: {}", shard.id, e);
                    timings.insert(
                        shard.id.id().to_string(),
                        shard_start.elapsed().as_millis() as u64,
                    );
                }
            }
        }

        Ok(results)
    }

    /// Search a single shard
    ///
    /// If the shard is local, executes the search on the local engine.
    /// If the shard is remote, uses the SearchFanout RPC to forward the request.
    async fn search_single_shard(
        shard: &Shard,
        request: &DistributedSearchRequest,
        local_node: &str,
        fanout: Option<&dyn SearchFanout>,
        node_registry: &NodeRegistry,
        timeout_ms: u64,
    ) -> Result<Vec<SearchResult>> {
        // Check if shard is on local node
        let is_local =
            shard.primary_node() == Some(local_node) || shard.replica_nodes().contains(&local_node);

        if is_local {
            // Execute search locally
            // In a real implementation, this would call the local engine
            debug!("Executing local search on shard {}", shard.id);
            Ok(Vec::new()) // Placeholder - would call local engine
        } else {
            // Forward to remote node via RPC
            Self::search_remote_shard(shard, request, fanout, node_registry, timeout_ms).await
        }
    }

    /// Execute search on a remote shard via RPC
    ///
    /// This method requires node address resolution. If the address cannot be
    /// determined from the shard's primary node ID, an error is returned.
    async fn search_remote_shard(
        shard: &Shard,
        request: &DistributedSearchRequest,
        fanout: Option<&dyn SearchFanout>,
        node_registry: &NodeRegistry,
        timeout_ms: u64,
    ) -> Result<Vec<SearchResult>> {
        // Get the target node ID
        let target_node = shard
            .primary_node()
            .ok_or_else(|| anyhow!("No primary node for shard {}", shard.id))?;

        // Look up the node address from the node registry
        let node_info = node_registry.get_node(target_node).await.ok_or_else(|| {
            anyhow!(
                "Node {} not found in registry for shard {}",
                target_node,
                shard.id
            )
        })?;

        // Check if we have a fanout implementation
        let fanout = fanout.ok_or_else(|| {
            anyhow!(
                "No SearchFanout implementation available for remote search to shard {}",
                shard.id
            )
        })?;

        // Create the endpoint for the target node
        let endpoint = NodeEndpoint::new(target_node, &node_info.address);

        // Build the shard search request
        let rpc_request = ShardSearchRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            collection: request.collection.clone(),
            shard_id: shard.id.id().to_string(),
            vector: request.vector.clone(),
            top_k: request.top_k as u32,
            filter: request.filter.as_ref().map(|f| f.to_string()),
            params: SearchParams::default(),
            timeout: std::time::Duration::from_millis(timeout_ms),
            include_vectors: false,
            tenant_id: request
                .query_context
                .as_ref()
                .and_then(|c| c.tenant_id.clone()),
            domain_id: request
                .query_context
                .as_ref()
                .and_then(|c| c.domain_id.clone()),
        };

        debug!(
            "Forwarding search to node {} ({}) for shard {}",
            target_node, node_info.address, shard.id
        );

        // Execute the RPC call
        let response = fanout
            .shard_search(&endpoint, rpc_request)
            .await
            .map_err(|e| anyhow!("RPC search failed for shard {}: {}", shard.id, e))?;

        // Log the response metrics
        let result_count = response.results.len();
        let vectors_scanned = response.vectors_scanned;
        let latency = response.latency;

        // Convert RPC results to SearchResult
        let results: Vec<SearchResult> = response
            .results
            .into_iter()
            .map(|r| SearchResult {
                id: r.id,
                distance: r.score,
                shard_id: shard.id.id().to_string(),
                metadata: r
                    .metadata
                    .as_ref()
                    .and_then(|m| serde_json::from_str(m).ok())
                    .unwrap_or_default(),
            })
            .collect();

        debug!(
            "Received {} results from shard {} (scanned {} vectors in {:?})",
            result_count, shard.id, vectors_scanned, latency
        );

        Ok(results)
    }

    /// Merge search results from multiple shards
    fn merge_search_results(
        &self,
        shard_results: Vec<Vec<SearchResult>>,
        top_k: usize,
    ) -> Vec<SearchResult> {
        let mut all_results: Vec<SearchResult> = shard_results.into_iter().flatten().collect();

        // Sort by distance (ascending - lower is better)
        all_results.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap());

        // Take top_k
        all_results.truncate(top_k);

        all_results
    }

    /// Prune shards based on metadata bounds from query context
    ///
    /// This method filters out shards that cannot contain data matching
    /// the query's tenant_id, domain_id, partition_key, or field filters.
    /// This optimization reduces the fan-out overhead for queries with
    /// known metadata constraints.
    fn prune_shards_by_metadata(
        &self,
        shards: &[Shard],
        query_context: &Option<QueryContext>,
    ) -> Vec<Shard> {
        let context = match query_context {
            Some(ctx) if ctx.has_filters() => ctx,
            _ => {
                // No pruning context provided, return all shards
                debug!("No query context provided, skipping shard pruning");
                return shards.to_vec();
            }
        };

        let mut retained_shards = Vec::with_capacity(shards.len());
        let mut pruned_shard_ids: Vec<String> = Vec::new();

        for shard in shards {
            let should_include = self.shard_matches_context(shard, context);

            if should_include {
                retained_shards.push(shard.clone());
            } else {
                pruned_shard_ids.push(shard.id.id().to_string());
            }
        }

        // Log pruned shards for debugging/monitoring
        if !pruned_shard_ids.is_empty() {
            debug!(
                pruned_shards = ?pruned_shard_ids,
                tenant_id = ?context.tenant_id,
                domain_id = ?context.domain_id,
                partition_key = ?context.partition_key,
                "Shards pruned based on metadata bounds"
            );
        }

        retained_shards
    }

    /// Check if a shard might contain data matching the query context
    fn shard_matches_context(&self, shard: &Shard, context: &QueryContext) -> bool {
        // Use the Shard's may_contain_data method for tenant/domain checks
        if !shard.may_contain_data(context.tenant_id.as_deref(), context.domain_id.as_deref()) {
            return false;
        }

        // Check partition key if provided
        if let Some(ref partition_key) = context.partition_key {
            if let Some(ref bounds) = shard.metadata_bounds {
                if !bounds.may_contain_partition(partition_key) {
                    debug!(
                        shard_id = %shard.id,
                        partition_key = %partition_key,
                        "Shard pruned: partition key not in bounds"
                    );
                    return false;
                }
            }
        }

        // Check additional field filters
        if !context.field_filters.is_empty() {
            if let Some(ref bounds) = shard.metadata_bounds {
                for (field, value) in &context.field_filters {
                    if !bounds.may_contain_field_value(field, value) {
                        debug!(
                            shard_id = %shard.id,
                            field = %field,
                            value = ?value,
                            "Shard pruned: field value not in bounds"
                        );
                        return false;
                    }
                }
            }
        }

        true
    }

    /// Execute a distributed write operation
    ///
    /// If tenant_id or domain_id are provided, they will be:
    /// 1. Used for tenant-aware shard routing (if partition strategy is Tenant/Domain)
    /// 2. Injected into record metadata for metadata bounds tracking
    pub async fn distributed_write(
        &self,
        request: DistributedWriteRequest,
    ) -> Result<DistributedWriteResult> {
        let start = Instant::now();

        // Get shards for the collection
        let shards = self
            .shard_manager
            .get_collection_shards(&request.collection)
            .await;

        if shards.is_empty() {
            return Err(anyhow!(
                "No shards found for collection '{}'",
                request.collection
            ));
        }

        // Log tenant context for distributed writes
        if request.tenant_id.is_some() || request.domain_id.is_some() {
            debug!(
                collection = %request.collection,
                tenant_id = ?request.tenant_id,
                domain_id = ?request.domain_id,
                record_count = request.records.len(),
                "Executing tenant-aware distributed write"
            );
        }

        // Partition records by shard using tenant-aware routing
        let partitioned = self.partition_records_by_shard_with_context(
            &request.records,
            &shards,
            request.tenant_id.as_deref(),
            request.domain_id.as_deref(),
        );

        let mut shards_written = Vec::new();
        let mut total_replicas_acked = 0;
        let mut records_written = 0;

        for (shard_id, shard_records) in partitioned {
            let shard = shards
                .iter()
                .find(|s| s.id == shard_id)
                .ok_or_else(|| anyhow!("Shard {} not found", shard_id))?;

            // Write to primary and replicas based on consistency level
            let replicas_acked = self
                .write_to_shard_with_consistency(
                    shard,
                    &shard_records,
                    self.config.write_consistency,
                )
                .await?;

            // Update shard metadata bounds after successful write
            let records_metadata: Vec<HashMap<String, serde_json::Value>> =
                shard_records.iter().map(|r| r.metadata.clone()).collect();

            if let Err(e) = self
                .shard_manager
                .update_shard_metadata_bounds(&shard_id, &records_metadata)
                .await
            {
                warn!(
                    shard_id = %shard_id,
                    error = %e,
                    "Failed to update shard metadata bounds"
                );
            }

            shards_written.push(shard_id.id().to_string());
            total_replicas_acked += replicas_acked;
            records_written += shard_records.len();
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_writes += 1;
            stats.total_write_time_ms += start.elapsed().as_millis() as u64;
        }

        Ok(DistributedWriteResult {
            records_written,
            shards_written,
            replicas_acknowledged: total_replicas_acked,
            total_time_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Partition records by shard using consistent hashing
    fn partition_records_by_shard(
        &self,
        records: &[WriteRecord],
        shards: &[Shard],
    ) -> HashMap<ShardId, Vec<WriteRecord>> {
        self.partition_records_by_shard_with_context(records, shards, None, None)
    }

    /// Partition records by shard with optional tenant/domain context
    ///
    /// This method supports multiple partitioning strategies:
    /// 1. Tenant-based: Routes all records for a tenant to shards that already contain that tenant's data
    /// 2. Domain-based: Routes all records for a domain to shards that already contain that domain's data
    /// 3. Hash-based (default): Uses consistent hashing on record ID
    ///
    /// When tenant_id or domain_id are provided, records are enriched with this metadata
    /// for metadata bounds tracking in the storage layer.
    fn partition_records_by_shard_with_context(
        &self,
        records: &[WriteRecord],
        shards: &[Shard],
        tenant_id: Option<&str>,
        domain_id: Option<&str>,
    ) -> HashMap<ShardId, Vec<WriteRecord>> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut partitioned: HashMap<ShardId, Vec<WriteRecord>> = HashMap::new();

        // Check if we should use tenant-based or domain-based routing
        let use_tenant_routing = tenant_id.is_some() && self.should_use_tenant_routing(shards);
        let use_domain_routing = domain_id.is_some() && self.should_use_domain_routing(shards);

        for record in records {
            // Enrich record metadata with tenant/domain context
            let mut enriched_record = record.clone();
            if let Some(tid) = tenant_id {
                enriched_record.metadata.insert(
                    "tenant_id".to_string(),
                    serde_json::Value::String(tid.to_string()),
                );
            }
            if let Some(did) = domain_id {
                enriched_record.metadata.insert(
                    "domain_id".to_string(),
                    serde_json::Value::String(did.to_string()),
                );
            }

            // Determine target shard based on routing strategy
            let shard_id = if use_tenant_routing {
                self.route_by_tenant(tenant_id.unwrap(), shards, &record.id)
            } else if use_domain_routing {
                self.route_by_domain(domain_id.unwrap(), shards, &record.id)
            } else {
                // Default: hash-based routing on record ID
                let mut hasher = DefaultHasher::new();
                record.id.hash(&mut hasher);
                let hash = hasher.finish();
                let shard_idx = (hash as usize) % shards.len();
                shards[shard_idx].id.clone()
            };

            partitioned
                .entry(shard_id)
                .or_default()
                .push(enriched_record);
        }

        partitioned
    }

    /// Check if tenant-based routing should be used based on shard partition config
    fn should_use_tenant_routing(&self, shards: &[Shard]) -> bool {
        shards.first().map_or(false, |s| {
            matches!(
                s.partition_config.as_ref().map(|c| &c.strategy),
                Some(super::shard::PartitionStrategy::Tenant)
                    | Some(super::shard::PartitionStrategy::TenantHash { .. })
            )
        })
    }

    /// Check if domain-based routing should be used based on shard partition config
    fn should_use_domain_routing(&self, shards: &[Shard]) -> bool {
        shards.first().map_or(false, |s| {
            matches!(
                s.partition_config.as_ref().map(|c| &c.strategy),
                Some(super::shard::PartitionStrategy::Domain)
            )
        })
    }

    /// Route a record to a shard based on tenant ID
    ///
    /// This method first tries to find shards that already contain data for this tenant,
    /// then falls back to hash-based routing if no existing shards are found.
    fn route_by_tenant(&self, tenant_id: &str, shards: &[Shard], record_id: &str) -> ShardId {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        // Find shards that already contain this tenant's data
        let tenant_shards: Vec<&Shard> = shards
            .iter()
            .filter(|s| {
                s.metadata_bounds
                    .as_ref()
                    .map(|b| b.tenant_ids.contains(tenant_id))
                    .unwrap_or(false)
            })
            .collect();

        if !tenant_shards.is_empty() {
            // Route to existing tenant shard using hash of record ID for distribution
            let mut hasher = DefaultHasher::new();
            record_id.hash(&mut hasher);
            let hash = hasher.finish();
            let idx = (hash as usize) % tenant_shards.len();
            tenant_shards[idx].id.clone()
        } else {
            // No existing shard for this tenant - use hash of tenant_id for consistency
            let mut hasher = DefaultHasher::new();
            tenant_id.hash(&mut hasher);
            let hash = hasher.finish();
            let idx = (hash as usize) % shards.len();
            shards[idx].id.clone()
        }
    }

    /// Route a record to a shard based on domain ID
    fn route_by_domain(&self, domain_id: &str, shards: &[Shard], record_id: &str) -> ShardId {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        // Find shards that already contain this domain's data
        let domain_shards: Vec<&Shard> = shards
            .iter()
            .filter(|s| {
                s.metadata_bounds
                    .as_ref()
                    .map(|b| b.domain_ids.contains(domain_id))
                    .unwrap_or(false)
            })
            .collect();

        if !domain_shards.is_empty() {
            // Route to existing domain shard using hash of record ID for distribution
            let mut hasher = DefaultHasher::new();
            record_id.hash(&mut hasher);
            let hash = hasher.finish();
            let idx = (hash as usize) % domain_shards.len();
            domain_shards[idx].id.clone()
        } else {
            // No existing shard for this domain - use hash of domain_id for consistency
            let mut hasher = DefaultHasher::new();
            domain_id.hash(&mut hasher);
            let hash = hasher.finish();
            let idx = (hash as usize) % shards.len();
            shards[idx].id.clone()
        }
    }

    /// Write to a shard with specified consistency level
    ///
    /// This method handles writing to both local and remote shards.
    /// For remote writes, it uses the SearchFanout's forward_write RPC.
    async fn write_to_shard_with_consistency(
        &self,
        shard: &Shard,
        records: &[WriteRecord],
        consistency: ConsistencyLevel,
    ) -> Result<usize> {
        let total_replicas = shard.placements.len();
        let required_acks = match consistency {
            ConsistencyLevel::One => 1,
            ConsistencyLevel::Quorum => (total_replicas / 2) + 1,
            ConsistencyLevel::All => total_replicas,
            ConsistencyLevel::LocalQuorum => (total_replicas / 2) + 1, // Simplified
        };

        // Write to primary first
        let primary_node = shard
            .primary_node()
            .ok_or_else(|| anyhow!("No primary for shard {}", shard.id))?;

        let is_local_primary = primary_node == self.local_node_id;

        let primary_acks = if is_local_primary {
            debug!(
                "Writing {} records to local primary shard {}",
                records.len(),
                shard.id
            );
            // In a real implementation, write to local engine
            1
        } else {
            // Forward write to remote primary via RPC
            self.forward_write_to_node(shard, &primary_node, records, consistency)
                .await?
        };

        // If we got enough acks from the primary (or it handled replication),
        // we may already have satisfied the consistency requirement
        if primary_acks >= required_acks {
            return Ok(primary_acks);
        }

        // Replicate to additional replicas if needed
        let mut acks = primary_acks;

        for replica_node in shard.replica_nodes() {
            if replica_node == self.local_node_id {
                debug!("Writing to local replica shard {}", shard.id);
                // Write locally
                acks += 1;
            } else if replica_node != primary_node {
                // Forward to remote replica
                debug!(
                    "Replicating to replica {} for shard {}",
                    replica_node, shard.id
                );
                match self
                    .forward_write_to_node(shard, &replica_node, records, ConsistencyLevel::One)
                    .await
                {
                    Ok(replica_acks) => {
                        acks += replica_acks;
                    }
                    Err(e) => {
                        warn!(
                            "Failed to replicate to replica {} for shard {}: {}",
                            replica_node, shard.id, e
                        );
                        // Continue trying other replicas
                    }
                }
            }

            if acks >= required_acks {
                break; // We have enough acks
            }
        }

        if acks < required_acks {
            return Err(anyhow!(
                "Insufficient replicas acknowledged: {} of {} required",
                acks,
                required_acks
            ));
        }

        Ok(acks)
    }

    /// Forward write to a remote node via RPC
    async fn forward_write_to_node(
        &self,
        shard: &Shard,
        target_node: &str,
        records: &[WriteRecord],
        consistency: ConsistencyLevel,
    ) -> Result<usize> {
        // Look up the node address from the node registry
        let node_info = self
            .node_registry
            .get_node(target_node)
            .await
            .ok_or_else(|| {
                anyhow!(
                    "Node {} not found in registry for shard {}",
                    target_node,
                    shard.id
                )
            })?;

        // Check if we have a fanout implementation
        let fanout = self.fanout.as_ref().ok_or_else(|| {
            anyhow!(
                "No SearchFanout implementation available for remote write to shard {}",
                shard.id
            )
        })?;

        // Create the endpoint for the target node
        let endpoint = NodeEndpoint::new(target_node, &node_info.address);

        // Convert WriteRecord to RPC WriteRecord
        let rpc_records: Vec<RpcWriteRecord> = records
            .iter()
            .map(|r| RpcWriteRecord {
                id: r.id.clone(),
                vector: r.vector.clone(),
                metadata: r.metadata.clone(),
            })
            .collect();

        // Build the forward write request
        let rpc_request = ForwardWriteRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            collection: shard.collection_id.clone(),
            shard_id: shard.id.id().to_string(),
            records: rpc_records,
            consistency: Self::convert_consistency_level(consistency),
            timeout: std::time::Duration::from_millis(self.config.operation_timeout_ms),
            tenant_id: None,
            domain_id: None,
        };

        debug!(
            "Forwarding {} records to node {} ({}) for shard {}",
            records.len(),
            target_node,
            node_info.address,
            shard.id
        );

        // Execute the RPC call
        let response = fanout
            .forward_write(&endpoint, rpc_request)
            .await
            .map_err(|e| anyhow!("RPC write failed for shard {}: {}", shard.id, e))?;

        // Check for errors
        if let Some(error) = response.error {
            return Err(anyhow!(
                "Remote write to shard {} failed: {}",
                shard.id,
                error
            ));
        }

        debug!(
            "Wrote {} records to shard {} on node {}, {} replicas acknowledged in {:?}",
            response.records_written,
            shard.id,
            target_node,
            response.replicas_acked,
            response.latency
        );

        Ok(response.replicas_acked as usize)
    }

    /// Convert local ConsistencyLevel to RPC ConsistencyLevel
    fn convert_consistency_level(level: ConsistencyLevel) -> super::rpc::ConsistencyLevel {
        match level {
            ConsistencyLevel::One => super::rpc::ConsistencyLevel::One,
            ConsistencyLevel::Quorum => super::rpc::ConsistencyLevel::Quorum,
            ConsistencyLevel::All => super::rpc::ConsistencyLevel::All,
            ConsistencyLevel::LocalQuorum => super::rpc::ConsistencyLevel::LocalQuorum,
        }
    }

    /// Get statistics for distributed operations
    pub async fn get_stats(&self) -> DistributedOpsStatsSummary {
        let stats = self.stats.read().await;
        DistributedOpsStatsSummary {
            total_searches: stats.total_searches,
            total_writes: stats.total_writes,
            failed_searches: stats.failed_searches,
            failed_writes: stats.failed_writes,
            avg_search_time_ms: if stats.total_searches > 0 {
                stats.total_search_time_ms / stats.total_searches
            } else {
                0
            },
            avg_write_time_ms: if stats.total_writes > 0 {
                stats.total_write_time_ms / stats.total_writes
            } else {
                0
            },
        }
    }

    /// Rebalance shards for a collection
    pub async fn rebalance_collection(&self, collection: &str) -> Result<RebalanceResult> {
        info!("Starting rebalance for collection '{}'", collection);

        let shards = self.shard_manager.get_collection_shards(collection).await;
        let distribution = self.shard_manager.get_distribution_stats().await;

        // Check if rebalance is needed
        if distribution.imbalance_ratio < 0.2 {
            return Ok(RebalanceResult {
                shards_moved: 0,
                success: true,
                message: "Cluster is already balanced".to_string(),
            });
        }

        // In a real implementation:
        // 1. Identify overloaded and underloaded nodes
        // 2. Plan shard movements
        // 3. Execute moves (copy data, update routing, delete old)

        Ok(RebalanceResult {
            shards_moved: 0,
            success: true,
            message: format!("Rebalance planned for {} shards", shards.len()),
        })
    }
}

/// Summary of distributed operations statistics
#[derive(Debug, Clone)]
pub struct DistributedOpsStatsSummary {
    pub total_searches: u64,
    pub total_writes: u64,
    pub failed_searches: u64,
    pub failed_writes: u64,
    pub avg_search_time_ms: u64,
    pub avg_write_time_ms: u64,
}

/// Result of a rebalance operation
#[derive(Debug, Clone)]
pub struct RebalanceResult {
    pub shards_moved: usize,
    pub success: bool,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::rpc::{
        ForwardWriteResponse, RpcResult, ShardSearchResponse, ShardSearchResult as RpcSearchResult,
    };
    use crate::cluster::{ConsensusConfig, NodeRegistryConfig, RoutingConfig, ShardConfig};
    use async_trait::async_trait;
    use futures::Stream;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn create_test_coordinator() -> DistributedCollectionOps {
        let shard_manager = Arc::new(ShardManager::new(ShardConfig::default()).unwrap());
        let routing_service = Arc::new(RoutingService::new(RoutingConfig::default()).unwrap());
        let node_registry = Arc::new(NodeRegistry::new(NodeRegistryConfig::default()).unwrap());
        let consensus = Arc::new(RwLock::new(
            RaftConsensus::new(ConsensusConfig::default()).unwrap(),
        ));

        DistributedCollectionOps::new(
            DistributedOpsConfig::default(),
            shard_manager,
            routing_service,
            node_registry,
            consensus,
            "local-node-1".to_string(),
        )
    }

    #[tokio::test]
    async fn test_coordinator_creation() {
        let coordinator = create_test_coordinator().await;
        let stats = coordinator.get_stats().await;
        assert_eq!(stats.total_searches, 0);
        assert_eq!(stats.total_writes, 0);
    }

    #[tokio::test]
    async fn test_partition_records_by_shard() {
        let coordinator = create_test_coordinator().await;

        let records = vec![
            WriteRecord {
                id: "rec1".to_string(),
                vector: vec![1.0, 2.0, 3.0],
                metadata: HashMap::new(),
            },
            WriteRecord {
                id: "rec2".to_string(),
                vector: vec![4.0, 5.0, 6.0],
                metadata: HashMap::new(),
            },
        ];

        let shards = vec![
            Shard::new("test-collection", 0),
            Shard::new("test-collection", 1),
        ];

        let partitioned = coordinator.partition_records_by_shard(&records, &shards);

        // Records should be distributed across shards
        let total_records: usize = partitioned.values().map(|v| v.len()).sum();
        assert_eq!(total_records, 2);
    }

    #[tokio::test]
    async fn test_merge_search_results() {
        let coordinator = create_test_coordinator().await;

        let shard_results = vec![
            vec![
                SearchResult {
                    id: "r1".to_string(),
                    distance: 0.5,
                    shard_id: "shard1".to_string(),
                    metadata: HashMap::new(),
                },
                SearchResult {
                    id: "r2".to_string(),
                    distance: 1.0,
                    shard_id: "shard1".to_string(),
                    metadata: HashMap::new(),
                },
            ],
            vec![SearchResult {
                id: "r3".to_string(),
                distance: 0.3,
                shard_id: "shard2".to_string(),
                metadata: HashMap::new(),
            }],
        ];

        let merged = coordinator.merge_search_results(shard_results, 2);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].id, "r3"); // Lowest distance first
        assert_eq!(merged[1].id, "r1");
    }

    #[tokio::test]
    async fn test_consistency_levels() {
        assert_eq!(ConsistencyLevel::One as usize, 0);
        assert_eq!(ConsistencyLevel::Quorum as usize, 1);
        assert_eq!(ConsistencyLevel::All as usize, 2);
    }

    #[test]
    fn test_query_context_builder() {
        // Test empty context
        let ctx = QueryContext::new();
        assert!(!ctx.has_filters());
        assert!(ctx.tenant_id.is_none());
        assert!(ctx.domain_id.is_none());

        // Test with tenant
        let ctx = QueryContext::with_tenant("tenant-1");
        assert!(ctx.has_filters());
        assert_eq!(ctx.tenant_id, Some("tenant-1".to_string()));

        // Test with domain
        let ctx = QueryContext::with_domain("domain-1");
        assert!(ctx.has_filters());
        assert_eq!(ctx.domain_id, Some("domain-1".to_string()));

        // Test builder pattern
        let ctx = QueryContext::new()
            .tenant("tenant-2")
            .domain("domain-2")
            .partition("partition-1")
            .with_field_filter("category", serde_json::json!("electronics"));

        assert!(ctx.has_filters());
        assert_eq!(ctx.tenant_id, Some("tenant-2".to_string()));
        assert_eq!(ctx.domain_id, Some("domain-2".to_string()));
        assert_eq!(ctx.partition_key, Some("partition-1".to_string()));
        assert_eq!(
            ctx.field_filters.get("category"),
            Some(&serde_json::json!("electronics"))
        );
    }

    #[tokio::test]
    async fn test_shard_pruning_with_tenant_context() {
        let coordinator = create_test_coordinator().await;

        // Create shards with metadata bounds
        let mut shard1 = Shard::new("test-collection", 0);
        shard1.enable_metadata_bounds();
        if let Some(ref mut bounds) = shard1.metadata_bounds {
            bounds.tenant_ids.insert("tenant-1".to_string());
            bounds.tenant_ids.insert("tenant-2".to_string());
        }
        shard1.state = ShardState::Active;

        let mut shard2 = Shard::new("test-collection", 1);
        shard2.enable_metadata_bounds();
        if let Some(ref mut bounds) = shard2.metadata_bounds {
            bounds.tenant_ids.insert("tenant-3".to_string());
        }
        shard2.state = ShardState::Active;

        let shards = vec![shard1, shard2];

        // Test: Query for tenant-1 should only include shard1
        let ctx = QueryContext::with_tenant("tenant-1");
        let pruned = coordinator.prune_shards_by_metadata(&shards, &Some(ctx));
        assert_eq!(pruned.len(), 1);
        assert_eq!(pruned[0].id.id(), "test-collection_0000");

        // Test: Query for tenant-3 should only include shard2
        let ctx = QueryContext::with_tenant("tenant-3");
        let pruned = coordinator.prune_shards_by_metadata(&shards, &Some(ctx));
        assert_eq!(pruned.len(), 1);
        assert_eq!(pruned[0].id.id(), "test-collection_0001");

        // Test: No context should return all shards
        let pruned = coordinator.prune_shards_by_metadata(&shards, &None);
        assert_eq!(pruned.len(), 2);
    }

    #[tokio::test]
    async fn test_partition_records_with_tenant_context() {
        let coordinator = create_test_coordinator().await;

        let records = vec![
            WriteRecord {
                id: "rec1".to_string(),
                vector: vec![1.0, 2.0, 3.0],
                metadata: HashMap::new(),
            },
            WriteRecord {
                id: "rec2".to_string(),
                vector: vec![4.0, 5.0, 6.0],
                metadata: HashMap::new(),
            },
        ];

        let shards = vec![
            Shard::new("test-collection", 0),
            Shard::new("test-collection", 1),
        ];

        // Partition with tenant context
        let partitioned = coordinator.partition_records_by_shard_with_context(
            &records,
            &shards,
            Some("tenant-1"),
            Some("domain-1"),
        );

        // Records should be distributed and enriched with tenant/domain metadata
        let total_records: usize = partitioned.values().map(|v| v.len()).sum();
        assert_eq!(total_records, 2);

        // Check that records are enriched with tenant/domain metadata
        for (_, shard_records) in &partitioned {
            for record in shard_records {
                assert_eq!(
                    record.metadata.get("tenant_id"),
                    Some(&serde_json::json!("tenant-1"))
                );
                assert_eq!(
                    record.metadata.get("domain_id"),
                    Some(&serde_json::json!("domain-1"))
                );
            }
        }
    }

    #[tokio::test]
    async fn test_distributed_write_request_with_tenant() {
        // Test that DistributedWriteRequest can be created with tenant context
        let request = DistributedWriteRequest {
            collection: "test-collection".to_string(),
            records: vec![WriteRecord {
                id: "rec1".to_string(),
                vector: vec![1.0, 2.0, 3.0],
                metadata: HashMap::new(),
            }],
            routing_key: None,
            tenant_id: Some("tenant-1".to_string()),
            domain_id: Some("domain-1".to_string()),
        };

        assert_eq!(request.tenant_id, Some("tenant-1".to_string()));
        assert_eq!(request.domain_id, Some("domain-1".to_string()));
    }

    #[tokio::test]
    async fn test_distributed_search_request_with_query_context() {
        // Test that DistributedSearchRequest can be created with query context
        let request = DistributedSearchRequest {
            collection: "test-collection".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            top_k: 10,
            filter: None,
            routing_key: None,
            include_shards: None,
            exclude_shards: None,
            query_context: Some(QueryContext::with_tenant("tenant-1").domain("domain-1")),
        };

        let ctx = request.query_context.as_ref().unwrap();
        assert_eq!(ctx.tenant_id, Some("tenant-1".to_string()));
        assert_eq!(ctx.domain_id, Some("domain-1".to_string()));
    }

    // =========================================================================
    // Mock SearchFanout for testing RPC integration
    // =========================================================================

    /// Mock implementation of SearchFanout for testing
    struct MockSearchFanout {
        search_call_count: AtomicUsize,
        write_call_count: AtomicUsize,
        /// Simulated search results to return
        search_results: Vec<RpcSearchResult>,
        /// Whether to simulate a failure
        should_fail: bool,
    }

    impl MockSearchFanout {
        fn new() -> Self {
            Self {
                search_call_count: AtomicUsize::new(0),
                write_call_count: AtomicUsize::new(0),
                search_results: vec![
                    RpcSearchResult {
                        id: "remote-result-1".to_string(),
                        score: 0.1,
                        vector: None,
                        metadata: Some(r#"{"key": "value1"}"#.to_string()),
                    },
                    RpcSearchResult {
                        id: "remote-result-2".to_string(),
                        score: 0.2,
                        vector: None,
                        metadata: Some(r#"{"key": "value2"}"#.to_string()),
                    },
                ],
                should_fail: false,
            }
        }

        fn with_failure() -> Self {
            Self {
                should_fail: true,
                ..Self::new()
            }
        }

        fn search_calls(&self) -> usize {
            self.search_call_count.load(Ordering::SeqCst)
        }

        fn write_calls(&self) -> usize {
            self.write_call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl SearchFanout for MockSearchFanout {
        async fn shard_search(
            &self,
            _target: &NodeEndpoint,
            req: ShardSearchRequest,
        ) -> RpcResult<ShardSearchResponse> {
            self.search_call_count.fetch_add(1, Ordering::SeqCst);

            if self.should_fail {
                return Err(crate::cluster::rpc::RpcError::connection(
                    "Simulated failure",
                ));
            }

            Ok(ShardSearchResponse {
                request_id: req.request_id,
                shard_id: req.shard_id,
                results: self.search_results.clone(),
                vectors_scanned: 1000,
                latency: std::time::Duration::from_millis(5),
                truncated: false,
            })
        }

        async fn shard_search_stream(
            &self,
            _target: &NodeEndpoint,
            _req: ShardSearchRequest,
        ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<RpcSearchResult>> + Send>>> {
            unimplemented!("Streaming not needed for tests")
        }

        async fn forward_write(
            &self,
            _target: &NodeEndpoint,
            req: ForwardWriteRequest,
        ) -> RpcResult<ForwardWriteResponse> {
            self.write_call_count.fetch_add(1, Ordering::SeqCst);

            if self.should_fail {
                return Err(crate::cluster::rpc::RpcError::connection(
                    "Simulated failure",
                ));
            }

            Ok(ForwardWriteResponse {
                request_id: req.request_id,
                records_written: req.records.len() as u32,
                replicas_acked: 3,
                latency: std::time::Duration::from_millis(10),
                error: None,
            })
        }

        async fn forward_write_batch(
            &self,
            _target: &NodeEndpoint,
            requests: Vec<ForwardWriteRequest>,
        ) -> RpcResult<Vec<ForwardWriteResponse>> {
            Ok(requests
                .into_iter()
                .map(|req| ForwardWriteResponse {
                    request_id: req.request_id,
                    records_written: req.records.len() as u32,
                    replicas_acked: 3,
                    latency: std::time::Duration::from_millis(10),
                    error: None,
                })
                .collect())
        }
    }

    async fn create_test_coordinator_with_fanout(
        fanout: Arc<dyn SearchFanout>,
    ) -> DistributedCollectionOps {
        let shard_manager = Arc::new(ShardManager::new(ShardConfig::default()).unwrap());
        let routing_service = Arc::new(RoutingService::new(RoutingConfig::default()).unwrap());
        let node_registry = Arc::new(NodeRegistry::new(NodeRegistryConfig::default()).unwrap());
        let consensus = Arc::new(RwLock::new(
            RaftConsensus::new(ConsensusConfig::default()).unwrap(),
        ));

        DistributedCollectionOps::with_fanout(
            DistributedOpsConfig::default(),
            shard_manager,
            routing_service,
            node_registry,
            consensus,
            "local-node-1".to_string(),
            fanout,
        )
    }

    #[tokio::test]
    async fn test_coordinator_with_fanout_creation() {
        let fanout = Arc::new(MockSearchFanout::new());
        let coordinator = create_test_coordinator_with_fanout(fanout.clone()).await;

        // Verify fanout is set
        assert!(coordinator.fanout().is_some());

        let stats = coordinator.get_stats().await;
        assert_eq!(stats.total_searches, 0);
        assert_eq!(stats.total_writes, 0);
    }

    #[tokio::test]
    async fn test_set_fanout() {
        let mut coordinator = create_test_coordinator().await;

        // Initially no fanout
        assert!(coordinator.fanout().is_none());

        // Set fanout
        let fanout = Arc::new(MockSearchFanout::new());
        coordinator.set_fanout(fanout);

        // Now has fanout
        assert!(coordinator.fanout().is_some());
    }

    /// Helper to create a node registry with test nodes pre-registered
    async fn create_node_registry_with_nodes() -> Arc<NodeRegistry> {
        let registry = Arc::new(NodeRegistry::new(NodeRegistryConfig::default()).unwrap());

        // Register a remote test node
        registry
            .register_node(super::super::node_registry::NodeInfo {
                node_id: "remote-node-1".to_string(),
                address: "192.168.1.100:5679".to_string(),
                ..Default::default()
            })
            .await
            .unwrap();

        // Register local test node
        registry
            .register_node(super::super::node_registry::NodeInfo {
                node_id: "local-node-1".to_string(),
                address: "127.0.0.1:5679".to_string(),
                ..Default::default()
            })
            .await
            .unwrap();

        registry
    }

    #[tokio::test]
    async fn test_remote_shard_search_uses_rpc() {
        let fanout = Arc::new(MockSearchFanout::new());
        let node_registry = create_node_registry_with_nodes().await;

        // Create a shard that is on a remote node
        let mut shard = Shard::new("test-collection", 0);
        shard.state = ShardState::Active;
        // Add a placement with a remote node
        shard.placements.push(super::super::shard::ShardPlacement {
            node_id: "remote-node-1".to_string(),
            is_primary: true,
            priority: 0,
            lag_ms: None,
        });

        // Create a search request
        let request = DistributedSearchRequest {
            collection: "test-collection".to_string(),
            vector: vec![0.1, 0.2, 0.3],
            top_k: 10,
            filter: None,
            routing_key: None,
            include_shards: None,
            exclude_shards: None,
            query_context: None,
        };

        // Execute remote search
        let result = DistributedCollectionOps::search_remote_shard(
            &shard,
            &request,
            Some(fanout.as_ref()),
            &node_registry,
            30000,
        )
        .await;

        // Verify RPC was called
        assert!(result.is_ok());
        assert_eq!(fanout.search_calls(), 1);

        // Verify results were converted correctly
        let results = result.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "remote-result-1");
        assert_eq!(results[0].distance, 0.1);
    }

    #[tokio::test]
    async fn test_remote_shard_search_without_fanout_fails() {
        let node_registry = create_node_registry_with_nodes().await;

        // Create a shard that is on a remote node
        let mut shard = Shard::new("test-collection", 0);
        shard.state = ShardState::Active;
        shard.placements.push(super::super::shard::ShardPlacement {
            node_id: "remote-node-1".to_string(),
            is_primary: true,
            priority: 0,
            lag_ms: None,
        });

        let request = DistributedSearchRequest {
            collection: "test-collection".to_string(),
            vector: vec![0.1, 0.2, 0.3],
            top_k: 10,
            filter: None,
            routing_key: None,
            include_shards: None,
            exclude_shards: None,
            query_context: None,
        };

        // Execute remote search without fanout
        let result = DistributedCollectionOps::search_remote_shard(
            &shard,
            &request,
            None,
            &node_registry,
            30000,
        )
        .await;

        // Should fail because no fanout
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("No SearchFanout implementation"));
    }

    #[tokio::test]
    async fn test_remote_shard_search_with_rpc_failure() {
        let fanout = Arc::new(MockSearchFanout::with_failure());
        let node_registry = create_node_registry_with_nodes().await;

        // Create a shard that is on a remote node
        let mut shard = Shard::new("test-collection", 0);
        shard.state = ShardState::Active;
        shard.placements.push(super::super::shard::ShardPlacement {
            node_id: "remote-node-1".to_string(),
            is_primary: true,
            priority: 0,
            lag_ms: None,
        });

        let request = DistributedSearchRequest {
            collection: "test-collection".to_string(),
            vector: vec![0.1, 0.2, 0.3],
            top_k: 10,
            filter: None,
            routing_key: None,
            include_shards: None,
            exclude_shards: None,
            query_context: None,
        };

        // Execute remote search that will fail
        let result = DistributedCollectionOps::search_remote_shard(
            &shard,
            &request,
            Some(fanout.as_ref()),
            &node_registry,
            30000,
        )
        .await;

        // Should fail with RPC error
        assert!(result.is_err());
        assert_eq!(fanout.search_calls(), 1);
    }

    #[tokio::test]
    async fn test_forward_write_to_node_uses_rpc() {
        let fanout = Arc::new(MockSearchFanout::new());
        let coordinator = create_test_coordinator_with_fanout(fanout.clone()).await;

        // Register the remote node in the coordinator's registry
        coordinator
            .node_registry
            .register_node(super::super::node_registry::NodeInfo {
                node_id: "remote-node-1".to_string(),
                address: "192.168.1.100:5679".to_string(),
                ..Default::default()
            })
            .await
            .unwrap();

        // Create a shard that is on a remote node
        let mut shard = Shard::new("test-collection", 0);
        shard.state = ShardState::Active;
        shard.placements.push(super::super::shard::ShardPlacement {
            node_id: "remote-node-1".to_string(),
            is_primary: true,
            priority: 0,
            lag_ms: None,
        });

        let records = vec![
            WriteRecord {
                id: "rec1".to_string(),
                vector: vec![1.0, 2.0, 3.0],
                metadata: HashMap::new(),
            },
            WriteRecord {
                id: "rec2".to_string(),
                vector: vec![4.0, 5.0, 6.0],
                metadata: HashMap::new(),
            },
        ];

        // Execute forward write
        let result = coordinator
            .forward_write_to_node(&shard, "remote-node-1", &records, ConsistencyLevel::Quorum)
            .await;

        // Verify RPC was called and succeeded
        assert!(result.is_ok());
        assert_eq!(fanout.write_calls(), 1);
        assert_eq!(result.unwrap(), 3); // replicas_acked from mock
    }

    #[tokio::test]
    async fn test_forward_write_without_fanout_fails() {
        let coordinator = create_test_coordinator().await;

        // Register the remote node in the registry first
        coordinator
            .node_registry
            .register_node(super::NodeInfo {
                node_id: "remote-node-1".to_string(),
                address: "localhost:5679".to_string(),
                status: super::NodeStatus::Running,
                ..Default::default()
            })
            .await
            .unwrap();

        // Create a shard that is on a remote node
        let mut shard = Shard::new("test-collection", 0);
        shard.state = ShardState::Active;
        shard.placements.push(super::super::shard::ShardPlacement {
            node_id: "remote-node-1".to_string(),
            is_primary: true,
            priority: 0,
            lag_ms: None,
        });

        let records = vec![WriteRecord {
            id: "rec1".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: HashMap::new(),
        }];

        // Execute forward write without fanout
        let result = coordinator
            .forward_write_to_node(&shard, "remote-node-1", &records, ConsistencyLevel::Quorum)
            .await;

        // Should fail because no fanout is configured
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("SearchFanout"),
            "Expected SearchFanout error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_consistency_level_conversion() {
        assert_eq!(
            DistributedCollectionOps::convert_consistency_level(ConsistencyLevel::One),
            super::super::rpc::ConsistencyLevel::One
        );
        assert_eq!(
            DistributedCollectionOps::convert_consistency_level(ConsistencyLevel::Quorum),
            super::super::rpc::ConsistencyLevel::Quorum
        );
        assert_eq!(
            DistributedCollectionOps::convert_consistency_level(ConsistencyLevel::All),
            super::super::rpc::ConsistencyLevel::All
        );
        assert_eq!(
            DistributedCollectionOps::convert_consistency_level(ConsistencyLevel::LocalQuorum),
            super::super::rpc::ConsistencyLevel::LocalQuorum
        );
    }

    #[tokio::test]
    async fn test_local_shard_search_does_not_use_rpc() {
        let fanout = Arc::new(MockSearchFanout::new());
        let node_registry = create_node_registry_with_nodes().await;

        // Create a shard that is on the local node
        let mut shard = Shard::new("test-collection", 0);
        shard.state = ShardState::Active;
        shard.placements.push(super::super::shard::ShardPlacement {
            node_id: "local-node-1".to_string(),
            is_primary: true,
            priority: 0,
            lag_ms: None,
        });

        let request = DistributedSearchRequest {
            collection: "test-collection".to_string(),
            vector: vec![0.1, 0.2, 0.3],
            top_k: 10,
            filter: None,
            routing_key: None,
            include_shards: None,
            exclude_shards: None,
            query_context: None,
        };

        // Execute search on local shard
        let result = DistributedCollectionOps::search_single_shard(
            &shard,
            &request,
            "local-node-1",
            Some(fanout.as_ref()),
            &node_registry,
            30000,
        )
        .await;

        // Should succeed locally without RPC
        assert!(result.is_ok());
        assert_eq!(fanout.search_calls(), 0); // No RPC call
    }
}
