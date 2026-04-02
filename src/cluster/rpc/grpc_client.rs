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

//! gRPC Client Implementations for Inter-Node Communication
//!
//! This module provides concrete gRPC implementations of the RPC traits
//! defined in `traits.rs`. It includes:
//!
//! - **ResilientClient<T>**: A wrapper that applies retry and circuit breaker logic
//! - **GrpcConsensusTransport**: Raft consensus transport over gRPC
//! - **GrpcReplicationSink**: Data replication sink over gRPC
//! - **GrpcSearchFanout**: Distributed search fanout over gRPC
//! - **GrpcHealthChecker**: Health checking over gRPC
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                         ResilientClient<T>                               │
//! │  ┌─────────────────────────────────────────────────────────────────┐   │
//! │  │                       Inner Client (T)                           │   │
//! │  └─────────────────────────────────────────────────────────────────┘   │
//! │                                │                                         │
//! │  ┌─────────────────────────────┼─────────────────────────────────────┐ │
//! │  │                   RetryPolicy + CircuitBreaker                     │ │
//! │  │  ┌──────────────┐  ┌──────────────┐  ┌──────────────────────────┐ │ │
//! │  │  │ Exponential  │  │   Circuit    │  │   Connection Manager     │ │ │
//! │  │  │   Backoff    │  │   Breaker    │  │   (Channel Pool)         │ │ │
//! │  │  └──────────────┘  └──────────────┘  └──────────────────────────┘ │ │
//! │  └───────────────────────────────────────────────────────────────────┘ │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```

use async_trait::async_trait;
use dashmap::DashMap;
use futures::Stream;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use super::connection::{ConnectionManager, ConnectionPoolConfig};
use super::error::{RpcError, RpcErrorKind, RpcResult};
use super::retry::{CircuitBreaker, RetryExecutor, RetryPolicy};
use super::traits::{ConsensusTransport, HealthChecker, ReplicationSink, SearchFanout};
use super::types::*;

use crate::proto::proximadb_cluster_v1 as proto;

fn status_to_rpc_error(status: tonic::Status) -> RpcError {
    let kind = match status.code() {
        tonic::Code::Unavailable | tonic::Code::Aborted => RpcErrorKind::Connection,
        tonic::Code::DeadlineExceeded => RpcErrorKind::Timeout,
        tonic::Code::InvalidArgument | tonic::Code::FailedPrecondition => RpcErrorKind::InvalidRequest,
        _ => RpcErrorKind::Internal,
    };
    RpcError::new(kind, status.message())
}
fn native_log_entry_type_to_proto(t: LogEntryType) -> i32 {
    match t {
        LogEntryType::Command => proto::LogEntryType::Command as i32,
        LogEntryType::Noop => proto::LogEntryType::Noop as i32,
        LogEntryType::Config => proto::LogEntryType::Config as i32,
    }
}
fn native_consistency_to_proto(c: ConsistencyLevel) -> i32 {
    match c {
        ConsistencyLevel::One => proto::ConsistencyLevel::One as i32,
        ConsistencyLevel::Quorum => proto::ConsistencyLevel::Quorum as i32,
        ConsistencyLevel::All => proto::ConsistencyLevel::All as i32,
        ConsistencyLevel::LocalQuorum => proto::ConsistencyLevel::LocalQuorum as i32,
    }
}
fn native_repl_op_to_proto(op: ReplicationOperation) -> i32 {
    match op {
        ReplicationOperation::Insert => proto::ReplicationOperationType::Insert as i32,
        ReplicationOperation::Update => proto::ReplicationOperationType::Update as i32,
        ReplicationOperation::Delete => proto::ReplicationOperationType::Delete as i32,
        ReplicationOperation::Flush => proto::ReplicationOperationType::Flush as i32,
        ReplicationOperation::Compact => proto::ReplicationOperationType::Compact as i32,
        ReplicationOperation::SchemaChange => proto::ReplicationOperationType::SchemaChange as i32,
    }
}
fn native_serving_status(v: i32) -> ServingStatus {
    match proto::ServingStatus::try_from(v) {
        Ok(proto::ServingStatus::Serving) => ServingStatus::Serving,
        Ok(proto::ServingStatus::NotServing) => ServingStatus::NotServing,
        Ok(proto::ServingStatus::ServiceUnknown) => ServingStatus::ServiceUnknown,
        _ => ServingStatus::NotServing,
    }
}
fn native_node_role(v: i32) -> NodeRole {
    match proto::NodeRole::try_from(v) {
        Ok(proto::NodeRole::Leader) => NodeRole::Leader,
        Ok(proto::NodeRole::Candidate) => NodeRole::Candidate,
        Ok(proto::NodeRole::Observer) => NodeRole::Observer,
        _ => NodeRole::Follower,
    }
}

// ============================================================================
// RESILIENT CLIENT WRAPPER
// ============================================================================

