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

//! Common Types for RPC Communication
//!
//! This module provides Rust-native types for inter-node communication.
//! These types are designed to be:
//! - Independent of the proto wire format (allowing trait-based abstraction)
//! - Efficient for in-memory operations
//! - Serializable when needed

pub use proximadb_distance_types::DistanceMetric;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

// ============================================================================
// NODE ENDPOINT
// ============================================================================

/// Represents a network endpoint for a cluster node
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeEndpoint {
    /// Unique node identifier
    pub node_id: String,

    /// Node address (host:port)
    pub address: String,

    /// Whether this endpoint uses TLS
    pub tls: bool,
}

impl NodeEndpoint {
    /// Create a new node endpoint
    pub fn new(node_id: impl Into<String>, address: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            address: address.into(),
            tls: false,
        }
    }

    /// Enable TLS for this endpoint
    pub fn with_tls(mut self) -> Self {
        self.tls = true;
        self
    }
}

impl std::fmt::Display for NodeEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.node_id, self.address)
    }
}

// ============================================================================
// CONSENSUS TYPES (Raft)
// ============================================================================

/// Request for Raft RequestVote RPC
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestVoteRequest {
    /// Candidate's term
    pub term: u64,

    /// Candidate requesting vote
    pub candidate_id: String,

    /// Index of candidate's last log entry
    pub last_log_index: u64,

    /// Term of candidate's last log entry
    pub last_log_term: u64,
}

/// Response for Raft RequestVote RPC
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestVoteResponse {
    /// Current term, for candidate to update itself
    pub term: u64,

    /// True means candidate received vote
    pub vote_granted: bool,
}

/// Request for Raft AppendEntries RPC
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendEntriesRequest {
    /// Leader's term
    pub term: u64,

    /// Leader ID so follower can redirect clients
    pub leader_id: String,

    /// Index of log entry immediately preceding new ones
    pub prev_log_index: u64,

    /// Term of prev_log_index entry
    pub prev_log_term: u64,

    /// Log entries to store (empty for heartbeat)
    pub entries: Vec<RpcLogEntry>,

    /// Leader's commit index
    pub leader_commit: u64,
}

/// Response for Raft AppendEntries RPC
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendEntriesResponse {
    /// Current term, for leader to update itself
    pub term: u64,

    /// True if follower contained entry matching prev_log_index and prev_log_term
    pub success: bool,

    /// For fast log reconciliation: last known matching index
    pub match_index: Option<u64>,

    /// For fast log reconciliation: conflicting entry's term
    pub conflict_term: Option<u64>,

    /// For fast log reconciliation: first index of conflicting term
    pub conflict_index: Option<u64>,
}

/// Raft log entry in over-the-wire RPC form (serialized command bytes).
///
/// Naming note: this type used to be called `LogEntry` and collided with
/// `cluster::consensus::LogEntry` (the in-memory typed Raft entry with a
/// `Command` enum payload). consensus.rs already imported this type with
/// `as RpcLogEntry` to disambiguate at every call site — renaming the
/// canonical type to match that alias eliminates the alias bookkeeping.
/// The 4 other LogEntry types in the workspace (proto v1, proto
/// cluster.v1, observability-query log search result, and the now-
/// distinguished consensus one) are unrelated domains.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcLogEntry {
    /// Term when entry was received by leader
    pub term: u64,

    /// Index of this entry in the log
    pub index: u64,

    /// The command to apply to the state machine
    pub command: Vec<u8>,

    /// Entry type
    pub entry_type: LogEntryType,
}

/// Types of log entries
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LogEntryType {
    /// Normal command entry
    #[default]
    Command,
    /// No-op entry for leader establishment
    Noop,
    /// Configuration change entry
    Config,
}

/// Request for Raft InstallSnapshot RPC
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallSnapshotRequest {
    /// Leader's term
    pub term: u64,

    /// Leader ID
    pub leader_id: String,

    /// The snapshot replaces all entries up through and including this index
    pub last_included_index: u64,

    /// Term of last_included_index
    pub last_included_term: u64,

    /// Byte offset where chunk is positioned
    pub offset: u64,

    /// Raw bytes of the snapshot chunk
    pub data: Vec<u8>,

    /// True if this is the last chunk
    pub done: bool,
}

/// Response for Raft InstallSnapshot RPC
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallSnapshotResponse {
    /// Current term
    pub term: u64,

    /// Bytes successfully stored
    pub bytes_stored: u64,
}

// ============================================================================
// REPLICATION TYPES
// ============================================================================

/// Request for data replication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicateRequest {
    /// Source node ID
    pub source_node_id: String,

    /// Shard this entry belongs to
    pub shard_id: String,

    /// Log sequence number for ordering
    pub lsn: u64,

    /// Timestamp of the write (nanoseconds since epoch)
    pub timestamp: i64,

    /// Type of replication operation
    pub operation: ReplicationOperation,

    /// Serialized data
    pub data: Vec<u8>,

    /// CRC32 checksum for integrity verification
    pub checksum: u32,

    /// Required consistency level
    pub consistency: ConsistencyLevel,

    /// Request timeout
    pub timeout: Duration,
}

