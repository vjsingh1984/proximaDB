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

//! # Cluster Integration Tests
//!
//! Comprehensive integration tests for ProximaDB's cluster functionality:
//! - F1: 3-node consensus testing with RequestVote and AppendEntries
//! - F2: Connection pool testing with ConnectionManager
//! - C5: Circuit breaker testing (Closed -> Open -> HalfOpen transitions)
//! - C6: Retry policy testing with exponential backoff and jitter
//! - C7: RaftConsensus integration with mock transport

use async_trait::async_trait;
use futures::Stream;
use proximadb::cluster::{
    consensus::{Command, ConsensusConfig, ConsensusState, RaftConsensus},
    rpc::{
        AckReplicationRequest, AckReplicationResponse, AppendEntriesRequest, AppendEntriesResponse,
        CachedHealth, CircuitBreaker, CircuitState, ConnectionManager, ConnectionPoolConfig,
        ConsensusTransport, ForwardWriteRequest, ForwardWriteResponse, HealthCheckRequest,
        HealthCheckResponse, HealthChecker, InstallSnapshotRequest, InstallSnapshotResponse,
        NodeClient, NodeEndpoint, PullEntriesRequest, ReplicateRequest, ReplicateResponse,
        ReplicationSink, RequestVoteRequest, RequestVoteResponse, RetryExecutor, RetryPolicy,
        RpcError, RpcResult, SearchFanout, ServingStatus, ShardSearchRequest, ShardSearchResponse,
        ShardSearchResult, StatusRequest, StatusResponse,
    },
};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

// ============================================================================
// MOCK IMPLEMENTATIONS
// ============================================================================

/// Mock ConsensusTransport for testing 3-node consensus
struct MockConsensusTransport {
    /// Count of vote requests received
    vote_count: Arc<AtomicUsize>,
    /// Count of append entries received
    append_count: Arc<AtomicUsize>,
    /// Whether to grant votes
    grant_votes: Arc<AtomicBool>,
    /// Whether append entries should succeed
    append_success: Arc<AtomicBool>,
    /// Simulate failures for specific nodes
    failing_nodes: Arc<Mutex<Vec<String>>>,
    /// Current term to respond with
    response_term: Arc<AtomicU32>,
}

impl MockConsensusTransport {
    fn new() -> Self {
        Self {
            vote_count: Arc::new(AtomicUsize::new(0)),
            append_count: Arc::new(AtomicUsize::new(0)),
            grant_votes: Arc::new(AtomicBool::new(true)),
            append_success: Arc::new(AtomicBool::new(true)),
            failing_nodes: Arc::new(Mutex::new(Vec::new())),
            response_term: Arc::new(AtomicU32::new(1)),
        }
    }

    fn with_votes(grant: bool) -> Self {
        let transport = Self::new();
        transport.grant_votes.store(grant, Ordering::SeqCst);
        transport
    }

    #[allow(dead_code)]
    fn set_response_term(&self, term: u32) {
        self.response_term.store(term, Ordering::SeqCst);
    }

    async fn add_failing_node(&self, node_id: &str) {
        let mut nodes = self.failing_nodes.lock().await;
        nodes.push(node_id.to_string());
    }

    async fn is_failing(&self, node_id: &str) -> bool {
        let nodes = self.failing_nodes.lock().await;
        nodes.contains(&node_id.to_string())
    }
}

#[async_trait]
impl ConsensusTransport for MockConsensusTransport {
    async fn request_vote(
        &self,
        target: &NodeEndpoint,
        _req: RequestVoteRequest,
    ) -> RpcResult<RequestVoteResponse> {
        self.vote_count.fetch_add(1, Ordering::SeqCst);

        if self.is_failing(&target.node_id).await {
            return Err(RpcError::connection("Node is failing"));
        }

        Ok(RequestVoteResponse {
            term: self.response_term.load(Ordering::SeqCst) as u64,
            vote_granted: self.grant_votes.load(Ordering::SeqCst),
        })
    }

    async fn append_entries(
        &self,
        target: &NodeEndpoint,
        _req: AppendEntriesRequest,
    ) -> RpcResult<AppendEntriesResponse> {
        self.append_count.fetch_add(1, Ordering::SeqCst);

        if self.is_failing(&target.node_id).await {
            return Err(RpcError::connection("Node is failing"));
        }

        Ok(AppendEntriesResponse {
            term: self.response_term.load(Ordering::SeqCst) as u64,
            success: self.append_success.load(Ordering::SeqCst),
            match_index: Some(1),
            conflict_term: None,
            conflict_index: None,
        })
    }

    async fn install_snapshot(
        &self,
        target: &NodeEndpoint,
        _req: InstallSnapshotRequest,
    ) -> RpcResult<InstallSnapshotResponse> {
        if self.is_failing(&target.node_id).await {
            return Err(RpcError::connection("Node is failing"));
        }

        Ok(InstallSnapshotResponse {
            term: self.response_term.load(Ordering::SeqCst) as u64,
            bytes_stored: 1024,
        })
    }

    async fn pre_vote(
        &self,
        target: &NodeEndpoint,
        _req: RequestVoteRequest,
    ) -> RpcResult<RequestVoteResponse> {
        if self.is_failing(&target.node_id).await {
            return Err(RpcError::connection("Node is failing"));
        }

        Ok(RequestVoteResponse {
            term: self.response_term.load(Ordering::SeqCst) as u64,
            vote_granted: self.grant_votes.load(Ordering::SeqCst),
        })
    }
}

/// Mock ReplicationSink for testing
struct MockReplicationSink {
    replicate_count: Arc<AtomicUsize>,
    fail_after: Arc<AtomicUsize>,
}

