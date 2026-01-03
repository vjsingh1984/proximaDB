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

//! Distributed Query Coordinator
//!
//! Coordinates query execution across multiple nodes in the cluster.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::cluster::{ClusterManager, NodeInfo, RoutingService, ShardManager};
use crate::query::unified::ast::MultiModelQuery;
use crate::query::unified::executor::ParallelExecutor;
use crate::query::unified::fusion::SubQueryResult;

use super::aggregator::{AggregationStrategy, ResultAggregator};
use super::planner::{DistributionStrategy, QueryPlanner, ShardedSubQuery};
use super::remote::RemoteExecutor;

/// Configuration for distributed query coordination
#[derive(Debug, Clone)]
pub struct DistributedQueryConfig {
    /// Maximum number of concurrent remote queries
    pub max_concurrent_remote_queries: usize,
    /// Timeout for remote queries
    pub remote_query_timeout: Duration,
    /// Enable result caching
    pub enable_result_cache: bool,
    /// Cache TTL in seconds
    pub cache_ttl_seconds: u64,
    /// Prefer local execution when possible
    pub prefer_local_execution: bool,
    /// Retry failed remote queries
    pub retry_failed_queries: bool,
    /// Maximum retries for failed queries
    pub max_retries: u32,
    /// Enable parallel remote execution
    pub parallel_remote_execution: bool,
}

impl Default for DistributedQueryConfig {
    fn default() -> Self {
        Self {
            max_concurrent_remote_queries: 10,
            remote_query_timeout: Duration::from_secs(30),
            enable_result_cache: true,
            cache_ttl_seconds: 60,
            prefer_local_execution: true,
            retry_failed_queries: true,
            max_retries: 3,
            parallel_remote_execution: true,
        }
    }
}

/// Statistics for distributed query execution
#[derive(Debug, Clone, Default)]
pub struct DistributedQueryStats {
    /// Total queries executed
    pub total_queries: u64,
    /// Queries executed locally only
    pub local_only_queries: u64,
    /// Queries requiring remote execution
    pub distributed_queries: u64,
    /// Total remote subqueries
    pub remote_subqueries: u64,
    /// Failed remote subqueries
    pub failed_remote_subqueries: u64,
    /// Average local execution time (microseconds)
    pub avg_local_time_us: u64,
    /// Average remote execution time (microseconds)
    pub avg_remote_time_us: u64,
    /// Cache hits
    pub cache_hits: u64,
}

/// Distributed Query Coordinator
///
/// Coordinates query execution across the cluster by:
/// 1. Analyzing queries to determine distribution strategy
/// 2. Routing subqueries to appropriate nodes
/// 3. Executing local portions using ParallelExecutor
/// 4. Aggregating results from all nodes
pub struct DistributedQueryCoordinator {
    config: DistributedQueryConfig,
    /// This node's ID
    local_node_id: String,
    /// Cluster manager for coordination
    cluster_manager: Option<Arc<ClusterManager>>,
    /// Routing service for shard-aware routing
    routing_service: Option<Arc<RoutingService>>,
    /// Shard manager for shard information
    shard_manager: Option<Arc<ShardManager>>,
    /// Query planner
    planner: QueryPlanner,
    /// Remote executor for cross-node queries
    remote_executor: RemoteExecutor,
    /// Result aggregator
    aggregator: ResultAggregator,
    /// Local parallel executor
    local_executor: ParallelExecutor,
    /// Execution statistics
    stats: Arc<RwLock<DistributedQueryStats>>,
    /// Result cache
    result_cache: Arc<RwLock<HashMap<String, CachedResult>>>,
}

/// Cached query result
struct CachedResult {
    result: Vec<SubQueryResult>,
    cached_at: Instant,
    ttl: Duration,
}

impl CachedResult {
    fn is_valid(&self) -> bool {
        self.cached_at.elapsed() < self.ttl
    }
}

