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

//! # Distributed Search Integration Tests
//!
//! Comprehensive tests for ProximaDB's distributed search functionality,
//! covering:
//! - SearchFanout trait with mock implementations
//! - Fan-in result merging from multiple shards
//! - Remote shard search via RPC (tested through mock fanout)
//! - Forward write to remote nodes (tested through mock fanout)
//! - Error handling for failed shards
//!
//! These tests validate the scatter-gather pattern implementation for
//! distributed queries across multiple nodes and shards.
//!
//! Note: The internal methods (search_remote_shard, search_single_shard,
//! forward_write_to_node) are private and tested through the public API
//! or by testing the mock SearchFanout trait directly.

use async_trait::async_trait;
use futures::Stream;
use proximadb::cluster::{
    ConsensusConfig, DistributedCollectionOps, DistributedOpsConfig, DistributedSearchRequest,
    DistributedWriteRequest, MetadataBounds, NodeRegistryConfig, QueryContext, RoutingConfig,
    SearchResult, Shard, ShardConfig, ShardPlacement, ShardState, WriteRecord,
};
use proximadb::cluster::consensus::RaftConsensus;
use proximadb::cluster::node_registry::NodeRegistry;
use proximadb::cluster::routing::RoutingService;
use proximadb::cluster::rpc::{
    ConsistencyLevel as RpcConsistencyLevel, ForwardWriteRequest, ForwardWriteResponse,
    NodeEndpoint, RpcError, RpcResult, SearchFanout, SearchParams, ShardSearchRequest,
    ShardSearchResponse, ShardSearchResult, WriteRecord as RpcWriteRecord,
};
use proximadb::cluster::shard::ShardManager;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

// ============================================================================
// MOCK SEARCH FANOUT IMPLEMENTATIONS
// ============================================================================

/// Configurable mock SearchFanout for testing various scenarios
struct ConfigurableMockFanout {
    /// Counter for search calls
    search_call_count: AtomicUsize,
    /// Counter for write calls
    write_call_count: AtomicUsize,
    /// Results to return per shard (shard_id -> results)
    shard_results: RwLock<HashMap<String, Vec<ShardSearchResult>>>,
    /// Whether to simulate failures
    should_fail_search: AtomicBool,
    /// Whether to simulate write failures
    should_fail_write: AtomicBool,
    /// Simulated latency in milliseconds
    simulated_latency_ms: u64,
    /// Partial failure: fail only specific shards
    failing_shards: RwLock<Vec<String>>,
    /// Replicas acknowledged on writes
    replicas_acked: u32,
}

impl ConfigurableMockFanout {
    fn new() -> Self {
        Self {
            search_call_count: AtomicUsize::new(0),
            write_call_count: AtomicUsize::new(0),
            shard_results: RwLock::new(HashMap::new()),
            should_fail_search: AtomicBool::new(false),
            should_fail_write: AtomicBool::new(false),
            simulated_latency_ms: 5,
            failing_shards: RwLock::new(Vec::new()),
            replicas_acked: 3,
        }
    }

    fn with_shard_results(mut self, shard_id: &str, results: Vec<ShardSearchResult>) -> Self {
        self.shard_results
            .get_mut()
            .insert(shard_id.to_string(), results);
        self
    }

    fn with_failing_search(self) -> Self {
        self.should_fail_search.store(true, Ordering::SeqCst);
        self
    }

    fn with_failing_write(self) -> Self {
        self.should_fail_write.store(true, Ordering::SeqCst);
        self
    }

    fn with_failing_shards(mut self, shards: Vec<String>) -> Self {
        *self.failing_shards.get_mut() = shards;
        self
    }

    fn with_replicas_acked(mut self, count: u32) -> Self {
        self.replicas_acked = count;
        self
    }

    fn search_calls(&self) -> usize {
        self.search_call_count.load(Ordering::SeqCst)
    }

    fn write_calls(&self) -> usize {
        self.write_call_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SearchFanout for ConfigurableMockFanout {
    async fn shard_search(
        &self,
        _target: &NodeEndpoint,
        req: ShardSearchRequest,
    ) -> RpcResult<ShardSearchResponse> {
        self.search_call_count.fetch_add(1, Ordering::SeqCst);

        // Simulate latency
        if self.simulated_latency_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.simulated_latency_ms)).await;
        }

        // Check for global failure
        if self.should_fail_search.load(Ordering::SeqCst) {
            return Err(RpcError::connection("Simulated search failure"));
        }

        // Check for shard-specific failure
        let failing_shards = self.failing_shards.read().await;
        if failing_shards.contains(&req.shard_id) {
            return Err(RpcError::connection(format!(
                "Simulated failure for shard {}",
                req.shard_id
            )));
        }

        // Get predefined results or return empty
        let shard_results = self.shard_results.read().await;
        let results = shard_results
            .get(&req.shard_id)
            .cloned()
            .unwrap_or_default();

        Ok(ShardSearchResponse {
            request_id: req.request_id,
            shard_id: req.shard_id,
            results,
            vectors_scanned: 1000,
            latency: Duration::from_millis(self.simulated_latency_ms),
            truncated: false,
        })
    }

    async fn shard_search_stream(
        &self,
        _target: &NodeEndpoint,
        _req: ShardSearchRequest,
    ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<ShardSearchResult>> + Send>>> {
        Err(RpcError::internal("Streaming not implemented in mock"))
    }

    async fn forward_write(
        &self,
        _target: &NodeEndpoint,
        req: ForwardWriteRequest,
    ) -> RpcResult<ForwardWriteResponse> {
        self.write_call_count.fetch_add(1, Ordering::SeqCst);

        // Simulate latency
        if self.simulated_latency_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.simulated_latency_ms)).await;
        }

        if self.should_fail_write.load(Ordering::SeqCst) {
            return Err(RpcError::connection("Simulated write failure"));
        }

        Ok(ForwardWriteResponse {
            request_id: req.request_id,
            records_written: req.records.len() as u32,
            replicas_acked: self.replicas_acked,
            latency: Duration::from_millis(self.simulated_latency_ms),
            error: None,
        })
    }

    async fn forward_write_batch(
        &self,
        _target: &NodeEndpoint,
        requests: Vec<ForwardWriteRequest>,
    ) -> RpcResult<Vec<ForwardWriteResponse>> {
        if self.should_fail_write.load(Ordering::SeqCst) {
            return Err(RpcError::connection("Simulated batch write failure"));
        }

        Ok(requests
            .into_iter()
            .map(|req| {
                self.write_call_count.fetch_add(1, Ordering::SeqCst);
                ForwardWriteResponse {
                    request_id: req.request_id,
                    records_written: req.records.len() as u32,
                    replicas_acked: self.replicas_acked,
                    latency: Duration::from_millis(self.simulated_latency_ms),
                    error: None,
                }
            })
            .collect())
    }
}