impl MockReplicationSink {
    fn new() -> Self {
        Self {
            replicate_count: Arc::new(AtomicUsize::new(0)),
            fail_after: Arc::new(AtomicUsize::new(usize::MAX)),
        }
    }

    #[allow(dead_code)]
    fn set_fail_after(&self, count: usize) {
        self.fail_after.store(count, Ordering::SeqCst);
    }
}

#[async_trait]
impl ReplicationSink for MockReplicationSink {
    async fn replicate(
        &self,
        _target: &NodeEndpoint,
        req: ReplicateRequest,
    ) -> RpcResult<ReplicateResponse> {
        let count = self.replicate_count.fetch_add(1, Ordering::SeqCst);

        if count >= self.fail_after.load(Ordering::SeqCst) {
            return Err(RpcError::connection("Simulated failure"));
        }

        Ok(ReplicateResponse {
            node_id: "mock-node".to_string(),
            acked_lsn: req.lsn,
            success: true,
            error: None,
            latency: Duration::from_micros(100),
        })
    }

    async fn replicate_batch(
        &self,
        _target: &NodeEndpoint,
        requests: Vec<ReplicateRequest>,
    ) -> RpcResult<Vec<ReplicateResponse>> {
        Ok(requests
            .into_iter()
            .map(|req| ReplicateResponse {
                node_id: "mock-node".to_string(),
                acked_lsn: req.lsn,
                success: true,
                error: None,
                latency: Duration::from_micros(100),
            })
            .collect())
    }