impl DistributedQueryCoordinator {
    /// Create a new distributed query coordinator
    pub fn new(config: DistributedQueryConfig, local_node_id: String) -> Self {
        Self {
            local_executor: ParallelExecutor::new(config.max_concurrent_remote_queries),
            planner: QueryPlanner::new(config.prefer_local_execution),
            remote_executor: RemoteExecutor::new(config.remote_query_timeout, config.max_retries),
            aggregator: ResultAggregator::new(AggregationStrategy::default()),
            config,
            local_node_id,
            cluster_manager: None,
            routing_service: None,
            shard_manager: None,
            stats: Arc::new(RwLock::new(DistributedQueryStats::default())),
            result_cache: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create coordinator with cluster integration
    pub fn with_cluster(mut self, cluster_manager: Arc<ClusterManager>) -> Self {
        self.routing_service = Some(cluster_manager.routing_service().clone());
        self.shard_manager = Some(cluster_manager.shard_manager().clone());
        self.cluster_manager = Some(cluster_manager);
        self
    }

    /// Set routing service
    pub fn with_routing_service(mut self, routing_service: Arc<RoutingService>) -> Self {
        self.routing_service = Some(routing_service);
        self
    }

    /// Set shard manager
    pub fn with_shard_manager(mut self, shard_manager: Arc<ShardManager>) -> Self {
        self.shard_manager = Some(shard_manager);
        self
    }

    /// Execute a distributed query
    ///
    /// This is the main entry point for distributed query execution.
    /// The query is analyzed, decomposed into subqueries, routed to appropriate
    /// nodes, executed (locally and remotely), and results are aggregated.
    pub async fn execute(&self, query: &MultiModelQuery) -> Result<Vec<SubQueryResult>> {
        let start = Instant::now();

        // Check cache
        if self.config.enable_result_cache {
            if let Some(cached) = self.check_cache(query).await {
                let mut stats = self.stats.write().await;
                stats.cache_hits += 1;
                stats.total_queries += 1;
                return Ok(cached);
            }
        }

        // Plan the query distribution
        let plan = self.plan_query(query).await?;

        debug!(
            "Query plan: {} local subqueries, {} remote subqueries",
            plan.local_subqueries.len(),
            plan.remote_subqueries.len()
        );

        // Execute based on distribution
        let results = match plan.strategy {
            DistributionStrategy::LocalOnly => self.execute_local_only(query, &plan).await?,
            DistributionStrategy::Distributed => self.execute_distributed(query, &plan).await?,
            DistributionStrategy::Broadcast => self.execute_broadcast(query, &plan).await?,
        };

        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.total_queries += 1;
            if matches!(plan.strategy, DistributionStrategy::LocalOnly) {
                stats.local_only_queries += 1;
            } else {
                stats.distributed_queries += 1;
            }
        }

        // Cache results
        if self.config.enable_result_cache {
            self.cache_results(query, &results).await;
        }

        info!(
            "Distributed query completed in {:?}, {} results",
            start.elapsed(),
            results.iter().map(|r| r.records.len()).sum::<usize>()
        );

        Ok(results)
    }

    /// Plan query distribution across the cluster
    async fn plan_query(&self, query: &MultiModelQuery) -> Result<QueryPlan> {
        // Get cluster topology
        let nodes = self.get_available_nodes().await?;

        // Get shard information for collections in query
        let shard_info = self.get_shard_info_for_query(query).await?;

        // Use planner to create distribution plan
        self.planner
            .plan(query, &self.local_node_id, &nodes, &shard_info)
    }

    /// Get available nodes from cluster
    async fn get_available_nodes(&self) -> Result<Vec<NodeInfo>> {
        if let Some(ref cluster_mgr) = self.cluster_manager {
            let nodes = cluster_mgr.node_registry().list_nodes().await;
            Ok(nodes
                .into_iter()
                .filter(|n| n.health == crate::cluster::NodeHealth::Healthy)
                .collect())
        } else {
            // Single-node mode
            Ok(vec![NodeInfo {
                node_id: self.local_node_id.clone(),
                address: "localhost:5679".to_string(),
                health: crate::cluster::NodeHealth::Healthy,
                ..Default::default()
            }])
        }
    }

    /// Get shard information for collections referenced in query
    async fn get_shard_info_for_query(
        &self,
        query: &MultiModelQuery,
    ) -> Result<HashMap<String, Vec<ShardInfo>>> {
        let mut shard_info = HashMap::new();

        if let Some(ref shard_mgr) = self.shard_manager {
            // Extract collection names from query components
            for component in &query.components {
                if let Some(collection) = component.collection_name() {
                    let shards = shard_mgr.get_collection_shards(&collection).await;
                    let info: Vec<ShardInfo> = shards
                        .iter()
                        .map(|s| ShardInfo {
                            shard_id: s.id.id().to_string(),
                            primary_node: s.primary_node().map(String::from),
                            replica_nodes: s
                                .replica_nodes()
                                .iter()
                                .map(|s| s.to_string())
                                .collect(),
                        })
                        .collect();
                    shard_info.insert(collection, info);
                }
            }
        }

        Ok(shard_info)
    }

    /// Execute query locally only (single-node or all data local)
    async fn execute_local_only(
        &self,
        _query: &MultiModelQuery,
        plan: &QueryPlan,
    ) -> Result<Vec<SubQueryResult>> {
        debug!("Executing {} local subqueries", plan.local_subqueries.len());

        // Execute all local subqueries
        let mut results = Vec::new();
        for subquery in &plan.local_subqueries {
            let result = self.execute_local_subquery(subquery).await?;
            results.extend(result);
        }

        Ok(results)
    }

    /// Execute distributed query (data on multiple nodes)
    async fn execute_distributed(
        &self,
        _query: &MultiModelQuery,
        plan: &QueryPlan,
    ) -> Result<Vec<SubQueryResult>> {
        let start = Instant::now();

        // Execute local subqueries
        let local_results_future = async {
            let mut results = Vec::new();
            for subquery in &plan.local_subqueries {
                let result = self.execute_local_subquery(subquery).await?;
                results.extend(result);
            }
            Ok::<Vec<SubQueryResult>, anyhow::Error>(results)
        };

        // Execute remote subqueries in parallel
        let remote_results_future = async {
            if self.config.parallel_remote_execution {
                self.remote_executor
                    .execute_parallel(&plan.remote_subqueries)
                    .await
            } else {
                self.remote_executor
                    .execute_sequential(&plan.remote_subqueries)
                    .await
            }
        };

        // Wait for both local and remote
        let (local_results, remote_results) =
            tokio::try_join!(local_results_future, remote_results_future)?;

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.remote_subqueries += plan.remote_subqueries.len() as u64;
            stats.avg_remote_time_us = start.elapsed().as_micros() as u64;
        }

        // Aggregate results
        self.aggregator.aggregate(local_results, remote_results)
    }