/// Multi-shard mock that returns different results per shard
struct MultiShardMockFanout {
    results_by_shard: HashMap<String, Vec<ShardSearchResult>>,
    call_count: AtomicUsize,
}

impl MultiShardMockFanout {
    fn new(results: HashMap<String, Vec<ShardSearchResult>>) -> Self {
        Self {
            results_by_shard: results,
            call_count: AtomicUsize::new(0),
        }
    }

    fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl SearchFanout for MultiShardMockFanout {
    async fn shard_search(
        &self,
        _target: &NodeEndpoint,
        req: ShardSearchRequest,
    ) -> RpcResult<ShardSearchResponse> {
        self.call_count.fetch_add(1, Ordering::SeqCst);

        let results = self
            .results_by_shard
            .get(&req.shard_id)
            .cloned()
            .unwrap_or_default();

        Ok(ShardSearchResponse {
            request_id: req.request_id,
            shard_id: req.shard_id,
            results,
            vectors_scanned: 500,
            latency: Duration::from_millis(2),
            truncated: false,
        })
    }

    async fn shard_search_stream(
        &self,
        _target: &NodeEndpoint,
        _req: ShardSearchRequest,
    ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<ShardSearchResult>> + Send>>> {
        Err(RpcError::internal("Streaming not implemented"))
    }

    async fn forward_write(
        &self,
        _target: &NodeEndpoint,
        req: ForwardWriteRequest,
    ) -> RpcResult<ForwardWriteResponse> {
        Ok(ForwardWriteResponse {
            request_id: req.request_id,
            records_written: req.records.len() as u32,
            replicas_acked: 2,
            latency: Duration::from_millis(3),
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
                replicas_acked: 2,
                latency: Duration::from_millis(3),
                error: None,
            })
            .collect())
    }
}

// ============================================================================
// TEST HELPERS
// ============================================================================

/// Create a test coordinator without fanout
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

/// Create a test coordinator with fanout
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

/// Create a shard with specified placement
fn create_shard_with_placement(
    collection: &str,
    shard_num: u32,
    primary_node: &str,
    replica_nodes: &[&str],
) -> Shard {
    let mut shard = Shard::new(collection, shard_num);
    shard.state = ShardState::Active;

    // Add primary placement
    shard.placements.push(ShardPlacement {
        node_id: primary_node.to_string(),
        is_primary: true,
        priority: 0,
        lag_ms: None,
    });

    // Add replica placements
    for (i, replica) in replica_nodes.iter().enumerate() {
        shard.placements.push(ShardPlacement {
            node_id: replica.to_string(),
            is_primary: false,
            priority: (i + 1) as u32,
            lag_ms: None,
        });
    }

    shard
}

/// Create test search results
fn create_test_results(shard_id: &str, scores: &[(f32, &str)]) -> Vec<ShardSearchResult> {
    scores
        .iter()
        .map(|(score, id)| ShardSearchResult {
            id: id.to_string(),
            score: *score,
            vector: None,
            metadata: Some(format!(r#"{{"shard":"{}"}}"#, shard_id)),
        })
        .collect()
}

// ============================================================================
// FANOUT TESTING - SearchFanout Trait
// ============================================================================

#[tokio::test]
async fn test_search_fanout_basic() {
    let fanout = ConfigurableMockFanout::new()
        .with_shard_results(
            "test-collection_0000",
            create_test_results("shard-0", &[(0.1, "vec-1"), (0.3, "vec-2")]),
        )
        .with_shard_results(
            "test-collection_0001",
            create_test_results("shard-1", &[(0.2, "vec-3"), (0.4, "vec-4")]),
        );

    let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");
    let req = ShardSearchRequest {
        request_id: "req-1".to_string(),
        collection: "test-collection".to_string(),
        shard_id: "test-collection_0000".to_string(),
        vector: vec![0.1, 0.2, 0.3],
        top_k: 10,
        filter: None,
        params: SearchParams::default(),
        timeout: Duration::from_secs(5),
        include_vectors: false,
        tenant_id: None,
        domain_id: None,
    };

    let response = fanout.shard_search(&endpoint, req).await.unwrap();

    assert_eq!(response.shard_id, "test-collection_0000");
    assert_eq!(response.results.len(), 2);
    assert_eq!(fanout.search_calls(), 1);
}

#[tokio::test]
async fn test_search_fanout_with_failure() {
    let fanout = ConfigurableMockFanout::new().with_failing_search();

    let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");
    let req = ShardSearchRequest {
        request_id: "req-1".to_string(),
        collection: "test-collection".to_string(),
        shard_id: "test-collection_0000".to_string(),
        vector: vec![0.1, 0.2, 0.3],
        top_k: 10,
        filter: None,
        params: SearchParams::default(),
        timeout: Duration::from_secs(5),
        include_vectors: false,
        tenant_id: None,
        domain_id: None,
    };

    let result = fanout.shard_search(&endpoint, req).await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.message().contains("Simulated search failure"));
}

#[tokio::test]
async fn test_search_fanout_partial_failure() {
    let fanout = ConfigurableMockFanout::new()
        .with_shard_results("shard-1", create_test_results("shard-1", &[(0.1, "vec-1")]))
        .with_failing_shards(vec!["shard-2".to_string()]);

    let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");

    // Search shard-1 should succeed
    let req1 = ShardSearchRequest {
        request_id: "req-1".to_string(),
        collection: "test-collection".to_string(),
        shard_id: "shard-1".to_string(),
        vector: vec![0.1, 0.2, 0.3],
        top_k: 10,
        filter: None,
        params: SearchParams::default(),
        timeout: Duration::from_secs(5),
        include_vectors: false,
        tenant_id: None,
        domain_id: None,
    };
    let result1 = fanout.shard_search(&endpoint, req1).await;
    assert!(result1.is_ok());
    assert_eq!(result1.unwrap().results.len(), 1);

    // Search shard-2 should fail
    let req2 = ShardSearchRequest {
        request_id: "req-2".to_string(),
        collection: "test-collection".to_string(),
        shard_id: "shard-2".to_string(),
        vector: vec![0.1, 0.2, 0.3],
        top_k: 10,
        filter: None,
        params: SearchParams::default(),
        timeout: Duration::from_secs(5),
        include_vectors: false,
        tenant_id: None,
        domain_id: None,
    };
    let result2 = fanout.shard_search(&endpoint, req2).await;
    assert!(result2.is_err());
    assert!(result2.unwrap_err().message().contains("shard-2"));
}

#[tokio::test]
async fn test_search_fanout_with_tenant_context() {
    let fanout = ConfigurableMockFanout::new().with_shard_results(
        "test-shard",
        create_test_results("test-shard", &[(0.1, "vec-1")]),
    );

    let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");
    let req = ShardSearchRequest {
        request_id: "req-1".to_string(),
        collection: "test-collection".to_string(),
        shard_id: "test-shard".to_string(),
        vector: vec![0.1, 0.2, 0.3],
        top_k: 10,
        filter: None,
        params: SearchParams::default(),
        timeout: Duration::from_secs(5),
        include_vectors: false,
        tenant_id: Some("tenant-1".to_string()),
        domain_id: Some("domain-1".to_string()),
    };

    let response = fanout.shard_search(&endpoint, req).await.unwrap();

    assert_eq!(response.shard_id, "test-shard");
    assert_eq!(response.results.len(), 1);
}

// ============================================================================
// FORWARD WRITE TESTS - via SearchFanout trait
// ============================================================================

#[tokio::test]
async fn test_forward_write_via_fanout() {
    let fanout = ConfigurableMockFanout::new().with_replicas_acked(3);

    let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");
    let req = ForwardWriteRequest {
        request_id: "write-1".to_string(),
        collection: "test-collection".to_string(),
        shard_id: "shard-1".to_string(),
        records: vec![
            RpcWriteRecord {
                id: "vec-1".to_string(),
                vector: vec![0.1, 0.2, 0.3],
                metadata: HashMap::new(),
            },
            RpcWriteRecord {
                id: "vec-2".to_string(),
                vector: vec![0.4, 0.5, 0.6],
                metadata: HashMap::new(),
            },
        ],
        consistency: RpcConsistencyLevel::Quorum,
        timeout: Duration::from_secs(5),
        tenant_id: None,
        domain_id: None,
    };

    let response = fanout.forward_write(&endpoint, req).await.unwrap();

    assert_eq!(response.records_written, 2);
    assert_eq!(response.replicas_acked, 3);
    assert!(response.error.is_none());
    assert_eq!(fanout.write_calls(), 1);
}

#[tokio::test]
async fn test_forward_write_failure_via_fanout() {
    let fanout = ConfigurableMockFanout::new().with_failing_write();

    let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");
    let req = ForwardWriteRequest {
        request_id: "write-1".to_string(),
        collection: "test-collection".to_string(),
        shard_id: "shard-1".to_string(),
        records: vec![RpcWriteRecord {
            id: "vec-1".to_string(),
            vector: vec![0.1, 0.2, 0.3],
            metadata: HashMap::new(),
        }],
        consistency: RpcConsistencyLevel::Quorum,
        timeout: Duration::from_secs(5),
        tenant_id: None,
        domain_id: None,
    };

    let result = fanout.forward_write(&endpoint, req).await;

    assert!(result.is_err());
    assert!(result.unwrap_err().message().contains("Simulated write failure"));
}

#[tokio::test]
async fn test_forward_write_batch_via_fanout() {
    let fanout = ConfigurableMockFanout::new().with_replicas_acked(2);

    let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");
    let requests = vec![
        ForwardWriteRequest {
            request_id: "write-1".to_string(),
            collection: "test-collection".to_string(),
            shard_id: "shard-1".to_string(),
            records: vec![RpcWriteRecord {
                id: "vec-1".to_string(),
                vector: vec![0.1, 0.2, 0.3],
                metadata: HashMap::new(),
            }],
            consistency: RpcConsistencyLevel::Quorum,
            timeout: Duration::from_secs(5),
            tenant_id: None,
            domain_id: None,
        },
        ForwardWriteRequest {
            request_id: "write-2".to_string(),
            collection: "test-collection".to_string(),
            shard_id: "shard-2".to_string(),
            records: vec![RpcWriteRecord {
                id: "vec-2".to_string(),
                vector: vec![0.4, 0.5, 0.6],
                metadata: HashMap::new(),
            }],
            consistency: RpcConsistencyLevel::Quorum,
            timeout: Duration::from_secs(5),
            tenant_id: None,
            domain_id: None,
        },
    ];

    let responses = fanout.forward_write_batch(&endpoint, requests).await.unwrap();

    assert_eq!(responses.len(), 2);
    for response in &responses {
        assert_eq!(response.records_written, 1);
        assert_eq!(response.replicas_acked, 2);
    }
    assert_eq!(fanout.write_calls(), 2);
}

// ============================================================================
// FAN-IN RESULT MERGING TESTS
// ============================================================================

#[tokio::test]
async fn test_merge_search_results_basic() {
    // Create results from two shards with different scores
    let shard1_results = vec![
        SearchResult {
            id: "vec-1".to_string(),
            distance: 0.5,
            shard_id: "shard-1".to_string(),
            metadata: HashMap::new(),
        },
        SearchResult {
            id: "vec-2".to_string(),
            distance: 1.0,
            shard_id: "shard-1".to_string(),
            metadata: HashMap::new(),
        },
    ];

    let shard2_results = vec![
        SearchResult {
            id: "vec-3".to_string(),
            distance: 0.3,
            shard_id: "shard-2".to_string(),
            metadata: HashMap::new(),
        },
        SearchResult {
            id: "vec-4".to_string(),
            distance: 0.8,
            shard_id: "shard-2".to_string(),
            metadata: HashMap::new(),
        },
    ];

    // Merge with top_k=3
    let merged = merge_results_helper(vec![shard1_results, shard2_results], 3);

    assert_eq!(merged.len(), 3);
    // Results should be sorted by distance (ascending)
    assert_eq!(merged[0].id, "vec-3"); // distance 0.3
    assert_eq!(merged[1].id, "vec-1"); // distance 0.5
    assert_eq!(merged[2].id, "vec-4"); // distance 0.8
}

#[tokio::test]
async fn test_merge_search_results_with_duplicates() {
    // Results with same IDs from different shards (edge case)
    let shard1_results = vec![SearchResult {
        id: "vec-1".to_string(),
        distance: 0.5,
        shard_id: "shard-1".to_string(),
        metadata: HashMap::new(),
    }];

    let shard2_results = vec![SearchResult {
        id: "vec-1".to_string(),
        distance: 0.3, // Same ID, lower distance
        shard_id: "shard-2".to_string(),
        metadata: HashMap::new(),
    }];

    let merged = merge_results_helper(vec![shard1_results, shard2_results], 10);

    // Both results should be present (no dedup in basic merge)
    assert_eq!(merged.len(), 2);
    // Sorted by distance
    assert_eq!(merged[0].distance, 0.3);
    assert_eq!(merged[1].distance, 0.5);
}

#[tokio::test]
async fn test_merge_search_results_empty_shards() {
    let shard1_results = vec![SearchResult {
        id: "vec-1".to_string(),
        distance: 0.5,
        shard_id: "shard-1".to_string(),
        metadata: HashMap::new(),
    }];

    // Empty results from second shard
    let shard2_results: Vec<SearchResult> = vec![];

    let merged = merge_results_helper(vec![shard1_results, shard2_results], 10);

    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].id, "vec-1");
}