    async fn replicate_stream(
        &self,
        _target: &NodeEndpoint,
        _requests: Pin<Box<dyn Stream<Item = ReplicateRequest> + Send>>,
    ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<ReplicateResponse>> + Send>>> {
        Err(RpcError::internal("Mock does not implement streaming"))
    }

    async fn pull_entries(
        &self,
        _target: &NodeEndpoint,
        _req: PullEntriesRequest,
    ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<ReplicateRequest>> + Send>>> {
        Err(RpcError::internal("Mock does not implement streaming"))
    }

    async fn ack_replication(
        &self,
        _target: &NodeEndpoint,
        req: AckReplicationRequest,
    ) -> RpcResult<AckReplicationResponse> {
        Ok(AckReplicationResponse {
            success: true,
            primary_lsn: req.acked_lsn + 10,
        })
    }
}

/// Mock SearchFanout for testing
struct MockSearchFanout {
    search_count: Arc<AtomicUsize>,
}

impl MockSearchFanout {
    fn new() -> Self {
        Self {
            search_count: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl SearchFanout for MockSearchFanout {
    async fn shard_search(
        &self,
        _target: &NodeEndpoint,
        req: ShardSearchRequest,
    ) -> RpcResult<ShardSearchResponse> {
        self.search_count.fetch_add(1, Ordering::SeqCst);

        Ok(ShardSearchResponse {
            request_id: req.request_id,
            shard_id: req.shard_id,
            results: vec![ShardSearchResult {
                id: "vec-1".to_string(),
                score: 0.95,
                vector: None,
                metadata: None,
            }],
            vectors_scanned: 1000,
            latency: Duration::from_micros(500),
            truncated: false,
        })
    }

    async fn shard_search_stream(
        &self,
        _target: &NodeEndpoint,
        _req: ShardSearchRequest,
    ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<ShardSearchResult>> + Send>>> {
        Err(RpcError::internal("Mock does not implement streaming"))
    }

    async fn forward_write(
        &self,
        _target: &NodeEndpoint,
        req: ForwardWriteRequest,
    ) -> RpcResult<ForwardWriteResponse> {
        Ok(ForwardWriteResponse {
            request_id: req.request_id,
            records_written: req.records.len() as u32,
            replicas_acked: 3,
            latency: Duration::from_millis(5),
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
                latency: Duration::from_millis(5),
                error: None,
            })
            .collect())
    }
}

/// Mock HealthChecker for testing
struct MockHealthChecker {
    healthy: Arc<AtomicBool>,
}

impl MockHealthChecker {
    fn new() -> Self {
        Self {
            healthy: Arc::new(AtomicBool::new(true)),
        }
    }

    #[allow(dead_code)]
    fn set_healthy(&self, healthy: bool) {
        self.healthy.store(healthy, Ordering::SeqCst);
    }
}

#[async_trait]
impl HealthChecker for MockHealthChecker {
    async fn check(
        &self,
        _target: &NodeEndpoint,
        _req: HealthCheckRequest,
    ) -> RpcResult<HealthCheckResponse> {
        let status = if self.healthy.load(Ordering::SeqCst) {
            ServingStatus::Serving
        } else {
            ServingStatus::NotServing
        };
        Ok(HealthCheckResponse { status })
    }

    async fn status(
        &self,
        _target: &NodeEndpoint,
        _req: StatusRequest,
    ) -> RpcResult<StatusResponse> {
        Ok(StatusResponse {
            node_id: "mock-node".to_string(),
            role: proximadb::cluster::rpc::NodeRole::Follower,
            current_term: 1,
            leader_id: Some("leader-1".to_string()),
            uptime_seconds: 3600,
            active_connections: 10,
            memory_bytes: 1024 * 1024 * 512,
            cpu_percent: 25.0,
            shards: vec![],
            replication_lag_ms: None,
        })
    }

    async fn watch(
        &self,
        _target: &NodeEndpoint,
        _req: HealthCheckRequest,
    ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<HealthCheckResponse>> + Send>>> {
        Err(RpcError::internal("Mock does not implement streaming"))
    }
}

// ============================================================================
// F1: 3-NODE CONSENSUS TESTING
// ============================================================================

/// Test RequestVote RPC with 3 nodes - all vote granted
#[tokio::test]
async fn test_3node_consensus_request_vote_all_granted() {
    let transport = Arc::new(MockConsensusTransport::new());
    let peers = vec![
        NodeEndpoint::new("node-2", "127.0.0.1:5680"),
        NodeEndpoint::new("node-3", "127.0.0.1:5681"),
    ];

    let config = ConsensusConfig {
        enable_pre_vote: false,
        ..Default::default()
    };

    let consensus =
        RaftConsensus::with_transport(config, "node-1", transport.clone(), peers).unwrap();

    // Start an election
    let result = consensus.start_election().await;
    assert!(result.is_ok());
    assert!(result.unwrap()); // Should become leader

    assert_eq!(consensus.get_state().await, ConsensusState::Leader);
    assert_eq!(consensus.get_leader().await, Some("node-1".to_string()));

    // Both peers should have been contacted
    assert_eq!(transport.vote_count.load(Ordering::SeqCst), 2);
}

/// Test RequestVote RPC with 3 nodes - no votes granted
#[tokio::test]
async fn test_3node_consensus_request_vote_none_granted() {
    let transport = Arc::new(MockConsensusTransport::with_votes(false));
    let peers = vec![
        NodeEndpoint::new("node-2", "127.0.0.1:5680"),
        NodeEndpoint::new("node-3", "127.0.0.1:5681"),
    ];

    let config = ConsensusConfig {
        enable_pre_vote: false,
        ..Default::default()
    };

    let consensus =
        RaftConsensus::with_transport(config, "node-1", transport.clone(), peers).unwrap();

    // Start an election - should fail to get majority
    let result = consensus.start_election().await;
    assert!(result.is_ok());
    assert!(!result.unwrap()); // Should NOT become leader

    assert_eq!(consensus.get_state().await, ConsensusState::Follower);
}

/// Test RequestVote RPC with 3 nodes - majority (2 of 3) votes granted
#[tokio::test]
async fn test_3node_consensus_request_vote_majority() {
    let transport = Arc::new(MockConsensusTransport::new());
    // Make one node fail, but majority should still be achieved
    transport.add_failing_node("node-3").await;

    let peers = vec![
        NodeEndpoint::new("node-2", "127.0.0.1:5680"),
        NodeEndpoint::new("node-3", "127.0.0.1:5681"),
    ];

    let config = ConsensusConfig {
        enable_pre_vote: false,
        ..Default::default()
    };

    let consensus =
        RaftConsensus::with_transport(config, "node-1", transport.clone(), peers).unwrap();

    // Start an election - should succeed with 2/3 votes (self + node-2)
    let result = consensus.start_election().await;
    assert!(result.is_ok());
    assert!(result.unwrap()); // Should become leader with majority

    assert_eq!(consensus.get_state().await, ConsensusState::Leader);
}

/// Test AppendEntries (heartbeat) with 3 nodes using public API
#[tokio::test]
async fn test_3node_consensus_append_entries_heartbeat() {
    let transport = Arc::new(MockConsensusTransport::new());
    let peers = vec![
        NodeEndpoint::new("node-2", "127.0.0.1:5680"),
        NodeEndpoint::new("node-3", "127.0.0.1:5681"),
    ];

    let config = ConsensusConfig {
        enable_pre_vote: false,
        ..Default::default()
    };
    let consensus =
        RaftConsensus::with_transport(config, "node-1", transport.clone(), peers).unwrap();

    // First become leader through election
    let election_result = consensus.start_election().await;
    assert!(election_result.is_ok());
    assert!(election_result.unwrap());
    assert_eq!(consensus.get_state().await, ConsensusState::Leader);

    // Reset counts after election
    transport.vote_count.store(0, Ordering::SeqCst);
    transport.append_count.store(0, Ordering::SeqCst);

    // Send heartbeat
    let result = consensus.send_heartbeat().await;
    assert!(result.is_ok());

    // Both peers should have received heartbeat
    assert_eq!(transport.append_count.load(Ordering::SeqCst), 2);
}

/// Test single node cluster becomes leader immediately
#[tokio::test]
async fn test_single_node_cluster_becomes_leader() {
    let transport = Arc::new(MockConsensusTransport::new());
    let peers: Vec<NodeEndpoint> = vec![];

    let config = ConsensusConfig::default();
    let consensus = RaftConsensus::with_transport(config, "node-1", transport, peers).unwrap();

    let result = consensus.start_election().await;
    assert!(result.is_ok());
    assert!(result.unwrap());

    assert_eq!(consensus.get_state().await, ConsensusState::Leader);
}

/// Test handle_request_vote rejects lower term
#[tokio::test]
async fn test_handle_request_vote_rejects_lower_term() {
    let transport = Arc::new(MockConsensusTransport::new());
    let consensus =
        RaftConsensus::with_transport(ConsensusConfig::default(), "node-1", transport, vec![])
            .unwrap();

    // First receive a higher term to set our term
    let (_term, _granted) = consensus.handle_request_vote(10, "candidate-1", 0, 0).await;

    // Now reject a lower term
    let (term, granted) = consensus.handle_request_vote(5, "candidate-2", 0, 0).await;
    assert_eq!(term, 10);
    assert!(!granted);
}

/// Test handle_request_vote grants vote for higher term
#[tokio::test]
async fn test_handle_request_vote_grants_higher_term() {
    let transport = Arc::new(MockConsensusTransport::new());
    let consensus =
        RaftConsensus::with_transport(ConsensusConfig::default(), "node-1", transport, vec![])
            .unwrap();

    // Request vote with higher term
    let (term, granted) = consensus.handle_request_vote(10, "candidate-1", 0, 0).await;
    assert_eq!(term, 10);
    assert!(granted);

    // Our term should now be updated
    assert_eq!(consensus.current_term().await, 10);
}

/// Test handle_append_entries rejects lower term
#[tokio::test]
async fn test_handle_append_entries_rejects_lower_term() {
    let transport = Arc::new(MockConsensusTransport::new());
    let consensus =
        RaftConsensus::with_transport(ConsensusConfig::default(), "node-1", transport, vec![])
            .unwrap();

    // First set our term higher via request_vote
    let (_term, _granted) = consensus.handle_request_vote(10, "candidate-1", 0, 0).await;
    assert_eq!(consensus.current_term().await, 10);

    // Now reject append entries with lower term
    let (term, success) = consensus
        .handle_append_entries(5, "leader-old", 0, 0, vec![], 0)
        .await;
    assert_eq!(term, 10);
    assert!(!success);
}

/// Test handle_append_entries accepts current term
#[tokio::test]
async fn test_handle_append_entries_accepts_current_term() {
    let transport = Arc::new(MockConsensusTransport::new());
    let consensus =
        RaftConsensus::with_transport(ConsensusConfig::default(), "node-1", transport, vec![])
            .unwrap();

    // AppendEntries with term 5 should succeed
    let (term, success) = consensus
        .handle_append_entries(5, "leader-1", 0, 0, vec![], 0)
        .await;
    assert_eq!(term, 5);
    assert!(success);

    // Should recognize the leader
    let leader = consensus.get_leader().await;
    assert_eq!(leader, Some("leader-1".to_string()));

    // Should transition to follower
    assert_eq!(consensus.get_state().await, ConsensusState::Follower);
}

// ============================================================================
// F2: CONNECTION POOL TESTING
// ============================================================================

/// Test ConnectionManager creation and basic operations
#[tokio::test]
async fn test_connection_manager_creation() {
    let config = ConnectionPoolConfig::default();
    let manager = ConnectionManager::new(config);

    assert!(manager.active_endpoints().is_empty());
    let stats = manager.stats();
    assert_eq!(stats.active_pools, 0);
    assert_eq!(stats.total_channels, 0);
}

/// Test ConnectionManager health tracking
#[tokio::test]
async fn test_connection_manager_health_tracking() {
    let config = ConnectionPoolConfig::default();
    let manager = ConnectionManager::new(config);

    let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");

    // Initially healthy (no cached status)
    assert!(manager.is_healthy(&endpoint).await);

    // Mark as unhealthy
    manager
        .mark_unhealthy(&endpoint, "connection refused")
        .await;
    assert!(!manager.is_healthy(&endpoint).await);

    // Verify cached health
    let health = manager.get_health(&endpoint).await;
    assert!(health.is_some());
    let health = health.unwrap();
    assert_eq!(health.status, ServingStatus::NotServing);
    assert_eq!(health.consecutive_failures, 1);

    // Mark as healthy again
    manager.mark_healthy(&endpoint).await;
    assert!(manager.is_healthy(&endpoint).await);
}

/// Test ConnectionManager with multiple endpoints
#[tokio::test]
async fn test_connection_manager_multiple_endpoints() {
    let config = ConnectionPoolConfig::default();
    let manager = ConnectionManager::new(config);

    let endpoints = vec![
        NodeEndpoint::new("node-1", "127.0.0.1:5679"),
        NodeEndpoint::new("node-2", "127.0.0.1:5680"),
        NodeEndpoint::new("node-3", "127.0.0.1:5681"),
    ];

    // Mark all as healthy first
    for endpoint in &endpoints {
        manager.mark_healthy(endpoint).await;
    }

    // All should be healthy
    for endpoint in &endpoints {
        assert!(manager.is_healthy(endpoint).await);
    }

    // Mark one as unhealthy
    manager
        .mark_unhealthy(&endpoints[1], "connection lost")
        .await;

    // Verify health states
    assert!(manager.is_healthy(&endpoints[0]).await);
    assert!(!manager.is_healthy(&endpoints[1]).await);
    assert!(manager.is_healthy(&endpoints[2]).await);
}

/// Test ConnectionPoolConfig builder pattern
#[tokio::test]
async fn test_connection_pool_config_builder() {
    let config = ConnectionPoolConfig::new()
        .with_max_connections(20)
        .with_connect_timeout(Duration::from_secs(10))
        .with_request_timeout(Duration::from_secs(60))
        .with_idle_timeout(Duration::from_secs(600))
        .with_tls(true);

    assert_eq!(config.max_connections_per_node, 20);
    assert_eq!(config.connect_timeout, Duration::from_secs(10));
    assert_eq!(config.request_timeout, Duration::from_secs(60));
    assert_eq!(config.idle_timeout, Duration::from_secs(600));
    assert!(config.use_tls);
}

/// Test CachedHealth expiry logic
#[tokio::test]
async fn test_cached_health_expiry() {
    let mut health = CachedHealth::healthy();
    assert_eq!(health.status, ServingStatus::Serving);
    assert_eq!(health.consecutive_failures, 0);

    // Should not be expired immediately
    assert!(!health.is_expired(Duration::from_secs(1)));

    // Record a failure
    health.record_failure("test error");
    assert_eq!(health.status, ServingStatus::NotServing);
    assert_eq!(health.consecutive_failures, 1);
    assert!(health.last_error.is_some());

    // Record success
    health.record_success();
    assert_eq!(health.status, ServingStatus::Serving);
    assert_eq!(health.consecutive_failures, 0);
    assert!(health.last_error.is_none());
}

// ============================================================================
// C5: CIRCUIT BREAKER TESTING
// ============================================================================

/// Test circuit breaker initial state
#[tokio::test]
async fn test_circuit_breaker_initial_state() {
    let cb = CircuitBreaker::new(5, Duration::from_secs(30));

    assert_eq!(cb.state(), CircuitState::Closed);
    assert!(cb.is_closed());
    assert!(!cb.is_open());
    assert!(cb.should_allow_request());
    assert_eq!(cb.failure_count(), 0);
}

/// Test circuit breaker Closed -> Open transition
#[tokio::test]
async fn test_circuit_breaker_closed_to_open() {
    let cb = CircuitBreaker::new(3, Duration::from_secs(30));

    // Record failures until threshold
    cb.record_failure();
    assert_eq!(cb.state(), CircuitState::Closed);
    assert_eq!(cb.failure_count(), 1);

    cb.record_failure();
    assert_eq!(cb.state(), CircuitState::Closed);
    assert_eq!(cb.failure_count(), 2);

    cb.record_failure();
    // Should now be open
    assert_eq!(cb.state(), CircuitState::Open);
    assert!(cb.is_open());
}

/// Test circuit breaker rejects requests when open
#[tokio::test]
async fn test_circuit_breaker_rejects_when_open() {
    let cb = CircuitBreaker::new(1, Duration::from_secs(60));

    cb.record_failure(); // Opens the circuit
    assert_eq!(cb.state(), CircuitState::Open);
    assert!(!cb.should_allow_request()); // Should reject
}

/// Test circuit breaker Open -> HalfOpen transition
#[tokio::test]
async fn test_circuit_breaker_open_to_half_open() {
    let cb = CircuitBreaker::new(1, Duration::from_millis(10));

    cb.record_failure(); // Opens
    assert!(cb.is_open());

    // Wait for reset timeout
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Should transition to half-open when request is allowed
    assert!(cb.should_allow_request());
    assert_eq!(cb.state(), CircuitState::HalfOpen);
}

/// Test circuit breaker HalfOpen -> Closed transition (on success)
#[tokio::test]
async fn test_circuit_breaker_half_open_to_closed() {
    let cb = CircuitBreaker::new(1, Duration::from_millis(1));

    cb.record_failure(); // Opens
    assert!(cb.is_open());

    // Wait for reset timeout
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Transition to half-open
    assert!(cb.should_allow_request());
    assert_eq!(cb.state(), CircuitState::HalfOpen);

    // Success should close
    cb.record_success();
    assert!(cb.is_closed());
    assert_eq!(cb.failure_count(), 0);
}

/// Test circuit breaker HalfOpen -> Open transition (on failure)
#[tokio::test]
async fn test_circuit_breaker_half_open_to_open() {
    let cb = CircuitBreaker::new(1, Duration::from_millis(1));

    cb.record_failure(); // Opens

    // Wait for reset timeout
    tokio::time::sleep(Duration::from_millis(10)).await;

    cb.should_allow_request(); // Transitions to half-open
    assert_eq!(cb.state(), CircuitState::HalfOpen);

    // Failure should reopen
    cb.record_failure();
    assert!(cb.is_open());
}

/// Test circuit breaker success resets failure count
#[tokio::test]
async fn test_circuit_breaker_success_resets_failures() {
    let cb = CircuitBreaker::new(5, Duration::from_secs(30));

    cb.record_failure();
    cb.record_failure();
    assert_eq!(cb.failure_count(), 2);

    cb.record_success();
    assert_eq!(cb.failure_count(), 0);
}

/// Test circuit breaker manual reset
#[tokio::test]
async fn test_circuit_breaker_manual_reset() {
    let cb = CircuitBreaker::new(1, Duration::from_secs(60));

    cb.record_failure(); // Opens
    assert!(cb.is_open());

    cb.reset();
    assert!(cb.is_closed());
    assert_eq!(cb.failure_count(), 0);
}

/// Test circuit breaker force open
#[tokio::test]
async fn test_circuit_breaker_force_open() {
    let cb = CircuitBreaker::new(100, Duration::from_secs(30));

    assert!(cb.is_closed());
    cb.force_open();
    assert!(cb.is_open());
}

/// Test circuit breaker with custom success threshold
#[tokio::test]
async fn test_circuit_breaker_custom_success_threshold() {
    let cb = CircuitBreaker::new(1, Duration::from_millis(1)).with_success_threshold(3);

    cb.record_failure(); // Opens
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Transition to half-open
    cb.should_allow_request();
    assert_eq!(cb.state(), CircuitState::HalfOpen);

    // First success - still half-open
    cb.record_success();
    assert_eq!(cb.state(), CircuitState::HalfOpen);
    assert_eq!(cb.success_count(), 1);

    // Second success - still half-open
    cb.record_success();
    assert_eq!(cb.state(), CircuitState::HalfOpen);
    assert_eq!(cb.success_count(), 2);

    // Third success - should close
    cb.record_success();
    assert!(cb.is_closed());
}

// ============================================================================
// C6: RETRY POLICY TESTING
// ============================================================================

/// Test retry policy default settings
#[tokio::test]
async fn test_retry_policy_default() {
    let policy = RetryPolicy::default();

    assert_eq!(policy.max_retries, 3);
    assert!(policy.exponential_backoff);
    assert_eq!(policy.backoff_multiplier, 2.0);
    assert_eq!(policy.base_delay, Duration::from_millis(100));
    assert!(policy.retry_on_timeout);
    assert!(policy.retry_on_connection_error);
}

/// Test retry policy no retry mode
#[tokio::test]
async fn test_retry_policy_no_retry() {
    let policy = RetryPolicy::no_retry();
    assert_eq!(policy.max_retries, 0);
}

/// Test retry policy aggressive mode
#[tokio::test]
async fn test_retry_policy_aggressive() {
    let policy = RetryPolicy::aggressive();
    assert_eq!(policy.max_retries, 5);
    assert_eq!(policy.base_delay, Duration::from_millis(50));
}

/// Test retry policy conservative mode
#[tokio::test]
async fn test_retry_policy_conservative() {
    let policy = RetryPolicy::conservative();
    assert_eq!(policy.max_retries, 2);
    assert_eq!(policy.base_delay, Duration::from_millis(500));
}

/// Test exponential backoff delay calculation
#[tokio::test]
async fn test_retry_policy_exponential_backoff() {
    let policy = RetryPolicy::default()
        .with_base_delay(Duration::from_millis(100))
        .with_jitter(0.0); // Disable jitter for predictable testing

    let delay0 = policy.compute_delay(0);
    let delay1 = policy.compute_delay(1);
    let delay2 = policy.compute_delay(2);

    assert_eq!(delay0, Duration::from_millis(100)); // 100 * 2^0
    assert_eq!(delay1, Duration::from_millis(200)); // 100 * 2^1
    assert_eq!(delay2, Duration::from_millis(400)); // 100 * 2^2
}

/// Test constant delay (no exponential backoff)
#[tokio::test]
async fn test_retry_policy_constant_delay() {
    let policy = RetryPolicy::default()
        .with_exponential_backoff(false)
        .with_jitter(0.0);

    let delay0 = policy.compute_delay(0);
    let delay1 = policy.compute_delay(1);
    let delay2 = policy.compute_delay(2);

    assert_eq!(delay0, policy.base_delay);
    assert_eq!(delay1, policy.base_delay);
    assert_eq!(delay2, policy.base_delay);
}

/// Test max delay cap
#[tokio::test]
async fn test_retry_policy_max_delay_cap() {
    let policy = RetryPolicy::default()
        .with_max_retries(10) // Allow more retries so we can test higher attempts
        .with_base_delay(Duration::from_secs(1))
        .with_max_delay(Duration::from_secs(5))
        .with_jitter(0.0);

    // At attempt 3, delay would be 1 * 2^3 = 8 seconds, but capped at 5
    let delay3 = policy.compute_delay(3);
    assert_eq!(delay3, Duration::from_secs(5));

    // At attempt 5, delay would be 1 * 2^5 = 32 seconds, but capped at 5
    let delay5 = policy.compute_delay(5);
    assert_eq!(delay5, Duration::from_secs(5));
}

/// Test jitter adds randomness within bounds
#[tokio::test]
async fn test_retry_policy_jitter() {
    let policy = RetryPolicy::default()
        .with_base_delay(Duration::from_millis(100))
        .with_jitter(0.2); // 20% jitter

    // Collect multiple samples
    let mut delays: Vec<Duration> = Vec::new();
    for _ in 0..100 {
        delays.push(policy.compute_delay(0));
    }

    // All should be within expected range (80-120ms for base 100ms with 20% jitter)
    for delay in &delays {
        let ms = delay.as_millis();
        assert!(
            ms >= 80 && ms <= 120,
            "Delay {}ms outside expected range 80-120ms",
            ms
        );
    }

    // Should have some variance (not all identical)
    let unique_values: std::collections::HashSet<_> = delays.iter().collect();
    assert!(
        unique_values.len() > 1,
        "Jitter should produce varied delays"
    );
}

/// Test retry executor success on first attempt
#[tokio::test]
async fn test_retry_executor_immediate_success() {
    let executor = RetryExecutor::new(
        RetryPolicy::default(),
        Arc::new(CircuitBreaker::new(5, Duration::from_secs(30))),
    );

    let call_count = Arc::new(AtomicU32::new(0));
    let call_count_clone = call_count.clone();

    let result = executor
        .execute(|| {
            let count = call_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Ok::<_, RpcError>(42)
            }
        })
        .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 42);
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

/// Test retry executor retries on transient failures
#[tokio::test]
async fn test_retry_executor_retries_on_failure() {
    let executor = RetryExecutor::new(
        RetryPolicy::default()
            .with_max_retries(3)
            .with_base_delay(Duration::from_millis(1)),
        Arc::new(CircuitBreaker::new(10, Duration::from_secs(30))),
    );

    let call_count = Arc::new(AtomicU32::new(0));
    let call_count_clone = call_count.clone();

    let result = executor
        .execute(|| {
            let count = call_count_clone.clone();
            async move {
                let n = count.fetch_add(1, Ordering::SeqCst) + 1;
                if n < 3 {
                    Err(RpcError::connection("temporary failure"))
                } else {
                    Ok::<_, RpcError>(42)
                }
            }
        })
        .await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), 42);
    // Should have been called 3 times (2 failures + 1 success)
    assert_eq!(call_count.load(Ordering::SeqCst), 3);
}

/// Test retry executor respects circuit breaker
#[tokio::test]
async fn test_retry_executor_circuit_breaker_open() {
    let cb = Arc::new(CircuitBreaker::new(1, Duration::from_secs(60)));
    cb.force_open();

    let executor = RetryExecutor::new(RetryPolicy::default(), cb);

    let result = executor.execute(|| async { Ok::<_, RpcError>(42) }).await;

    assert!(result.is_err());
    assert!(result.unwrap_err().message().contains("Circuit breaker"));
}

/// Test retry executor exhausts retries
#[tokio::test]
async fn test_retry_executor_exhausts_retries() {
    let executor = RetryExecutor::new(
        RetryPolicy::default()
            .with_max_retries(2)
            .with_base_delay(Duration::from_millis(1)),
        Arc::new(CircuitBreaker::new(10, Duration::from_secs(30))),
    );

    let call_count = Arc::new(AtomicU32::new(0));
    let call_count_clone = call_count.clone();

    let result = executor
        .execute(|| {
            let count = call_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err::<i32, _>(RpcError::connection("permanent failure"))
            }
        })
        .await;

    assert!(result.is_err());
    // Initial + 2 retries = 3 calls
    assert_eq!(call_count.load(Ordering::SeqCst), 3);
}

/// Test retry timing with exponential backoff
#[tokio::test]
async fn test_retry_executor_backoff_timing() {
    let executor = RetryExecutor::new(
        RetryPolicy::default()
            .with_max_retries(2)
            .with_base_delay(Duration::from_millis(50))
            .with_jitter(0.0), // Disable jitter for predictable timing
        Arc::new(CircuitBreaker::new(10, Duration::from_secs(30))),
    );

    let call_count = Arc::new(AtomicU32::new(0));
    let call_count_clone = call_count.clone();

    let start = Instant::now();

    let _ = executor
        .execute(|| {
            let count = call_count_clone.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                Err::<i32, _>(RpcError::connection("failure"))
            }
        })
        .await;

    let elapsed = start.elapsed();

    // Should have waited: 50ms (first retry) + 100ms (second retry) = 150ms minimum
    assert!(
        elapsed >= Duration::from_millis(140), // Allow small margin
        "Expected at least 140ms, got {:?}",
        elapsed
    );
}

// ============================================================================
// C7: RAFT CONSENSUS WITH TRANSPORT INTEGRATION
// ============================================================================

/// Test RaftConsensus creation with transport
#[tokio::test]
async fn test_raft_consensus_with_transport_creation() {
    let transport = Arc::new(MockConsensusTransport::new());
    let peers = vec![
        NodeEndpoint::new("node-2", "127.0.0.1:5680"),
        NodeEndpoint::new("node-3", "127.0.0.1:5681"),
    ];

    let consensus = RaftConsensus::with_transport(
        ConsensusConfig::default(),
        "node-1",
        transport,
        peers.clone(),
    );

    assert!(consensus.is_ok());
    let consensus = consensus.unwrap();
    assert_eq!(consensus.node_id(), "node-1");

    let stored_peers = consensus.get_peers().await;
    assert_eq!(stored_peers.len(), 2);
}

/// Test adding and removing peers dynamically
#[tokio::test]
async fn test_raft_consensus_dynamic_peer_management() {
    let transport = Arc::new(MockConsensusTransport::new());
    let consensus =
        RaftConsensus::with_transport(ConsensusConfig::default(), "node-1", transport, vec![])
            .unwrap();

    // Add peers
    consensus
        .add_peer(NodeEndpoint::new("node-2", "127.0.0.1:5680"))
        .await;
    consensus
        .add_peer(NodeEndpoint::new("node-3", "127.0.0.1:5681"))
        .await;

    let peers = consensus.get_peers().await;
    assert_eq!(peers.len(), 2);

    // Remove a peer
    consensus.remove_peer("node-2").await;
    let peers = consensus.get_peers().await;
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].node_id, "node-3");
}

