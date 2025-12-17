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

//! Raft Consensus Module
//!
//! Implements Raft consensus protocol for distributed coordination in ProximaDB.
//! Handles leader election, log replication, and state machine management.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Configuration for the Raft consensus module
#[derive(Debug, Clone)]
pub struct ConsensusConfig {
    /// Election timeout range (min, max) in milliseconds
    pub election_timeout_ms: (u64, u64),
    /// Heartbeat interval in milliseconds
    pub heartbeat_interval_ms: u64,
    /// Maximum entries per append entries RPC
    pub max_entries_per_request: usize,
    /// Snapshot threshold (number of log entries before snapshot)
    pub snapshot_threshold: u64,
    /// Enable pre-vote to prevent disruptions from partitioned nodes
    pub enable_pre_vote: bool,
}

impl Default for ConsensusConfig {
    fn default() -> Self {
        Self {
            election_timeout_ms: (150, 300),
            heartbeat_interval_ms: 50,
            max_entries_per_request: 100,
            snapshot_threshold: 10000,
            enable_pre_vote: true,
        }
    }
}

/// State of a node in the Raft protocol
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConsensusState {
    /// Node is a follower
    Follower,
    /// Node is a candidate seeking election
    Candidate,
    /// Node is the leader
    Leader,
}

/// A log entry in the Raft log
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// Term when entry was received by leader
    pub term: u64,
    /// Index of this entry in the log
    pub index: u64,
    /// Command to be applied to state machine
    pub command: Command,
}

/// Commands that can be applied to the state machine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    /// No operation (used for leader establishment)
    Noop,
    /// Create a collection
    CreateCollection {
        collection_id: String,
        name: String,
        dimension: u32,
        shard_count: u32,
    },
    /// Delete a collection
    DeleteCollection { collection_id: String },
    /// Update shard placement
    UpdateShardPlacement {
        shard_id: String,
        collection_id: String,
        primary_node: String,
        replica_nodes: Vec<String>,
    },
    /// Add a node to the cluster
    AddNode { node_id: String, address: String },
    /// Remove a node from the cluster
    RemoveNode { node_id: String },
    /// Update cluster configuration
    UpdateConfig { key: String, value: String },
}

/// Persistent state on all servers
#[derive(Debug, Clone)]
struct PersistentState {
    /// Latest term server has seen
    current_term: u64,
    /// Candidate that received vote in current term
    voted_for: Option<String>,
    /// Log entries
    log: Vec<LogEntry>,
}

impl Default for PersistentState {
    fn default() -> Self {
        Self {
            current_term: 0,
            voted_for: None,
            log: Vec::new(),
        }
    }
}

/// Volatile state on all servers
#[derive(Debug, Clone, Default)]
struct VolatileState {
    /// Index of highest log entry known to be committed
    commit_index: u64,
    /// Index of highest log entry applied to state machine
    last_applied: u64,
}

/// Volatile state on leaders (reinitialized after election)
#[derive(Debug, Clone)]
struct LeaderState {
    /// For each server, index of next log entry to send
    next_index: HashMap<String, u64>,
    /// For each server, index of highest log entry known to be replicated
    match_index: HashMap<String, u64>,
}

impl Default for LeaderState {
    fn default() -> Self {
        Self {
            next_index: HashMap::new(),
            match_index: HashMap::new(),
        }
    }
}

/// Result of applying a command
#[derive(Debug)]
pub struct ApplyResult {
    /// Whether the command was successfully applied
    pub success: bool,
    /// Optional response data
    pub response: Option<Vec<u8>>,
    /// Error message if failed
    pub error: Option<String>,
}

/// Raft consensus implementation
pub struct RaftConsensus {
    config: ConsensusConfig,
    /// This node's ID
    node_id: String,
    /// Current state (follower, candidate, leader)
    state: Arc<RwLock<ConsensusState>>,
    /// Persistent state
    persistent: Arc<RwLock<PersistentState>>,
    /// Volatile state
    volatile: Arc<RwLock<VolatileState>>,
    /// Leader-specific state
    leader_state: Arc<RwLock<Option<LeaderState>>>,
    /// Current leader ID
    current_leader: Arc<RwLock<Option<String>>>,
    /// Whether the consensus module is running
    running: Arc<RwLock<bool>>,
}