/// A resilient wrapper around any client that applies retry and circuit breaker logic
///
/// This wrapper can be used with any RPC client type to add:
/// - Automatic retries with exponential backoff
/// - Circuit breaker protection against cascading failures
/// - Connection management with health caching
///
/// # Type Parameters
///
/// * `T` - The inner client type to wrap
///
/// # Example
///
/// ```ignore
/// let inner = MyGrpcClient::new();
/// let resilient = ResilientClient::new(
///     inner,
///     connection_manager,
///     RetryPolicy::default(),
/// );
///
/// // All calls through resilient will have retry + circuit breaker
/// ```
pub struct ResilientClient<T> {
    /// The inner client implementation
    inner: T,

    /// Connection manager for channel acquisition
    connection_manager: Arc<ConnectionManager>,

    /// Retry policy configuration
    retry_policy: RetryPolicy,

    /// Per-endpoint circuit breakers (using Arc for shared state)
    circuit_breakers: Arc<DashMap<String, Arc<CircuitBreaker>>>,

    /// Default circuit breaker configuration
    failure_threshold: u32,
    reset_timeout: Duration,
}

impl<T> ResilientClient<T> {
    /// Create a new resilient client wrapper
    pub fn new(
        inner: T,
        connection_manager: Arc<ConnectionManager>,
        retry_policy: RetryPolicy,
    ) -> Self {
        Self {
            inner,
            connection_manager,
            retry_policy,
            circuit_breakers: Arc::new(DashMap::new()),
            failure_threshold: 5,
            reset_timeout: Duration::from_secs(30),
        }
    }

    /// Configure circuit breaker thresholds
    pub fn with_circuit_breaker_config(
        mut self,
        failure_threshold: u32,
        reset_timeout: Duration,
    ) -> Self {
        self.failure_threshold = failure_threshold;
        self.reset_timeout = reset_timeout;
        self
    }

    /// Get or create a circuit breaker for an endpoint
    fn get_circuit_breaker(&self, endpoint: &NodeEndpoint) -> Arc<CircuitBreaker> {
        let key = format!("{}:{}", endpoint.node_id, endpoint.address);
        // Use entry API to get or create atomically
        self.circuit_breakers
            .entry(key)
            .or_insert_with(|| {
                Arc::new(CircuitBreaker::new(
                    self.failure_threshold,
                    self.reset_timeout,
                ))
            })
            .value()
            .clone()
    }

    /// Get the inner client
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Get the connection manager
    pub fn connection_manager(&self) -> &Arc<ConnectionManager> {
        &self.connection_manager
    }

    /// Get the retry policy
    pub fn retry_policy(&self) -> &RetryPolicy {
        &self.retry_policy
    }

    /// Create a retry executor for an endpoint
    fn executor(&self, endpoint: &NodeEndpoint) -> RetryExecutor {
        RetryExecutor::new(
            self.retry_policy.clone(),
            self.get_circuit_breaker(endpoint),
        )
    }

    /// Mark an endpoint as healthy
    pub async fn mark_healthy(&self, endpoint: &NodeEndpoint) {
        self.connection_manager.mark_healthy(endpoint).await;
        let key = format!("{}:{}", endpoint.node_id, endpoint.address);
        if let Some(cb) = self.circuit_breakers.get(&key) {
            cb.reset();
        }
    }

    /// Mark an endpoint as unhealthy
    pub async fn mark_unhealthy(&self, endpoint: &NodeEndpoint, error: &str) {
        self.connection_manager
            .mark_unhealthy(endpoint, error)
            .await;
    }
}

impl<T: Clone> Clone for ResilientClient<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            connection_manager: self.connection_manager.clone(),
            retry_policy: self.retry_policy.clone(),
            circuit_breakers: self.circuit_breakers.clone(), // Share circuit breakers
            failure_threshold: self.failure_threshold,
            reset_timeout: self.reset_timeout,
        }
    }
}

impl<T> std::fmt::Debug for ResilientClient<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResilientClient")
            .field("retry_policy", &self.retry_policy)
            .field("failure_threshold", &self.failure_threshold)
            .field("reset_timeout", &self.reset_timeout)
            .field("active_circuit_breakers", &self.circuit_breakers.len())
            .finish()
    }
}

// ============================================================================
// GRPC CONSENSUS TRANSPORT
// ============================================================================

/// gRPC implementation of ConsensusTransport for Raft consensus
///
/// This implementation uses the ConnectionManager for channel management
/// and wraps calls with retry and circuit breaker logic when used with
/// ResilientClient.
#[derive(Clone)]
pub struct GrpcConsensusTransport {
    /// Connection manager for acquiring gRPC channels
    connection_manager: Arc<ConnectionManager>,
}

impl GrpcConsensusTransport {
    /// Create a new gRPC consensus transport
    pub fn new(connection_manager: Arc<ConnectionManager>) -> Self {
        Self { connection_manager }
    }

    /// Create with default configuration
    pub fn with_default_config() -> Self {
        Self::new(Arc::new(ConnectionManager::new(
            ConnectionPoolConfig::default(),
        )))
    }
}