#[tokio::test]
async fn test_merge_search_results_top_k_limit() {
    // Create many results
    let shard1_results: Vec<SearchResult> = (0..50)
        .map(|i| SearchResult {
            id: format!("vec-{}", i),
            distance: i as f32 * 0.1,
            shard_id: "shard-1".to_string(),
            metadata: HashMap::new(),
        })
        .collect();

    let shard2_results: Vec<SearchResult> = (50..100)
        .map(|i| SearchResult {
            id: format!("vec-{}", i),
            distance: i as f32 * 0.1,
            shard_id: "shard-2".to_string(),
            metadata: HashMap::new(),
        })
        .collect();

    // Request only top 10
    let merged = merge_results_helper(vec![shard1_results, shard2_results], 10);

    assert_eq!(merged.len(), 10);
    // Should be the 10 lowest distances
    for i in 0..10 {
        assert_eq!(merged[i].id, format!("vec-{}", i));
    }
}

#[tokio::test]
async fn test_merge_search_results_single_shard() {
    let results = vec![
        SearchResult {
            id: "vec-1".to_string(),
            distance: 0.5,
            shard_id: "shard-1".to_string(),
            metadata: HashMap::new(),
        },
        SearchResult {
            id: "vec-2".to_string(),
            distance: 0.3,
            shard_id: "shard-1".to_string(),
            metadata: HashMap::new(),
        },
    ];

    let merged = merge_results_helper(vec![results], 10);

    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].id, "vec-2"); // lower distance first
    assert_eq!(merged[1].id, "vec-1");
}