impl RaftConsensus {
    /// Create a new Raft consensus instance
    pub fn new(config: ConsensusConfig) -> Result<Self> {
        Ok(Self {
            config,
            node_id: uuid::Uuid::new_v4().to_string(),
            state: Arc::new(RwLock::new(ConsensusState::Follower)),
            persistent: Arc::new(RwLock::new(PersistentState::default())),
            volatile: Arc::new(RwLock::new(VolatileState::default())),
            leader_state: Arc::new(RwLock::new(None)),
            current_leader: Arc::new(RwLock::new(None)),
            running: Arc::new(RwLock::new(false)),
        })
    }

    /// Start the consensus module
    pub async fn start(&mut self) -> Result<()> {
        let mut running = self.running.write().await;
        if *running {
            return Ok(());
        }
        *running = true;

        tracing::info!(node_id = %self.node_id, "Starting Raft consensus module");

        // In a full implementation, this would start:
        // 1. Election timer
        // 2. Heartbeat sender (if leader)
        // 3. Log replication

        Ok(())
    }

    /// Stop the consensus module
    pub async fn stop(&mut self) -> Result<()> {
        let mut running = self.running.write().await;
        if !*running {
            return Ok(());
        }
        *running = false;

        tracing::info!(node_id = %self.node_id, "Stopping Raft consensus module");

        Ok(())
    }

    /// Get current consensus state
    pub async fn get_state(&self) -> ConsensusState {
        *self.state.read().await
    }

    /// Check if this node is the leader
    pub async fn is_leader(&self) -> bool {
        *self.state.read().await == ConsensusState::Leader
    }

    /// Get current term
    pub async fn current_term(&self) -> u64 {
        self.persistent.read().await.current_term
    }

    /// Get current leader ID
    pub async fn get_leader(&self) -> Option<String> {
        self.current_leader.read().await.clone()
    }

    /// Propose a command to be replicated
    pub async fn propose(&self, command: Command) -> Result<ApplyResult> {
        let state = self.state.read().await;

        if *state != ConsensusState::Leader {
            return Ok(ApplyResult {
                success: false,
                response: None,
                error: Some("Not the leader".to_string()),
            });
        }

        // In a full implementation, this would:
        // 1. Append to local log
        // 2. Replicate to followers
        // 3. Wait for majority acknowledgment
        // 4. Apply to state machine

        let mut persistent = self.persistent.write().await;
        let index = persistent.log.len() as u64 + 1;
        let entry = LogEntry {
            term: persistent.current_term,
            index,
            command,
        };
        persistent.log.push(entry);

        tracing::debug!(index, "Command proposed and appended to log");

        Ok(ApplyResult {
            success: true,
            response: None,
            error: None,
        })
    }

    /// Handle RequestVote RPC
    pub async fn handle_request_vote(
        &self,
        term: u64,
        candidate_id: &str,
        last_log_index: u64,
        last_log_term: u64,
    ) -> (u64, bool) {
        let mut persistent = self.persistent.write().await;

        // Reply false if term < currentTerm
        if term < persistent.current_term {
            return (persistent.current_term, false);
        }

        // If term > currentTerm, update currentTerm and become follower
        if term > persistent.current_term {
            persistent.current_term = term;
            persistent.voted_for = None;
            let mut state = self.state.write().await;
            *state = ConsensusState::Follower;
        }

        // Check if we can grant vote
        let can_vote = persistent.voted_for.is_none()
            || persistent.voted_for.as_ref() == Some(&candidate_id.to_string());

        // Check if candidate's log is at least as up-to-date as ours
        let our_last_log = persistent.log.last();
        let log_ok = match our_last_log {
            None => true,
            Some(entry) => {
                last_log_term > entry.term
                    || (last_log_term == entry.term && last_log_index >= entry.index)
            }
        };

        if can_vote && log_ok {
            persistent.voted_for = Some(candidate_id.to_string());
            (persistent.current_term, true)
        } else {
            (persistent.current_term, false)
        }
    }

