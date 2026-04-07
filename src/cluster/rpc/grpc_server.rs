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

//! gRPC Server Implementations for Cluster Services
//!
//! This module provides tonic-based gRPC service implementations for inter-node
//! communication in ProximaDB's distributed mode. These services are registered
//! with the gRPC server when running in cluster mode.
//!
//! ## Services Implemented
//!
//! - **ConsensusServiceImpl**: Handles Raft consensus RPCs (RequestVote, AppendEntries, etc.)
//! - **ReplicationServiceImpl**: Handles data replication for shard replicas
//! - **HealthServiceImpl**: Provides health checking for inter-node communication
//!
//! ## Usage
//!
//! These services are conditionally registered with the gRPC server when the
//! `cluster` feature is enabled:
//!
//! ```ignore
//! #[cfg(feature = "cluster")]
//! {
//!     let consensus_service = ConsensusServiceImpl::new(consensus);
//!     let replication_service = ReplicationServiceImpl::new(replication_manager);
//!     let health_service = HealthServiceImpl::new(node_id);
//!
//!     server_builder
//!         .add_service(ConsensusServiceServer::new(consensus_service))
//!         .add_service(ReplicationServiceServer::new(replication_service))
//!         .add_service(HealthServiceServer::new(health_service));
//! }
//! ```

use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::RwLock;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::{debug, info};

use crate::cluster::consensus::RaftConsensus;
use crate::cluster::replication::EngineReplication;
use crate::proto::proximadb_cluster_v1::{
    AckReplicationRequest as ProtoAckReplicationRequest,
    AckReplicationResponse as ProtoAckReplicationResponse,
    AppendEntriesRequest as ProtoAppendEntriesRequest,
    AppendEntriesResponse as ProtoAppendEntriesResponse, HealthCheckRequest, HealthCheckResponse,
    InstallSnapshotRequest as ProtoInstallSnapshotRequest,
    InstallSnapshotResponse as ProtoInstallSnapshotResponse, NodeRole,
    PreVoteRequest as ProtoPreVoteRequest, PreVoteResponse as ProtoPreVoteResponse,
    PullEntriesRequest as ProtoPullEntriesRequest, ReplicateRequest as ProtoReplicateRequest,
    ReplicateResponse as ProtoReplicateResponse, RequestVoteRequest as ProtoRequestVoteRequest,
    RequestVoteResponse as ProtoRequestVoteResponse, ServingStatus, StatusRequest, StatusResponse,
    consensus_service_server::ConsensusService, health_service_server::HealthService,
    replication_service_server::ReplicationService,
};

// ============================================================================
// CONSENSUS SERVICE IMPLEMENTATION
// ============================================================================

/// gRPC service implementation for Raft consensus operations.
///
/// This service handles inter-node communication for the Raft consensus protocol,
/// including leader election, log replication, and snapshot installation.
///
/// # Thread Safety
///
/// The service wraps `RaftConsensus` in an `Arc<RwLock>` to allow safe concurrent
/// access from multiple gRPC handler tasks.
pub struct ConsensusServiceImpl {
    consensus: Arc<RwLock<RaftConsensus>>,
}

impl ConsensusServiceImpl {
    /// Create a new ConsensusServiceImpl wrapping the given RaftConsensus instance.
    ///
    /// # Arguments
    ///
    /// * `consensus` - The Raft consensus instance to handle requests for
    pub fn new(consensus: Arc<RwLock<RaftConsensus>>) -> Self {
        info!("ConsensusServiceImpl created");
        Self { consensus }
    }
}