/// Helper function that replicates the merge logic
fn merge_results_helper(shard_results: Vec<Vec<SearchResult>>, top_k: usize) -> Vec<SearchResult> {
    let mut all_results: Vec<SearchResult> = shard_results.into_iter().flatten().collect();
    all_results.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap());
    all_results.truncate(top_k);
    all_results
}

// ============================================================================
// PARALLEL SHARD SEARCH TESTS - via mock fanout
// ============================================================================

#[tokio::test]
async fn test_parallel_shard_search_multi_shard() {
    // Create results for multiple shards
    let mut shard_results = HashMap::new();
    shard_results.insert(
        "shard-0".to_string(),
        vec![
            ShardSearchResult {
                id: "vec-0-1".to_string(),
                score: 0.1,
                vector: None,
                metadata: None,
            },
            ShardSearchResult {
                id: "vec-0-2".to_string(),
                score: 0.5,
                vector: None,
                metadata: None,
            },
        ],
    );
    shard_results.insert(
        "shard-1".to_string(),
        vec![
            ShardSearchResult {
                id: "vec-1-1".to_string(),
                score: 0.2,
                vector: None,
                metadata: None,
            },
            ShardSearchResult {
                id: "vec-1-2".to_string(),
                score: 0.4,
                vector: None,
                metadata: None,
            },
        ],
    );
    shard_results.insert(
        "shard-2".to_string(),
        vec![ShardSearchResult {
            id: "vec-2-1".to_string(),
            score: 0.3,
            vector: None,
            metadata: None,
        }],
    );

    let fanout = Arc::new(MultiShardMockFanout::new(shard_results));

    // Verify all shards are queried
    let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");

    for shard_id in &["shard-0", "shard-1", "shard-2"] {
        let req = ShardSearchRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            collection: "test-collection".to_string(),
            shard_id: shard_id.to_string(),
            vector: vec![0.1, 0.2, 0.3],
            top_k: 10,
            filter: None,
            params: SearchParams::default(),
            timeout: Duration::from_secs(5),
            include_vectors: false,
            tenant_id: None,
            domain_id: None,
        };

        let result = fanout.shard_search(&endpoint, req).await;
        assert!(result.is_ok());
    }

    assert_eq!(fanout.call_count(), 3);
}

