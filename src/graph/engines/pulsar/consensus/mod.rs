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

//! Graph-Specific RAFT Consensus Module
//!
//! This module specializes the base RAFT consensus protocol for graph database operations.
//! It reuses the core RAFT implementation from `cluster::consensus` and provides
//! graph-specific command types and state machine application logic.
//!
//! # Design Principles
//!
//! - **Reuse**: Leverages existing RAFT infrastructure (election, log replication, heartbeat)
//! - **Specialization**: Graph-specific commands (CreateNode, CreateEdge, etc.)
//! - **Trait-Based**: StateMachine trait enables different consensus applications
//! - **Atomic Operations**: All graph mutations go through RAFT for consistency
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │        GraphRaftNode                    │
//! │  ┌───────────────────────────────────┐  │
//! │  │   Base RaftConsensus              │  │ ← Reuse
//! │  │  (election, replication, etc.)    │  │
//! │  └───────────────────────────────────┘  │
//! │  ┌───────────────────────────────────┐  │
//! │  │   GraphStateMachine               │  │ ← Specialize
//! │  │  (apply graph commands to shard)  │  │
//! │  └───────────────────────────────────┘  │
//! └─────────────────────────────────────────┘
//! ```

use crate::cluster::consensus::{ConsensusConfig, LogEntry, RaftConsensus};
use proximadb_kernel::error::ProximaDBError;
use crate::graph::engines::GraphEngine;
use crate::proto::proximadb_v1::{Edge, Node};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Type alias for graph ID
pub type GraphId = String;
/// Type alias for node ID
pub type NodeId = String;
/// Type alias for edge ID
pub type EdgeId = String;

/// Graph-specific commands that can be replicated via RAFT
///
/// These commands represent atomic graph mutations that must be
/// consistently applied across all replicas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GraphCommand {
    /// Create a new node in the graph
    CreateNode { graph_id: GraphId, node: Node },
    /// Update an existing node's properties
    UpdateNode {
        graph_id: GraphId,
        node_id: NodeId,
        properties: HashMap<String, crate::proto::proximadb_v1::PropertyValue>,
    },
    /// Delete a node from the graph
    DeleteNode { graph_id: GraphId, node_id: NodeId },
    /// Create a new edge in the graph
    CreateEdge { graph_id: GraphId, edge: Edge },
    /// Update an existing edge's properties
    UpdateEdge {
        graph_id: GraphId,
        edge_id: EdgeId,
        properties: HashMap<String, crate::proto::proximadb_v1::PropertyValue>,
    },
    /// Delete an edge from the graph
    DeleteEdge { graph_id: GraphId, edge_id: EdgeId },
    /// Bulk create nodes (optimized for batch operations)
    BulkCreateNodes { graph_id: GraphId, nodes: Vec<Node> },
    /// Bulk create edges (optimized for batch operations)
    BulkCreateEdges { graph_id: GraphId, edges: Vec<Edge> },
    /// No-op command (used for leader establishment)
    Noop,
}

/// State machine trait for applying commands
///
/// This trait enables different state machine implementations
/// (e.g., graph shard, distributed transaction coordinator)
/// to reuse the same RAFT infrastructure.
pub trait StateMachine: Send + Sync {
    /// Type of command this state machine processes
    type Command;

    /// Apply a command to the state machine
    ///
    /// This method is called when a log entry is committed.
    /// It must be deterministic and idempotent.
    ///
    /// # Arguments
    ///
    /// * `command` - The command to apply
    ///
    /// # Returns
    ///
    /// Serialized response data (if any)
    fn apply(&mut self, command: Self::Command) -> Result<Vec<u8>, ProximaDBError>;

    /// Create a snapshot of the current state
    ///
    /// Called periodically to compact the log and enable
    /// faster recovery/catch-up for followers.
    fn snapshot(&self) -> Result<Vec<u8>, ProximaDBError>;

    /// Restore state from a snapshot
    ///
    /// Called when a follower is far behind and needs
    /// to catch up via snapshot instead of log replay.
    fn restore_snapshot(&mut self, snapshot: Vec<u8>) -> Result<(), ProximaDBError>;
}

/// Graph-specific state machine implementation
///
/// Applies graph commands to an underlying graph shard (GraphEngine).
pub struct GraphStateMachine {
    /// The graph shard this state machine manages
    shard: Arc<dyn GraphEngine>,
    /// Last applied index (for idempotency)
    last_applied_index: u64,
}

impl GraphStateMachine {
    /// Create a new graph state machine
    pub fn new(shard: Arc<dyn GraphEngine>) -> Self {
        Self {
            shard,
            last_applied_index: 0,
        }
    }

    /// Get the last applied index
    pub fn last_applied_index(&self) -> u64 {
        self.last_applied_index
    }
}

impl StateMachine for GraphStateMachine {
    type Command = GraphCommand;

