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

//! SOLID-Compliant RPC Traits for Inter-Node Communication
//!
//! This module provides trait definitions following SOLID principles:
//!
//! - **Single Responsibility**: Each trait handles one specific concern
//! - **Open/Closed**: Traits can be extended without modifying existing code
//! - **Liskov Substitution**: Implementations are interchangeable
//! - **Interface Segregation**: Clients depend only on traits they use
//! - **Dependency Inversion**: High-level modules depend on abstractions
//!
//! ## Trait Hierarchy
//!
//! ```text
//! NodeClient (composite struct)
//! ├── ConsensusTransport  (Raft consensus operations)
//! ├── ReplicationSink     (Data replication operations)
//! ├── SearchFanout        (Distributed search operations)
//! └── HealthChecker       (Node health monitoring)
//! ```
//!
//! ## Usage
//!
//! Cluster modules depend on these traits, not concrete implementations.
//! This allows for:
//! - Easy testing with mock implementations
//! - Swapping transport layers (gRPC, HTTP, in-memory)
//! - Independent development of components

use async_trait::async_trait;
use futures::Stream;
use std::pin::Pin;

use super::error::RpcResult;
use super::types::*;

// ============================================================================
// CONSENSUS TRANSPORT TRAIT
// ============================================================================

/// Transport layer for Raft consensus operations
///
/// Implements the RPC layer for Raft consensus protocol. This trait
/// abstracts the network transport, allowing the consensus module to
/// work with any implementation (gRPC, HTTP, mock, etc.).
///
/// # Thread Safety
///
/// All implementations must be `Send + Sync + 'static` to allow safe
/// sharing across async tasks and threads.
///
/// # Example
///
/// ```ignore
/// async fn run_election(transport: &impl ConsensusTransport, peers: &[NodeEndpoint]) {
///     for peer in peers {
///         let req = RequestVoteRequest { /* ... */ };
///         match transport.request_vote(peer, req).await {
///             Ok(resp) if resp.vote_granted => { /* count vote */ }
///             _ => { /* handle rejection */ }
///         }
///     }
/// }
/// ```
#[async_trait]
pub trait ConsensusTransport: Send + Sync + 'static {
    /// Send a RequestVote RPC to a peer
    ///
    /// Called by candidates during leader election to request votes
    /// from other nodes.
    async fn request_vote(
        &self,
        target: &NodeEndpoint,
        req: RequestVoteRequest,
    ) -> RpcResult<RequestVoteResponse>;

    /// Send an AppendEntries RPC to a peer
    ///
    /// Called by the leader to replicate log entries and as a heartbeat
    /// to maintain leadership.
    async fn append_entries(
        &self,
        target: &NodeEndpoint,
        req: AppendEntriesRequest,
    ) -> RpcResult<AppendEntriesResponse>;

    /// Send an InstallSnapshot RPC to a peer
    ///
    /// Called by the leader to send a snapshot to a follower that has
    /// fallen too far behind in the log.
    async fn install_snapshot(
        &self,
        target: &NodeEndpoint,
        req: InstallSnapshotRequest,
    ) -> RpcResult<InstallSnapshotResponse>;

    /// Send a PreVote RPC to a peer (Raft extension)
    ///
    /// Called before starting an election to check if the node would
    /// get enough votes without disrupting the cluster.
    async fn pre_vote(
        &self,
        target: &NodeEndpoint,
        req: RequestVoteRequest,
    ) -> RpcResult<RequestVoteResponse>;
}

// ============================================================================
// REPLICATION SINK TRAIT
// ============================================================================