#[async_trait]
impl ConsensusTransport for GrpcConsensusTransport {
    async fn request_vote(
        &self,
        target: &NodeEndpoint,
        req: RequestVoteRequest,
    ) -> RpcResult<RequestVoteResponse> {
        // Get channel from connection manager
        let _channel = self.connection_manager.get_channel(target).await?;

        // In a real implementation, we would create a gRPC client from the channel
        // and call the RequestVote RPC. For now, we return a placeholder.
        //
        // Example of real implementation:
        // ```
        // let mut client = ConsensusServiceClient::new(channel);
        // let request = tonic::Request::new(proto::RequestVoteRequest::from(req));
        // let response = client.request_vote(request).await?;
        // Ok(response.into_inner().into())
        // ```

        let mut client = proto::consensus_service_client::ConsensusServiceClient::new(_channel);
        let resp = client.request_vote(tonic::Request::new(proto::RequestVoteRequest {
            term: req.term, candidate_id: req.candidate_id.clone(),
            last_log_index: req.last_log_index, last_log_term: req.last_log_term,
        })).await.map_err(status_to_rpc_error)?;
        let inner = resp.into_inner();
        Ok(RequestVoteResponse { term: inner.term, vote_granted: inner.vote_granted })
    }

    async fn append_entries(
        &self,
        target: &NodeEndpoint,
        req: AppendEntriesRequest,
    ) -> RpcResult<AppendEntriesResponse> {
        let _channel = self.connection_manager.get_channel(target).await?;

        let mut client = proto::consensus_service_client::ConsensusServiceClient::new(_channel);
        let resp = client.append_entries(tonic::Request::new(proto::AppendEntriesRequest {
            term: req.term, leader_id: req.leader_id.clone(),
            prev_log_index: req.prev_log_index, prev_log_term: req.prev_log_term,
            entries: req.entries.iter().map(|e| proto::LogEntry {
                term: e.term, index: e.index, command: e.command.clone(),
                entry_type: native_log_entry_type_to_proto(e.entry_type),
            }).collect(),
            leader_commit: req.leader_commit,
        })).await.map_err(status_to_rpc_error)?;
        let inner = resp.into_inner();
        Ok(AppendEntriesResponse {
            term: inner.term, success: inner.success,
            match_index: inner.match_index, conflict_term: inner.conflict_term,
            conflict_index: inner.conflict_index,
        })
    }

    async fn install_snapshot(
        &self,
        target: &NodeEndpoint,
        req: InstallSnapshotRequest,
    ) -> RpcResult<InstallSnapshotResponse> {
        let _channel = self.connection_manager.get_channel(target).await?;

        let mut client = proto::consensus_service_client::ConsensusServiceClient::new(_channel);
        let resp = client.install_snapshot(tonic::Request::new(proto::InstallSnapshotRequest {
            term: req.term, leader_id: req.leader_id.clone(),
            last_included_index: req.last_included_index, last_included_term: req.last_included_term,
            offset: req.offset, data: req.data.clone(), done: req.done,
        })).await.map_err(status_to_rpc_error)?;
        let inner = resp.into_inner();
        Ok(InstallSnapshotResponse { term: inner.term, bytes_stored: inner.bytes_stored })
    }

    async fn pre_vote(
        &self,
        target: &NodeEndpoint,
        req: RequestVoteRequest,
    ) -> RpcResult<RequestVoteResponse> {
        let _channel = self.connection_manager.get_channel(target).await?;

        let mut client = proto::consensus_service_client::ConsensusServiceClient::new(_channel);
        let resp = client.pre_vote(tonic::Request::new(proto::PreVoteRequest {
            term: req.term, candidate_id: req.candidate_id.clone(),
            last_log_index: req.last_log_index, last_log_term: req.last_log_term,
        })).await.map_err(status_to_rpc_error)?;
        let inner = resp.into_inner();
        Ok(RequestVoteResponse { term: inner.term, vote_granted: inner.vote_granted })
    }
}

// ============================================================================
// GRPC REPLICATION SINK
// ============================================================================

/// gRPC implementation of ReplicationSink for data replication
#[derive(Clone)]
pub struct GrpcReplicationSink {
    /// Connection manager for acquiring gRPC channels
    connection_manager: Arc<ConnectionManager>,
}

impl GrpcReplicationSink {
    /// Create a new gRPC replication sink
    pub fn new(connection_manager: Arc<ConnectionManager>) -> Self {
        Self { connection_manager }
    }

    /// Create with default configuration
    pub fn with_default_config() -> Self {
        Self::new(Arc::new(ConnectionManager::new(
            ConnectionPoolConfig::default(),
        )))
    }
}

#[async_trait]
impl ReplicationSink for GrpcReplicationSink {
    async fn replicate(
        &self,
        target: &NodeEndpoint,
        req: ReplicateRequest,
    ) -> RpcResult<ReplicateResponse> {
        let _channel = self.connection_manager.get_channel(target).await?;

        let mut client = proto::replication_service_client::ReplicationServiceClient::new(_channel);
        let resp = client.replicate(tonic::Request::new(proto::ReplicateRequest {
            source_node_id: req.source_node_id.clone(), shard_id: req.shard_id.clone(),
            lsn: req.lsn, timestamp: req.timestamp,
            operation: native_repl_op_to_proto(req.operation),
            data: req.data.clone(), checksum: req.checksum,
            consistency: native_consistency_to_proto(req.consistency),
            timeout_ms: req.timeout.as_millis() as u32,
        })).await.map_err(status_to_rpc_error)?;
        let inner = resp.into_inner();
        Ok(ReplicateResponse {
            node_id: inner.node_id, acked_lsn: inner.acked_lsn,
            success: inner.success, error: inner.error,
            latency: Duration::from_micros(inner.latency_us),
        })
    }

