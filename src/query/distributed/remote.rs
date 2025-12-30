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
//! Executes queries on remote nodes via gRPC.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use crate::query::unified::fusion::SubQueryResult;

use super::planner::ShardedSubQuery;

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

/// Executor for remote queries via gRPC
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
        }
    }

    /// Execute subqueries in parallel on remote nodes
    pub async fn execute_parallel(
        &self,
        subqueries: &[ShardedSubQuery],
    ) -> Result<Vec<SubQueryResult>> {
        if subqueries.is_empty() {
            return Ok(Vec::new());
        }

        info!("Executing {} remote subqueries in parallel", subqueries.len());

        let mut handles = Vec::with_capacity(subqueries.len());

        for subquery in subqueries {
            let sq = subquery.clone();
            let semaphore = self.semaphore.clone();
            let timeout = self.timeout;
            let max_retries = self.max_retries;

            let handle = tokio::spawn(async move {
                // Acquire semaphore
                let _permit = semaphore
                    .acquire()
                    .await
                    .map_err(|e| anyhow!("Semaphore error: {}", e))?;

                Self::execute_single_with_retry(&sq, timeout, max_retries).await
            });

            handles.push(handle);
        }

        // Collect results
        let mut all_results = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(Ok(remote_result)) => {
                    all_results.extend(remote_result.results);
                }
                Ok(Err(e)) => {
                    warn!("Remote query failed: {}", e);
                    // Continue with other results
                }
                Err(e) => {
                    warn!("Task join error: {}", e);
                }
            }
        }

        Ok(all_results)
    }

    /// Execute subqueries sequentially on remote nodes
    pub async fn execute_sequential(
        &self,
        subqueries: &[ShardedSubQuery],
    ) -> Result<Vec<SubQueryResult>> {
        let mut all_results = Vec::new();

        for subquery in subqueries {
            match Self::execute_single_with_retry(subquery, self.timeout, self.max_retries).await {
                Ok(remote_result) => {
                    all_results.extend(remote_result.results);
                }
                Err(e) => {
                    warn!("Remote query to {} failed: {}", subquery.target_node, e);
                }
            }
        }

        Ok(all_results)
    }

    /// Execute a single subquery with retries
    async fn execute_single_with_retry(
        subquery: &ShardedSubQuery,
        timeout: Duration,
        max_retries: u32,
    ) -> Result<RemoteQueryResult> {
        let mut last_error = None;

        for attempt in 0..=max_retries {
            match Self::execute_single(subquery, timeout).await {
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
    ) -> Result<RemoteQueryResult> {
        let start = Instant::now();

        debug!(
            "Executing remote query on {} ({}) for {} shards",
            subquery.target_node,
            subquery.target_address,
            subquery.shard_ids.len()
        );

        // In a full implementation, this would:
        // 1. Get or create gRPC connection to target node
        // 2. Serialize the subquery components
        // 3. Send QueryRequest via gRPC
        // 4. Wait for response (with timeout)
        // 5. Deserialize results

        // For now, simulate the remote execution
        let result = tokio::time::timeout(timeout, async {
            // Simulated remote execution
            // In real implementation:
            // - Connect to target_address via gRPC
            // - Send query with shard_ids filter
            // - Receive SubQueryResult response

            tokio::time::sleep(Duration::from_millis(1)).await;

            Ok::<RemoteQueryResult, anyhow::Error>(RemoteQueryResult {
                node_id: subquery.target_node.clone(),
                results: Vec::new(), // Remote node would return actual results
                remote_execution_time_us: 0,
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
            pool.insert(node_id.to_string(), RemoteConnection {
                address: address.to_string(),
                last_used: Instant::now(),
                healthy: true,
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let subquery = ShardedSubQuery {
            target_node: "node-2".to_string(),
            target_address: "node2:5679".to_string(),
            shard_ids: vec!["shard-1".to_string()],
            components: Vec::new(),
            collection: Some("test".to_string()),
            priority: 0,
        };

        let results = executor.execute_parallel(&[subquery]).await.unwrap();
        // Results will be empty since we're simulating
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_execute_sequential() {
        let executor = RemoteExecutor::new(Duration::from_secs(5), 1);

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
        // Results will be empty since we're simulating
        assert!(results.is_empty());
    }
}
