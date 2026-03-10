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
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::rpc::{
    AppendEntriesRequest, AppendEntriesResponse, CircuitBreaker, ConnectionManager,
    ConnectionPoolConfig, ConsensusTransport, LogEntry as RpcLogEntry, LogEntryType, NodeEndpoint,
    RequestVoteRequest, RetryPolicy,
};
// Re-export for external use
pub use super::rpc::{RequestVoteResponse, RpcResult};

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
    _last_applied: u64,
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
    /// Transport layer for RPC communication (optional, required for distributed mode)
    transport: Option<Arc<dyn ConsensusTransport>>,
    /// Connection manager for resilient connections
    _connection_manager: Option<Arc<ConnectionManager>>,
    /// Peer nodes in the cluster
    peers: Arc<RwLock<Vec<NodeEndpoint>>>,
    /// Circuit breakers per peer (node_id -> CircuitBreaker)
    circuit_breakers: Arc<RwLock<HashMap<String, Arc<CircuitBreaker>>>>,
    /// Retry policy for RPC calls
    _retry_policy: RetryPolicy,
    /// Shutdown signal sender
    shutdown_tx: Option<mpsc::Sender<()>>,
    /// Background task handles
    task_handles: Arc<RwLock<Vec<JoinHandle<()>>>>,
}

impl RaftConsensus {
    /// Create a new Raft consensus instance (standalone mode without RPC transport)
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
            transport: None,
            _connection_manager: None,
            peers: Arc::new(RwLock::new(Vec::new())),
            circuit_breakers: Arc::new(RwLock::new(HashMap::new())),
            _retry_policy: RetryPolicy::default(),
            shutdown_tx: None,
            task_handles: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Create a new Raft consensus instance with RPC transport for distributed mode
    ///
    /// # Arguments
    ///
    /// * `config` - Consensus configuration
    /// * `node_id` - This node's unique identifier
    /// * `transport` - The transport layer for RPC communication
    /// * `peers` - Initial list of peer nodes
    ///
    /// # Example
    ///
    /// ```ignore
    /// let config = ConsensusConfig::default();
    /// let transport = Arc::new(GrpcConsensusTransport::new());
    /// let peers = vec![
    ///     NodeEndpoint::new("node-2", "192.168.1.2:5679"),
    ///     NodeEndpoint::new("node-3", "192.168.1.3:5679"),
    /// ];
    /// let consensus = RaftConsensus::with_transport(config, "node-1", transport, peers)?;
    /// ```
    pub fn with_transport(
        config: ConsensusConfig,
        node_id: impl Into<String>,
        transport: Arc<dyn ConsensusTransport>,
        peers: Vec<NodeEndpoint>,
    ) -> Result<Self> {
        let connection_config = ConnectionPoolConfig::default()
            .with_connect_timeout(Duration::from_millis(config.heartbeat_interval_ms * 2))
            .with_request_timeout(Duration::from_millis(config.election_timeout_ms.0));

        let connection_manager = Arc::new(ConnectionManager::new(connection_config));

        // Initialize circuit breakers for each peer
        let mut breakers = HashMap::new();
        for peer in &peers {
            let breaker = Arc::new(CircuitBreaker::new(
                5,                                                       // Failure threshold
                Duration::from_millis(config.election_timeout_ms.1 * 2), // Reset timeout
            ));
            breakers.insert(peer.node_id.clone(), breaker);
        }

        Ok(Self {
            config,
            node_id: node_id.into(),
            state: Arc::new(RwLock::new(ConsensusState::Follower)),
            persistent: Arc::new(RwLock::new(PersistentState::default())),
            volatile: Arc::new(RwLock::new(VolatileState::default())),
            leader_state: Arc::new(RwLock::new(None)),
            current_leader: Arc::new(RwLock::new(None)),
            running: Arc::new(RwLock::new(false)),
            transport: Some(transport),
            _connection_manager: Some(connection_manager),
            peers: Arc::new(RwLock::new(peers)),
            circuit_breakers: Arc::new(RwLock::new(breakers)),
            _retry_policy: RetryPolicy::default()
                .with_max_retries(2)
                .with_base_delay(Duration::from_millis(50)),
            shutdown_tx: None,
            task_handles: Arc::new(RwLock::new(Vec::new())),
        })
    }

    /// Get this node's ID
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Get the list of peer nodes
    pub async fn get_peers(&self) -> Vec<NodeEndpoint> {
        self.peers.read().await.clone()
    }

    /// Add a peer node
    pub async fn add_peer(&self, peer: NodeEndpoint) {
        let mut peers = self.peers.write().await;
        if !peers.iter().any(|p| p.node_id == peer.node_id) {
            // Create circuit breaker for new peer
            let breaker = Arc::new(CircuitBreaker::new(
                5,
                Duration::from_millis(self.config.election_timeout_ms.1 * 2),
            ));
            {
                let mut breakers = self.circuit_breakers.write().await;
                breakers.insert(peer.node_id.clone(), breaker);
            }
            peers.push(peer);
        }
    }

    /// Remove a peer node
    pub async fn remove_peer(&self, node_id: &str) {
        let mut peers = self.peers.write().await;
        peers.retain(|p| p.node_id != node_id);
        let mut breakers = self.circuit_breakers.write().await;
        breakers.remove(node_id);
    }