/// Sink for data replication to shard replicas
///
/// This trait handles the actual data replication separate from Raft
/// consensus. While Raft handles cluster metadata, this trait handles
/// the replication of vector data to shard replicas.
///
/// # Streaming
///
/// For high-throughput scenarios, use `replicate_stream` which provides
/// bidirectional streaming for continuous replication.
#[async_trait]
pub trait ReplicationSink: Send + Sync + 'static {
    /// Replicate a single entry to a replica
    ///
    /// Sends a replication entry and waits for acknowledgment.
    /// Use this for low-volume or critical writes that need
    /// immediate confirmation.
    async fn replicate(
        &self,
        target: &NodeEndpoint,
        req: ReplicateRequest,
    ) -> RpcResult<ReplicateResponse>;

    /// Replicate a batch of entries to a replica
    ///
    /// More efficient than multiple `replicate` calls for bulk operations.
    /// Returns responses in the same order as requests.
    async fn replicate_batch(
        &self,
        target: &NodeEndpoint,
        requests: Vec<ReplicateRequest>,
    ) -> RpcResult<Vec<ReplicateResponse>>;

    /// Open a streaming replication channel
    ///
    /// Returns a stream of responses for each request sent.
    /// The implementation should handle backpressure appropriately.
    ///
    /// # Returns
    ///
    /// A boxed stream of replication responses
    async fn replicate_stream(
        &self,
        target: &NodeEndpoint,
        requests: Pin<Box<dyn Stream<Item = ReplicateRequest> + Send>>,
    ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<ReplicateResponse>> + Send>>>;

    /// Pull missed entries from a peer (catch-up replication)
    ///
    /// Called by a replica to catch up after being offline or behind.
    async fn pull_entries(
        &self,
        target: &NodeEndpoint,
        req: PullEntriesRequest,
    ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<ReplicateRequest>> + Send>>>;

    /// Acknowledge successful replication (for async replication)
    ///
    /// Used in async replication mode to acknowledge that entries
    /// have been durably stored.
    async fn ack_replication(
        &self,
        target: &NodeEndpoint,
        req: AckReplicationRequest,
    ) -> RpcResult<AckReplicationResponse>;
}

// ============================================================================
// SEARCH FANOUT TRAIT
// ============================================================================

/// Fanout layer for distributed search and write operations
///
/// This trait implements the scatter-gather pattern for distributed
/// queries. The coordinator fans out requests to relevant shards and
/// merges the results.
///
/// # Write Forwarding
///
/// When a node receives a write for a shard it doesn't own, it uses
/// `forward_write` to send the write to the correct primary.
#[async_trait]
pub trait SearchFanout: Send + Sync + 'static {
    /// Execute a search on a specific shard
    ///
    /// Called by the coordinator to search a single shard.
    /// Results are merged by the coordinator.
    async fn shard_search(
        &self,
        target: &NodeEndpoint,
        req: ShardSearchRequest,
    ) -> RpcResult<ShardSearchResponse>;

    /// Stream search results from a shard
    ///
    /// For large result sets, streaming avoids memory pressure
    /// on both coordinator and shard nodes.
    async fn shard_search_stream(
        &self,
        target: &NodeEndpoint,
        req: ShardSearchRequest,
    ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<ShardSearchResult>> + Send>>>;

    /// Forward a write to the shard's primary
    ///
    /// Called when a node receives a write for a shard owned
    /// by another node.
    async fn forward_write(
        &self,
        target: &NodeEndpoint,
        req: ForwardWriteRequest,
    ) -> RpcResult<ForwardWriteResponse>;

    /// Forward a batch of writes
    ///
    /// More efficient when forwarding multiple writes to the same node.
    async fn forward_write_batch(
        &self,
        target: &NodeEndpoint,
        requests: Vec<ForwardWriteRequest>,
    ) -> RpcResult<Vec<ForwardWriteResponse>>;
}

// ============================================================================
// HEALTH CHECKER TRAIT
// ============================================================================

/// Health checking for inter-node communication
///
/// This trait provides health monitoring compatible with the gRPC
/// Health Checking Protocol, with extensions for detailed status.
#[async_trait]
pub trait HealthChecker: Send + Sync + 'static {
    /// Perform a basic health check
    ///
    /// Returns the serving status of the target node.
    /// Compatible with gRPC Health Checking Protocol.
    async fn check(
        &self,
        target: &NodeEndpoint,
        req: HealthCheckRequest,
    ) -> RpcResult<HealthCheckResponse>;

    /// Get detailed status information
    ///
    /// Returns comprehensive status including metrics, shard info,
    /// and replication lag.
    async fn status(&self, target: &NodeEndpoint, req: StatusRequest) -> RpcResult<StatusResponse>;

    /// Watch health status changes
    ///
    /// Returns a stream of health updates. The stream continues
    /// until the connection is closed or the target becomes unavailable.
    async fn watch(
        &self,
        target: &NodeEndpoint,
        req: HealthCheckRequest,
    ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<HealthCheckResponse>> + Send>>>;
}

// ============================================================================
// NODE CLIENT COMPOSITE
// ============================================================================

/// Composite client for all RPC operations to a cluster node
///
/// This struct composes all the individual RPC traits, providing a
/// single entry point for inter-node communication. It follows the
/// Dependency Inversion Principle by accepting trait objects.
///
/// # Example
///
/// ```ignore
/// let client = NodeClient::new(
///     Arc::new(GrpcConsensusTransport::new()),
///     Arc::new(GrpcReplicationSink::new()),
///     Arc::new(GrpcSearchFanout::new()),
///     Arc::new(GrpcHealthChecker::new()),
/// );
///
/// // Use specific capabilities
/// client.consensus().request_vote(&target, req).await?;
/// client.health().check(&target, req).await?;
/// ```
pub struct NodeClient {
    consensus: Box<dyn ConsensusTransport>,
    replication: Box<dyn ReplicationSink>,
    search: Box<dyn SearchFanout>,
    health: Box<dyn HealthChecker>,
}

impl NodeClient {
    /// Create a new NodeClient with the provided implementations
    pub fn new(
        consensus: impl ConsensusTransport,
        replication: impl ReplicationSink,
        search: impl SearchFanout,
        health: impl HealthChecker,
    ) -> Self {
        Self {
            consensus: Box::new(consensus),
            replication: Box::new(replication),
            search: Box::new(search),
            health: Box::new(health),
        }
    }

    /// Get the consensus transport
    pub fn consensus(&self) -> &dyn ConsensusTransport {
        &*self.consensus
    }

    /// Get the replication sink
    pub fn replication(&self) -> &dyn ReplicationSink {
        &*self.replication
    }

    /// Get the search fanout
    pub fn search(&self) -> &dyn SearchFanout {
        &*self.search
    }

    /// Get the health checker
    pub fn health(&self) -> &dyn HealthChecker {
        &*self.health
    }
}

// ============================================================================
// OPTIONAL: CONNECTION POOL TRAIT
// ============================================================================

/// Connection pool for managing connections to cluster nodes
///
/// This trait is optional and can be used to manage connection pooling
/// for the RPC layer. Implementations should handle connection lifecycle,
/// health checking, and load balancing.
#[async_trait]
pub trait ConnectionPool: Send + Sync + 'static {
    /// Get a client for a specific node
    ///
    /// Returns a client that can be used to communicate with the target node.
    /// The implementation should handle connection pooling and reuse.
    async fn get_client(&self, target: &NodeEndpoint) -> RpcResult<NodeClient>;

    /// Mark a node as unhealthy
    ///
    /// Called when a node is detected as unhealthy. The implementation
    /// should update its internal state and potentially reconnect.
    async fn mark_unhealthy(&self, target: &NodeEndpoint);

    /// Get all known healthy nodes
    ///
    /// Returns endpoints for all nodes currently considered healthy.
    async fn healthy_nodes(&self) -> Vec<NodeEndpoint>;

    /// Refresh the connection pool
    ///
    /// Called periodically to refresh connections and update health status.
    async fn refresh(&self) -> RpcResult<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Mock implementation for testing
    struct MockConsensusTransport {
        vote_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ConsensusTransport for MockConsensusTransport {
        async fn request_vote(
            &self,
            _target: &NodeEndpoint,
            _req: RequestVoteRequest,
        ) -> RpcResult<RequestVoteResponse> {
            self.vote_count.fetch_add(1, Ordering::SeqCst);
            Ok(RequestVoteResponse {
                term: 1,
                vote_granted: true,
            })
        }

        async fn append_entries(
            &self,
            _target: &NodeEndpoint,
            _req: AppendEntriesRequest,
        ) -> RpcResult<AppendEntriesResponse> {
            Ok(AppendEntriesResponse {
                term: 1,
                success: true,
                match_index: None,
                conflict_term: None,
                conflict_index: None,
            })
        }

        async fn install_snapshot(
            &self,
            _target: &NodeEndpoint,
            _req: InstallSnapshotRequest,
        ) -> RpcResult<InstallSnapshotResponse> {
            Ok(InstallSnapshotResponse {
                term: 1,
                bytes_stored: 1024,
            })
        }

        async fn pre_vote(
            &self,
            _target: &NodeEndpoint,
            _req: RequestVoteRequest,
        ) -> RpcResult<RequestVoteResponse> {
            Ok(RequestVoteResponse {
                term: 1,
                vote_granted: true,
            })
        }
    }

    struct MockReplicationSink;

    #[async_trait]
    impl ReplicationSink for MockReplicationSink {
        async fn replicate(
            &self,
            _target: &NodeEndpoint,
            req: ReplicateRequest,
        ) -> RpcResult<ReplicateResponse> {
            Ok(ReplicateResponse {
                node_id: "mock-node".to_string(),
                acked_lsn: req.lsn,
                success: true,
                error: None,
                latency: std::time::Duration::from_micros(100),
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
                    latency: std::time::Duration::from_micros(100),
                })
                .collect())
        }

        async fn replicate_stream(
            &self,
            _target: &NodeEndpoint,
            _requests: Pin<Box<dyn Stream<Item = ReplicateRequest> + Send>>,
        ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<ReplicateResponse>> + Send>>> {
            unimplemented!("Mock does not implement streaming")
        }

        async fn pull_entries(
            &self,
            _target: &NodeEndpoint,
            _req: PullEntriesRequest,
        ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<ReplicateRequest>> + Send>>> {
            unimplemented!("Mock does not implement streaming")
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

    struct MockSearchFanout;

    #[async_trait]
    impl SearchFanout for MockSearchFanout {
        async fn shard_search(
            &self,
            _target: &NodeEndpoint,
            req: ShardSearchRequest,
        ) -> RpcResult<ShardSearchResponse> {
            Ok(ShardSearchResponse {
                request_id: req.request_id,
                shard_id: req.shard_id,
                results: vec![],
                vectors_scanned: 0,
                latency: std::time::Duration::from_micros(500),
                truncated: false,
            })
        }

        async fn shard_search_stream(
            &self,
            _target: &NodeEndpoint,
            _req: ShardSearchRequest,
        ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<ShardSearchResult>> + Send>>> {
            unimplemented!("Mock does not implement streaming")
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
                latency: std::time::Duration::from_millis(5),
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
                    latency: std::time::Duration::from_millis(5),
                    error: None,
                })
                .collect())
        }
    }

    struct MockHealthChecker;

    #[async_trait]
    impl HealthChecker for MockHealthChecker {
        async fn check(
            &self,
            _target: &NodeEndpoint,
            _req: HealthCheckRequest,
        ) -> RpcResult<HealthCheckResponse> {
            Ok(HealthCheckResponse {
                status: ServingStatus::Serving,
            })
        }

        async fn status(
            &self,
            _target: &NodeEndpoint,
            _req: StatusRequest,
        ) -> RpcResult<StatusResponse> {
            Ok(StatusResponse {
                node_id: "mock-node".to_string(),
                role: NodeRole::Follower,
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
            unimplemented!("Mock does not implement streaming")
        }
    }

    #[tokio::test]
    async fn test_consensus_transport_trait() {
        let vote_count = Arc::new(AtomicUsize::new(0));
        let transport = MockConsensusTransport {
            vote_count: vote_count.clone(),
        };

        let target = NodeEndpoint::new("node-1", "127.0.0.1:5679");
        let req = RequestVoteRequest {
            term: 1,
            candidate_id: "node-2".to_string(),
            last_log_index: 0,
            last_log_term: 0,
        };

        let response = transport.request_vote(&target, req).await.unwrap();
        assert!(response.vote_granted);
        assert_eq!(vote_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_replication_sink_trait() {
        let sink = MockReplicationSink;
        let target = NodeEndpoint::new("node-1", "127.0.0.1:5679");

        let req = ReplicateRequest {
            source_node_id: "node-2".to_string(),
            shard_id: "shard-1".to_string(),
            lsn: 100,
            timestamp: 0,
            operation: ReplicationOperation::Insert,
            data: vec![],
            checksum: 0,
            consistency: ConsistencyLevel::Quorum,
            timeout: std::time::Duration::from_secs(5),
        };

        let response = sink.replicate(&target, req).await.unwrap();
        assert!(response.success);
        assert_eq!(response.acked_lsn, 100);
    }

    #[tokio::test]
    async fn test_search_fanout_trait() {
        let fanout = MockSearchFanout;
        let target = NodeEndpoint::new("node-1", "127.0.0.1:5679");

        let req = ShardSearchRequest {
            request_id: "req-1".to_string(),
            collection: "test".to_string(),
            shard_id: "shard-1".to_string(),
            vector: vec![0.1, 0.2, 0.3],
            top_k: 10,
            filter: None,
            params: SearchParams::default(),
            timeout: std::time::Duration::from_secs(5),
            include_vectors: false,
            tenant_id: None,
            domain_id: None,
        };

        let response = fanout.shard_search(&target, req).await.unwrap();
        assert_eq!(response.shard_id, "shard-1");
        assert!(!response.truncated);
    }

    #[tokio::test]
    async fn test_health_checker_trait() {
        let checker = MockHealthChecker;
        let target = NodeEndpoint::new("node-1", "127.0.0.1:5679");

        let response = checker
            .check(&target, HealthCheckRequest::default())
            .await
            .unwrap();
        assert_eq!(response.status, ServingStatus::Serving);

        let status = checker
            .status(&target, StatusRequest::default())
            .await
            .unwrap();
        assert_eq!(status.node_id, "mock-node");
        assert_eq!(status.role, NodeRole::Follower);
    }

    #[tokio::test]
    async fn test_node_client_composite() {
        let client = NodeClient::new(
            MockConsensusTransport {
                vote_count: Arc::new(AtomicUsize::new(0)),
            },
            MockReplicationSink,
            MockSearchFanout,
            MockHealthChecker,
        );

        let target = NodeEndpoint::new("node-1", "127.0.0.1:5679");

        // Test consensus
        let vote_req = RequestVoteRequest {
            term: 1,
            candidate_id: "node-2".to_string(),
            last_log_index: 0,
            last_log_term: 0,
        };
        let vote_resp = client
            .consensus()
            .request_vote(&target, vote_req)
            .await
            .unwrap();
        assert!(vote_resp.vote_granted);

        // Test health
        let health_resp = client
            .health()
            .check(&target, HealthCheckRequest::default())
            .await
            .unwrap();
        assert_eq!(health_resp.status, ServingStatus::Serving);
    }
}