#[tokio::test]
async fn test_parallel_shard_search_result_aggregation() {
    let mut shard_results = HashMap::new();
    shard_results.insert(
        "shard-0".to_string(),
        vec![ShardSearchResult {
            id: "vec-0".to_string(),
            score: 0.5,
            vector: None,
            metadata: None,
        }],
    );
    shard_results.insert(
        "shard-1".to_string(),
        vec![ShardSearchResult {
            id: "vec-1".to_string(),
            score: 0.3,
            vector: None,
            metadata: None,
        }],
    );

    let fanout = Arc::new(MultiShardMockFanout::new(shard_results));
    let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");

    let mut all_results = Vec::new();

    for shard_id in &["shard-0", "shard-1"] {
        let req = ShardSearchRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            collection: "test-collection".to_string(),
            shard_id: shard_id.to_string(),
            vector: vec![0.1, 0.2, 0.3],
            top_k: 10,
            filter: None,
            params: SearchParams::default(),
            timeout: Duration::from_secs(5),
            include_vectors: false,
            tenant_id: None,
            domain_id: None,
        };

        let response = fanout.shard_search(&endpoint, req).await.unwrap();
        all_results.extend(response.results);
    }

    // Sort by score (ascending for distance-based)
    all_results.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap());

    assert_eq!(all_results.len(), 2);
    assert_eq!(all_results[0].id, "vec-1"); // score 0.3
    assert_eq!(all_results[1].id, "vec-0"); // score 0.5
}

// ============================================================================
// QUERY CONTEXT AND SHARD PRUNING TESTS
// ============================================================================

#[tokio::test]
async fn test_query_context_creation() {
    let ctx = QueryContext::new();
    assert!(!ctx.has_filters());

    let ctx = QueryContext::with_tenant("tenant-1");
    assert!(ctx.has_filters());
    assert_eq!(ctx.tenant_id, Some("tenant-1".to_string()));

    let ctx = QueryContext::new()
        .tenant("tenant-2")
        .domain("domain-1")
        .partition("partition-1")
        .with_field_filter("category", serde_json::json!("electronics"));

    assert!(ctx.has_filters());
    assert_eq!(ctx.tenant_id, Some("tenant-2".to_string()));
    assert_eq!(ctx.domain_id, Some("domain-1".to_string()));
    assert_eq!(ctx.partition_key, Some("partition-1".to_string()));
    assert!(ctx.field_filters.contains_key("category"));
}

#[tokio::test]
async fn test_shard_pruning_by_tenant() {
    // Create shards with different tenant data
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

    // Query for tenant-1 should only include shard1
    let ctx = QueryContext::with_tenant("tenant-1");
    let pruned = prune_shards_helper(&shards, &Some(ctx));
    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0].id.id(), "test-collection_0000");

    // Query for tenant-3 should only include shard2
    let ctx = QueryContext::with_tenant("tenant-3");
    let pruned = prune_shards_helper(&shards, &Some(ctx));
    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0].id.id(), "test-collection_0001");

    // Query without context should include all shards
    let pruned = prune_shards_helper(&shards, &None);
    assert_eq!(pruned.len(), 2);
}

#[tokio::test]
async fn test_shard_pruning_by_domain() {
    let mut shard1 = Shard::new("test-collection", 0);
    shard1.enable_metadata_bounds();
    if let Some(ref mut bounds) = shard1.metadata_bounds {
        bounds.domain_ids.insert("domain-a".to_string());
    }
    shard1.state = ShardState::Active;

    let mut shard2 = Shard::new("test-collection", 1);
    shard2.enable_metadata_bounds();
    if let Some(ref mut bounds) = shard2.metadata_bounds {
        bounds.domain_ids.insert("domain-b".to_string());
    }
    shard2.state = ShardState::Active;

    let shards = vec![shard1, shard2];

    // Query for domain-a
    let ctx = QueryContext::with_domain("domain-a");
    let pruned = prune_shards_helper(&shards, &Some(ctx));
    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0].id.id(), "test-collection_0000");
}

#[tokio::test]
async fn test_shard_pruning_combined_filters() {
    // Shard with both tenant-1 and domain-a
    let mut shard1 = Shard::new("test-collection", 0);
    shard1.enable_metadata_bounds();
    if let Some(ref mut bounds) = shard1.metadata_bounds {
        bounds.tenant_ids.insert("tenant-1".to_string());
        bounds.domain_ids.insert("domain-a".to_string());
    }
    shard1.state = ShardState::Active;

    // Shard with tenant-1 but domain-b
    let mut shard2 = Shard::new("test-collection", 1);
    shard2.enable_metadata_bounds();
    if let Some(ref mut bounds) = shard2.metadata_bounds {
        bounds.tenant_ids.insert("tenant-1".to_string());
        bounds.domain_ids.insert("domain-b".to_string());
    }
    shard2.state = ShardState::Active;

    let shards = vec![shard1, shard2];

    // Query for tenant-1 AND domain-a should only match shard1
    let ctx = QueryContext::with_tenant("tenant-1").domain("domain-a");
    let pruned = prune_shards_helper(&shards, &Some(ctx));
    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0].id.id(), "test-collection_0000");
}

#[tokio::test]
async fn test_shard_pruning_unknown_tenant() {
    let mut shard1 = Shard::new("test-collection", 0);
    shard1.enable_metadata_bounds();
    if let Some(ref mut bounds) = shard1.metadata_bounds {
        bounds.tenant_ids.insert("tenant-1".to_string());
    }
    shard1.state = ShardState::Active;

    let shards = vec![shard1];

    // Query for unknown tenant should return no shards
    let ctx = QueryContext::with_tenant("unknown-tenant");
    let pruned = prune_shards_helper(&shards, &Some(ctx));
    assert_eq!(pruned.len(), 0);
}

