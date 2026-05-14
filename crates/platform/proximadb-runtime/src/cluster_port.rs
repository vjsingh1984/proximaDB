//! Cluster orchestration port trait for `proximadb-runtime`.
//!
//! `ClusterPort` is the stable contract that server bootstrap and health
//! endpoints use to query and control the cluster without importing
//! root-crate concrete types.
//!
//! Implemented by the root-crate `ClusterManager`.  When no port is injected
//! (single-node mode), callers hold `None` and skip cluster operations.

use anyhow::Result;
use async_trait::async_trait;

/// Cluster health snapshot as visible from this node.
#[derive(Debug, Clone)]
pub struct ClusterHealthStatus {
    pub cluster_id: String,
    pub is_leader: bool,
    pub total_nodes: usize,
    pub healthy_nodes: usize,
    pub unhealthy_nodes: usize,
    pub shard_count: usize,
}

/// Port for cluster lifecycle and health operations.
///
/// Implemented by the root-crate `ClusterManager`.  In single-node mode
/// the server simply holds `None`; callers must handle that case explicitly.
#[async_trait]
pub trait ClusterPort: Send + Sync {
    /// Return `true` if this node is the current Raft leader.
    async fn is_leader(&self) -> bool;

    /// Snapshot cluster health visible from this node.
    async fn health(&self) -> ClusterHealthStatus;

    /// Start cluster services (Raft, replication, routing).
    async fn start(&self) -> Result<()>;

    /// Gracefully stop cluster services.
    async fn stop(&self) -> Result<()>;
}