/// Test consensus start/stop lifecycle
#[tokio::test]
async fn test_raft_consensus_start_stop_lifecycle() {
    let transport = Arc::new(MockConsensusTransport::new());
    let peers = vec![NodeEndpoint::new("node-2", "127.0.0.1:5680")];

    let config = ConsensusConfig {
        election_timeout_ms: (1000, 2000), // Long timeout to prevent election during test
        heartbeat_interval_ms: 500,
        ..Default::default()
    };

    let mut consensus =
        RaftConsensus::with_transport(config, "node-1", transport, peers).unwrap();

    // Start should succeed
    let result = consensus.start().await;
    assert!(result.is_ok());

    // Stop should succeed
    let result = consensus.stop().await;
    assert!(result.is_ok());
}

/// Test propose command when leader
#[tokio::test]
async fn test_raft_consensus_propose_as_leader() {
    let transport = Arc::new(MockConsensusTransport::new());
    let consensus =
        RaftConsensus::with_transport(ConsensusConfig::default(), "node-1", transport, vec![])
            .unwrap();

    // Become leader
    let election_result = consensus.start_election().await;
    assert!(election_result.is_ok());
    assert!(election_result.unwrap());
    assert_eq!(consensus.get_state().await, ConsensusState::Leader);

    // Propose a command
    let command = Command::CreateCollection {
        collection_id: "col-1".to_string(),
        name: "Test Collection".to_string(),
        dimension: 128,
        shard_count: 3,
    };

    let result = consensus.propose(command).await;
    assert!(result.is_ok());
    let apply_result = result.unwrap();
    assert!(apply_result.success);
}

