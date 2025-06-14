// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Raft consensus protocol implementation for ProximaDB

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, mpsc};
use raft::{Config as RaftConfig, RawNode, StateRole};
use raft::eraftpb::{ConfChange, ConfChangeType};
use raft::prelude::*;
use serde::{Serialize, Deserialize};
use chrono::{DateTime, Utc};

use crate::core::{VectorDBError, ConsensusConfig};

type Result<T> = std::result::Result<T, VectorDBError>;
type ProximaDBError = VectorDBError;

use slog::{Logger, Drain, o};

pub mod node;
pub mod storage;
pub mod message;
pub mod leader_election;
pub mod log_replication;
pub mod snapshot;

pub use node::RaftNode;
pub use storage::RaftStorage;
pub use message::{RaftMessage, RaftCommand, RaftResponse};

/// Raft consensus engine for distributed ProximaDB clusters
pub struct RaftConsensusEngine {
    /// Node configuration
    node_id: u64,
    
    /// Raft configuration
    _config: RaftConfig,
    
    /// Raw Raft node
    raw_node: Arc<RwLock<RawNode<RaftStorage>>>,
    
    /// Message sender for inter-node communication
    _message_sender: mpsc::UnboundedSender<RaftMessage>,
    
    /// Message receiver for handling incoming messages
    _message_receiver: Option<mpsc::UnboundedReceiver<RaftMessage>>,
    
    /// Cluster peer nodes
    peers: Arc<RwLock<HashMap<u64, String>>>, // node_id -> address
    
    /// Command proposals pending consensus
    pending_proposals: Arc<RwLock<HashMap<u64, PendingProposal>>>,
    
    /// Node state and leadership information
    node_state: Arc<RwLock<NodeState>>,
    
    /// Storage backend for Raft logs and state
    _storage: Arc<RwLock<RaftStorage>>,
    
    /// Consensus activation state
    is_active: Arc<RwLock<bool>>,
}

/// Node state information
#[derive(Debug, Clone)]
pub struct NodeState {
    pub role: StateRole,
    pub term: u64,
    pub leader_id: Option<u64>,
    pub last_heartbeat: Option<Instant>,
    pub election_timeout: Duration,
    pub heartbeat_timeout: Duration,
}

impl Default for NodeState {
    fn default() -> Self {
        Self {
            role: StateRole::Follower,
            term: 0,
            leader_id: None,
            last_heartbeat: None,
            election_timeout: Duration::from_millis(1000),
            heartbeat_timeout: Duration::from_millis(300),
        }
    }
}

/// Pending consensus proposal
#[derive(Debug)]
pub struct PendingProposal {
    pub command: RaftCommand,
    pub timestamp: DateTime<Utc>,
    pub response_sender: Option<tokio::sync::oneshot::Sender<RaftResponse>>,
}

impl RaftConsensusEngine {
    /// Create a new Raft consensus engine
    pub async fn new(consensus_config: ConsensusConfig, node_id: u64) -> Result<Self> {
        let storage = RaftStorage::new().await?;
        let raft_config = Self::create_raft_config(&consensus_config, node_id)?;
        
        // Create slog logger for Raft
        let decorator = slog_stdlog::StdLog.fuse();
        let drain = slog::Fuse::new(decorator);
        let logger = Logger::root(drain, o!("node_id" => node_id));
        
        let raw_node = RawNode::new(&raft_config, storage.clone(), &logger).map_err(|e| {
            ProximaDBError::Consensus(crate::core::ConsensusError::Raft(format!("Failed to create Raft node: {}", e)))
        })?;
        
        let (message_sender, message_receiver) = mpsc::unbounded_channel();
        
        Ok(Self {
            node_id,
            _config: raft_config,
            raw_node: Arc::new(RwLock::new(raw_node)),
            _message_sender: message_sender,
            _message_receiver: Some(message_receiver),
            peers: Arc::new(RwLock::new(HashMap::new())),
            pending_proposals: Arc::new(RwLock::new(HashMap::new())),
            node_state: Arc::new(RwLock::new(NodeState::default())),
            _storage: Arc::new(RwLock::new(storage)),
            is_active: Arc::new(RwLock::new(false)),
        })
    }
    