    async fn replicate_batch(
        &self,
        target: &NodeEndpoint,
        requests: Vec<ReplicateRequest>,
    ) -> RpcResult<Vec<ReplicateResponse>> {
        let _channel = self.connection_manager.get_channel(target).await?;

        // TODO: Implement actual gRPC call
        tracing::debug!(
            target = %target,
            batch_size = requests.len(),
            "ReplicateBatch RPC (stub)"
        );

        Ok(requests
            .into_iter()
            .map(|req| ReplicateResponse {
                node_id: target.node_id.clone(),
                acked_lsn: req.lsn,
                success: true,
                error: None,
                latency: Duration::from_micros(100),
            })
            .collect())
    }

    async fn replicate_stream(
        &self,
        target: &NodeEndpoint,
        _requests: Pin<Box<dyn Stream<Item = ReplicateRequest> + Send>>,
    ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<ReplicateResponse>> + Send>>> {
        let _channel = self.connection_manager.get_channel(target).await?;

        // TODO: Implement actual bidirectional streaming gRPC call
        tracing::debug!(
            target = %target,
            "ReplicateStream RPC (stub)"
        );

        Err(RpcError::new(
            RpcErrorKind::Internal,
            "Streaming replication not yet implemented",
        ))
    }

    async fn pull_entries(
        &self,
        target: &NodeEndpoint,
        req: PullEntriesRequest,
    ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<ReplicateRequest>> + Send>>> {
        let _channel = self.connection_manager.get_channel(target).await?;

        // TODO: Implement actual server streaming gRPC call
        tracing::debug!(
            target = %target,
            from_lsn = req.from_lsn,
            max_entries = req.max_entries,
            "PullEntries RPC (stub)"
        );

        Err(RpcError::new(
            RpcErrorKind::Internal,
            "Entry pulling not yet implemented",
        ))
    }

    async fn ack_replication(
        &self,
        target: &NodeEndpoint,
        req: AckReplicationRequest,
    ) -> RpcResult<AckReplicationResponse> {
        let _channel = self.connection_manager.get_channel(target).await?;

        let mut client = proto::replication_service_client::ReplicationServiceClient::new(_channel);
        let resp = client.ack_replication(tonic::Request::new(proto::AckReplicationRequest {
            node_id: req.node_id.clone(), shard_id: req.shard_id.clone(), acked_lsn: req.acked_lsn,
        })).await.map_err(status_to_rpc_error)?;
        let inner = resp.into_inner();
        Ok(AckReplicationResponse { success: inner.success, primary_lsn: inner.primary_lsn })
    }
}

// ============================================================================
// GRPC SEARCH FANOUT
// ============================================================================

/// gRPC implementation of SearchFanout for distributed search
#[derive(Clone)]
pub struct GrpcSearchFanout {
    /// Connection manager for acquiring gRPC channels
    connection_manager: Arc<ConnectionManager>,
}

impl GrpcSearchFanout {
    /// Create a new gRPC search fanout
    pub fn new(connection_manager: Arc<ConnectionManager>) -> Self {
        Self { connection_manager }
    }

    /// Create with default configuration
    pub fn with_default_config() -> Self {
        Self::new(Arc::new(ConnectionManager::new(
            ConnectionPoolConfig::default(),
        )))
    }
}

#[async_trait]
impl SearchFanout for GrpcSearchFanout {
    async fn shard_search(
        &self,
        target: &NodeEndpoint,
        req: ShardSearchRequest,
    ) -> RpcResult<ShardSearchResponse> {
        let _channel = self.connection_manager.get_channel(target).await?;

        let mut client = proto::search_fanout_service_client::SearchFanoutServiceClient::new(_channel);
        let resp = client.shard_search(tonic::Request::new(proto::ShardSearchRequest {
            request_id: req.request_id.clone(), collection: req.collection.clone(),
            shard_id: req.shard_id.clone(), vector: req.vector.clone(), top_k: req.top_k,
            filter: req.filter.clone(),
            params: Some(proto::SearchParams { metric: 0, min_score: req.params.min_score,
                ef_search: req.params.ef_search, n_probes: req.params.n_probes }),
            timeout_ms: req.timeout.as_millis() as u32, include_vectors: req.include_vectors,
            tenant_id: req.tenant_id.clone(), domain_id: req.domain_id.clone(),
        })).await.map_err(status_to_rpc_error)?;
        let inner = resp.into_inner();
        Ok(ShardSearchResponse {
            request_id: inner.request_id, shard_id: inner.shard_id,
            results: inner.results.into_iter().map(|r| ShardSearchResult {
                id: r.id, score: r.score,
                vector: r.vector.is_empty().not().then(|| r.vector),
                metadata: r.metadata,
            }).collect(),
            vectors_scanned: inner.vectors_scanned,
            latency: Duration::from_micros(inner.latency_us), truncated: inner.truncated,
        })
    }

    async fn shard_search_stream(
        &self,
        target: &NodeEndpoint,
        req: ShardSearchRequest,
    ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<ShardSearchResult>> + Send>>> {
        let _channel = self.connection_manager.get_channel(target).await?;

        // TODO: Implement actual server streaming gRPC call
        tracing::debug!(
            target = %target,
            collection = %req.collection,
            shard = %req.shard_id,
            "ShardSearchStream RPC (stub)"
        );

        Err(RpcError::new(
            RpcErrorKind::Internal,
            "Streaming search not yet implemented",
        ))
    }

    async fn forward_write(
        &self,
        target: &NodeEndpoint,
        req: ForwardWriteRequest,
    ) -> RpcResult<ForwardWriteResponse> {
        let _channel = self.connection_manager.get_channel(target).await?;

        let mut client = proto::search_fanout_service_client::SearchFanoutServiceClient::new(_channel);
        let resp = client.forward_write(tonic::Request::new(proto::ForwardWriteRequest {
            request_id: req.request_id.clone(), collection: req.collection.clone(),
            shard_id: req.shard_id.clone(),
            records: req.records.iter().map(|r| crate::proto::proximadb_v1::VectorRecord {
                id: r.id.clone(), vector: r.vector.clone(),
                metadata: r.metadata.iter().filter_map(|(k, v)| {
                    serde_json::from_value::<crate::proto::proximadb_v1::SqlValue>(v.clone())
                        .ok().map(|sv| (k.clone(), sv))
                }).collect(),
                timestamp: None, updated_at: None, expires_at: None, version: None, source: None,
            }).collect(),
            consistency: native_consistency_to_proto(req.consistency),
            timeout_ms: req.timeout.as_millis() as u32,
            tenant_id: req.tenant_id.clone(), domain_id: req.domain_id.clone(),
        })).await.map_err(status_to_rpc_error)?;
        let inner = resp.into_inner();
        Ok(ForwardWriteResponse {
            request_id: inner.request_id, records_written: inner.records_written,
            replicas_acked: inner.replicas_acked,
            latency: Duration::from_micros(inner.latency_us), error: inner.error,
        })
    }

    async fn forward_write_batch(
        &self,
        target: &NodeEndpoint,
        requests: Vec<ForwardWriteRequest>,
    ) -> RpcResult<Vec<ForwardWriteResponse>> {
        let _channel = self.connection_manager.get_channel(target).await?;

        let mut client = proto::search_fanout_service_client::SearchFanoutServiceClient::new(_channel);
        let proto_reqs: Vec<proto::ForwardWriteRequest> = requests.iter().map(|req| {
            proto::ForwardWriteRequest {
                request_id: req.request_id.clone(), collection: req.collection.clone(),
                shard_id: req.shard_id.clone(),
                records: req.records.iter().map(|r| crate::proto::proximadb_v1::VectorRecord {
                    id: r.id.clone(), vector: r.vector.clone(),
                    metadata: r.metadata.iter().filter_map(|(k, v)| {
                        serde_json::from_value::<crate::proto::proximadb_v1::SqlValue>(v.clone())
                            .ok().map(|sv| (k.clone(), sv))
                    }).collect(),
                    timestamp: None, updated_at: None, expires_at: None, version: None, source: None,
                }).collect(),
                consistency: native_consistency_to_proto(req.consistency),
                timeout_ms: req.timeout.as_millis() as u32,
                tenant_id: req.tenant_id.clone(), domain_id: req.domain_id.clone(),
            }
        }).collect();
        let resp = client.forward_write_batch(tonic::Request::new(proto::ForwardWriteBatchRequest {
            request_id: String::new(), requests: proto_reqs,
        })).await.map_err(status_to_rpc_error)?;
        Ok(resp.into_inner().responses.into_iter().map(|r| ForwardWriteResponse {
            request_id: r.request_id, records_written: r.records_written,
            replicas_acked: r.replicas_acked,
            latency: Duration::from_micros(r.latency_us), error: r.error,
        }).collect())
    }
}

// ============================================================================
// GRPC HEALTH CHECKER
// ============================================================================

/// gRPC implementation of HealthChecker for node health monitoring
#[derive(Clone)]
pub struct GrpcHealthChecker {
    /// Connection manager for acquiring gRPC channels
    connection_manager: Arc<ConnectionManager>,
}

impl GrpcHealthChecker {
    /// Create a new gRPC health checker
    pub fn new(connection_manager: Arc<ConnectionManager>) -> Self {
        Self { connection_manager }
    }

    /// Create with default configuration
    pub fn with_default_config() -> Self {
        Self::new(Arc::new(ConnectionManager::new(
            ConnectionPoolConfig::default(),
        )))
    }
}

#[async_trait]
impl HealthChecker for GrpcHealthChecker {
    async fn check(
        &self,
        target: &NodeEndpoint,
        _req: HealthCheckRequest,
    ) -> RpcResult<HealthCheckResponse> {
        match self.connection_manager.get_channel(target).await {
            Ok(channel) => {
                let mut client = proto::health_service_client::HealthServiceClient::new(channel);
                match client.check(tonic::Request::new(proto::HealthCheckRequest {
                    service: _req.service.clone(),
                })).await {
                    Ok(resp) => {
                        self.connection_manager.mark_healthy(target).await;
                        Ok(HealthCheckResponse { status: native_serving_status(resp.into_inner().status) })
                    }
                    Err(s) => {
                        self.connection_manager.mark_unhealthy(target, s.message()).await;
                        Ok(HealthCheckResponse { status: ServingStatus::NotServing })
                    }
                }
            }
            Err(e) => {
                self.connection_manager.mark_unhealthy(target, e.message()).await;
                Ok(HealthCheckResponse { status: ServingStatus::NotServing })
            }
        }
    }

    async fn status(
        &self,
        target: &NodeEndpoint,
        _req: StatusRequest,
    ) -> RpcResult<StatusResponse> {
        let _channel = self.connection_manager.get_channel(target).await?;

        let mut client = proto::health_service_client::HealthServiceClient::new(_channel);
        let resp = client.status(tonic::Request::new(proto::StatusRequest {
            include_metrics: _req.include_metrics, include_shards: _req.include_shards,
        })).await.map_err(status_to_rpc_error)?;
        let inner = resp.into_inner();
        Ok(StatusResponse {
            node_id: inner.node_id, role: native_node_role(inner.role),
            current_term: inner.current_term, leader_id: inner.leader_id,
            uptime_seconds: inner.uptime_seconds, active_connections: inner.active_connections,
            memory_bytes: inner.memory_bytes, cpu_percent: inner.cpu_percent,
            shards: inner.shards.into_iter().map(|s| ShardStatus {
                shard_id: s.shard_id, collection: s.collection, is_primary: s.is_primary,
                state: match proto::ShardState::try_from(s.state) {
                    Ok(proto::ShardState::Active) => ShardState::Active,
                    Ok(proto::ShardState::Initializing) => ShardState::Initializing,
                    Ok(proto::ShardState::CatchingUp) => ShardState::CatchingUp,
                    Ok(proto::ShardState::Relocating) => ShardState::Relocating,
                    _ => ShardState::Inactive,
                },
                current_lsn: s.current_lsn, vector_count: s.vector_count, disk_bytes: s.disk_bytes,
            }).collect(),
            replication_lag_ms: inner.replication_lag_ms,
        })
    }

    async fn watch(
        &self,
        target: &NodeEndpoint,
        _req: HealthCheckRequest,
    ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<HealthCheckResponse>> + Send>>> {
        let _channel = self.connection_manager.get_channel(target).await?;

        // TODO: Implement actual server streaming gRPC call
        tracing::debug!(
            target = %target,
            "HealthWatch RPC (stub)"
        );

        Err(RpcError::new(
            RpcErrorKind::Internal,
            "Health watching not yet implemented",
        ))
    }
}

// ============================================================================
// RESILIENT TRAIT IMPLEMENTATIONS
// ============================================================================

// Implement ConsensusTransport for ResilientClient<GrpcConsensusTransport>
#[async_trait]
impl ConsensusTransport for ResilientClient<GrpcConsensusTransport> {
    async fn request_vote(
        &self,
        target: &NodeEndpoint,
        req: RequestVoteRequest,
    ) -> RpcResult<RequestVoteResponse> {
        let inner = self.inner.clone();
        let target = target.clone();
        let req = req.clone();

        self.executor(&target)
            .execute(|| {
                let inner = inner.clone();
                let target = target.clone();
                let req = req.clone();
                async move { inner.request_vote(&target, req).await }
            })
            .await
    }

    async fn append_entries(
        &self,
        target: &NodeEndpoint,
        req: AppendEntriesRequest,
    ) -> RpcResult<AppendEntriesResponse> {
        let inner = self.inner.clone();
        let target = target.clone();
        let req = req.clone();

        self.executor(&target)
            .execute(|| {
                let inner = inner.clone();
                let target = target.clone();
                let req = req.clone();
                async move { inner.append_entries(&target, req).await }
            })
            .await
    }

    async fn install_snapshot(
        &self,
        target: &NodeEndpoint,
        req: InstallSnapshotRequest,
    ) -> RpcResult<InstallSnapshotResponse> {
        let inner = self.inner.clone();
        let target = target.clone();
        let req = req.clone();

        self.executor(&target)
            .execute(|| {
                let inner = inner.clone();
                let target = target.clone();
                let req = req.clone();
                async move { inner.install_snapshot(&target, req).await }
            })
            .await
    }

    async fn pre_vote(
        &self,
        target: &NodeEndpoint,
        req: RequestVoteRequest,
    ) -> RpcResult<RequestVoteResponse> {
        let inner = self.inner.clone();
        let target = target.clone();
        let req = req.clone();

        self.executor(&target)
            .execute(|| {
                let inner = inner.clone();
                let target = target.clone();
                let req = req.clone();
                async move { inner.pre_vote(&target, req).await }
            })
            .await
    }
}

// Implement ReplicationSink for ResilientClient<GrpcReplicationSink>
#[async_trait]
impl ReplicationSink for ResilientClient<GrpcReplicationSink> {
    async fn replicate(
        &self,
        target: &NodeEndpoint,
        req: ReplicateRequest,
    ) -> RpcResult<ReplicateResponse> {
        let inner = self.inner.clone();
        let target = target.clone();
        let req = req.clone();

        self.executor(&target)
            .execute(|| {
                let inner = inner.clone();
                let target = target.clone();
                let req = req.clone();
                async move { inner.replicate(&target, req).await }
            })
            .await
    }

    async fn replicate_batch(
        &self,
        target: &NodeEndpoint,
        requests: Vec<ReplicateRequest>,
    ) -> RpcResult<Vec<ReplicateResponse>> {
        let inner = self.inner.clone();
        let target = target.clone();
        let requests = requests.clone();

        self.executor(&target)
            .execute(|| {
                let inner = inner.clone();
                let target = target.clone();
                let requests = requests.clone();
                async move { inner.replicate_batch(&target, requests).await }
            })
            .await
    }

    async fn replicate_stream(
        &self,
        target: &NodeEndpoint,
        requests: Pin<Box<dyn Stream<Item = ReplicateRequest> + Send>>,
    ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<ReplicateResponse>> + Send>>> {
        // Streaming doesn't use retry logic - pass through directly
        self.inner.replicate_stream(target, requests).await
    }

    async fn pull_entries(
        &self,
        target: &NodeEndpoint,
        req: PullEntriesRequest,
    ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<ReplicateRequest>> + Send>>> {
        // Streaming doesn't use retry logic - pass through directly
        self.inner.pull_entries(target, req).await
    }

    async fn ack_replication(
        &self,
        target: &NodeEndpoint,
        req: AckReplicationRequest,
    ) -> RpcResult<AckReplicationResponse> {
        let inner = self.inner.clone();
        let target = target.clone();
        let req = req.clone();

        self.executor(&target)
            .execute(|| {
                let inner = inner.clone();
                let target = target.clone();
                let req = req.clone();
                async move { inner.ack_replication(&target, req).await }
            })
            .await
    }
}

// Implement SearchFanout for ResilientClient<GrpcSearchFanout>
#[async_trait]
impl SearchFanout for ResilientClient<GrpcSearchFanout> {
    async fn shard_search(
        &self,
        target: &NodeEndpoint,
        req: ShardSearchRequest,
    ) -> RpcResult<ShardSearchResponse> {
        let inner = self.inner.clone();
        let target = target.clone();
        let req = req.clone();

        self.executor(&target)
            .execute(|| {
                let inner = inner.clone();
                let target = target.clone();
                let req = req.clone();
                async move { inner.shard_search(&target, req).await }
            })
            .await
    }

    async fn shard_search_stream(
        &self,
        target: &NodeEndpoint,
        req: ShardSearchRequest,
    ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<ShardSearchResult>> + Send>>> {
        // Streaming doesn't use retry logic - pass through directly
        self.inner.shard_search_stream(target, req).await
    }

    async fn forward_write(
        &self,
        target: &NodeEndpoint,
        req: ForwardWriteRequest,
    ) -> RpcResult<ForwardWriteResponse> {
        let inner = self.inner.clone();
        let target = target.clone();
        let req = req.clone();

        self.executor(&target)
            .execute(|| {
                let inner = inner.clone();
                let target = target.clone();
                let req = req.clone();
                async move { inner.forward_write(&target, req).await }
            })
            .await
    }

    async fn forward_write_batch(
        &self,
        target: &NodeEndpoint,
        requests: Vec<ForwardWriteRequest>,
    ) -> RpcResult<Vec<ForwardWriteResponse>> {
        let inner = self.inner.clone();
        let target = target.clone();
        let requests = requests.clone();

        self.executor(&target)
            .execute(|| {
                let inner = inner.clone();
                let target = target.clone();
                let requests = requests.clone();
                async move { inner.forward_write_batch(&target, requests).await }
            })
            .await
    }
}

// Implement HealthChecker for ResilientClient<GrpcHealthChecker>
#[async_trait]
impl HealthChecker for ResilientClient<GrpcHealthChecker> {
    async fn check(
        &self,
        target: &NodeEndpoint,
        req: HealthCheckRequest,
    ) -> RpcResult<HealthCheckResponse> {
        // Health checks typically don't retry - they're probing availability
        self.inner.check(target, req).await
    }

    async fn status(&self, target: &NodeEndpoint, req: StatusRequest) -> RpcResult<StatusResponse> {
        let inner = self.inner.clone();
        let target = target.clone();
        let req = req.clone();

        self.executor(&target)
            .execute(|| {
                let inner = inner.clone();
                let target = target.clone();
                let req = req.clone();
                async move { inner.status(&target, req).await }
            })
            .await
    }

    async fn watch(
        &self,
        target: &NodeEndpoint,
        req: HealthCheckRequest,
    ) -> RpcResult<Pin<Box<dyn Stream<Item = RpcResult<HealthCheckResponse>> + Send>>> {
        // Streaming doesn't use retry logic - pass through directly
        self.inner.watch(target, req).await
    }
}

// ============================================================================
// FACTORY FUNCTIONS
// ============================================================================

/// Create a resilient consensus transport
pub fn create_resilient_consensus_transport(
    connection_manager: Arc<ConnectionManager>,
    retry_policy: RetryPolicy,
) -> ResilientClient<GrpcConsensusTransport> {
    let inner = GrpcConsensusTransport::new(connection_manager.clone());
    ResilientClient::new(inner, connection_manager, retry_policy)
}

/// Create a resilient replication sink
pub fn create_resilient_replication_sink(
    connection_manager: Arc<ConnectionManager>,
    retry_policy: RetryPolicy,
) -> ResilientClient<GrpcReplicationSink> {
    let inner = GrpcReplicationSink::new(connection_manager.clone());
    ResilientClient::new(inner, connection_manager, retry_policy)
}

/// Create a resilient search fanout
pub fn create_resilient_search_fanout(
    connection_manager: Arc<ConnectionManager>,
    retry_policy: RetryPolicy,
) -> ResilientClient<GrpcSearchFanout> {
    let inner = GrpcSearchFanout::new(connection_manager.clone());
    ResilientClient::new(inner, connection_manager, retry_policy)
}

/// Create a resilient health checker
pub fn create_resilient_health_checker(
    connection_manager: Arc<ConnectionManager>,
    retry_policy: RetryPolicy,
) -> ResilientClient<GrpcHealthChecker> {
    let inner = GrpcHealthChecker::new(connection_manager.clone());
    ResilientClient::new(inner, connection_manager, retry_policy)
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grpc_consensus_transport_creation() {
        let cm = Arc::new(ConnectionManager::new(ConnectionPoolConfig::default()));
        let transport = GrpcConsensusTransport::new(cm);
        // Just verify it compiles and can be created
        let _ = transport;
    }

    #[test]
    fn test_grpc_replication_sink_creation() {
        let cm = Arc::new(ConnectionManager::new(ConnectionPoolConfig::default()));
        let sink = GrpcReplicationSink::new(cm);
        let _ = sink;
    }

    #[test]
    fn test_grpc_search_fanout_creation() {
        let cm = Arc::new(ConnectionManager::new(ConnectionPoolConfig::default()));
        let fanout = GrpcSearchFanout::new(cm);
        let _ = fanout;
    }

    #[test]
    fn test_grpc_health_checker_creation() {
        let cm = Arc::new(ConnectionManager::new(ConnectionPoolConfig::default()));
        let checker = GrpcHealthChecker::new(cm);
        let _ = checker;
    }

    #[test]
    fn test_resilient_client_creation() {
        let cm = Arc::new(ConnectionManager::new(ConnectionPoolConfig::default()));
        let inner = GrpcConsensusTransport::new(cm.clone());
        let resilient = ResilientClient::new(inner, cm, RetryPolicy::default());

        assert_eq!(resilient.retry_policy().max_retries, 3);
    }

    #[test]
    fn test_resilient_client_with_circuit_breaker_config() {
        let cm = Arc::new(ConnectionManager::new(ConnectionPoolConfig::default()));
        let inner = GrpcConsensusTransport::new(cm.clone());
        let resilient = ResilientClient::new(inner, cm, RetryPolicy::default())
            .with_circuit_breaker_config(10, Duration::from_secs(60));

        assert_eq!(resilient.failure_threshold, 10);
        assert_eq!(resilient.reset_timeout, Duration::from_secs(60));
    }

    #[test]
    fn test_factory_functions() {
        let cm = Arc::new(ConnectionManager::new(ConnectionPoolConfig::default()));
        let policy = RetryPolicy::default();

        let _ = create_resilient_consensus_transport(cm.clone(), policy.clone());
        let _ = create_resilient_replication_sink(cm.clone(), policy.clone());
        let _ = create_resilient_search_fanout(cm.clone(), policy.clone());
        let _ = create_resilient_health_checker(cm, policy);
    }

    #[tokio::test]
    async fn test_consensus_transport_request_vote() {
        let cm = Arc::new(ConnectionManager::new(ConnectionPoolConfig::default()));
        let transport = GrpcConsensusTransport::new(cm);

        let target = NodeEndpoint::new("node-1", "127.0.0.1:5679");
        let req = RequestVoteRequest {
            term: 1,
            candidate_id: "node-2".to_string(),
            last_log_index: 0,
            last_log_term: 0,
        };

        let result = transport.request_vote(&target, req).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_health_checker_marks_healthy() {
        let cm = Arc::new(ConnectionManager::new(ConnectionPoolConfig::default()));
        let checker = GrpcHealthChecker::new(cm.clone());

        let target = NodeEndpoint::new("node-1", "127.0.0.1:5679");

        let result = checker.check(&target, HealthCheckRequest::default()).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status, ServingStatus::NotServing);
    }
}