    /// Start the consensus module
    ///
    /// When transport is configured, this starts background tasks for:
    /// 1. Election timer - triggers elections when no heartbeat received
    /// 2. Heartbeat sender - sends heartbeats when leader
    pub async fn start(&mut self) -> Result<()> {
        {
            let mut running = self.running.write().await;
            if *running {
                return Ok(());
            }
            *running = true;
        }

        tracing::info!(node_id = %self.node_id, "Starting Raft consensus module");

        // Only start background tasks if transport is configured
        if self.transport.is_some() {
            // Create shutdown channels for both tasks
            let (election_shutdown_tx, election_shutdown_rx) = mpsc::channel(1);
            let (heartbeat_shutdown_tx, heartbeat_shutdown_rx) = mpsc::channel(1);

            // Store the first sender for shutdown (we send to both)
            self.shutdown_tx = Some(election_shutdown_tx);

            // Start election timer task
            let election_handle = self.start_election_timer(election_shutdown_rx).await;
            self.task_handles.write().await.push(election_handle);

            // Start heartbeat task (only runs when leader)
            let heartbeat_handle = self.start_heartbeat_task(heartbeat_shutdown_rx).await;
            self.task_handles.write().await.push(heartbeat_handle);

            // Note: heartbeat_shutdown_tx is dropped here, which will cause the receiver
            // to return None when polled, effectively shutting down the task when running is false.
            drop(heartbeat_shutdown_tx);
        }

        Ok(())
    }

