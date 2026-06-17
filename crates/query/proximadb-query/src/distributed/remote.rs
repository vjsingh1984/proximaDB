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

//! Remote Query Executor
//!
//! Executes queries on remote nodes via registered handlers today, with gRPC
//! transport intended to sit behind the same execution contract later.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use crate::fusion::SubQueryResult;

use super::planner::ShardedSubQuery;

/// Executable remote subquery handler.
///
/// This provides a real execution contract for remote subqueries even before
/// wire transport is fully productized. Implementations can be loopback,
/// in-process test nodes, or future gRPC-backed handlers.
#[async_trait]
pub trait RemoteQueryHandler: Send + Sync {
    async fn execute_remote_subquery(
        &self,
        subquery: &ShardedSubQuery,
    ) -> Result<Vec<SubQueryResult>>;
}

/// Result from a remote query execution
#[derive(Debug, Clone)]
pub struct RemoteQueryResult {
    /// Node that executed the query
    pub node_id: String,
    /// Query results
    pub results: Vec<SubQueryResult>,
    /// Execution time on remote node (microseconds)
    pub remote_execution_time_us: u64,
    /// Total round-trip time (microseconds)
    pub round_trip_time_us: u64,
    /// Whether this was a retry
    pub is_retry: bool,
}

/// Executor for remote queries across nodes.
pub struct RemoteExecutor {
    /// Timeout for remote queries
    timeout: Duration,
    /// Maximum retries
    max_retries: u32,
    /// Concurrency limiter
    semaphore: Arc<Semaphore>,
    /// Connection pool (node_id -> client)
    /// In a real implementation, this would hold gRPC clients
    #[allow(dead_code)]
    connection_pool: Arc<tokio::sync::RwLock<HashMap<String, RemoteConnection>>>,
    /// Registered remote handlers keyed by node id or address.
    handler_registry: Arc<tokio::sync::RwLock<HashMap<String, Arc<dyn RemoteQueryHandler>>>>,
}

/// A remote connection (placeholder for gRPC client)
#[allow(dead_code)]
struct RemoteConnection {
    address: String,
    #[allow(dead_code)]
    last_used: Instant,
    #[allow(dead_code)]
    healthy: bool,
}

impl RemoteExecutor {
    /// Create a new remote executor
    pub fn new(timeout: Duration, max_retries: u32) -> Self {
        Self {
            timeout,
            max_retries,
            semaphore: Arc::new(Semaphore::new(10)), // Max 10 concurrent remote queries
            connection_pool: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            handler_registry: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }

    /// Register a handler for a remote node. The handler is addressable by both
    /// node id and target address to simplify planner/coordinator wiring.
    pub async fn register_handler(
        &self,
        node_id: &str,
        address: &str,
        handler: Arc<dyn RemoteQueryHandler>,
    ) {
        let mut registry = self.handler_registry.write().await;
        registry.insert(node_id.to_string(), handler.clone());
        registry.insert(address.to_string(), handler);
    }

    /// Execute subqueries in parallel on remote nodes
    pub async fn execute_parallel(
        &self,
        subqueries: &[ShardedSubQuery],
    ) -> Result<Vec<SubQueryResult>> {
        if subqueries.is_empty() {
            return Ok(Vec::new());
        }

        info!(
            "Executing {} remote subqueries in parallel",
            subqueries.len()
        );

        let mut handles = Vec::with_capacity(subqueries.len());

        for subquery in subqueries {
            let sq = subquery.clone();
            let semaphore = self.semaphore.clone();
            let timeout = self.timeout;
            let max_retries = self.max_retries;
            let handler_registry = self.handler_registry.clone();

            let handle = tokio::spawn(async move {
                // Acquire semaphore
                let _permit = semaphore
                    .acquire()
                    .await
                    .map_err(|e| anyhow!("Semaphore error: {}", e))?;

                Self::execute_single_with_retry(&sq, timeout, max_retries, handler_registry).await
            });

            handles.push(handle);
        }

        // Collect results
        let mut all_results = Vec::new();
        let mut errors = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(Ok(remote_result)) => {
                    all_results.extend(remote_result.results);
                }
                Ok(Err(e)) => {
                    warn!("Remote query failed: {}", e);
                    errors.push(e.to_string());
                }
                Err(e) => {
                    warn!("Task join error: {}", e);
                    errors.push(e.to_string());
                }
            }
        }