    /// Execute broadcast query (query needs all nodes, e.g., aggregations)
    async fn execute_broadcast(
        &self,
        _query: &MultiModelQuery,
        plan: &QueryPlan,
    ) -> Result<Vec<SubQueryResult>> {
        // For broadcast, send to all nodes and aggregate
        let mut all_subqueries = plan.local_subqueries.clone();
        all_subqueries.extend(plan.remote_subqueries.clone());

        // Execute on all nodes in parallel
        let results = self
            .remote_executor
            .execute_parallel(&all_subqueries)
            .await?;

        Ok(results)
    }

    /// Execute a local subquery
    async fn execute_local_subquery(
        &self,
        _subquery: &ShardedSubQuery,
    ) -> Result<Vec<SubQueryResult>> {
        // For now, return empty - actual implementation would use local_executor
        // with proper service wiring
        Ok(vec![])
    }

    /// Check cache for query results
    async fn check_cache(&self, query: &MultiModelQuery) -> Option<Vec<SubQueryResult>> {
        let cache_key = self.compute_cache_key(query);
        let cache = self.result_cache.read().await;

        cache.get(&cache_key).and_then(|cached| {
            if cached.is_valid() {
                Some(cached.result.clone())
            } else {
                None
            }
        })
    }

    /// Cache query results
    async fn cache_results(&self, query: &MultiModelQuery, results: &[SubQueryResult]) {
        let cache_key = self.compute_cache_key(query);
        let mut cache = self.result_cache.write().await;

        cache.insert(
            cache_key,
            CachedResult {
                result: results.to_vec(),
                cached_at: Instant::now(),
                ttl: Duration::from_secs(self.config.cache_ttl_seconds),
            },
        );

        // Evict expired entries
        cache.retain(|_, v| v.is_valid());
    }