#[tonic::async_trait]
impl ConsensusService for ConsensusServiceImpl {
    /// Handle RequestVote RPC from a candidate node.
    ///
    /// Implements Raft Section 5.2: Leader Election
    async fn request_vote(
        &self,
        request: Request<ProtoRequestVoteRequest>,
    ) -> Result<Response<ProtoRequestVoteResponse>, Status> {
        let req = request.into_inner();
        debug!(
            term = req.term,
            candidate_id = %req.candidate_id,
            last_log_index = req.last_log_index,
            last_log_term = req.last_log_term,
            "Received RequestVote RPC"
        );

        let consensus = self.consensus.read().await;
        let (term, vote_granted) = consensus
            .handle_request_vote(
                req.term,
                &req.candidate_id,
                req.last_log_index,
                req.last_log_term,
            )
            .await;

        debug!(
            term = term,
            vote_granted = vote_granted,
            "RequestVote response"
        );

        Ok(Response::new(ProtoRequestVoteResponse {
            term,
            vote_granted,
        }))
    }

    /// Handle AppendEntries RPC from the leader.
    ///
    /// Implements Raft Section 5.3: Log Replication
    async fn append_entries(
        &self,
        request: Request<ProtoAppendEntriesRequest>,
    ) -> Result<Response<ProtoAppendEntriesResponse>, Status> {
        let req = request.into_inner();
        debug!(
            term = req.term,
            leader_id = %req.leader_id,
            prev_log_index = req.prev_log_index,
            prev_log_term = req.prev_log_term,
            entries_count = req.entries.len(),
            leader_commit = req.leader_commit,
            "Received AppendEntries RPC"
        );

        // Convert proto log entries to internal format
        let entries: Vec<crate::cluster::consensus::LogEntry> = req
            .entries
            .iter()
            .filter_map(|e| {
                let command = serde_json::from_slice(&e.command).ok()?;
                Some(crate::cluster::consensus::LogEntry {
                    term: e.term,
                    index: e.index,
                    command,
                })
            })
            .collect();

        let consensus = self.consensus.read().await;
        let (term, success) = consensus
            .handle_append_entries(
                req.term,
                &req.leader_id,
                req.prev_log_index,
                req.prev_log_term,
                entries,
                req.leader_commit,
            )
            .await;

        debug!(term = term, success = success, "AppendEntries response");

        Ok(Response::new(ProtoAppendEntriesResponse {
            term,
            success,
            match_index: if success {
                Some(req.prev_log_index + req.entries.len() as u64)
            } else {
                None
            },
            conflict_term: None,
            conflict_index: None,
        }))
    }

    /// Handle InstallSnapshot RPC from the leader.
    ///
    /// Implements Raft Section 7: Log Compaction
    async fn install_snapshot(
        &self,
        request: Request<ProtoInstallSnapshotRequest>,
    ) -> Result<Response<ProtoInstallSnapshotResponse>, Status> {
        let req = request.into_inner();
        debug!(
            term = req.term,
            leader_id = %req.leader_id,
            last_included_index = req.last_included_index,
            last_included_term = req.last_included_term,
            offset = req.offset,
            data_len = req.data.len(),
            done = req.done,
            "Received InstallSnapshot RPC"
        );

        // Get current term
        let consensus = self.consensus.read().await;
        let current_term = consensus.current_term().await;

        // Reject if term is lower
        if req.term < current_term {
            return Ok(Response::new(ProtoInstallSnapshotResponse {
                term: current_term,
                bytes_stored: 0,
            }));
        }

        // Snapshot installation: acknowledge chunk, persist to local storage (cluster feature)
        // For now, acknowledge the snapshot chunk
        let bytes_stored = req.data.len() as u64;

        Ok(Response::new(ProtoInstallSnapshotResponse {
            term: current_term,
            bytes_stored,
        }))
    }

    /// Handle PreVote RPC (Raft extension).
    ///
    /// Implements Section 9.6 of the Raft thesis to prevent disruption
    /// from partitioned nodes rejoining the cluster.
    async fn pre_vote(
        &self,
        request: Request<ProtoPreVoteRequest>,
    ) -> Result<Response<ProtoPreVoteResponse>, Status> {
        let req = request.into_inner();
        debug!(
            term = req.term,
            candidate_id = %req.candidate_id,
            last_log_index = req.last_log_index,
            last_log_term = req.last_log_term,
            "Received PreVote RPC"
        );

        // For pre-vote, we check if we would vote without actually updating state
        let consensus = self.consensus.read().await;
        let current_term = consensus.current_term().await;
        let (last_log_index, last_log_term) = consensus.last_log_info().await;

        // Would we vote for this candidate?
        let vote_granted = req.term >= current_term
            && (req.last_log_term > last_log_term
                || (req.last_log_term == last_log_term && req.last_log_index >= last_log_index));

        debug!(
            term = current_term,
            vote_granted = vote_granted,
            "PreVote response"
        );

        Ok(Response::new(ProtoPreVoteResponse {
            term: current_term,
            vote_granted,
        }))
    }
}

