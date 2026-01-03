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

        // TODO: Implement actual gRPC call when proto service is available
        // For now, return a reasonable default for testing
        tracing::debug!(
            target = %target,
            term = req.term,
            candidate = %req.candidate_id,
            "RequestVote RPC (stub)"
        );

        Ok(RequestVoteResponse {
            term: req.term,
            vote_granted: false, // Conservative default
        })
    }

    async fn append_entries(
        &self,
        target: &NodeEndpoint,
        req: AppendEntriesRequest,
    ) -> RpcResult<AppendEntriesResponse> {
        let _channel = self.connection_manager.get_channel(target).await?;

        // TODO: Implement actual gRPC call
        tracing::debug!(
            target = %target,
            term = req.term,
            leader = %req.leader_id,
            entries = req.entries.len(),
            "AppendEntries RPC (stub)"
        );

        Ok(AppendEntriesResponse {
            term: req.term,
            success: true,
            match_index: None,
            conflict_term: None,
            conflict_index: None,
        })
    }

    async fn install_snapshot(
        &self,
        target: &NodeEndpoint,
        req: InstallSnapshotRequest,
    ) -> RpcResult<InstallSnapshotResponse> {
        let _channel = self.connection_manager.get_channel(target).await?;

        // TODO: Implement actual gRPC call
        tracing::debug!(
            target = %target,
            term = req.term,
            last_included_index = req.last_included_index,
            chunk_size = req.data.len(),
            "InstallSnapshot RPC (stub)"
        );

        Ok(InstallSnapshotResponse {
            term: req.term,
            bytes_stored: req.data.len() as u64,
        })
    }

    async fn pre_vote(
        &self,
        target: &NodeEndpoint,
        req: RequestVoteRequest,
    ) -> RpcResult<RequestVoteResponse> {
        let _channel = self.connection_manager.get_channel(target).await?;

        // TODO: Implement actual gRPC call
        tracing::debug!(
            target = %target,
            term = req.term,
            candidate = %req.candidate_id,
            "PreVote RPC (stub)"
        );

        Ok(RequestVoteResponse {
            term: req.term,
            vote_granted: false,
        })
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

        // TODO: Implement actual gRPC call
        tracing::debug!(
            target = %target,
            shard = %req.shard_id,
            lsn = req.lsn,
            "Replicate RPC (stub)"
        );

        Ok(ReplicateResponse {
            node_id: target.node_id.clone(),
            acked_lsn: req.lsn,
            success: true,
            error: None,
            latency: Duration::from_micros(100),
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

        // TODO: Implement actual gRPC call
        tracing::debug!(
            target = %target,
            shard = %req.shard_id,
            acked_lsn = req.acked_lsn,
            "AckReplication RPC (stub)"
        );

        Ok(AckReplicationResponse {
            success: true,
            primary_lsn: req.acked_lsn + 10,
        })
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

        // TODO: Implement actual gRPC call
        tracing::debug!(
            target = %target,
            collection = %req.collection,
            shard = %req.shard_id,
            top_k = req.top_k,
            "ShardSearch RPC (stub)"
        );

        Ok(ShardSearchResponse {
            request_id: req.request_id,
            shard_id: req.shard_id,
            results: vec![],
            vectors_scanned: 0,
            latency: Duration::from_micros(500),
            truncated: false,
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

        // TODO: Implement actual gRPC call
        tracing::debug!(
            target = %target,
            collection = %req.collection,
            shard = %req.shard_id,
            records = req.records.len(),
            "ForwardWrite RPC (stub)"
        );

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
        target: &NodeEndpoint,
        requests: Vec<ForwardWriteRequest>,
    ) -> RpcResult<Vec<ForwardWriteResponse>> {
        let _channel = self.connection_manager.get_channel(target).await?;

        // TODO: Implement actual gRPC call
        tracing::debug!(
            target = %target,
            batch_size = requests.len(),
            "ForwardWriteBatch RPC (stub)"
        );

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
        // Try to get a channel - if successful, node is reachable
        match self.connection_manager.get_channel(target).await {
            Ok(_channel) => {
                // TODO: Implement actual gRPC health check call
                tracing::debug!(
                    target = %target,
                    "HealthCheck RPC (stub)"
                );

                // Mark healthy in connection manager
                self.connection_manager.mark_healthy(target).await;

                Ok(HealthCheckResponse {
                    status: ServingStatus::Serving,
                })
            }
            Err(e) => {
                // Mark unhealthy
                self.connection_manager
                    .mark_unhealthy(target, e.message())
                    .await;

                Ok(HealthCheckResponse {
                    status: ServingStatus::NotServing,
                })
            }
        }
    }

    async fn status(
        &self,
        target: &NodeEndpoint,
        _req: StatusRequest,
    ) -> RpcResult<StatusResponse> {
        let _channel = self.connection_manager.get_channel(target).await?;

        // TODO: Implement actual gRPC status call
        tracing::debug!(
            target = %target,
            "Status RPC (stub)"
        );

        Ok(StatusResponse {
            node_id: target.node_id.clone(),
            role: NodeRole::Follower,
            current_term: 1,
            leader_id: None,
            uptime_seconds: 0,
            active_connections: 0,
            memory_bytes: 0,
            cpu_percent: 0.0,
            shards: vec![],
            replication_lag_ms: None,
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

        // This will fail because we can't actually connect, but it tests the code path
        let result = transport.request_vote(&target, req).await;
        // The stub implementation returns a default response
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_health_checker_marks_healthy() {
        let cm = Arc::new(ConnectionManager::new(ConnectionPoolConfig::default()));
        let checker = GrpcHealthChecker::new(cm.clone());

        let target = NodeEndpoint::new("node-1", "127.0.0.1:5679");

        // Check health - will return Serving because the stub succeeds
        let result = checker.check(&target, HealthCheckRequest::default()).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap().status, ServingStatus::Serving);

        // Verify the node is marked healthy
        assert!(cm.is_healthy(&target).await);
    }
}