    /// Handle AppendEntries RPC
    pub async fn handle_append_entries(
        &self,
        term: u64,
        leader_id: &str,
        prev_log_index: u64,
        prev_log_term: u64,
        entries: Vec<LogEntry>,
        leader_commit: u64,
    ) -> (u64, bool) {
        let mut persistent = self.persistent.write().await;

        // Reply false if term < currentTerm
        if term < persistent.current_term {
            return (persistent.current_term, false);
        }

        // If term >= currentTerm, recognize leader
        if term >= persistent.current_term {
            persistent.current_term = term;
            let mut state = self.state.write().await;
            *state = ConsensusState::Follower;
            let mut current_leader = self.current_leader.write().await;
            *current_leader = Some(leader_id.to_string());
        }

        // Check if log contains entry at prevLogIndex with prevLogTerm
        if prev_log_index > 0 {
            if let Some(entry) = persistent.log.get(prev_log_index as usize - 1) {
                if entry.term != prev_log_term {
                    // Log inconsistency - delete conflicting entries
                    persistent.log.truncate(prev_log_index as usize - 1);
                    return (persistent.current_term, false);
                }
            } else {
                return (persistent.current_term, false);
            }
        }

        // Append new entries
        for entry in entries {
            if entry.index as usize <= persistent.log.len() {
                // Entry already exists, check for conflict
                if let Some(existing) = persistent.log.get(entry.index as usize - 1) {
                    if existing.term != entry.term {
                        persistent.log.truncate(entry.index as usize - 1);
                        persistent.log.push(entry);
                    }
                }
            } else {
                persistent.log.push(entry);
            }
        }

        // Update commit index
        if leader_commit > self.volatile.read().await.commit_index {
            let last_index = persistent.log.len() as u64;
            let mut volatile = self.volatile.write().await;
            volatile.commit_index = std::cmp::min(leader_commit, last_index);
        }

        (persistent.current_term, true)
    }

    /// Get log entries starting from an index
    pub async fn get_log_entries(&self, from_index: u64) -> Vec<LogEntry> {
        let persistent = self.persistent.read().await;
        if from_index == 0 || from_index > persistent.log.len() as u64 {
            return Vec::new();
        }
        persistent.log[from_index as usize - 1..].to_vec()
    }

    /// Get the last log index and term
    pub async fn last_log_info(&self) -> (u64, u64) {
        let persistent = self.persistent.read().await;
        match persistent.log.last() {
            Some(entry) => (entry.index, entry.term),
            None => (0, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_consensus_creation() {
        let config = ConsensusConfig::default();
        let consensus = RaftConsensus::new(config);
        assert!(consensus.is_ok());
    }

    #[tokio::test]
    async fn test_initial_state() {
        let consensus = RaftConsensus::new(ConsensusConfig::default()).unwrap();

        assert_eq!(consensus.get_state().await, ConsensusState::Follower);
        assert_eq!(consensus.current_term().await, 0);
        assert!(!consensus.is_leader().await);
    }

    #[tokio::test]
    async fn test_start_stop() {
        let mut consensus = RaftConsensus::new(ConsensusConfig::default()).unwrap();

        consensus.start().await.unwrap();
        assert!(*consensus.running.read().await);

        consensus.stop().await.unwrap();
        assert!(!*consensus.running.read().await);
    }

    #[tokio::test]
    async fn test_request_vote_term_check() {
        let consensus = RaftConsensus::new(ConsensusConfig::default()).unwrap();

        // Set current term to 5
        {
            let mut persistent = consensus.persistent.write().await;
            persistent.current_term = 5;
        }

        // Request vote with lower term should be rejected
        let (term, granted) = consensus.handle_request_vote(3, "candidate-1", 0, 0).await;
        assert_eq!(term, 5);
        assert!(!granted);

        // Request vote with higher term should update our term
        let (term, granted) = consensus.handle_request_vote(10, "candidate-1", 0, 0).await;
        assert_eq!(term, 10);
        assert!(granted);
    }
}