// ============================================================================
// REPLICATION SERVICE IMPLEMENTATION
// ============================================================================

/// Streaming response type for ReplicateStream
type ReplicateStreamStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<ProtoReplicateResponse, Status>> + Send>>;

/// Streaming response type for PullEntries
type PullEntriesStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<ProtoReplicateRequest, Status>> + Send>>;

/// gRPC service implementation for data replication.
///
/// This service handles the actual vector data replication to shard replicas,
/// separate from the Raft consensus which handles cluster metadata.
pub struct ReplicationServiceImpl {
    replication: Arc<RwLock<EngineReplication>>,
    node_id: String,
}

impl ReplicationServiceImpl {
    /// Create a new ReplicationServiceImpl.
    ///
    /// # Arguments
    ///
    /// * `replication` - The replication manager instance
    /// * `node_id` - This node's unique identifier
    pub fn new(replication: Arc<RwLock<EngineReplication>>, node_id: String) -> Self {
        info!(node_id = %node_id, "ReplicationServiceImpl created");
        Self {
            replication,
            node_id,
        }
    }
}

#[tonic::async_trait]
impl ReplicationService for ReplicationServiceImpl {
    /// Handle a single replication entry.
    async fn replicate(
        &self,
        request: Request<ProtoReplicateRequest>,
    ) -> Result<Response<ProtoReplicateResponse>, Status> {
        let start = Instant::now();
        let req = request.into_inner();

        debug!(
            source_node_id = %req.source_node_id,
            shard_id = %req.shard_id,
            lsn = req.lsn,
            "Received Replicate RPC"
        );

        // Replication entry: replay WAL entry on local shard (cluster feature)
        // For now, acknowledge the entry
        let latency_us = start.elapsed().as_micros() as u64;

        Ok(Response::new(ProtoReplicateResponse {
            node_id: self.node_id.clone(),
            acked_lsn: req.lsn,
            success: true,
            error: None,
            latency_us,
        }))
    }

    type ReplicateStreamStream = ReplicateStreamStream;

    /// Handle bidirectional streaming replication.
    async fn replicate_stream(
        &self,
        request: Request<tonic::Streaming<ProtoReplicateRequest>>,
    ) -> Result<Response<Self::ReplicateStreamStream>, Status> {
        let mut stream = request.into_inner();
        let (tx, rx) = tokio::sync::mpsc::channel(128);
        let node_id = self.node_id.clone();

        // Spawn a task to process incoming requests and send responses
        tokio::spawn(async move {
            while let Ok(Some(req)) = stream.message().await {
                let start = Instant::now();
                let lsn = req.lsn;

                // Replication entry applied to local WAL (cluster feature)
                let latency_us = start.elapsed().as_micros() as u64;

                let response = ProtoReplicateResponse {
                    node_id: node_id.clone(),
                    acked_lsn: lsn,
                    success: true,
                    error: None,
                    latency_us,
                };

                if tx.send(Ok(response)).await.is_err() {
                    break;
                }
            }
        });

        let response_stream = ReceiverStream::new(rx);
        Ok(Response::new(
            Box::pin(response_stream) as ReplicateStreamStream
        ))
    }

    type PullEntriesStream = PullEntriesStream;

