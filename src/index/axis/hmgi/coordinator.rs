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
 * WITHOUT WARRANTIES OR CONDITIONS OF OR KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! HMGI Query Coordinator for Distributed Search
//!
//! Coordinates queries across nodes in a distributed cluster.

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use super::distributed::{ClusterNodeId, DistributedPartitionLocator};
use super::{HmgiPartitionKey, HmgiRegistry};
use crate::index::axis::management::{ScoredResult, VectorQuery};

/// HMGI query coordinator for distributed search
///
/// Coordinates parallel search across nodes:
/// 1. Groups partitions by owning node
/// 2. Sends parallel RPCs to each node
/// 3. Merges results maintaining top-k
pub struct HmgiQueryCoordinator {
    /// Partition locator for determining partition ownership
    locator: Arc<DistributedPartitionLocator>,

    /// Local registry for searching local partitions
    local_registry: Arc<HmgiRegistry>,

    /// Network service for remote RPC calls
    network: Arc<dyn NetworkService>,

    /// Query timeout
    query_timeout: Duration,
}

/// Network service trait for remote query execution
///
/// In production, this would use a real RPC framework like gRPC or tonic.
#[async_trait::async_trait]
pub trait NetworkService: Send + Sync {
    /// Execute a search request on a remote node
    async fn remote_search(
        &self,
        node_id: ClusterNodeId,
        request: SearchRequest,
    ) -> Result<Vec<ScoredResult>>;
}

/// Search request for remote execution
#[derive(Debug, Clone)]
pub struct SearchRequest {
    /// Partitions to search on the remote node
    pub partitions: Vec<HmgiPartitionKey>,

    /// Query vector
    pub query_vector: Vec<f32>,

    /// Number of results to return
    pub top_k: usize,
}

impl HmgiQueryCoordinator {
    /// Create a new query coordinator
    pub fn new(
        locator: Arc<DistributedPartitionLocator>,
        local_registry: Arc<HmgiRegistry>,
        network: Arc<dyn NetworkService>,
    ) -> Self {
        Self {
            locator,
            local_registry,
            network,
            query_timeout: Duration::from_secs(5),
        }
    }

    /// Create with custom timeout
    pub fn with_timeout(
        locator: Arc<DistributedPartitionLocator>,
        local_registry: Arc<HmgiRegistry>,
        network: Arc<dyn NetworkService>,
        timeout: Duration,
    ) -> Self {
        Self {
            locator,
            local_registry,
            network,
            query_timeout: timeout,
        }
    }

    /// Execute distributed search across partitions
    ///
    /// ## Process
    ///
    /// 1. Group partitions by owning node
    /// 2. For local partitions: search directly using local registry
    /// 3. For remote partitions: send parallel RPCs
    /// 4. Merge all results maintaining top-k
    pub async fn distributed_search(
        &self,
        partitions: Vec<HmgiPartitionKey>,
        query: &VectorQuery,
        top_k: usize,
    ) -> Result<Vec<ScoredResult>> {
        if partitions.is_empty() {
            return Ok(Vec::new());
        }

        let query_vector = match query {
            VectorQuery::Dense { vector, .. } => vector.clone(),
            _ => return Ok(Vec::new()),
        };

        // Split partitions into local and remote
        let (local_partitions, remote_partitions) =
            self.locator.split_local_remote(partitions).await;

        tracing::debug!(
            "Distributed search: {} local, {} remote partitions across {} nodes",
            local_partitions.len(),
            remote_partitions.values().map(|v| v.len()).sum::<usize>(),
            remote_partitions.len()
        );

        // Search local partitions
        let mut all_results = self
            .search_local_partitions(&local_partitions, &query_vector, top_k)
            .await?;

        // Search remote partitions in parallel
        if !remote_partitions.is_empty() {
            let remote_results = self
                .search_remote_partitions(remote_partitions, &query_vector, top_k)
                .await?;
            all_results.extend(remote_results);
        }

        // Merge results: sort by similarity (descending) and take top-k
        all_results.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        all_results.truncate(top_k);
        Ok(all_results)
    }

    /// Search local partitions directly
    async fn search_local_partitions(
        &self,
        partitions: &[HmgiPartitionKey],
        query_vector: &[f32],
        top_k: usize,
    ) -> Result<Vec<ScoredResult>> {
        let mut results = Vec::new();

        for partition in partitions {
            match self
                .search_single_local_partition(partition, query_vector, top_k)
                .await
            {
                Ok(mut partition_results) => results.append(&mut partition_results),
                Err(e) => {
                    tracing::warn!("Failed to search local partition {}: {}", partition, e);
                }
            }
        }

        Ok(results)
    }