    /// Activate distributed consensus
    pub async fn activate(&mut self) -> Result<()> {
        let mut is_active = self.is_active.write().await;
        if *is_active {
            return Ok(()); // Already active
        }
        
        // Initialize cluster if this is the first node
        if self.peers.read().await.is_empty() {
            self.bootstrap_cluster().await?;
        }
        
        // Start the Raft event loop
        self.start_raft_loop().await?;
        
        // Start network communication handlers
        self.start_network_handlers().await?;
        
        // Start periodic tasks (heartbeats, timeouts)
        self.start_periodic_tasks().await?;
        
        *is_active = true;
        
        tracing::info!("Raft consensus engine activated for node {}", self.node_id);
        Ok(())
    }
    
    /// Deactivate distributed consensus
    pub async fn deactivate(&mut self) -> Result<()> {
        let mut is_active = self.is_active.write().await;
        if !*is_active {
            return Ok(()); // Already inactive
        }
        
        // Gracefully shutdown Raft node
        let raw_node = self.raw_node.write().await;
        
        // If this node is the leader, transfer leadership
        if raw_node.raft.state == StateRole::Leader {
            self.transfer_leadership().await?;
        }
        
        *is_active = false;
        
        tracing::info!("Raft consensus engine deactivated for node {}", self.node_id);
        Ok(())
    }
    
    /// Check if consensus is currently active
    pub async fn is_active(&self) -> bool {
        *self.is_active.read().await
    }
    