    /// Compute cache key for a query
    fn compute_cache_key(&self, query: &MultiModelQuery) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        // Hash key query properties
        query.components.len().hash(&mut hasher);
        for component in &query.components {
            format!("{:?}", component.model).hash(&mut hasher);
        }
        format!("{:016x}", hasher.finish())
    }

    /// Get execution statistics
    pub async fn get_stats(&self) -> DistributedQueryStats {
        self.stats.read().await.clone()
    }

    /// Clear the result cache
    pub async fn clear_cache(&self) {
        let mut cache = self.result_cache.write().await;
        cache.clear();
    }
}

/// Shard information for query planning
#[derive(Debug, Clone)]
pub struct ShardInfo {
    pub shard_id: String,
    pub primary_node: Option<String>,
    pub replica_nodes: Vec<String>,
}

/// Query plan with distribution strategy
#[derive(Debug, Clone)]
pub struct QueryPlan {
    /// Distribution strategy to use
    pub strategy: DistributionStrategy,
    /// Subqueries to execute locally
    pub local_subqueries: Vec<ShardedSubQuery>,
    /// Subqueries to execute on remote nodes
    pub remote_subqueries: Vec<ShardedSubQuery>,
    /// Estimated cost of the plan
    pub estimated_cost: f64,
}

impl QueryPlan {
    /// Create an empty query plan
    pub fn empty() -> Self {
        Self {
            strategy: DistributionStrategy::LocalOnly,
            local_subqueries: Vec::new(),
            remote_subqueries: Vec::new(),
            estimated_cost: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = DistributedQueryConfig::default();
        assert_eq!(config.max_concurrent_remote_queries, 10);
        assert_eq!(config.remote_query_timeout, Duration::from_secs(30));
        assert!(config.enable_result_cache);
        assert!(config.prefer_local_execution);
    }

    #[tokio::test]
    async fn test_coordinator_creation() {
        let config = DistributedQueryConfig::default();
        let coordinator = DistributedQueryCoordinator::new(config, "node-1".to_string());

        assert_eq!(coordinator.local_node_id, "node-1");
        assert!(coordinator.cluster_manager.is_none());
    }

    #[tokio::test]
    async fn test_stats_tracking() {
        let config = DistributedQueryConfig::default();
        let coordinator = DistributedQueryCoordinator::new(config, "node-1".to_string());

        let stats = coordinator.get_stats().await;
        assert_eq!(stats.total_queries, 0);
        assert_eq!(stats.distributed_queries, 0);
    }

    #[tokio::test]
    async fn test_cache_operations() {
        let config = DistributedQueryConfig {
            enable_result_cache: true,
            cache_ttl_seconds: 60,
            ..Default::default()
        };
        let coordinator = DistributedQueryCoordinator::new(config, "node-1".to_string());

        // Cache should be empty initially
        {
            let cache = coordinator.result_cache.read().await;
            assert!(cache.is_empty());
        }

        // Clear cache (should work even when empty)
        coordinator.clear_cache().await;
    }
}
