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

//! RPC Abstraction Layer for Inter-Node Communication
//!
//! This module provides SOLID-compliant abstractions for cluster communication.
//! It defines traits that separate concerns and enable dependency inversion,
//! allowing the cluster infrastructure to work with any RPC implementation.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                          Cluster Modules                                 │
//! │  (Consensus, Replication, Distributed Ops)                              │
//! └─────────────────────────────────────────────────────────────────────────┘
//!                                    │
//!                                    │ depends on
//!                                    ▼
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                         RPC Traits (this module)                         │
//! │  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐       │
//! │  │ ConsensusTransport│  │ ReplicationSink │  │ SearchFanout    │       │
//! │  └──────────────────┘  └──────────────────┘  └──────────────────┘       │
//! │  ┌──────────────────┐  ┌──────────────────┐                              │
//! │  │ HealthChecker    │  │ ConnectionPool  │                              │
//! │  └──────────────────┘  └──────────────────┘                              │
//! └─────────────────────────────────────────────────────────────────────────┘
//!                                    │
//!                                    │ implemented by
//!                                    ▼
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                      Concrete Implementations                            │
//! │  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐       │
//! │  │ gRPC Transport   │  │ HTTP Transport  │  │ In-Memory Mock  │       │
//! │  └──────────────────┘  └──────────────────┘  └──────────────────┘       │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## SOLID Principles
//!
//! - **Single Responsibility**: Each trait handles one concern
//!   - `ConsensusTransport`: Raft consensus RPCs only
//!   - `ReplicationSink`: Data replication only
//!   - `SearchFanout`: Distributed query execution only
//!   - `HealthChecker`: Health monitoring only
//!
//! - **Open/Closed**: New transport implementations can be added without
//!   modifying existing code
//!
//! - **Liskov Substitution**: Any implementation can replace another
//!   (e.g., mock for testing, gRPC for production)
//!
//! - **Interface Segregation**: Modules depend only on traits they need
//!   (e.g., consensus module only needs `ConsensusTransport`)
//!
//! - **Dependency Inversion**: High-level modules (consensus, replication)
//!   depend on abstractions (traits), not concrete implementations
//!
//! ## Usage
//!
//! ```ignore
//! use proximadb::cluster::rpc::{ConsensusTransport, NodeEndpoint, RequestVoteRequest};
//!
//! async fn request_votes<T: ConsensusTransport>(
//!     transport: &T,
//!     peers: &[NodeEndpoint],
//!     term: u64,
//! ) -> usize {
//!     let mut votes = 0;
//!     for peer in peers {
//!         let req = RequestVoteRequest {
//!             term,
//!             candidate_id: "this-node".to_string(),
//!             last_log_index: 0,
//!             last_log_term: 0,
//!         };
//!         if let Ok(resp) = transport.request_vote(peer, req).await {
//!             if resp.vote_granted {
//!                 votes += 1;
//!             }
//!         }
//!     }
//!     votes
//! }
//! ```
//!
//! ## Proto Definitions
//!
//! The wire protocol is defined in `proto/proximadb/v1/cluster.proto`.
//! This includes:
//! - `ConsensusService`: Raft consensus RPCs
//! - `ReplicationService`: Data replication RPCs
//! - `SearchFanoutService`: Distributed query RPCs
//! - `HealthService`: Node health monitoring

pub mod connection;
pub mod error;
pub mod grpc_client;
pub mod grpc_server;
pub mod retry;
pub mod traits;
pub mod types;

// Re-export commonly used types for convenience
pub use error::{RpcError, RpcErrorKind, RpcResult};
pub use traits::{
    ConnectionPool, ConsensusTransport, HealthChecker, NodeClient, ReplicationSink, SearchFanout,
};

// Re-export connection management types
pub use connection::{
    CachedHealth, ChannelPool, ConnectionManager, ConnectionPoolConfig, ConnectionStats,
};

// Re-export retry and circuit breaker types
pub use retry::{CircuitBreaker, CircuitState, RetryExecutor, RetryPolicy};

// Re-export gRPC client implementations
pub use grpc_client::{
    GrpcConsensusTransport, GrpcHealthChecker, GrpcReplicationSink, GrpcSearchFanout,
    ResilientClient, create_resilient_consensus_transport, create_resilient_health_checker,
    create_resilient_replication_sink, create_resilient_search_fanout,
};

// Re-export gRPC server implementations
pub use grpc_server::{ConsensusServiceImpl, HealthServiceImpl, ReplicationServiceImpl};
pub use types::{
    // Replication types
    AckReplicationRequest,
    AckReplicationResponse,
    // Consensus types
    AppendEntriesRequest,
    AppendEntriesResponse,
    ConsistencyLevel,
    // Search types
    DistanceMetric,
    ForwardWriteRequest,
    ForwardWriteResponse,
    // Health types
    HealthCheckRequest,
    HealthCheckResponse,
    InstallSnapshotRequest,
    InstallSnapshotResponse,
    LogEntryType,
    // Common types
    NodeEndpoint,
    NodeRole,
    PullEntriesRequest,
    ReplicateRequest,
    ReplicateResponse,
    ReplicationOperation,
    RequestVoteRequest,
    RequestVoteResponse,
    RpcLogEntry,
    SearchParams,
    ServingStatus,
    ShardSearchRequest,
    ShardSearchResponse,
    ShardSearchResult,
    ShardState,
    ShardStatus,
    StatusRequest,
    StatusResponse,
    WriteRecord,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_module_exports() {
        // Verify that all public types are accessible
        let _: NodeEndpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");
        let _: RpcError = RpcError::internal("test");
        let _: ConsistencyLevel = ConsistencyLevel::Quorum;
    }

    #[test]
    fn test_error_types() {
        let err = RpcError::connection("failed to connect");
        assert!(err.is_retryable());

        let err = RpcError::node_not_found("node-1");
        assert!(!err.is_retryable());
    }

    #[test]
    fn test_node_endpoint_creation() {
        let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");
        assert_eq!(endpoint.node_id, "node-1");
        assert_eq!(endpoint.address, "127.0.0.1:5679");
        assert!(!endpoint.tls);

        let endpoint = endpoint.with_tls();
        assert!(endpoint.tls);
    }
}