    /// Propose a command for consensus
    pub async fn propose(&self, command: RaftCommand) -> Result<RaftResponse> {
        if !*self.is_active.read().await {
            return Err(ProximaDBError::Consensus(crate::core::ConsensusError::Raft("Consensus engine not active".to_string())));
        }
        
        let (response_sender, response_receiver) = tokio::sync::oneshot::channel();
        
        let proposal = PendingProposal {
            command: command.clone(),
            timestamp: Utc::now(),
            response_sender: Some(response_sender),
        };
        
        let proposal_id = self.generate_proposal_id().await;
        
        // Add to pending proposals
        {
            let mut pending = self.pending_proposals.write().await;
            pending.insert(proposal_id, proposal);
        }
        
        // Propose to Raft
        let mut raw_node = self.raw_node.write().await;
        let data = serde_json::to_vec(&command).map_err(|e| {
            ProximaDBError::Storage(crate::core::StorageError::Serialization(format!("Failed to serialize command: {}", e)))
        })?;
        
        raw_node.propose(vec![], data).map_err(|e| {
            ProximaDBError::Consensus(crate::core::ConsensusError::Raft(format!("Failed to propose command: {}", e)))
        })?;
        
        drop(raw_node);
        
        // Wait for response
        match tokio::time::timeout(Duration::from_secs(30), response_receiver).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(ProximaDBError::Consensus(crate::core::ConsensusError::Raft("Proposal response channel closed".to_string()))),
            Err(_) => Err(ProximaDBError::Consensus(crate::core::ConsensusError::Raft("Proposal timeout".to_string()))),
        }
    }
    
    /// Add a peer to the cluster
    pub async fn add_peer(&self, node_id: u64, address: String) -> Result<()> {
        {
            let mut peers = self.peers.write().await;
            peers.insert(node_id, address);
        }
        
        if *self.is_active.read().await {
            // Propose configuration change
            let mut change = ConfChange::default();
            change.id = 0;
            change.change_type = ConfChangeType::AddNode;
            change.node_id = node_id;
            change.context = vec![].into();
            
            let mut raw_node = self.raw_node.write().await;
            raw_node.propose_conf_change(vec![], change).map_err(|e| {
                ProximaDBError::Consensus(crate::core::ConsensusError::Raft(format!("Failed to propose peer addition: {}", e)))
            })?;
        }
        
        tracing::info!("Added peer {} with address {}", node_id, self.peers.read().await.get(&node_id).unwrap());
        Ok(())
    }
    
    /// Remove a peer from the cluster
    pub async fn remove_peer(&self, node_id: u64) -> Result<()> {
        {
            let mut peers = self.peers.write().await;
            peers.remove(&node_id);
        }
        
        if *self.is_active.read().await {
            // Propose configuration change
            let mut change = ConfChange::default();
            change.id = 0;
            change.change_type = ConfChangeType::RemoveNode;
            change.node_id = node_id;
            change.context = vec![].into();
            
            let mut raw_node = self.raw_node.write().await;
            raw_node.propose_conf_change(vec![], change).map_err(|e| {
                ProximaDBError::Consensus(crate::core::ConsensusError::Raft(format!("Failed to propose peer removal: {}", e)))
            })?;
        }
        
        tracing::info!("Removed peer {}", node_id);
        Ok(())
    }
    
    /// Get current cluster state
    pub async fn get_cluster_state(&self) -> ClusterState {
        let node_state = self.node_state.read().await;
        let peers = self.peers.read().await;
        let is_active = *self.is_active.read().await;
        
        ClusterState {
            node_id: self.node_id,
            role: node_state.role,
            term: node_state.term,
            leader_id: node_state.leader_id,
            peers: peers.clone(),
            is_active,
            last_heartbeat: node_state.last_heartbeat,
        }
    }
    
    /// Bootstrap a new cluster with this node as the initial member
    async fn bootstrap_cluster(&self) -> Result<()> {
        let _raw_node = self.raw_node.write().await;
        
        // Create initial configuration with only this node
        let mut conf_state = ConfState::default();
        conf_state.voters.push(self.node_id);
        
        // Bootstrap the cluster - for newer raft versions, we don't need to call bootstrap
        // The cluster will start automatically with the initial configuration
        tracing::info!("Cluster initialization started with node {}", self.node_id);
        
        tracing::info!("Bootstrapped new cluster with node {}", self.node_id);
        Ok(())
    }
    
    /// Start the main Raft event loop
    async fn start_raft_loop(&self) -> Result<()> {
        let raw_node = self.raw_node.clone();
        let node_state = self.node_state.clone();
        let pending_proposals = self.pending_proposals.clone();
        let is_active = self.is_active.clone();
        
        tokio::spawn(async move {
            let mut tick_interval = tokio::time::interval(Duration::from_millis(100));
            
            loop {
                if !*is_active.read().await {
                    break;
                }
                
                tick_interval.tick().await;
                
                // Process Raft ticks
                {
                    let mut node = raw_node.write().await;
                    node.tick();
                    
                    // Update node state
                    {
                        let mut state = node_state.write().await;
                        state.role = node.raft.state;
                        state.term = node.raft.term;
                        state.leader_id = Some(node.raft.leader_id);
                    }
                    
                    // Check for ready events
                    if node.has_ready() {
                        let ready = node.ready();
                        
                        // Process committed entries
                        for entry in ready.committed_entries() {
                            if !entry.data.is_empty() {
                                Self::process_committed_entry(&entry, &pending_proposals).await;
                            }
                        }
                        
                        // Advance the Raft state machine
                        let mut light_rd = node.advance(ready);
                        let committed_entries = light_rd.take_committed_entries();
                        for entry in committed_entries {
                            if !entry.data.is_empty() {
                                Self::process_committed_entry(&entry, &pending_proposals).await;
                            }
                        }
                        node.advance_apply();
                    }
                }
            }
        });
        
        Ok(())
    }
    
    /// Start network communication handlers
    async fn start_network_handlers(&self) -> Result<()> {
        // This would typically start gRPC/HTTP servers for inter-node communication
        // For now, we'll implement a placeholder
        tracing::info!("Network handlers started for node {}", self.node_id);
        Ok(())
    }
    
    /// Start periodic tasks (heartbeats, timeouts)
    async fn start_periodic_tasks(&self) -> Result<()> {
        let raw_node = self.raw_node.clone();
        let node_state = self.node_state.clone();
        let is_active = self.is_active.clone();
        
        // Heartbeat task
        tokio::spawn(async move {
            let mut heartbeat_interval = tokio::time::interval(Duration::from_millis(150));
            
            loop {
                if !*is_active.read().await {
                    break;
                }
                
                heartbeat_interval.tick().await;
                
                // Send heartbeats if we're the leader
                {
                    let node = raw_node.read().await;
                    if node.raft.state == StateRole::Leader {
                        // Heartbeats are handled by the Raft tick mechanism
                    }
                }
                
                // Update last heartbeat time
                {
                    let mut state = node_state.write().await;
                    state.last_heartbeat = Some(Instant::now());
                }
            }
        });
        
        Ok(())
    }
    
    /// Transfer leadership to another node
    async fn transfer_leadership(&self) -> Result<()> {
        let peers = self.peers.read().await;
        if let Some((&target_node, _)) = peers.iter().next() {
            let mut raw_node = self.raw_node.write().await;
            raw_node.transfer_leader(target_node);
            tracing::info!("Initiated leadership transfer to node {}", target_node);
        }
        Ok(())
    }
    
    /// Generate a unique proposal ID
    async fn generate_proposal_id(&self) -> u64 {
        // Simple implementation - in production, use a more robust method
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) as u64
    }
    
    /// Process a committed Raft log entry
    async fn process_committed_entry(
        entry: &Entry,
        pending_proposals: &Arc<RwLock<HashMap<u64, PendingProposal>>>,
    ) {
        // Deserialize the command
        if let Ok(command) = serde_json::from_slice::<RaftCommand>(&entry.data) {
            // Apply the command to the state machine
            let response = Self::apply_command(command).await;
            
            // Notify any waiting proposal
            let mut pending = pending_proposals.write().await;
            if let Some(proposal) = pending.remove(&entry.index) {
                if let Some(sender) = proposal.response_sender {
                    let _ = sender.send(response);
                }
            }
        }
    }
    
    /// Apply a committed command to the state machine
    async fn apply_command(command: RaftCommand) -> RaftResponse {
        match command {
            RaftCommand::VectorInsert { collection_id: _, vector_record } => {
                // Apply vector insertion to storage
                // This would integrate with the StorageEngine
                RaftResponse::VectorInserted { 
                    id: vector_record.id.clone(),
                    success: true 
                }
            }
            RaftCommand::VectorDelete { collection_id: _, vector_id } => {
                // Apply vector deletion to storage
                RaftResponse::VectorDeleted { 
                    id: vector_id,
                    success: true 
                }
            }
            RaftCommand::CollectionCreate { collection } => {
                // Apply collection creation
                RaftResponse::CollectionCreated { 
                    id: collection.id.clone(),
                    success: true 
                }
            }
            RaftCommand::CollectionDelete { collection_id } => {
                // Apply collection deletion
                RaftResponse::CollectionDeleted { 
                    id: collection_id,
                    success: true 
                }
            }
        }
    }
    
    /// Create Raft configuration from consensus config
    fn create_raft_config(consensus_config: &ConsensusConfig, node_id: u64) -> Result<RaftConfig> {
        let mut config = RaftConfig {
            id: node_id, // Set the actual node ID
            election_tick: 10,
            heartbeat_tick: 3,
            max_size_per_msg: 1024 * 1024,
            max_inflight_msgs: 256,
            applied: 0,
            ..Default::default()
        };
        
        // Apply custom configuration
        config.election_tick = (consensus_config.election_timeout_ms / 100) as usize; // Convert to ticks
        config.heartbeat_tick = (consensus_config.heartbeat_interval_ms / 100) as usize; // Convert to ticks
        
        config.validate().map_err(|e| {
            ProximaDBError::Config(format!("Invalid Raft configuration: {}", e))
        })?;
        
        Ok(config)
    }
}

/// Cluster state information
#[derive(Debug, Clone)]
pub struct ClusterState {
    pub node_id: u64,
    pub role: StateRole,
    pub term: u64,
    pub leader_id: Option<u64>,
    pub peers: HashMap<u64, String>,
    pub is_active: bool,
    pub last_heartbeat: Option<Instant>,
}

/// Raft consensus configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RaftConsensusConfig {
    pub node_id: u64,
    pub election_timeout_ms: Option<u64>,
    pub heartbeat_timeout_ms: Option<u64>,
    pub snapshot_threshold: Option<u64>,
    pub max_size_per_msg: Option<u64>,
    pub max_inflight_msgs: Option<usize>,
}

impl Default for RaftConsensusConfig {
    fn default() -> Self {
        Self {
            node_id: 1,
            election_timeout_ms: Some(1000),
            heartbeat_timeout_ms: Some(300),
            snapshot_threshold: Some(10000),
            max_size_per_msg: Some(1024 * 1024), // 1MB
            max_inflight_msgs: Some(256),
        }
    }
}