/// Test propose command when not leader
#[tokio::test]
async fn test_raft_consensus_propose_as_follower() {
    let transport = Arc::new(MockConsensusTransport::new());
    let consensus =
        RaftConsensus::with_transport(ConsensusConfig::default(), "node-1", transport, vec![])
            .unwrap();

    // Don't become leader - stay as follower
    assert_eq!(consensus.get_state().await, ConsensusState::Follower);

    // Propose should fail
    let command = Command::Noop;
    let result = consensus.propose(command).await;
    assert!(result.is_ok());
    let apply_result = result.unwrap();
    assert!(!apply_result.success);
    assert!(apply_result.error.is_some());
    assert!(apply_result.error.unwrap().contains("Not the leader"));
}

/// Test random election timeout
#[tokio::test]
async fn test_raft_consensus_random_election_timeout() {
    let config = ConsensusConfig {
        election_timeout_ms: (100, 200),
        ..Default::default()
    };
    let transport = Arc::new(MockConsensusTransport::new());
    let consensus = RaftConsensus::with_transport(config, "node-1", transport, vec![]).unwrap();

    // Generate multiple timeouts and verify they're in range
    for _ in 0..100 {
        let timeout = consensus.random_election_timeout();
        let ms = timeout.as_millis() as u64;
        assert!(
            ms >= 100 && ms <= 200,
            "Timeout {}ms outside expected range 100-200ms",
            ms
        );
    }
}