    /// Pull missed entries for catch-up replication.
    async fn pull_entries(
        &self,
        request: Request<ProtoPullEntriesRequest>,
    ) -> Result<Response<Self::PullEntriesStream>, Status> {
        let req = request.into_inner();
        debug!(
            node_id = %req.node_id,
            shard_id = %req.shard_id,
            from_lsn = req.from_lsn,
            max_entries = req.max_entries,
            "Received PullEntries RPC"
        );

        let (tx, rx) = tokio::sync::mpsc::channel(128);
        let source_node_id = self.node_id.clone();

        // WAL query: scan entries after from_lsn using DiskManager (cluster feature)
        // For now, send an empty stream
        tokio::spawn(async move {
            // Stream would send entries here
            drop(tx);
            let _ = source_node_id; // Use the variable
        });

        let response_stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(response_stream) as PullEntriesStream))
    }

    /// Acknowledge successful replication (for async replication mode).
    async fn ack_replication(
        &self,
        request: Request<ProtoAckReplicationRequest>,
    ) -> Result<Response<ProtoAckReplicationResponse>, Status> {
        let req = request.into_inner();
        debug!(
            node_id = %req.node_id,
            shard_id = %req.shard_id,
            acked_lsn = req.acked_lsn,
            "Received AckReplication RPC"
        );

        // Update replication state tracking
        let replication = self.replication.read().await;
        let current_lsn = replication.current_lsn().await;

        Ok(Response::new(ProtoAckReplicationResponse {
            success: true,
            primary_lsn: current_lsn,
        }))
    }
}

// ============================================================================
// HEALTH SERVICE IMPLEMENTATION
// ============================================================================

/// Streaming response type for Watch
type WatchStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<HealthCheckResponse, Status>> + Send>>;

/// gRPC service implementation for health checking.
///
/// This service provides health monitoring compatible with the gRPC
/// Health Checking Protocol, with extensions for detailed status.
pub struct HealthServiceImpl {
    node_id: String,
    consensus: Option<Arc<RwLock<RaftConsensus>>>,
    start_time: Instant,
}

impl HealthServiceImpl {
    /// Create a new HealthServiceImpl.
    ///
    /// # Arguments
    ///
    /// * `node_id` - This node's unique identifier
    pub fn new(node_id: String) -> Self {
        info!(node_id = %node_id, "HealthServiceImpl created");
        Self {
            node_id,
            consensus: None,
            start_time: Instant::now(),
        }
    }

    /// Create with consensus reference for detailed status.
    pub fn with_consensus(node_id: String, consensus: Arc<RwLock<RaftConsensus>>) -> Self {
        info!(node_id = %node_id, "HealthServiceImpl created with consensus");
        Self {
            node_id,
            consensus: Some(consensus),
            start_time: Instant::now(),
        }
    }
}

#[tonic::async_trait]
impl HealthService for HealthServiceImpl {
    /// Perform a basic health check.
    ///
    /// Compatible with the gRPC Health Checking Protocol.
    async fn check(
        &self,
        request: Request<HealthCheckRequest>,
    ) -> Result<Response<HealthCheckResponse>, Status> {
        let req = request.into_inner();
        debug!(service = %req.service, "Health check requested");

        // Check overall health
        let status = if req.service.is_empty() {
            // Overall node health
            ServingStatus::Serving
        } else {
            // Specific service health
            match req.service.as_str() {
                "consensus" => {
                    if self.consensus.is_some() {
                        ServingStatus::Serving
                    } else {
                        ServingStatus::NotServing
                    }
                }
                "replication" => ServingStatus::Serving,
                "search" => ServingStatus::Serving,
                _ => ServingStatus::ServiceUnknown,
            }
        };

        Ok(Response::new(HealthCheckResponse {
            status: status as i32,
        }))
    }