/// Helper to test shard pruning logic
fn prune_shards_helper(shards: &[Shard], query_context: &Option<QueryContext>) -> Vec<Shard> {
    let context = match query_context {
        Some(ctx) if ctx.has_filters() => ctx,
        _ => return shards.to_vec(),
    };

    shards
        .iter()
        .filter(|shard| shard.may_contain_data(context.tenant_id.as_deref(), context.domain_id.as_deref()))
        .cloned()
        .collect()
}

// ============================================================================
// COORDINATOR TESTS
// ============================================================================

#[tokio::test]
async fn test_coordinator_creation() {
    let coordinator = create_test_coordinator().await;
    let stats = coordinator.get_stats().await;

    assert_eq!(stats.total_searches, 0);
    assert_eq!(stats.total_writes, 0);
}

#[tokio::test]
async fn test_coordinator_with_fanout() {
    let fanout = Arc::new(ConfigurableMockFanout::new());
    let coordinator = create_test_coordinator_with_fanout(fanout).await;

    assert!(coordinator.fanout().is_some());
}

#[tokio::test]
async fn test_set_fanout() {
    let mut coordinator = create_test_coordinator().await;

    // Initially no fanout
    assert!(coordinator.fanout().is_none());

    // Set fanout
    let fanout = Arc::new(ConfigurableMockFanout::new());
    coordinator.set_fanout(fanout);

    // Now has fanout
    assert!(coordinator.fanout().is_some());
}

#[tokio::test]
async fn test_partition_records_by_shard() {
    let records = vec![
        WriteRecord {
            id: "rec-1".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: HashMap::new(),
        },
        WriteRecord {
            id: "rec-2".to_string(),
            vector: vec![4.0, 5.0, 6.0],
            metadata: HashMap::new(),
        },
        WriteRecord {
            id: "rec-3".to_string(),
            vector: vec![7.0, 8.0, 9.0],
            metadata: HashMap::new(),
        },
    ];

    let shards = vec![
        Shard::new("test-collection", 0),
        Shard::new("test-collection", 1),
    ];

    let partitioned = partition_records_helper(&records, &shards);

    // All records should be distributed
    let total: usize = partitioned.values().map(|v| v.len()).sum();
    assert_eq!(total, 3);
}

#[tokio::test]
async fn test_partition_records_with_tenant_context() {
    let records = vec![
        WriteRecord {
            id: "rec-1".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: HashMap::new(),
        },
        WriteRecord {
            id: "rec-2".to_string(),
            vector: vec![4.0, 5.0, 6.0],
            metadata: HashMap::new(),
        },
    ];

    let shards = vec![
        Shard::new("test-collection", 0),
        Shard::new("test-collection", 1),
    ];

    let partitioned =
        partition_records_with_context_helper(&records, &shards, Some("tenant-1"), Some("domain-1"));

    // All records should have tenant/domain metadata injected
    let total: usize = partitioned.values().map(|v| v.len()).sum();
    assert_eq!(total, 2);

    for (_shard_id, shard_records) in &partitioned {
        for record in shard_records {
            assert!(record.metadata.contains_key("tenant_id"));
            assert!(record.metadata.contains_key("domain_id"));
        }
    }
}

/// Helper for testing record partitioning
fn partition_records_helper(
    records: &[WriteRecord],
    shards: &[Shard],
) -> HashMap<String, Vec<WriteRecord>> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut partitioned: HashMap<String, Vec<WriteRecord>> = HashMap::new();

    for record in records {
        let mut hasher = DefaultHasher::new();
        record.id.hash(&mut hasher);
        let hash = hasher.finish();
        let shard_idx = (hash as usize) % shards.len();
        let shard_id = shards[shard_idx].id.id().to_string();

        partitioned.entry(shard_id).or_default().push(record.clone());
    }

    partitioned
}

/// Helper for testing record partitioning with context
fn partition_records_with_context_helper(
    records: &[WriteRecord],
    shards: &[Shard],
    tenant_id: Option<&str>,
    domain_id: Option<&str>,
) -> HashMap<String, Vec<WriteRecord>> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut partitioned: HashMap<String, Vec<WriteRecord>> = HashMap::new();

    for record in records {
        let mut enriched = record.clone();
        if let Some(tid) = tenant_id {
            enriched
                .metadata
                .insert("tenant_id".to_string(), serde_json::json!(tid));
        }
        if let Some(did) = domain_id {
            enriched
                .metadata
                .insert("domain_id".to_string(), serde_json::json!(did));
        }

        let mut hasher = DefaultHasher::new();
        record.id.hash(&mut hasher);
        let hash = hasher.finish();
        let shard_idx = (hash as usize) % shards.len();
        let shard_id = shards[shard_idx].id.id().to_string();

        partitioned.entry(shard_id).or_default().push(enriched);
    }

    partitioned
}

// ============================================================================
// ERROR HANDLING TESTS
// ============================================================================

#[tokio::test]
async fn test_rpc_error_types() {
    // Test connection error
    let err = RpcError::connection("connection refused");
    assert!(err.is_retryable());
    assert!(err.message().contains("connection refused"));

    // Test timeout error
    let err = RpcError::timeout(Duration::from_secs(5));
    assert!(err.is_retryable());
    assert!(err.message().contains("timed out"));

    // Test node not found error
    let err = RpcError::node_not_found("unknown-node");
    assert!(!err.is_retryable());
    assert!(err.message().contains("unknown-node"));

    // Test shard not found error
    let err = RpcError::shard_not_found("unknown-shard");
    assert!(!err.is_retryable());
    assert!(err.message().contains("unknown-shard"));

    // Test internal error
    let err = RpcError::internal("something went wrong");
    assert!(!err.is_retryable());
}