/// Response for data replication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplicateResponse {
    /// Node that processed the request
    pub node_id: String,

    /// LSN that was acknowledged
    pub acked_lsn: u64,

    /// Whether replication was successful
    pub success: bool,

    /// Error message if failed
    pub error: Option<String>,

    /// Processing latency
    pub latency: Duration,
}

/// Types of replication operations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplicationOperation {
    /// Insert new vectors
    Insert,
    /// Update existing vectors
    Update,
    /// Delete vectors
    Delete,
    /// Flush memtable to disk
    Flush,
    /// Compaction operation
    Compact,
    /// Schema change
    SchemaChange,
}

/// Consistency levels for replication
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ConsistencyLevel {
    /// Only one node needs to acknowledge
    One,
    /// Majority of nodes must acknowledge
    #[default]
    Quorum,
    /// All nodes must acknowledge
    All,
    /// Local datacenter quorum
    LocalQuorum,
}

/// Request to pull missed entries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullEntriesRequest {
    /// Node requesting entries
    pub node_id: String,

    /// Shard to pull entries for
    pub shard_id: String,

    /// Start LSN (exclusive)
    pub from_lsn: u64,

    /// Maximum number of entries to return
    pub max_entries: u32,
}

/// Request to acknowledge replication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckReplicationRequest {
    /// Node acknowledging
    pub node_id: String,

    /// Shard ID
    pub shard_id: String,

    /// Highest LSN that was successfully applied
    pub acked_lsn: u64,
}

/// Response to replication acknowledgment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckReplicationResponse {
    /// Whether acknowledgment was recorded
    pub success: bool,

    /// Current primary's LSN (for lag detection)
    pub primary_lsn: u64,
}

// ============================================================================
// SEARCH FANOUT TYPES
// ============================================================================

/// Request for shard search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardSearchRequest {
    /// Request ID for tracing
    pub request_id: String,

    /// Collection to search
    pub collection: String,

    /// Shard to search
    pub shard_id: String,

    /// Query vector
    pub vector: Vec<f32>,

    /// Number of results to return
    pub top_k: u32,

    /// Optional metadata filter (JSON)
    pub filter: Option<String>,

    /// Search parameters
    pub params: RpcSearchParams,

    /// Request timeout
    pub timeout: Duration,

    /// Whether to include vectors in response
    pub include_vectors: bool,

    /// Tenant context
    pub tenant_id: Option<String>,

    /// Domain context
    pub domain_id: Option<String>,
}

/// Backwards-compat alias for [`RpcSearchParams`].
pub type SearchParams = RpcSearchParams;

/// Search parameters
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RpcSearchParams {
    /// Distance metric to use
    pub metric: DistanceMetric,

    /// Minimum similarity score threshold
    pub min_score: Option<f32>,

    /// EF search parameter for HNSW
    pub ef_search: Option<u32>,

    /// Number of probes for IVF
    pub n_probes: Option<u32>,
}

/// Response for shard search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardSearchResponse {
    /// Request ID (echoed)
    pub request_id: String,

    /// Shard that was searched
    pub shard_id: String,

    /// Search results
    pub results: Vec<ShardSearchResult>,

    /// Number of vectors scanned
    pub vectors_scanned: u64,

    /// Search latency
    pub latency: Duration,

    /// Whether search was truncated due to timeout
    pub truncated: bool,
}

/// Individual search result from a shard
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardSearchResult {
    /// Vector ID
    pub id: String,

    /// Distance/similarity score
    pub score: f32,

    /// Optional vector data
    pub vector: Option<Vec<f32>>,

    /// Metadata (JSON)
    pub metadata: Option<String>,
}

/// Request to forward a write
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardWriteRequest {
    /// Request ID for tracing
    pub request_id: String,

    /// Collection to write to
    pub collection: String,

    /// Target shard
    pub shard_id: String,

    /// Records to write
    pub records: Vec<WriteRecord>,

    /// Required consistency level
    pub consistency: ConsistencyLevel,

    /// Request timeout
    pub timeout: Duration,

    /// Tenant context
    pub tenant_id: Option<String>,

    /// Domain context
    pub domain_id: Option<String>,
}