        if !errors.is_empty() {
            return Err(anyhow!(
                "{} remote subqueries failed: {}",
                errors.len(),
                errors.join("; ")
            ));
        }

        Ok(all_results)
    }

    /// Execute subqueries sequentially on remote nodes
    pub async fn execute_sequential(
        &self,
        subqueries: &[ShardedSubQuery],
    ) -> Result<Vec<SubQueryResult>> {
        let mut all_results = Vec::new();
        let mut errors = Vec::new();

        for subquery in subqueries {
            match Self::execute_single_with_retry(
                subquery,
                self.timeout,
                self.max_retries,
                self.handler_registry.clone(),
            )
            .await
            {
                Ok(remote_result) => {
                    all_results.extend(remote_result.results);
                }
                Err(e) => {
                    warn!("Remote query to {} failed: {}", subquery.target_node, e);
                    errors.push(e.to_string());
                }
            }
        }

        if !errors.is_empty() {
            return Err(anyhow!(
                "{} remote subqueries failed: {}",
                errors.len(),
                errors.join("; ")
            ));
        }

        Ok(all_results)
    }

    /// Execute a single subquery with retries
    async fn execute_single_with_retry(
        subquery: &ShardedSubQuery,
        timeout: Duration,
        max_retries: u32,
        handler_registry: Arc<tokio::sync::RwLock<HashMap<String, Arc<dyn RemoteQueryHandler>>>>,
    ) -> Result<RemoteQueryResult> {
        let mut last_error = None;

        for attempt in 0..=max_retries {
            match Self::execute_single(subquery, timeout, handler_registry.clone()).await {
                Ok(mut result) => {
                    result.is_retry = attempt > 0;
                    return Ok(result);
                }
                Err(e) => {
                    warn!(
                        "Remote query attempt {} failed for {}: {}",
                        attempt + 1,
                        subquery.target_node,
                        e
                    );
                    last_error = Some(e);

                    // Exponential backoff
                    if attempt < max_retries {
                        tokio::time::sleep(Duration::from_millis(100 * (1 << attempt))).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("Unknown error")))
    }

    /// Execute a single subquery on a remote node
    async fn execute_single(
        subquery: &ShardedSubQuery,
        timeout: Duration,
        handler_registry: Arc<tokio::sync::RwLock<HashMap<String, Arc<dyn RemoteQueryHandler>>>>,
    ) -> Result<RemoteQueryResult> {
        let start = Instant::now();

        debug!(
            "Executing remote query on {} ({}) for {} shards",
            subquery.target_node,
            subquery.target_address,
            subquery.shard_ids.len()
        );

        let handler = {
            let registry = handler_registry.read().await;
            registry
                .get(&subquery.target_node)
                .cloned()
                .or_else(|| registry.get(&subquery.target_address).cloned())
        }
        .ok_or_else(|| {
            anyhow!(
                "Remote query execution is not wired for node '{}' ({})",
                subquery.target_node,
                subquery.target_address
            )
        })?;

        let result = tokio::time::timeout(timeout, async {
            let remote_start = Instant::now();
            let results = handler.execute_remote_subquery(subquery).await?;

            Ok::<RemoteQueryResult, anyhow::Error>(RemoteQueryResult {
                node_id: subquery.target_node.clone(),
                results,
                remote_execution_time_us: remote_start.elapsed().as_micros() as u64,
                round_trip_time_us: start.elapsed().as_micros() as u64,
                is_retry: false,
            })
        })
        .await;

        match result {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(anyhow!("Remote query timed out after {:?}", timeout)),
        }
    }

    /// Get or create a connection to a remote node
    #[allow(dead_code)]
    async fn get_connection(&self, node_id: &str, address: &str) -> Result<()> {
        let mut pool = self.connection_pool.write().await;

        if !pool.contains_key(node_id) {
            // Create new connection
            pool.insert(
                node_id.to_string(),
                RemoteConnection {
                    address: address.to_string(),
                    last_used: Instant::now(),
                    healthy: true,
                },
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fusion::SubQueryResult;
    use async_trait::async_trait;
    use proximadb_data_model::DataModel;

    struct StaticRemoteHandler {
        results: Vec<SubQueryResult>,
    }

    #[async_trait]
    impl RemoteQueryHandler for StaticRemoteHandler {
        async fn execute_remote_subquery(
            &self,
            _subquery: &ShardedSubQuery,
        ) -> Result<Vec<SubQueryResult>> {
            Ok(self.results.clone())
        }
    }

    #[test]
    fn test_remote_executor_creation() {
        let executor = RemoteExecutor::new(Duration::from_secs(30), 3);
        assert_eq!(executor.timeout, Duration::from_secs(30));
        assert_eq!(executor.max_retries, 3);
    }

    #[tokio::test]
    async fn test_execute_empty_subqueries() {
        let executor = RemoteExecutor::new(Duration::from_secs(30), 3);
        let results = executor.execute_parallel(&[]).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_execute_single_subquery() {
        let executor = RemoteExecutor::new(Duration::from_secs(5), 1);
        executor
            .register_handler(
                "node-2",
                "node2:5679",
                Arc::new(StaticRemoteHandler {
                    results: vec![SubQueryResult::empty(DataModel::Document)],
                }),
            )
            .await;

        let subquery = ShardedSubQuery {
            target_node: "node-2".to_string(),
            target_address: "node2:5679".to_string(),
            shard_ids: vec!["shard-1".to_string()],
            components: Vec::new(),
            collection: Some("test".to_string()),
            priority: 0,
        };

        let results = executor.execute_parallel(&[subquery]).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn test_execute_sequential() {
        let executor = RemoteExecutor::new(Duration::from_secs(5), 1);
        executor
            .register_handler(
                "node-1",
                "node1:5679",
                Arc::new(StaticRemoteHandler {
                    results: vec![SubQueryResult::empty(DataModel::Vector)],
                }),
            )
            .await;
        executor
            .register_handler(
                "node-2",
                "node2:5679",
                Arc::new(StaticRemoteHandler {
                    results: vec![SubQueryResult::empty(DataModel::Graph)],
                }),
            )
            .await;

        let subqueries = vec![
            ShardedSubQuery {
                target_node: "node-1".to_string(),
                target_address: "node1:5679".to_string(),
                shard_ids: vec!["shard-1".to_string()],
                components: Vec::new(),
                collection: Some("test".to_string()),
                priority: 0,
            },
            ShardedSubQuery {
                target_node: "node-2".to_string(),
                target_address: "node2:5679".to_string(),
                shard_ids: vec!["shard-2".to_string()],
                components: Vec::new(),
                collection: Some("test".to_string()),
                priority: 1,
            },
        ];

        let results = executor.execute_sequential(&subqueries).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_execute_single_subquery_without_registered_handler_errors() {
        let executor = RemoteExecutor::new(Duration::from_secs(5), 0);

        let subquery = ShardedSubQuery {
            target_node: "node-missing".to_string(),
            target_address: "missing:5679".to_string(),
            shard_ids: vec!["shard-1".to_string()],
            components: Vec::new(),
            collection: Some("test".to_string()),
            priority: 0,
        };

        let error = executor
            .execute_parallel(&[subquery])
            .await
            .expect_err("missing remote handler should fail explicitly");

        assert!(
            error
                .to_string()
                .contains("Remote query execution is not wired")
        );
    }
}