#[tokio::test]
async fn test_rpc_error_with_source_node() {
    let err = RpcError::internal("error details").with_source_node("node-1");

    assert_eq!(err.source_node(), Some("node-1"));
    let display = format!("{}", err);
    assert!(display.contains("node-1"));
}

#[tokio::test]
async fn test_rpc_error_retryable_override() {
    // Internal errors are not retryable by default
    let err = RpcError::internal("transient error");
    assert!(!err.is_retryable());

    // But can be overridden
    let err = err.with_retryable(true);
    assert!(err.is_retryable());
}

// ============================================================================
// CONSISTENCY LEVEL TESTS
// ============================================================================

#[tokio::test]
async fn test_consistency_level_conversion() {
    use proximadb::cluster::ConsistencyLevel;

    // Verify consistency levels exist and can be compared
    assert_ne!(ConsistencyLevel::One, ConsistencyLevel::Quorum);
    assert_ne!(ConsistencyLevel::Quorum, ConsistencyLevel::All);
    assert_ne!(ConsistencyLevel::All, ConsistencyLevel::LocalQuorum);
}

#[tokio::test]
async fn test_rpc_consistency_levels() {
    // Test RPC consistency level creation
    let level = RpcConsistencyLevel::One;
    assert_eq!(level, RpcConsistencyLevel::One);

    let level = RpcConsistencyLevel::Quorum;
    assert_eq!(level, RpcConsistencyLevel::Quorum);

    let level = RpcConsistencyLevel::All;
    assert_eq!(level, RpcConsistencyLevel::All);

    let level = RpcConsistencyLevel::LocalQuorum;
    assert_eq!(level, RpcConsistencyLevel::LocalQuorum);
}

// ============================================================================
// DISTRIBUTED WRITE REQUEST TESTS
// ============================================================================

#[tokio::test]
async fn test_distributed_write_request_creation() {
    let request = DistributedWriteRequest {
        collection: "test-collection".to_string(),
        records: vec![WriteRecord {
            id: "rec-1".to_string(),
            vector: vec![1.0, 2.0, 3.0],
            metadata: HashMap::new(),
        }],
        routing_key: Some("key-1".to_string()),
        tenant_id: Some("tenant-1".to_string()),
        domain_id: Some("domain-1".to_string()),
    };

    assert_eq!(request.collection, "test-collection");
    assert_eq!(request.records.len(), 1);
    assert_eq!(request.tenant_id, Some("tenant-1".to_string()));
    assert_eq!(request.domain_id, Some("domain-1".to_string()));
}

// ============================================================================
// DISTRIBUTED SEARCH REQUEST TESTS
// ============================================================================

#[tokio::test]
async fn test_distributed_search_request_creation() {
    let request = DistributedSearchRequest {
        collection: "test-collection".to_string(),
        vector: vec![0.1, 0.2, 0.3],
        top_k: 10,
        filter: Some(serde_json::json!({"category": "electronics"})),
        routing_key: Some("key-1".to_string()),
        include_shards: Some(vec!["shard-1".to_string()]),
        exclude_shards: Some(vec!["shard-2".to_string()]),
        query_context: Some(QueryContext::with_tenant("tenant-1").domain("domain-1")),
    };

    assert_eq!(request.collection, "test-collection");
    assert_eq!(request.vector.len(), 3);
    assert_eq!(request.top_k, 10);
    assert!(request.filter.is_some());
    assert!(request.query_context.is_some());

    let ctx = request.query_context.as_ref().unwrap();
    assert_eq!(ctx.tenant_id, Some("tenant-1".to_string()));
    assert_eq!(ctx.domain_id, Some("domain-1".to_string()));
}

// ============================================================================
// SHARD PLACEMENT TESTS
// ============================================================================

#[tokio::test]
async fn test_shard_primary_and_replica_nodes() {
    let shard = create_shard_with_placement(
        "test-collection",
        0,
        "primary-node",
        &["replica-1", "replica-2"],
    );

    assert_eq!(shard.primary_node(), Some("primary-node"));

    let replicas = shard.replica_nodes();
    assert_eq!(replicas.len(), 2);
    assert!(replicas.contains(&"replica-1"));
    assert!(replicas.contains(&"replica-2"));
}

#[tokio::test]
async fn test_shard_without_placements() {
    let shard = Shard::new("test-collection", 0);

    assert!(shard.primary_node().is_none());
    assert!(shard.replica_nodes().is_empty());
}

#[tokio::test]
async fn test_shard_state_transitions() {
    let mut shard = Shard::new("test-collection", 0);

    assert_eq!(shard.state, ShardState::Initializing);

    shard.state = ShardState::Active;
    assert_eq!(shard.state, ShardState::Active);

    shard.state = ShardState::Rebalancing;
    assert_eq!(shard.state, ShardState::Rebalancing);
}

// ============================================================================
// METADATA BOUNDS TESTS
// ============================================================================

#[tokio::test]
async fn test_metadata_bounds_tenant_tracking() {
    let mut bounds = MetadataBounds::new();

    // Initially empty
    assert!(bounds.tenant_ids.is_empty());

    // Add some tenants
    bounds.tenant_ids.insert("tenant-1".to_string());
    bounds.tenant_ids.insert("tenant-2".to_string());

    assert!(bounds.may_contain_tenant("tenant-1"));
    assert!(bounds.may_contain_tenant("tenant-2"));
    assert!(!bounds.may_contain_tenant("tenant-3"));
}

#[tokio::test]
async fn test_metadata_bounds_domain_tracking() {
    let mut bounds = MetadataBounds::new();

    bounds.domain_ids.insert("domain-a".to_string());

    assert!(bounds.may_contain_domain("domain-a"));
    assert!(!bounds.may_contain_domain("domain-b"));
}

#[tokio::test]
async fn test_metadata_bounds_empty_allows_all() {
    let bounds = MetadataBounds::new();

    // Empty bounds should allow any tenant/domain
    assert!(bounds.may_contain_tenant("any-tenant"));
    assert!(bounds.may_contain_domain("any-domain"));
    assert!(bounds.may_contain_partition("any-partition"));
}

