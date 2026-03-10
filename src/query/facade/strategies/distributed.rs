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

//! # Distributed Query Strategy
//!
//! This strategy wraps the DistributedQueryCoordinator to enable cluster-aware
//! query execution that spans multiple nodes in a ProximaDB cluster.

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tracing::debug;

use crate::cluster::ClusterManager;
use crate::query::distributed::DistributedQueryConfig;
use crate::query::distributed::DistributedQueryCoordinator;
use crate::query::facade::{
    QueryContext, QueryRequest, QueryResult, QueryResultData, QueryStrategy,
};
use crate::query::unified::fusion::SubQueryResult;

/// Configuration for distributed query strategy
#[derive(Debug, Clone)]
pub struct DistributedStrategyConfig {
    /// Maximum concurrent remote queries
    pub max_concurrent_remote_queries: usize,
    /// Remote query timeout in seconds
    pub remote_query_timeout_secs: u64,
    /// Enable result caching
    pub enable_result_cache: bool,
    /// Cache TTL in seconds
    pub cache_ttl_secs: u64,
    /// Prefer local execution when possible
    pub prefer_local_execution: bool,
    /// Enable shuffle exchange for cross-shard joins
    pub enable_shuffle: bool,
}

impl Default for DistributedStrategyConfig {
    fn default() -> Self {
        Self {
            max_concurrent_remote_queries: 10,
            remote_query_timeout_secs: 30,
            enable_result_cache: true,
            cache_ttl_secs: 60,
            prefer_local_execution: true,
            enable_shuffle: true,
        }
    }
}

/// Distributed query strategy
///
/// Wraps the DistributedQueryCoordinator to provide cluster-aware query
/// execution through the unified query facade.
pub struct DistributedQueryStrategy {
    /// Distributed query coordinator
    coordinator: Arc<DistributedQueryCoordinator>,
    /// Local node ID
    local_node_id: String,
    /// Strategy configuration
    config: DistributedStrategyConfig,
}

impl DistributedQueryStrategy {
    /// Create a new distributed query strategy
    pub fn new(local_node_id: String, config: DistributedStrategyConfig) -> Self {
        let dist_config = DistributedQueryConfig {
            max_concurrent_remote_queries: config.max_concurrent_remote_queries,
            remote_query_timeout: Duration::from_secs(config.remote_query_timeout_secs),
            enable_result_cache: config.enable_result_cache,
            cache_ttl_seconds: config.cache_ttl_secs,
            prefer_local_execution: config.prefer_local_execution,
            retry_failed_queries: true,
            max_retries: 3,
            parallel_remote_execution: true,
            enable_shuffle: config.enable_shuffle,
            shuffle_batch_size: 1000,
        };

        let coordinator = Arc::new(DistributedQueryCoordinator::new(
            dist_config,
            local_node_id.clone(),
        ));

        Self {
            coordinator,
            local_node_id,
            config,
        }
    }

    /// Set cluster manager for distributed execution
    pub fn with_cluster(mut self, cluster_manager: Arc<ClusterManager>) -> Self {
        let coordinator = Arc::new(
            DistributedQueryCoordinator::new(
                DistributedQueryConfig {
                    max_concurrent_remote_queries: self.config.max_concurrent_remote_queries,
                    remote_query_timeout: Duration::from_secs(
                        self.config.remote_query_timeout_secs,
                    ),
                    enable_result_cache: self.config.enable_result_cache,
                    cache_ttl_seconds: self.config.cache_ttl_secs,
                    prefer_local_execution: self.config.prefer_local_execution,
                    retry_failed_queries: true,
                    max_retries: 3,
                    parallel_remote_execution: true,
                    enable_shuffle: self.config.enable_shuffle,
                    shuffle_batch_size: 1000,
                },
                self.local_node_id.clone(),
            )
            .with_cluster(cluster_manager),
        );

        self.coordinator = coordinator;
        self
    }

    /// Get local node ID
    pub fn local_node_id(&self) -> &str {
        &self.local_node_id
    }

    /// Convert SubQueryResults to QueryResultData
    fn convert_results(&self, results: Vec<SubQueryResult>) -> QueryResultData {
        // Convert SubQueryResults to JSON format
        let json_results: Vec<serde_json::Value> = results
            .iter()
            .flat_map(|r| {
                r.records.iter().map(|record| {
                    // Extract the JSON data from UnifiedRecord instead of serializing the whole record
                    serde_json::json!({
                        "source_model": format!("{:?}", r.source_model),
                        "execution_time_us": r.execution_time_us,
                        "records_returned": r.records_returned,
                        "id": record.id,
                        "score": record.score,
                        "metadata": record.metadata,
                        "data": record.data,
                    })
                })
            })
            .collect();

        QueryResultData::Rows(json_results)
    }
}

#[async_trait]
impl QueryStrategy for DistributedQueryStrategy {
    /// Strategy name for metrics/debugging
    fn name(&self) -> &str {
        "distributed"
    }

    /// Check if this strategy can handle the given query
    fn can_handle(&self, request: &QueryRequest) -> bool {
        // Handle distributed query type requests
        request.query_type == crate::query::facade::QueryType::Federated
            && request.params.force_path.as_deref() == Some("distributed")
    }

    /// Execute the query and return results
    async fn execute(&self, request: QueryRequest, _ctx: &QueryContext) -> Result<QueryResult> {
        debug!("Executing distributed query: {:?}", request.query_type);

        // For distributed queries, we create a basic MultiModelQuery
        // In a real implementation, this would be parsed from the request
        let query = crate::query::unified::ast::MultiModelQuery::new();

        // Execute via distributed coordinator
        let results = self
            .coordinator
            .execute(&query)
            .await
            .map_err(|e| anyhow!("Distributed query failed: {}", e))?;

        // Convert results
        let data = self.convert_results(results.clone());

        // Create execution metrics
        let metrics = serde_json::json!({
            "query_type": "distributed",
            "num_results": results.len(),
            "local_node_id": self.local_node_id,
        });

        Ok(QueryResult {
            data,
            metrics: Some(serde_json::from_value(metrics)?),
        })
    }
}

/// Statistics for distributed query execution
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Cache hits
    pub cache_hits: u64,
    /// Number of shuffle operations executed
    pub shuffle_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_distributed_strategy_config_default() {
        let config = DistributedStrategyConfig::default();
        assert_eq!(config.max_concurrent_remote_queries, 10);
        assert_eq!(config.remote_query_timeout_secs, 30);
        assert!(config.enable_result_cache);
        assert!(config.prefer_local_execution);
    }

    #[test]
    fn test_distributed_query_stats_default() {
        let stats = DistributedQueryStats {
            total_queries: 0,
            local_only_queries: 0,
            distributed_queries: 0,
            remote_subqueries: 0,
            failed_remote_subqueries: 0,
            cache_hits: 0,
            shuffle_count: 0,
        };
        assert_eq!(stats.total_queries, 0);
    }
}
