pub mod raft;

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::core::{ConsensusConfig, VectorDBError, ConsensusError};

type Result<T> = std::result::Result<T, VectorDBError>;
use raft::{RaftConsensusEngine, RaftCommand, RaftResponse, ClusterState};

/// Main consensus engine that manages distributed consensus
pub struct ConsensusEngine {
    /// Configuration for consensus
    config: ConsensusConfig,
    
    /// Raft consensus implementation
    raft_engine: Option<Arc<RwLock<RaftConsensusEngine>>>,
    
    /// Node ID for this instance
    node_id: u64,
    
    /// Whether consensus is currently active
    is_active: bool,
}

impl ConsensusEngine {
    /// Create a new consensus engine
    pub async fn new(config: ConsensusConfig) -> Result<Self> {
        let node_id = config.node_id.unwrap_or(1);
        
        Ok(Self {
            config,
            raft_engine: None,
            node_id,
            is_active: false,
        })
    }

    /// Start the consensus engine
    pub async fn start(&mut self) -> Result<()> {
        if self.is_active {
            return Ok(());
        }
        
        // Initialize Raft consensus engine
        let raft_engine = RaftConsensusEngine::new(self.config.clone(), self.node_id).await?;
        self.raft_engine = Some(Arc::new(RwLock::new(raft_engine)));
        
        // Activate consensus
        if let Some(ref raft) = self.raft_engine {
            let mut raft_guard = raft.write().await;
            raft_guard.activate().await?;
        }
        
        self.is_active = true;
        tracing::info!("Consensus engine started for node {}", self.node_id);
        
        Ok(())
    }

    /// Stop the consensus engine
    pub async fn stop(&mut self) -> Result<()> {
        if !self.is_active {
            return Ok(());
        }
        
        // Deactivate consensus
        if let Some(ref raft) = self.raft_engine {
            let mut raft_guard = raft.write().await;
            raft_guard.deactivate().await?;
        }
        
        self.raft_engine = None;
        self.is_active = false;
        
        tracing::info!("Consensus engine stopped for node {}", self.node_id);
        Ok(())
    }
    
    /// Check if consensus is active
    pub fn is_active(&self) -> bool {
        self.is_active
    }
    
    /// Propose a command for consensus
    pub async fn propose(&self, command: RaftCommand) -> Result<RaftResponse> {
        if let Some(ref raft) = self.raft_engine {
            let raft_guard = raft.read().await;
            raft_guard.propose(command).await
        } else {
            Err(VectorDBError::Consensus(ConsensusError::Raft(
"Consensus engine not initialized".to_string())
            ))
        }
    }
    
    /// Add a peer to the cluster
    pub async fn add_peer(&self, node_id: u64, address: String) -> Result<()> {
        if let Some(ref raft) = self.raft_engine {
            let raft_guard = raft.read().await;
            raft_guard.add_peer(node_id, address).await
        } else {
            Err(VectorDBError::Consensus(ConsensusError::Raft(
"Consensus engine not initialized".to_string())
            ))
        }
    }
    
    /// Remove a peer from the cluster
    pub async fn remove_peer(&self, node_id: u64) -> Result<()> {
        if let Some(ref raft) = self.raft_engine {
            let raft_guard = raft.read().await;
            raft_guard.remove_peer(node_id).await
        } else {
            Err(VectorDBError::Consensus(ConsensusError::Raft(
"Consensus engine not initialized".to_string())
            ))
        }
    }
    
    /// Get current cluster state
    pub async fn get_cluster_state(&self) -> Option<ClusterState> {
        if let Some(ref raft) = self.raft_engine {
            let raft_guard = raft.read().await;
            Some(raft_guard.get_cluster_state().await)
        } else {
            None
        }
    }
    
    /// Get node ID
    pub fn node_id(&self) -> u64 {
        self.node_id
    }
}