/// A record to write
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WriteRecord {
    /// Record ID
    pub id: String,

    /// Vector data
    pub vector: Vec<f32>,

    /// Metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Response to a write forward
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardWriteResponse {
    /// Request ID (echoed)
    pub request_id: String,

    /// Number of records written
    pub records_written: u32,

    /// Number of replicas that acknowledged
    pub replicas_acked: u32,

    /// Write latency
    pub latency: Duration,

    /// Error message if partially failed
    pub error: Option<String>,
}

// ============================================================================
// HEALTH TYPES
// ============================================================================

/// Request for health check
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HealthCheckRequest {
    /// Service name to check (empty for overall node health)
    pub service: String,
}

/// Response to health check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResponse {
    /// Health status
    pub status: ServingStatus,
}

/// Serving status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ServingStatus {
    /// Node is healthy and serving requests
    #[default]
    Serving,
    /// Node is not serving requests
    NotServing,
    /// Service is unknown
    ServiceUnknown,
}

/// Request for detailed status
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatusRequest {
    /// Include detailed metrics
    pub include_metrics: bool,

    /// Include shard information
    pub include_shards: bool,
}

/// Detailed status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusResponse {
    /// Node identifier
    pub node_id: String,

    /// Node role in the cluster
    pub role: NodeRole,

    /// Current Raft term
    pub current_term: u64,

    /// Current leader ID (if known)
    pub leader_id: Option<String>,

    /// Node uptime in seconds
    pub uptime_seconds: u64,

    /// Number of active connections
    pub active_connections: u32,

    /// Memory usage in bytes
    pub memory_bytes: u64,

    /// CPU usage percentage (0-100)
    pub cpu_percent: f32,

    /// Shard status (if requested)
    pub shards: Vec<ShardStatus>,

    /// Replication lag in milliseconds (if replica)
    pub replication_lag_ms: Option<u64>,
}

/// Node roles
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NodeRole {
    /// Raft follower
    #[default]
    Follower,
    /// Raft candidate
    Candidate,
    /// Raft leader
    Leader,
    /// Observer (non-voting member)
    Observer,
}

/// Status of a shard on a node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardStatus {
    /// Shard identifier
    pub shard_id: String,

    /// Collection this shard belongs to
    pub collection: String,

    /// Whether this node is primary for this shard
    pub is_primary: bool,

    /// Shard state
    pub state: ShardState,

    /// Current LSN for this shard
    pub current_lsn: u64,

    /// Vector count in this shard
    pub vector_count: u64,

    /// Disk usage in bytes
    pub disk_bytes: u64,
}

/// Shard states
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ShardState {
    /// Shard is active and serving requests
    Active,
    /// Shard is being created/initialized
    #[default]
    Initializing,
    /// Shard is catching up (replica behind primary)
    CatchingUp,
    /// Shard is being relocated to another node
    Relocating,
    /// Shard is inactive/offline
    Inactive,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_endpoint() {
        let endpoint = NodeEndpoint::new("node-1", "127.0.0.1:5679");
        assert_eq!(endpoint.node_id, "node-1");
        assert_eq!(endpoint.address, "127.0.0.1:5679");
        assert!(!endpoint.tls);

        let endpoint = endpoint.with_tls();
        assert!(endpoint.tls);

        assert_eq!(format!("{}", endpoint), "node-1@127.0.0.1:5679");
    }

    #[test]
    fn test_request_vote_request() {
        let req = RequestVoteRequest {
            term: 5,
            candidate_id: "node-1".to_string(),
            last_log_index: 100,
            last_log_term: 4,
        };

        assert_eq!(req.term, 5);
        assert_eq!(req.candidate_id, "node-1");
    }

    #[test]
    fn test_consistency_level_default() {
        let level: ConsistencyLevel = Default::default();
        assert_eq!(level, ConsistencyLevel::Quorum);
    }

    #[test]
    fn test_search_params_default() {
        let params: RpcSearchParams = Default::default();
        assert_eq!(params.metric, DistanceMetric::L2);
        assert!(params.min_score.is_none());
    }

    #[test]
    fn test_shard_state_default() {
        let state: ShardState = Default::default();
        assert_eq!(state, ShardState::Initializing);
    }

    #[test]
    fn test_node_role_default() {
        let role: NodeRole = Default::default();
        assert_eq!(role, NodeRole::Follower);
    }

    #[test]
    fn test_log_entry() {
        let entry = RpcLogEntry {
            term: 1,
            index: 1,
            command: vec![1, 2, 3],
            entry_type: LogEntryType::Command,
        };

        assert_eq!(entry.term, 1);
        assert_eq!(entry.index, 1);
        assert_eq!(entry.entry_type, LogEntryType::Command);
    }

    #[test]
    fn test_write_record() {
        let mut metadata = HashMap::new();
        metadata.insert("key".to_string(), serde_json::json!("value"));

        let record = WriteRecord {
            id: "vec-1".to_string(),
            vector: vec![0.1, 0.2, 0.3],
            metadata,
        };

        assert_eq!(record.id, "vec-1");
        assert_eq!(record.vector.len(), 3);
        assert!(record.metadata.contains_key("key"));
    }
}