/// Test get_log_entries and last_log_info
#[tokio::test]
async fn test_raft_consensus_log_operations() {
    let transport = Arc::new(MockConsensusTransport::new());
    let consensus =
        RaftConsensus::with_transport(ConsensusConfig::default(), "node-1", transport, vec![])
            .unwrap();

    // Initially empty
    let (last_idx, last_term) = consensus.last_log_info().await;
    assert_eq!(last_idx, 0);
    assert_eq!(last_term, 0);

    // Become leader and propose
    let _ = consensus.start_election().await;
    let _ = consensus.propose(Command::Noop).await;

    // Now should have entries
    let (last_idx, _last_term) = consensus.last_log_info().await;
    assert!(last_idx > 0);
}

// ============================================================================
// ADDITIONAL INTEGRATION TESTS
// ============================================================================

/// Test mock implementations can be used with NodeClient
#[tokio::test]
async fn test_mock_implementations_with_node_client() {
    let client = NodeClient::new(
        MockConsensusTransport::new(),
        MockReplicationSink::new(),
        MockSearchFanout::new(),
        MockHealthChecker::new(),
    );

    let target = NodeEndpoint::new("node-1", "127.0.0.1:5679");

    // Test consensus
    let vote_req = RequestVoteRequest {
        term: 1,
        candidate_id: "node-2".to_string(),
        last_log_index: 0,
        last_log_term: 0,
    };
    let vote_resp = client.consensus().request_vote(&target, vote_req).await;
    assert!(vote_resp.is_ok());
    assert!(vote_resp.unwrap().vote_granted);

    // Test health
    let health_resp = client
        .health()
        .check(&target, HealthCheckRequest::default())
        .await;
    assert!(health_resp.is_ok());
    assert_eq!(health_resp.unwrap().status, ServingStatus::Serving);
}