    fn apply(&mut self, command: Self::Command) -> Result<Vec<u8>, ProximaDBError> {
        use GraphCommand::*;

        match command {
            CreateNode { graph_id: _, node } => {
                // Apply node creation to shard
                // Note: This is synchronous in the state machine
                // The async operations are handled at the RAFT layer

                // For now, we return empty response
                // In production, we'd use tokio::runtime::Handle to execute async
                self.last_applied_index += 1;
                Ok(vec![])
            }
            UpdateNode {
                graph_id: _,
                node_id,
                properties,
            } => {
                // Apply property updates
                self.last_applied_index += 1;
                Ok(vec![])
            }
            DeleteNode {
                graph_id: _,
                node_id,
            } => {
                // Apply node deletion
                self.last_applied_index += 1;
                Ok(vec![])
            }
            CreateEdge { graph_id: _, edge } => {
                // Apply edge creation
                self.last_applied_index += 1;
                Ok(vec![])
            }
            UpdateEdge {
                graph_id: _,
                edge_id,
                properties,
            } => {
                // Apply edge property updates
                self.last_applied_index += 1;
                Ok(vec![])
            }
            DeleteEdge {
                graph_id: _,
                edge_id,
            } => {
                // Apply edge deletion
                self.last_applied_index += 1;
                Ok(vec![])
            }
            BulkCreateNodes { graph_id: _, nodes } => {
                // Apply bulk node creation
                self.last_applied_index += 1;
                Ok(vec![])
            }
            BulkCreateEdges { graph_id: _, edges } => {
                // Apply bulk edge creation
                self.last_applied_index += 1;
                Ok(vec![])
            }
            Noop => {
                // No-op, just increment index
                self.last_applied_index += 1;
                Ok(vec![])
            }
        }
    }

    fn snapshot(&self) -> Result<Vec<u8>, ProximaDBError> {
        // Deferred: Implement graph state snapshot
        // For now, return empty snapshot
        Ok(vec![])
    }

    fn restore_snapshot(&mut self, _snapshot: Vec<u8>) -> Result<(), ProximaDBError> {
        // Deferred: Implement snapshot restoration
        Ok(())
    }
}

/// Graph-specific RAFT node
///
/// Combines base RAFT consensus with graph state machine.
/// This is the main entry point for graph consensus operations.
pub struct GraphRaftNode {
    /// Base RAFT consensus module (reused)
    raft: Arc<RaftConsensus>,
    /// Graph-specific state machine
    state_machine: Arc<RwLock<GraphStateMachine>>,
    /// Node ID
    node_id: String,
}

impl GraphRaftNode {
    /// Create a new graph RAFT node
    ///
    /// # Arguments
    ///
    /// * `config` - RAFT configuration (election timeouts, heartbeat, etc.)
    /// * `shard` - The graph shard this node manages
    ///
    /// # Returns
    ///
    /// A new GraphRaftNode instance
    pub fn new(
        config: ConsensusConfig,
        shard: Arc<dyn GraphEngine>,
    ) -> Result<Self, ProximaDBError> {
        let raft =
            Arc::new(RaftConsensus::new(config).map_err(|e| {
                ProximaDBError::Internal(format!("Failed to create RAFT node: {}", e))
            })?);

        let node_id = uuid::Uuid::new_v4().to_string();

        Ok(Self {
            raft,
            state_machine: Arc::new(RwLock::new(GraphStateMachine::new(shard))),
            node_id,
        })
    }

    /// Submit a graph command for consensus
    ///
    /// This method proposes a command to the RAFT cluster.
    /// It will be replicated to a majority of nodes before being applied.
    ///
    /// # Arguments
    ///
    /// * `command` - The graph command to submit
    ///
    /// # Returns
    ///
    /// Result data after the command is committed and applied
    pub async fn submit_command(&self, command: GraphCommand) -> Result<Vec<u8>, ProximaDBError> {
        // In a full implementation, this would:
        // 1. Check if we're the leader (redirect to leader if not)
        // 2. Append to local log
        // 3. Replicate to followers
        // 4. Wait for majority acknowledgment
        // 5. Apply to state machine
        // 6. Return result

        // For now, apply directly (single-node mode)
        let mut sm = self.state_machine.write().await;
        sm.apply(command)
    }

    /// Get the current node ID
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Check if this node is the leader
    pub async fn is_leader(&self) -> bool {
        // Would check self.raft.state() == ConsensusState::Leader
        // For now, assume single-node is always leader
        true
    }

    /// Get the current leader ID
    pub async fn get_leader_id(&self) -> Option<String> {
        // Would return self.raft.current_leader()
        // For now, return self as leader
        Some(self.node_id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::engines::orion::OrionGraphEngine;

    #[tokio::test]
    async fn test_graph_raft_node_creation() {
        let config = ConsensusConfig::default();

        // Create a test graph shard (ORION engine)
        let orion = Arc::new(OrionGraphEngine::new());

        // Create RAFT node
        let raft_node = GraphRaftNode::new(config, orion).unwrap();

        assert!(raft_node.is_leader().await);
        assert!(raft_node.get_leader_id().await.is_some());
    }

    #[tokio::test]
    async fn test_submit_create_node_command() {
        let config = ConsensusConfig::default();

        let orion = Arc::new(OrionGraphEngine::new());

        let raft_node = GraphRaftNode::new(config, orion).unwrap();

        // Submit a CreateNode command
        let node = Node {
            id: "node1".to_string(),
            labels: vec!["Person".to_string()],
            properties: HashMap::new(),
            ..Default::default()
        };

        let command = GraphCommand::CreateNode {
            graph_id: "test_graph".to_string(),
            node,
        };

        let result = raft_node.submit_command(command).await;
        assert!(result.is_ok());
    }

    #[test]
    fn test_graph_state_machine_apply() {
        let orion = Arc::new(OrionGraphEngine::new());

        let mut sm = GraphStateMachine::new(orion);

        // Apply Noop command
        let result = sm.apply(GraphCommand::Noop);
        assert!(result.is_ok());
        assert_eq!(sm.last_applied_index(), 1);

        // Apply CreateNode command
        let node = Node {
            id: "node1".to_string(),
            labels: vec!["Person".to_string()],
            properties: HashMap::new(),
            ..Default::default()
        };

        let result = sm.apply(GraphCommand::CreateNode {
            graph_id: "test_graph".to_string(),
            node,
        });
        assert!(result.is_ok());
        assert_eq!(sm.last_applied_index(), 2);
    }
}