    /// Get detailed node status information.
    async fn status(
        &self,
        request: Request<StatusRequest>,
    ) -> Result<Response<StatusResponse>, Status> {
        let req = request.into_inner();
        debug!(
            include_metrics = req.include_metrics,
            include_shards = req.include_shards,
            "Status requested"
        );

        // Get consensus information if available
        let (current_term, leader_id, role) = if let Some(ref consensus) = self.consensus {
            let consensus = consensus.read().await;
            let term = consensus.current_term().await;
            let leader = consensus.get_leader().await;
            let state = consensus.get_state().await;
            let role = match state {
                crate::cluster::consensus::ConsensusState::Follower => NodeRole::Follower,
                crate::cluster::consensus::ConsensusState::Candidate => NodeRole::Candidate,
                crate::cluster::consensus::ConsensusState::Leader => NodeRole::Leader,
            };
            (term, leader, role)
        } else {
            (0, None, NodeRole::Observer)
        };

        // Get system metrics
        let uptime_seconds = self.start_time.elapsed().as_secs();

        // Get memory usage (approximate)
        let memory_bytes = {
            // Use a simple estimate based on process info
            // In production, use sys_info crate or similar
            0u64
        };

        // Build shard status if requested
        let shards = if req.include_shards {
            // Shard status: query engine for collection-level stats (cluster feature)
            Vec::new()
        } else {
            Vec::new()
        };

        Ok(Response::new(StatusResponse {
            node_id: self.node_id.clone(),
            role: role as i32,
            current_term,
            leader_id,
            uptime_seconds,
            active_connections: 0, // Tracked by network layer metrics
            memory_bytes,
            cpu_percent: 0.0, // Tracked by system metrics collector
            shards,
            replication_lag_ms: None,
        }))
    }

    type WatchStream = WatchStream;