    /// Search a single local partition
    async fn search_single_local_partition(
        &self,
        partition: &HmgiPartitionKey,
        query_vector: &[f32],
        top_k: usize,
    ) -> Result<Vec<ScoredResult>> {
        let index = self
            .local_registry
            .get_partition(partition)
            .await
            .ok_or_else(|| anyhow::anyhow!("Local partition not found: {}", partition))?;

        // See `hmgi/router.rs::search_single_partition_impl` for the
        // distance→similarity contract — same conversion applies here
        // because the HNSW index returns raw distances regardless of
        // which HMGI surface reads them.
        let search_results = index.search_simple(query_vector, top_k).await?;
        let metric = index.distance_metric();

        use crate::compute::distance_computation::engine::SimilarityResult;
        Ok(search_results
            .into_iter()
            .map(|(id, raw_distance)| ScoredResult {
                vector_id: id,
                similarity: SimilarityResult::new(raw_distance, metric).normalized_score,
                expires_at: None,
            })
            .collect())
    }

    /// Search remote partitions in parallel
    async fn search_remote_partitions(
        &self,
        remote_partitions: HashMap<ClusterNodeId, Vec<HmgiPartitionKey>>,
        query_vector: &[f32],
        top_k: usize,
    ) -> Result<Vec<ScoredResult>> {
        use tokio::task::JoinSet;

        let mut join_set = JoinSet::new();

        for (node_id, partitions) in remote_partitions {
            let network = self.network.clone();
            let partitions_clone = partitions.clone();
            let query_vector_clone = query_vector.to_vec();
            let timeout = self.query_timeout;

            join_set.spawn(async move {
                tokio::time::timeout(
                    timeout,
                    network.remote_search(
                        node_id,
                        SearchRequest {
                            partitions: partitions_clone,
                            query_vector: query_vector_clone,
                            top_k,
                        },
                    ),
                )
                .await
                .map_err(|_| anyhow::anyhow!("Remote search timeout"))
            });
        }

        let mut results = Vec::new();

        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(Ok(Ok(node_results))) => results.extend(node_results),
                Ok(Ok(Err(e))) => tracing::warn!("Remote search failed: {}", e),
                Ok(Err(e)) => tracing::warn!("Remote search task failed: {}", e),
                Err(e) => tracing::warn!("Remote search join failed: {}", e),
            }
        }

        Ok(results)
    }

    /// Get query timeout
    pub fn query_timeout(&self) -> Duration {
        self.query_timeout
    }

    /// Set query timeout
    pub fn set_query_timeout(&mut self, timeout: Duration) {
        self.query_timeout = timeout;
    }
}

/// Mock network service for testing
pub struct MockNetworkService {
    /// Simulated results to return
    pub mock_results: HashMap<ClusterNodeId, Vec<ScoredResult>>,

    /// Simulated delay for RPC calls
    pub simulated_delay: Duration,

    /// Whether to simulate failures
    pub simulate_failure: bool,
}

#[async_trait::async_trait]
impl NetworkService for MockNetworkService {
    async fn remote_search(
        &self,
        node_id: ClusterNodeId,
        _request: SearchRequest,
    ) -> Result<Vec<ScoredResult>> {
        // Simulate network delay
        tokio::time::sleep(self.simulated_delay).await;

        if self.simulate_failure {
            return Err(anyhow::anyhow!("Simulated network failure"));
        }

        Ok(self.mock_results.get(&node_id).cloned().unwrap_or_default())
    }
}

impl Default for MockNetworkService {
    fn default() -> Self {
        Self {
            mock_results: HashMap::new(),
            simulated_delay: Duration::from_millis(10),
            simulate_failure: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_coordinator_local_search() {
        let locator = Arc::new(DistributedPartitionLocator::new(3, 1));
        let registry = Arc::new(HmgiRegistry::new());
        let network = Arc::new(MockNetworkService::default());

        let coordinator = HmgiQueryCoordinator::new(locator, registry, network);

        // No partitions - should return empty
        let results = coordinator
            .distributed_search(
                vec![],
                &VectorQuery::Dense {
                    vector: vec![0.1, 0.2],
                    similarity_threshold: 0.0,
                },
                10,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 0);
    }

    #[tokio::test]
    async fn test_mock_network_service() {
        let mut mock = MockNetworkService::default();

        // Set up mock results
        let results = vec![ScoredResult {
            vector_id: "remote_id".to_string(),
            similarity: 0.9,
            expires_at: None,
        }];
        mock.mock_results.insert(1, results);

        let request = SearchRequest {
            partitions: vec![HmgiPartitionKey::new(123, 1, "text".to_string(), None)],
            query_vector: vec![0.1, 0.2],
            top_k: 10,
        };

        let result = mock.remote_search(1, request).await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].vector_id, "remote_id");
    }

    #[tokio::test]
    async fn test_mock_network_failure() {
        let mut mock = MockNetworkService::default();
        mock.simulate_failure = true;

        let request = SearchRequest {
            partitions: vec![],
            query_vector: vec![],
            top_k: 10,
        };

        let result = mock.remote_search(1, request).await;
        assert!(result.is_err());
    }
}