    /// Start the election timer background task
    async fn start_election_timer(&self, mut shutdown_rx: mpsc::Receiver<()>) -> JoinHandle<()> {
        let state = Arc::clone(&self.state);
        let running = Arc::clone(&self.running);
        let node_id = self.node_id.clone();
        let election_timeout_range = self.config.election_timeout_ms;

        // Clone what we need for the election
        let transport = self.transport.clone();
        let peers = Arc::clone(&self.peers);
        let persistent = Arc::clone(&self.persistent);
        let _volatile = Arc::clone(&self.volatile); // Reserved for future use (commit index tracking)
        let current_leader = Arc::clone(&self.current_leader);
        let leader_state = Arc::clone(&self.leader_state);
        let circuit_breakers = Arc::clone(&self.circuit_breakers);

        tokio::spawn(async move {
            // Use StdRng which is Send-safe
            use rand::SeedableRng;
            let mut rng = rand::rngs::StdRng::from_entropy();

            loop {
                // Random election timeout
                let timeout_ms = rng.gen_range(election_timeout_range.0..=election_timeout_range.1);
                let timeout = Duration::from_millis(timeout_ms);

                tokio::select! {
                    _ = tokio::time::sleep(timeout) => {
                        // Check if still running
                        if !*running.read().await {
                            break;
                        }

                        // Only start election if we're a follower or candidate
                        let current_state = *state.read().await;
                        if current_state == ConsensusState::Leader {
                            continue;
                        }

                        tracing::debug!(
                            node_id = %node_id,
                            "Election timeout elapsed, checking if election needed"
                        );

                        // Create a mini-consensus to run the election
                        // This is a workaround since we can't call self methods from the spawned task
                        if let Some(ref transport) = transport {
                            // Run election logic inline
                            let peers_snapshot = peers.read().await.clone();
                            if peers_snapshot.is_empty() {
                                // Single node - become leader
                                let mut s = state.write().await;
                                *s = ConsensusState::Leader;
                                let mut cl = current_leader.write().await;
                                *cl = Some(node_id.clone());
                                tracing::info!(node_id = %node_id, "Single node cluster, becoming leader");
                                continue;
                            }

                            // Increment term and vote for self
                            let (term, last_idx, last_term) = {
                                let mut p = persistent.write().await;
                                p.current_term += 1;
                                p.voted_for = Some(node_id.clone());
                                let (li, lt) = match p.log.last() {
                                    Some(e) => (e.index, e.term),
                                    None => (0, 0),
                                };
                                (p.current_term, li, lt)
                            };

                            {
                                let mut s = state.write().await;
                                *s = ConsensusState::Candidate;
                            }

                            // Send vote requests
                            let request = RequestVoteRequest {
                                term,
                                candidate_id: node_id.clone(),
                                last_log_index: last_idx,
                                last_log_term: last_term,
                            };

                            let breakers = circuit_breakers.read().await;
                            let mut votes = 1; // Self vote

                            for peer in &peers_snapshot {
                                if let Some(cb) = breakers.get(&peer.node_id) {
                                    if !cb.should_allow_request() {
                                        continue;
                                    }
                                }

                                match transport.request_vote(peer, request.clone()).await {
                                    Ok(resp) => {
                                        if resp.vote_granted {
                                            votes += 1;
                                        }
                                        if let Some(cb) = breakers.get(&peer.node_id) {
                                            cb.record_success();
                                        }
                                    }
                                    Err(_) => {
                                        if let Some(cb) = breakers.get(&peer.node_id) {
                                            cb.record_failure();
                                        }
                                    }
                                }
                            }

                            let majority = (peers_snapshot.len() + 1) / 2 + 1;
                            if votes >= majority {
                                // Become leader
                                let mut s = state.write().await;
                                *s = ConsensusState::Leader;
                                drop(s);

                                let mut cl = current_leader.write().await;
                                *cl = Some(node_id.clone());
                                drop(cl);

                                // Initialize leader state
                                let next_index = {
                                    let p = persistent.read().await;
                                    p.log.len() as u64 + 1
                                };

                                let mut ls = LeaderState::default();
                                for peer in &peers_snapshot {
                                    ls.next_index.insert(peer.node_id.clone(), next_index);
                                    ls.match_index.insert(peer.node_id.clone(), 0);
                                }

                                let mut leader_state_guard = leader_state.write().await;
                                *leader_state_guard = Some(ls);

                                tracing::info!(
                                    node_id = %node_id,
                                    term = term,
                                    votes = votes,
                                    "Won election, became leader"
                                );
                            } else {
                                // Revert to follower
                                let mut s = state.write().await;
                                *s = ConsensusState::Follower;
                            }
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        tracing::info!(node_id = %node_id, "Election timer shutting down");
                        break;
                    }
                }
            }
        })
    }

    /// Start the heartbeat sender background task
    async fn start_heartbeat_task(&self, mut shutdown_rx: mpsc::Receiver<()>) -> JoinHandle<()> {
        let state = Arc::clone(&self.state);
        let running = Arc::clone(&self.running);
        let node_id = self.node_id.clone();
        let heartbeat_interval = Duration::from_millis(self.config.heartbeat_interval_ms);

        let transport = self.transport.clone();
        let peers = Arc::clone(&self.peers);
        let persistent = Arc::clone(&self.persistent);
        let volatile = Arc::clone(&self.volatile);
        let leader_state = Arc::clone(&self.leader_state);
        let circuit_breakers = Arc::clone(&self.circuit_breakers);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(heartbeat_interval);

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // Check if still running
                        if !*running.read().await {
                            break;
                        }

                        // Only send heartbeats if we're the leader
                        if *state.read().await != ConsensusState::Leader {
                            continue;
                        }

                        if let Some(ref transport) = transport {
                            let peers_snapshot = peers.read().await.clone();
                            if peers_snapshot.is_empty() {
                                continue;
                            }

                            let (term, commit_index) = {
                                let p = persistent.read().await;
                                let v = volatile.read().await;
                                (p.current_term, v.commit_index)
                            };

                            let breakers = circuit_breakers.read().await;

                            for peer in &peers_snapshot {
                                if let Some(cb) = breakers.get(&peer.node_id) {
                                    if !cb.should_allow_request() {
                                        continue;
                                    }
                                }

                                let request = AppendEntriesRequest {
                                    term,
                                    leader_id: node_id.clone(),
                                    prev_log_index: 0,
                                    prev_log_term: 0,
                                    entries: Vec::new(),
                                    leader_commit: commit_index,
                                };

                                match transport.append_entries(peer, request).await {
                                    Ok(response) => {
                                        if let Some(cb) = breakers.get(&peer.node_id) {
                                            cb.record_success();
                                        }

                                        // Step down if we see a higher term
                                        if response.term > term {
                                            let mut p = persistent.write().await;
                                            p.current_term = response.term;
                                            p.voted_for = None;
                                            drop(p);

                                            let mut s = state.write().await;
                                            *s = ConsensusState::Follower;
                                            drop(s);

                                            let mut ls = leader_state.write().await;
                                            *ls = None;

                                            tracing::info!(
                                                node_id = %node_id,
                                                new_term = response.term,
                                                "Received higher term, stepping down"
                                            );
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        if let Some(cb) = breakers.get(&peer.node_id) {
                                            cb.record_failure();
                                        }
                                        tracing::debug!(
                                            peer = %peer.node_id,
                                            error = %e,
                                            "Failed to send heartbeat"
                                        );
                                    }
                                }
                            }
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        tracing::info!(node_id = %node_id, "Heartbeat task shutting down");
                        break;
                    }
                }
            }
        })
    }

    /// Stop the consensus module
    pub async fn stop(&mut self) -> Result<()> {
        {
            let mut running = self.running.write().await;
            if !*running {
                return Ok(());
            }
            *running = false;
        }

        tracing::info!(node_id = %self.node_id, "Stopping Raft consensus module");

        // Signal background tasks to shut down
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }

        // Wait for all background tasks to complete (with timeout)
        let handles: Vec<_> = {
            let mut task_handles = self.task_handles.write().await;
            std::mem::take(&mut *task_handles)
        };

        for handle in handles {
            // Give each task a short time to complete
            let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
        }

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

    // =========================================================================
    // ELECTION AND HEARTBEAT METHODS (RPC-BACKED)
    // =========================================================================

    /// Start an election by requesting votes from all peers
    ///
    /// This method implements the Raft election algorithm:
    /// 1. Increment current term
    /// 2. Transition to candidate state
    /// 3. Vote for self
    /// 4. Send RequestVote RPCs to all peers in parallel
    /// 5. If majority votes received, become leader
    ///
    /// Returns `Ok(true)` if this node became leader, `Ok(false)` otherwise.
    pub async fn start_election(&self) -> Result<bool> {
        // Check if transport is available
        let transport = match &self.transport {
            Some(t) => Arc::clone(t),
            None => {
                tracing::warn!("No transport configured, cannot start election");
                return Ok(false);
            }
        };

        // Increment term and transition to candidate
        let (current_term, last_log_index, last_log_term) = {
            let mut persistent = self.persistent.write().await;
            persistent.current_term += 1;
            persistent.voted_for = Some(self.node_id.clone());

            let (last_idx, last_term) = match persistent.log.last() {
                Some(entry) => (entry.index, entry.term),
                None => (0, 0),
            };

            (persistent.current_term, last_idx, last_term)
        };

        {
            let mut state = self.state.write().await;
            *state = ConsensusState::Candidate;
        }

        tracing::info!(
            node_id = %self.node_id,
            term = current_term,
            "Starting election"
        );

        // Get peers
        let peers = self.peers.read().await.clone();
        if peers.is_empty() {
            // Single node cluster - become leader immediately
            tracing::info!(node_id = %self.node_id, "Single node cluster, becoming leader");
            return self.become_leader().await;
        }

        // Pre-vote phase (if enabled) to prevent disruption
        if self.config.enable_pre_vote {
            let pre_vote_success = self
                .run_pre_vote(
                    &transport,
                    &peers,
                    current_term,
                    last_log_index,
                    last_log_term,
                )
                .await?;

            if !pre_vote_success {
                tracing::debug!(
                    node_id = %self.node_id,
                    "Pre-vote failed, aborting election"
                );
                // Revert to follower
                let mut state = self.state.write().await;
                *state = ConsensusState::Follower;
                return Ok(false);
            }
        }

        // Build the request
        let request = RequestVoteRequest {
            term: current_term,
            candidate_id: self.node_id.clone(),
            last_log_index,
            last_log_term,
        };

        // Send RequestVote RPCs to all peers in parallel
        let breakers = self.circuit_breakers.read().await;
        let votes = self
            .send_vote_requests(&transport, &peers, &breakers, request)
            .await;

        // Count votes (self vote + peer votes)
        let total_nodes = peers.len() + 1;
        let votes_received = votes + 1; // +1 for self vote
        let majority = total_nodes / 2 + 1;

        tracing::info!(
            node_id = %self.node_id,
            term = current_term,
            votes = votes_received,
            majority = majority,
            "Election vote count"
        );

        if votes_received >= majority {
            self.become_leader().await
        } else {
            // Revert to follower
            let mut state = self.state.write().await;
            *state = ConsensusState::Follower;
            Ok(false)
        }
    }

    /// Run pre-vote phase to check if election would succeed without disrupting cluster
    async fn run_pre_vote(
        &self,
        transport: &Arc<dyn ConsensusTransport>,
        peers: &[NodeEndpoint],
        term: u64,
        last_log_index: u64,
        last_log_term: u64,
    ) -> Result<bool> {
        let request = RequestVoteRequest {
            term,
            candidate_id: self.node_id.clone(),
            last_log_index,
            last_log_term,
        };

        let pre_votes = self.send_pre_vote_requests(transport, peers, request).await;

        let total_nodes = peers.len() + 1;
        let votes_received = pre_votes + 1; // +1 for self
        let majority = total_nodes / 2 + 1;

        Ok(votes_received >= majority)
    }

    /// Send pre-vote requests to all peers in parallel
    async fn send_pre_vote_requests(
        &self,
        transport: &Arc<dyn ConsensusTransport>,
        peers: &[NodeEndpoint],
        request: RequestVoteRequest,
    ) -> usize {
        let futures: Vec<_> = peers
            .iter()
            .map(|peer| {
                let transport = Arc::clone(transport);
                let req = request.clone();
                let peer = peer.clone();
                async move { transport.pre_vote(&peer, req).await }
            })
            .collect();

        let results = futures::future::join_all(futures).await;

        results
            .into_iter()
            .filter_map(|r| r.ok())
            .filter(|r| r.vote_granted)
            .count()
    }

    /// Send vote requests to all peers in parallel with circuit breaker support
    async fn send_vote_requests(
        &self,
        transport: &Arc<dyn ConsensusTransport>,
        peers: &[NodeEndpoint],
        breakers: &HashMap<String, Arc<CircuitBreaker>>,
        request: RequestVoteRequest,
    ) -> usize {
        let futures: Vec<_> = peers
            .iter()
            .map(|peer| {
                let transport = Arc::clone(transport);
                let req = request.clone();
                let peer = peer.clone();
                let breaker = breakers.get(&peer.node_id).cloned();

                async move {
                    // Check circuit breaker
                    if let Some(ref cb) = breaker {
                        if !cb.should_allow_request() {
                            tracing::debug!(
                                peer = %peer.node_id,
                                "Circuit breaker open, skipping vote request"
                            );
                            return None;
                        }
                    }

                    match transport.request_vote(&peer, req).await {
                        Ok(response) => {
                            if let Some(cb) = breaker {
                                cb.record_success();
                            }
                            Some(response)
                        }
                        Err(e) => {
                            tracing::warn!(
                                peer = %peer.node_id,
                                error = %e,
                                "Failed to send vote request"
                            );
                            if let Some(cb) = breaker {
                                cb.record_failure();
                            }
                            None
                        }
                    }
                }
            })
            .collect();

        let results = futures::future::join_all(futures).await;

        results
            .into_iter()
            .flatten()
            .filter(|r| r.vote_granted)
            .count()
    }

    /// Transition to leader state and start heartbeats
    async fn become_leader(&self) -> Result<bool> {
        {
            let mut state = self.state.write().await;
            *state = ConsensusState::Leader;
        }

        {
            let mut current_leader = self.current_leader.write().await;
            *current_leader = Some(self.node_id.clone());
        }

        // Initialize leader state
        {
            let peers = self.peers.read().await;
            let persistent = self.persistent.read().await;
            let next_index = persistent.log.len() as u64 + 1;

            let mut leader_state = LeaderState::default();
            for peer in peers.iter() {
                leader_state
                    .next_index
                    .insert(peer.node_id.clone(), next_index);
                leader_state.match_index.insert(peer.node_id.clone(), 0);
            }

            let mut ls = self.leader_state.write().await;
            *ls = Some(leader_state);
        }

        tracing::info!(
            node_id = %self.node_id,
            term = self.current_term().await,
            "Became leader"
        );

        // Send initial heartbeat to establish leadership
        self.send_heartbeat().await?;

        Ok(true)
    }

    /// Send heartbeat (empty AppendEntries) to all followers
    ///
    /// This method is called periodically by the leader to:
    /// 1. Maintain leadership and prevent elections
    /// 2. Replicate log entries to followers
    pub async fn send_heartbeat(&self) -> Result<()> {
        // Only leader can send heartbeats
        if *self.state.read().await != ConsensusState::Leader {
            return Ok(());
        }

        let transport = match &self.transport {
            Some(t) => Arc::clone(t),
            None => return Ok(()),
        };

        let peers = self.peers.read().await.clone();
        if peers.is_empty() {
            return Ok(());
        }

        let (term, commit_index) = {
            let persistent = self.persistent.read().await;
            let volatile = self.volatile.read().await;
            (persistent.current_term, volatile.commit_index)
        };

        let leader_state = self.leader_state.read().await;
        let ls = match leader_state.as_ref() {
            Some(ls) => ls,
            None => return Ok(()),
        };

        // Send AppendEntries to each peer
        let breakers = self.circuit_breakers.read().await;
        let futures: Vec<_> = peers
            .iter()
            .map(|peer| {
                let transport = Arc::clone(&transport);
                let peer = peer.clone();
                let breaker = breakers.get(&peer.node_id).cloned();
                let node_id = self.node_id.clone();

                // Get entries to send to this peer
                let next_idx = ls.next_index.get(&peer.node_id).copied().unwrap_or(1);
                let (prev_log_index, prev_log_term, entries) = self.get_entries_for_peer(next_idx);

                let request = AppendEntriesRequest {
                    term,
                    leader_id: node_id,
                    prev_log_index,
                    prev_log_term,
                    entries,
                    leader_commit: commit_index,
                };

                async move {
                    // Check circuit breaker
                    if let Some(ref cb) = breaker {
                        if !cb.should_allow_request() {
                            return (
                                peer.node_id.clone(),
                                Err("Circuit breaker open".to_string()),
                            );
                        }
                    }

                    match transport.append_entries(&peer, request).await {
                        Ok(response) => {
                            if let Some(cb) = breaker {
                                cb.record_success();
                            }
                            (peer.node_id.clone(), Ok(response))
                        }
                        Err(e) => {
                            if let Some(cb) = breaker {
                                cb.record_failure();
                            }
                            (peer.node_id.clone(), Err(e.to_string()))
                        }
                    }
                }
            })
            .collect();

        drop(leader_state); // Release read lock before awaiting

        let results = futures::future::join_all(futures).await;

        // Process responses and update leader state
        self.process_append_entries_responses(term, results).await?;

        Ok(())
    }

    /// Helper to get log entries for a specific peer
    fn get_entries_for_peer(&self, _next_index: u64) -> (u64, u64, Vec<RpcLogEntry>) {
        // For heartbeat, we send empty entries
        // Full log replication would include actual entries
        (0, 0, Vec::new())
    }

    /// Process AppendEntries responses and update match_index/next_index
    async fn process_append_entries_responses(
        &self,
        our_term: u64,
        results: Vec<(String, Result<AppendEntriesResponse, String>)>,
    ) -> Result<()> {
        for (peer_id, result) in results {
            match result {
                Ok(response) => {
                    // If response term is higher, step down
                    if response.term > our_term {
                        tracing::info!(
                            node_id = %self.node_id,
                            response_term = response.term,
                            "Received higher term, stepping down"
                        );
                        self.step_down(response.term).await;
                        return Ok(());
                    }

                    if response.success {
                        // Update match_index and next_index
                        if let Some(match_idx) = response.match_index {
                            let mut leader_state = self.leader_state.write().await;
                            if let Some(ref mut ls) = *leader_state {
                                ls.match_index.insert(peer_id.clone(), match_idx);
                                ls.next_index.insert(peer_id, match_idx + 1);
                            }
                        }
                    } else {
                        // Log inconsistency - decrement next_index
                        let mut leader_state = self.leader_state.write().await;
                        if let Some(ref mut ls) = *leader_state {
                            let next = ls.next_index.get(&peer_id).copied().unwrap_or(1);
                            if next > 1 {
                                ls.next_index.insert(peer_id, next - 1);
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        peer = %peer_id,
                        error = %e,
                        "AppendEntries failed"
                    );
                }
            }
        }

        // Update commit index based on match_index
        self.maybe_update_commit_index().await;

        Ok(())
    }

    /// Update commit index if a majority has replicated entries
    async fn maybe_update_commit_index(&self) {
        let leader_state = self.leader_state.read().await;
        let Some(ls) = leader_state.as_ref() else {
            return;
        };

        let mut match_indices: Vec<u64> = ls.match_index.values().copied().collect();
        // Add our own log length as our match index
        let our_log_len = {
            let persistent = self.persistent.read().await;
            persistent.log.len() as u64
        };
        match_indices.push(our_log_len);

        if match_indices.is_empty() {
            return;
        }

        match_indices.sort();
        let majority_idx = match_indices.len() / 2;
        let new_commit = match_indices[majority_idx];

        // Only update if new commit index is higher and the entry is from current term
        let persistent = self.persistent.read().await;
        if new_commit > 0 {
            if let Some(entry) = persistent.log.get(new_commit as usize - 1) {
                if entry.term == persistent.current_term {
                    drop(persistent);
                    let mut volatile = self.volatile.write().await;
                    if new_commit > volatile.commit_index {
                        volatile.commit_index = new_commit;
                    }
                }
            }
        }
    }

    /// Step down to follower when higher term is discovered
    async fn step_down(&self, new_term: u64) {
        {
            let mut persistent = self.persistent.write().await;
            persistent.current_term = new_term;
            persistent.voted_for = None;
        }

        {
            let mut state = self.state.write().await;
            *state = ConsensusState::Follower;
        }

        {
            let mut leader_state = self.leader_state.write().await;
            *leader_state = None;
        }

        tracing::info!(
            node_id = %self.node_id,
            term = new_term,
            "Stepped down to follower"
        );
    }

    /// Get a random election timeout within the configured range
    pub fn random_election_timeout(&self) -> Duration {
        let mut rng = rand::thread_rng();
        let timeout_ms =
            rng.gen_range(self.config.election_timeout_ms.0..=self.config.election_timeout_ms.1);
        Duration::from_millis(timeout_ms)
    }

    /// Convert internal LogEntry to RPC LogEntry
    #[allow(dead_code)]
    fn log_entry_to_rpc(entry: &LogEntry) -> RpcLogEntry {
        let command_bytes = serde_json::to_vec(&entry.command).unwrap_or_default();
        let entry_type = match &entry.command {
            Command::Noop => LogEntryType::Noop,
            Command::UpdateConfig { .. } => LogEntryType::Config,
            _ => LogEntryType::Command,
        };

        RpcLogEntry {
            term: entry.term,
            index: entry.index,
            command: command_bytes,
            entry_type,
        }
    }

    /// Convert RPC LogEntry to internal LogEntry
    #[allow(dead_code)]
    fn rpc_to_log_entry(rpc_entry: &RpcLogEntry) -> Option<LogEntry> {
        let command: Command = if rpc_entry.entry_type == LogEntryType::Noop {
            Command::Noop
        } else {
            serde_json::from_slice(&rpc_entry.command).ok()?
        };

        Some(LogEntry {
            term: rpc_entry.term,
            index: rpc_entry.index,
            command,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Mock ConsensusTransport for testing
    struct MockConsensusTransport {
        vote_count: Arc<AtomicUsize>,
        grant_votes: bool,
        append_count: Arc<AtomicUsize>,
        append_success: bool,
    }

    impl MockConsensusTransport {
        fn new(grant_votes: bool, append_success: bool) -> Self {
            Self {
                vote_count: Arc::new(AtomicUsize::new(0)),
                grant_votes,
                append_count: Arc::new(AtomicUsize::new(0)),
                append_success,
            }
        }
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
                vote_granted: self.grant_votes,
            })
        }

        async fn append_entries(
            &self,
            _target: &NodeEndpoint,
            _req: AppendEntriesRequest,
        ) -> RpcResult<AppendEntriesResponse> {
            self.append_count.fetch_add(1, Ordering::SeqCst);
            Ok(AppendEntriesResponse {
                term: 1,
                success: self.append_success,
                match_index: Some(1),
                conflict_term: None,
                conflict_index: None,
            })
        }

        async fn install_snapshot(
            &self,
            _target: &NodeEndpoint,
            _req: super::super::rpc::InstallSnapshotRequest,
        ) -> RpcResult<super::super::rpc::InstallSnapshotResponse> {
            Ok(super::super::rpc::InstallSnapshotResponse {
                term: 1,
                bytes_stored: 0,
            })
        }

        async fn pre_vote(
            &self,
            _target: &NodeEndpoint,
            _req: RequestVoteRequest,
        ) -> RpcResult<RequestVoteResponse> {
            Ok(RequestVoteResponse {
                term: 1,
                vote_granted: self.grant_votes,
            })
        }
    }

    #[tokio::test]
    async fn test_consensus_creation() {
        let config = ConsensusConfig::default();
        let consensus = RaftConsensus::new(config);
        assert!(consensus.is_ok());
    }

    #[tokio::test]
    async fn test_initial_state() {
        let consensus = RaftConsensus::new(ConsensusConfig::default())
            .expect("failed to create consensus instance");

        assert_eq!(consensus.get_state().await, ConsensusState::Follower);
        assert_eq!(consensus.current_term().await, 0);
        assert!(!consensus.is_leader().await);
    }

    #[tokio::test]
    async fn test_start_stop() {
        let mut consensus = RaftConsensus::new(ConsensusConfig::default())
            .expect("failed to create consensus instance");

        consensus.start().await.expect("failed to start consensus");
        assert!(*consensus.running.read().await);

        consensus.stop().await.expect("failed to stop consensus");
        assert!(!*consensus.running.read().await);
    }

    #[tokio::test]
    async fn test_request_vote_term_check() {
        let consensus = RaftConsensus::new(ConsensusConfig::default())
            .expect("failed to create consensus instance");

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

    #[tokio::test]
    async fn test_with_transport_creation() {
        let config = ConsensusConfig::default();
        let transport = Arc::new(MockConsensusTransport::new(true, true));
        let peers = vec![
            NodeEndpoint::new("node-2", "127.0.0.1:5680"),
            NodeEndpoint::new("node-3", "127.0.0.1:5681"),
        ];

        let consensus = RaftConsensus::with_transport(config, "node-1", transport, peers)
            .expect("failed to create consensus with transport");
        assert_eq!(consensus.node_id(), "node-1");
        assert!(consensus.transport.is_some());
        assert!(consensus._connection_manager.is_some());

        // Verify peers are set
        let peers = consensus.get_peers().await;
        assert_eq!(peers.len(), 2);
    }

    #[tokio::test]
    async fn test_add_remove_peer() {
        let config = ConsensusConfig::default();
        let transport = Arc::new(MockConsensusTransport::new(true, true));
        let consensus = RaftConsensus::with_transport(config, "node-1", transport, vec![])
            .expect("failed to create consensus instance");

        // Add a peer
        let peer = NodeEndpoint::new("node-2", "127.0.0.1:5680");
        consensus.add_peer(peer.clone()).await;

        let peers = consensus.get_peers().await;
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].node_id, "node-2");

        // Remove the peer
        consensus.remove_peer("node-2").await;
        let peers = consensus.get_peers().await;
        assert!(peers.is_empty());
    }

    #[tokio::test]
    async fn test_start_election_single_node() {
        let config = ConsensusConfig::default();
        let transport = Arc::new(MockConsensusTransport::new(true, true));

        // Single node cluster (no peers)
        let consensus = RaftConsensus::with_transport(config, "node-1", transport, vec![])
            .expect("failed to create consensus instance");

        // Start election should succeed and become leader
        let result = consensus.start_election().await;
        assert!(result.is_ok());
        let became_leader = result.expect("election result should be Ok");
        assert!(became_leader, "should become leader in single node cluster");

        assert_eq!(consensus.get_state().await, ConsensusState::Leader);
        assert_eq!(consensus.get_leader().await, Some("node-1".to_string()));
    }

    #[tokio::test]
    async fn test_start_election_with_majority() {
        let config = ConsensusConfig {
            enable_pre_vote: false, // Disable pre-vote for simpler test
            ..Default::default()
        };
        let transport = Arc::new(MockConsensusTransport::new(true, true));
        let peers = vec![
            NodeEndpoint::new("node-2", "127.0.0.1:5680"),
            NodeEndpoint::new("node-3", "127.0.0.1:5681"),
        ];

        let consensus = RaftConsensus::with_transport(config, "node-1", transport.clone(), peers)
            .expect("failed to create consensus instance");

        // Start election - should get majority (self + 2 votes from mocks)
        let result = consensus.start_election().await;
        assert!(result.is_ok());
        let became_leader = result.expect("election result should be Ok");
        assert!(became_leader, "should become leader with majority");

        assert_eq!(consensus.get_state().await, ConsensusState::Leader);
        // Both peers should have been contacted
        assert_eq!(transport.vote_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_start_election_without_majority() {
        let config = ConsensusConfig {
            enable_pre_vote: false,
            ..Default::default()
        };
        // Transport that rejects votes
        let transport = Arc::new(MockConsensusTransport::new(false, true));
        let peers = vec![
            NodeEndpoint::new("node-2", "127.0.0.1:5680"),
            NodeEndpoint::new("node-3", "127.0.0.1:5681"),
        ];

        let consensus = RaftConsensus::with_transport(config, "node-1", transport.clone(), peers)
            .expect("failed to create consensus instance");

        // Start election - should fail (only self vote)
        let result = consensus.start_election().await;
        assert!(result.is_ok());
        let became_leader = result.expect("election result should be Ok");
        assert!(!became_leader, "should NOT become leader without majority");

        // Should revert to follower
        assert_eq!(consensus.get_state().await, ConsensusState::Follower);
    }

    #[tokio::test]
    async fn test_send_heartbeat() {
        let config = ConsensusConfig::default();
        let transport = Arc::new(MockConsensusTransport::new(true, true));
        let peers = vec![
            NodeEndpoint::new("node-2", "127.0.0.1:5680"),
            NodeEndpoint::new("node-3", "127.0.0.1:5681"),
        ];

        let consensus = RaftConsensus::with_transport(config, "node-1", transport.clone(), peers)
            .expect("failed to create consensus instance");

        // First become leader
        {
            let mut state = consensus.state.write().await;
            *state = ConsensusState::Leader;
        }
        {
            let mut ls = consensus.leader_state.write().await;
            let mut leader_state = LeaderState::default();
            leader_state.next_index.insert("node-2".to_string(), 1);
            leader_state.next_index.insert("node-3".to_string(), 1);
            leader_state.match_index.insert("node-2".to_string(), 0);
            leader_state.match_index.insert("node-3".to_string(), 0);
            *ls = Some(leader_state);
        }

        // Send heartbeat
        let result = consensus.send_heartbeat().await;
        assert!(result.is_ok(), "heartbeat should succeed");

        // Both peers should have received heartbeat
        assert_eq!(transport.append_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_heartbeat_not_sent_as_follower() {
        let config = ConsensusConfig::default();
        let transport = Arc::new(MockConsensusTransport::new(true, true));
        let peers = vec![NodeEndpoint::new("node-2", "127.0.0.1:5680")];

        let consensus = RaftConsensus::with_transport(config, "node-1", transport.clone(), peers)
            .expect("failed to create consensus instance");

        // As a follower, heartbeat should not be sent
        let result = consensus.send_heartbeat().await;
        assert!(
            result.is_ok(),
            "heartbeat should return Ok even as follower"
        );

        // No append entries should have been sent
        assert_eq!(transport.append_count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_random_election_timeout() {
        let config = ConsensusConfig {
            election_timeout_ms: (100, 200),
            ..Default::default()
        };
        let consensus = RaftConsensus::new(config).expect("failed to create consensus instance");

        // Generate multiple timeouts and verify they're in range
        for _ in 0..100 {
            let timeout = consensus.random_election_timeout();
            let ms = timeout.as_millis() as u64;
            assert!(ms >= 100 && ms <= 200);
        }
    }

    #[tokio::test]
    async fn test_log_entry_conversion() {
        let entry = LogEntry {
            term: 5,
            index: 10,
            command: Command::Noop,
        };

        let rpc_entry = RaftConsensus::log_entry_to_rpc(&entry);
        assert_eq!(rpc_entry.term, 5);
        assert_eq!(rpc_entry.index, 10);
        assert_eq!(rpc_entry.entry_type, LogEntryType::Noop);

        // Convert back
        let converted = RaftConsensus::rpc_to_log_entry(&rpc_entry);
        assert!(converted.is_some(), "conversion should succeed");
        let converted = converted.expect("converted entry should be Some");
        assert_eq!(converted.term, 5);
        assert_eq!(converted.index, 10);
        assert!(matches!(converted.command, Command::Noop));
    }

    #[tokio::test]
    async fn test_step_down() {
        let config = ConsensusConfig::default();
        let consensus = RaftConsensus::new(config).expect("failed to create consensus instance");

        // Set up as leader
        {
            let mut state = consensus.state.write().await;
            *state = ConsensusState::Leader;
        }
        {
            let mut persistent = consensus.persistent.write().await;
            persistent.current_term = 5;
            persistent.voted_for = Some("node-1".to_string());
        }
        {
            let mut ls = consensus.leader_state.write().await;
            *ls = Some(LeaderState::default());
        }

        // Step down to higher term
        consensus.step_down(10).await;

        assert_eq!(consensus.get_state().await, ConsensusState::Follower);
        assert_eq!(consensus.current_term().await, 10);
        assert!(consensus.persistent.read().await.voted_for.is_none());
        assert!(consensus.leader_state.read().await.is_none());
    }

    #[tokio::test]
    async fn test_start_stop_with_transport() {
        let config = ConsensusConfig {
            election_timeout_ms: (1000, 2000), // Long timeout to prevent actual election
            heartbeat_interval_ms: 500,
            ..Default::default()
        };
        let transport = Arc::new(MockConsensusTransport::new(true, true));
        let peers = vec![NodeEndpoint::new("node-2", "127.0.0.1:5680")];

        let mut consensus = RaftConsensus::with_transport(config, "node-1", transport, peers)
            .expect("failed to create consensus instance");

        // Start should create background tasks
        consensus.start().await.expect("failed to start consensus");
        assert!(*consensus.running.read().await);
        assert!(!consensus.task_handles.read().await.is_empty());

        // Stop should clean up tasks
        consensus.stop().await.expect("failed to stop consensus");
        assert!(!*consensus.running.read().await);
    }

    #[tokio::test]
    async fn test_no_transport_election_returns_false() {
        let config = ConsensusConfig::default();
        let consensus = RaftConsensus::new(config).expect("failed to create consensus instance");

        // Without transport, election should return false
        let result = consensus.start_election().await;
        assert!(result.is_ok());
        let became_leader = result.expect("election result should be Ok");
        assert!(!became_leader, "should not become leader without transport");
    }
}