#[tokio::test]
async fn test_metadata_bounds_update_with_record() {
    let mut bounds = MetadataBounds::new();

    let mut metadata = HashMap::new();
    metadata.insert("tenant_id".to_string(), serde_json::json!("tenant-1"));
    metadata.insert("domain_id".to_string(), serde_json::json!("domain-a"));
    metadata.insert("score".to_string(), serde_json::json!(95));

    bounds.update_with_record(&metadata, Some("partition-1"));

    assert!(bounds.may_contain_tenant("tenant-1"));
    assert!(bounds.may_contain_domain("domain-a"));
    assert!(bounds.may_contain_partition("partition-1"));
    assert!(bounds.field_ranges.contains_key("score"));
}

#[tokio::test]
async fn test_metadata_bounds_partition_tracking() {
    let mut bounds = MetadataBounds::new();

    bounds.partition_values.insert("partition-1".to_string());
    bounds.partition_values.insert("partition-2".to_string());

    assert!(bounds.may_contain_partition("partition-1"));
    assert!(bounds.may_contain_partition("partition-2"));
    assert!(!bounds.may_contain_partition("partition-3"));
}

// ============================================================================
// WRITE RECORD TESTS
// ============================================================================

#[tokio::test]
async fn test_write_record_creation() {
    let mut metadata = HashMap::new();
    metadata.insert("key".to_string(), serde_json::json!("value"));

    let record = WriteRecord {
        id: "vec-1".to_string(),
        vector: vec![0.1, 0.2, 0.3, 0.4],
        metadata,
    };

    assert_eq!(record.id, "vec-1");
    assert_eq!(record.vector.len(), 4);
    assert!(record.metadata.contains_key("key"));
}

#[tokio::test]
async fn test_rpc_write_record_conversion() {
    let local_record = WriteRecord {
        id: "vec-1".to_string(),
        vector: vec![0.1, 0.2],
        metadata: HashMap::from([("key".to_string(), serde_json::json!("value"))]),
    };

    // Simulate conversion to RPC WriteRecord
    let rpc_record = RpcWriteRecord {
        id: local_record.id.clone(),
        vector: local_record.vector.clone(),
        metadata: local_record.metadata.clone(),
    };

    assert_eq!(rpc_record.id, "vec-1");
    assert_eq!(rpc_record.vector.len(), 2);
    assert!(rpc_record.metadata.contains_key("key"));
}

// ============================================================================
// SEARCH PARAMS TESTS
// ============================================================================

#[tokio::test]
async fn test_search_params_default() {
    let params = SearchParams::default();

    assert!(params.min_score.is_none());
    assert!(params.ef_search.is_none());
    assert!(params.n_probes.is_none());
}

#[tokio::test]
async fn test_search_params_with_values() {
    let params = SearchParams {
        metric: proximadb::cluster::rpc::DistanceMetric::Cosine,
        min_score: Some(0.5),
        ef_search: Some(100),
        n_probes: Some(10),
    };

    assert_eq!(params.min_score, Some(0.5));
    assert_eq!(params.ef_search, Some(100));
    assert_eq!(params.n_probes, Some(10));
}

// ============================================================================
// NODE ENDPOINT TESTS
// ============================================================================

#[tokio::test]
async fn test_node_endpoint_creation() {
    let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");

    assert_eq!(endpoint.node_id, "node-1");
    assert_eq!(endpoint.address, "127.0.0.1:5679");
    assert!(!endpoint.tls);
}

#[tokio::test]
async fn test_node_endpoint_with_tls() {
    let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679").with_tls();

    assert!(endpoint.tls);
}

#[tokio::test]
async fn test_node_endpoint_display() {
    let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");
    let display = format!("{}", endpoint);

    assert!(display.contains("node-1"));
    assert!(display.contains("127.0.0.1:5679"));
}

// ============================================================================
// SHARD SEARCH REQUEST TESTS
// ============================================================================

#[tokio::test]
async fn test_shard_search_request_creation() {
    let req = ShardSearchRequest {
        request_id: "req-123".to_string(),
        collection: "test-collection".to_string(),
        shard_id: "shard-0".to_string(),
        vector: vec![0.1, 0.2, 0.3],
        top_k: 10,
        filter: Some(r#"{"category":"electronics"}"#.to_string()),
        params: SearchParams::default(),
        timeout: Duration::from_secs(5),
        include_vectors: true,
        tenant_id: Some("tenant-1".to_string()),
        domain_id: Some("domain-1".to_string()),
    };

    assert_eq!(req.request_id, "req-123");
    assert_eq!(req.collection, "test-collection");
    assert_eq!(req.shard_id, "shard-0");
    assert_eq!(req.vector.len(), 3);
    assert_eq!(req.top_k, 10);
    assert!(req.filter.is_some());
    assert!(req.include_vectors);
    assert_eq!(req.tenant_id, Some("tenant-1".to_string()));
    assert_eq!(req.domain_id, Some("domain-1".to_string()));
}

// ============================================================================
// SHARD SEARCH RESPONSE TESTS
// ============================================================================

#[tokio::test]
async fn test_shard_search_response_creation() {
    let response = ShardSearchResponse {
        request_id: "req-123".to_string(),
        shard_id: "shard-0".to_string(),
        results: vec![
            ShardSearchResult {
                id: "vec-1".to_string(),
                score: 0.95,
                vector: Some(vec![0.1, 0.2, 0.3]),
                metadata: Some(r#"{"key":"value"}"#.to_string()),
            },
        ],
        vectors_scanned: 1000,
        latency: Duration::from_millis(5),
        truncated: false,
    };

    assert_eq!(response.request_id, "req-123");
    assert_eq!(response.shard_id, "shard-0");
    assert_eq!(response.results.len(), 1);
    assert_eq!(response.vectors_scanned, 1000);
    assert!(!response.truncated);
}