/// Test full 3-node election simulation
#[tokio::test]
async fn test_full_3node_election_simulation() {
    let transport = Arc::new(MockConsensusTransport::new());

    let config = ConsensusConfig {
        enable_pre_vote: false,
        election_timeout_ms: (50, 100),
        heartbeat_interval_ms: 25,
        ..Default::default()
    };

    // Node 1 starts election
    let consensus = RaftConsensus::with_transport(
        config.clone(),
        "node-1",
        transport.clone(),
        vec![
            NodeEndpoint::new("node-2", "127.0.0.1:5680"),
            NodeEndpoint::new("node-3", "127.0.0.1:5681"),
        ],
    )
    .unwrap();

    // Node 1 starts election and becomes leader
    let result = consensus.start_election().await;
    assert!(result.is_ok());
    assert!(result.unwrap());
    assert_eq!(consensus.get_state().await, ConsensusState::Leader);

    // Verify vote count
    assert_eq!(transport.vote_count.load(Ordering::SeqCst), 2);

    // Reset for heartbeat test
    transport.vote_count.store(0, Ordering::SeqCst);
    transport.append_count.store(0, Ordering::SeqCst);

    // Leader sends heartbeat
    let hb_result = consensus.send_heartbeat().await;
    assert!(hb_result.is_ok());
    assert_eq!(transport.append_count.load(Ordering::SeqCst), 2);
}