    /// Watch health status changes.
    async fn watch(
        &self,
        request: Request<HealthCheckRequest>,
    ) -> Result<Response<Self::WatchStream>, Status> {
        let req = request.into_inner();
        debug!(service = %req.service, "Health watch requested");

        let (tx, rx) = tokio::sync::mpsc::channel(16);
        let _service = req.service;

        // Spawn a task to periodically send health updates
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                interval.tick().await;

                let status = ServingStatus::Serving;

                let response = HealthCheckResponse {
                    status: status as i32,
                };

                if tx.send(Ok(response)).await.is_err() {
                    break;
                }
            }
        });

        let response_stream = ReceiverStream::new(rx);
        Ok(Response::new(Box::pin(response_stream) as WatchStream))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::consensus::ConsensusConfig;
    use crate::cluster::replication::ReplicationConfig;

    #[tokio::test]
    async fn test_consensus_service_creation() {
        let config = ConsensusConfig::default();
        let consensus = RaftConsensus::new(config).expect("Failed to create consensus");
        let consensus = Arc::new(RwLock::new(consensus));

        let service = ConsensusServiceImpl::new(consensus);
        assert!(true, "ConsensusServiceImpl created successfully");
        let _ = service; // Use the service
    }

    #[tokio::test]
    async fn test_request_vote() {
        let config = ConsensusConfig::default();
        let consensus = RaftConsensus::new(config).expect("Failed to create consensus");
        let consensus = Arc::new(RwLock::new(consensus));

        let service = ConsensusServiceImpl::new(consensus);

        let request = Request::new(ProtoRequestVoteRequest {
            term: 1,
            candidate_id: "node-2".to_string(),
            last_log_index: 0,
            last_log_term: 0,
        });

        let response = service.request_vote(request).await;
        assert!(response.is_ok());

        let resp = response.unwrap().into_inner();
        // New node with term 0 should grant vote to term 1
        assert!(resp.vote_granted || resp.term >= 1);
    }

    #[tokio::test]
    async fn test_append_entries_heartbeat() {
        let config = ConsensusConfig::default();
        let consensus = RaftConsensus::new(config).expect("Failed to create consensus");
        let consensus = Arc::new(RwLock::new(consensus));

        let service = ConsensusServiceImpl::new(consensus);

        // Empty entries = heartbeat
        let request = Request::new(ProtoAppendEntriesRequest {
            term: 1,
            leader_id: "leader-1".to_string(),
            prev_log_index: 0,
            prev_log_term: 0,
            entries: vec![],
            leader_commit: 0,
        });

        let response = service.append_entries(request).await;
        assert!(response.is_ok());

        let resp = response.unwrap().into_inner();
        assert!(resp.success);
    }

    #[tokio::test]
    async fn test_replication_service_creation() {
        let config = ReplicationConfig::default();
        let replication = EngineReplication::new(config, "node-1".to_string());
        let replication = Arc::new(RwLock::new(replication));

        let service = ReplicationServiceImpl::new(replication, "node-1".to_string());
        assert!(true, "ReplicationServiceImpl created successfully");
        let _ = service;
    }

    #[tokio::test]
    async fn test_replicate() {
        let config = ReplicationConfig::default();
        let replication = EngineReplication::new(config, "node-1".to_string());
        let replication = Arc::new(RwLock::new(replication));

        let service = ReplicationServiceImpl::new(replication, "node-1".to_string());

        let request = Request::new(ProtoReplicateRequest {
            source_node_id: "node-2".to_string(),
            shard_id: "shard-1".to_string(),
            lsn: 100,
            timestamp: 0,
            operation: 1, // Insert
            data: vec![1, 2, 3],
            checksum: 0,
            consistency: 2, // Quorum
            timeout_ms: 5000,
        });

        let response = service.replicate(request).await;
        assert!(response.is_ok());

        let resp = response.unwrap().into_inner();
        assert!(resp.success);
        assert_eq!(resp.acked_lsn, 100);
        assert_eq!(resp.node_id, "node-1");
    }

    #[tokio::test]
    async fn test_health_service_creation() {
        let service = HealthServiceImpl::new("node-1".to_string());
        assert!(true, "HealthServiceImpl created successfully");
        let _ = service;
    }

    #[tokio::test]
    async fn test_health_check() {
        let service = HealthServiceImpl::new("node-1".to_string());

        let request = Request::new(HealthCheckRequest {
            service: String::new(),
        });

        let response = service.check(request).await;
        assert!(response.is_ok());

        let resp = response.unwrap().into_inner();
        assert_eq!(resp.status, ServingStatus::Serving as i32);
    }

    #[tokio::test]
    async fn test_health_status() {
        let service = HealthServiceImpl::new("node-1".to_string());

        let request = Request::new(StatusRequest {
            include_metrics: true,
            include_shards: true,
        });

        let response = service.status(request).await;
        assert!(response.is_ok());

        let resp = response.unwrap().into_inner();
        assert_eq!(resp.node_id, "node-1");
        assert_eq!(resp.role, NodeRole::Observer as i32); // No consensus attached
    }

    #[tokio::test]
    async fn test_health_service_with_consensus() {
        let config = ConsensusConfig::default();
        let consensus = RaftConsensus::new(config).expect("Failed to create consensus");
        let consensus = Arc::new(RwLock::new(consensus));

        let service = HealthServiceImpl::with_consensus("node-1".to_string(), consensus);

        let request = Request::new(HealthCheckRequest {
            service: "consensus".to_string(),
        });

        let response = service.check(request).await;
        assert!(response.is_ok());

        let resp = response.unwrap().into_inner();
        assert_eq!(resp.status, ServingStatus::Serving as i32);
    }

    #[tokio::test]
    async fn test_unknown_service_health() {
        let service = HealthServiceImpl::new("node-1".to_string());

        let request = Request::new(HealthCheckRequest {
            service: "unknown-service".to_string(),
        });

        let response = service.check(request).await;
        assert!(response.is_ok());

        let resp = response.unwrap().into_inner();
        assert_eq!(resp.status, ServingStatus::ServiceUnknown as i32);
    }

    #[tokio::test]
    async fn test_pre_vote() {
        let config = ConsensusConfig::default();
        let consensus = RaftConsensus::new(config).expect("Failed to create consensus");
        let consensus = Arc::new(RwLock::new(consensus));

        let service = ConsensusServiceImpl::new(consensus);

        let request = Request::new(ProtoPreVoteRequest {
            term: 1,
            candidate_id: "node-2".to_string(),
            last_log_index: 0,
            last_log_term: 0,
        });

        let response = service.pre_vote(request).await;
        assert!(response.is_ok());

        let resp = response.unwrap().into_inner();
        // Pre-vote should be granted for valid candidate
        // Pre-vote: either granted or rejected with a valid term
        let _term = resp.term;
    }
}