/// Test connection manager cleanup of idle connections
#[tokio::test]
async fn test_connection_manager_cleanup_idle() {
    let config = ConnectionPoolConfig::default().with_idle_timeout(Duration::from_millis(10));

    let manager = ConnectionManager::new(config);

    // Add some endpoints by marking them healthy
    let endpoint1 = NodeEndpoint::new("node-1", "127.0.0.1:5679");
    let endpoint2 = NodeEndpoint::new("node-2", "127.0.0.1:5680");

    manager.mark_healthy(&endpoint1).await;
    manager.mark_healthy(&endpoint2).await;

    // Initially both should be healthy
    assert!(manager.is_healthy(&endpoint1).await);
    assert!(manager.is_healthy(&endpoint2).await);

    // Close specific endpoint
    manager.close_endpoint(&endpoint1);

    // Close all
    manager.close_all();

    // Stats should show no active pools
    let stats = manager.stats();
    assert_eq!(stats.active_pools, 0);
}

/// Test circuit breaker integration with retry executor under load
#[tokio::test]
async fn test_circuit_breaker_opens_under_load() {
    let cb = Arc::new(CircuitBreaker::new(3, Duration::from_secs(60)));
    let executor = RetryExecutor::new(
        RetryPolicy::default()
            .with_max_retries(1)
            .with_base_delay(Duration::from_millis(1)),
        cb.clone(),
    );

    // Simulate multiple failed requests
    for _ in 0..5 {
        let _ = executor
            .execute(|| async { Err::<i32, _>(RpcError::connection("failure")) })
            .await;
    }

    // Circuit should be open now
    assert!(cb.is_open());

    // Further requests should fail fast
    let result = executor.execute(|| async { Ok::<_, RpcError>(42) }).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().message().contains("Circuit breaker"));
}

/// Test pre-vote prevents disruption
#[tokio::test]
async fn test_prevote_prevents_disruption() {
    // Create transport that will reject pre-votes
    let transport = Arc::new(MockConsensusTransport::with_votes(false));
    let peers = vec![
        NodeEndpoint::new("node-2", "127.0.0.1:5680"),
        NodeEndpoint::new("node-3", "127.0.0.1:5681"),
    ];

    let config = ConsensusConfig {
        enable_pre_vote: true, // Enable pre-vote
        ..Default::default()
    };

    let consensus =
        RaftConsensus::with_transport(config, "node-1", transport.clone(), peers).unwrap();

    // Election should fail during pre-vote phase
    let result = consensus.start_election().await;
    assert!(result.is_ok());
    assert!(!result.unwrap()); // Should NOT become leader

    // Should revert to follower
    assert_eq!(consensus.get_state().await, ConsensusState::Follower);
}
