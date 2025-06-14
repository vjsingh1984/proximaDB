// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Raft leader election implementation

use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use std::sync::Arc;

use crate::core::VectorDBError;

type Result<T> = std::result::Result<T, VectorDBError>;
use super::{RaftNode, RaftMessage};
use super::node::NodeState;

/// Leader election manager
pub struct LeaderElection {
    /// Current node
    node: Arc<RwLock<RaftNode>>,
    
    /// Peer nodes in the cluster
    peers: Arc<RwLock<HashMap<u64, String>>>,
    
    /// Election timeout configuration
    election_timeout: Duration,
    
    /// Random election timeout jitter
    election_jitter: Duration,
    
    /// Last election start time
    last_election_start: Option<Instant>,
}

impl LeaderElection {
    /// Create a new leader election manager
    pub fn new(
        node: Arc<RwLock<RaftNode>>,
        peers: Arc<RwLock<HashMap<u64, String>>>,
        election_timeout: Duration,
    ) -> Self {
        Self {
            node,
            peers,
            election_timeout,
            election_jitter: Duration::from_millis(150), // Random jitter up to 150ms
            last_election_start: None,
        }
    }
    
    /// Start a new election
    pub async fn start_election(&mut self) -> Result<()> {
        let mut node = self.node.write().await;
        let peers = self.peers.read().await;
        
        // Only start election if we're a follower or candidate
        if node.is_leader() {
            return Ok(());
        }
        
        let cluster_size = peers.len() + 1; // +1 for this node
        let votes_needed = (cluster_size / 2) + 1;
        
        // Transition to candidate state
        let election_start = Instant::now();
        node.update_state(NodeState::Candidate {
            votes_received: 1, // Vote for ourselves
            votes_needed: votes_needed as u64,
            election_start,
        });
        
        self.last_election_start = Some(election_start);
        
        tracing::info!("Node {} starting election, need {} votes from {} nodes", 
                  node.id, votes_needed, cluster_size);
        
        // Send vote requests to all peers
        self.send_vote_requests(&*node, &*peers).await?;
        
        Ok(())
    }
    
    /// Handle a vote request from another candidate
    pub async fn handle_vote_request(
        &mut self,
        candidate_id: u64,
        term: u64,
        last_log_index: u64,
        last_log_term: u64,
    ) -> Result<RaftMessage> {
        let vote_granted = {
            let node = self.node.read().await;
            
            // Check if we should grant the vote
            self.should_grant_vote(
                &*node,
                candidate_id,
                term,
                last_log_index,
                last_log_term,
            )
        };
        
        if vote_granted {
            tracing::info!("Granting vote to candidate {} for term {}", candidate_id, term);
            
            // Reset election timeout since we granted a vote
            self.reset_election_timeout();
        } else {
            tracing::info!("Rejecting vote request from candidate {} for term {}", candidate_id, term);
        }
        
        Ok(RaftMessage::VoteResponse {
            term,
            vote_granted,
        })
    }
    
    /// Handle a vote response from a peer
    pub async fn handle_vote_response(
        &mut self,
        peer_id: u64,
        _term: u64,
        vote_granted: bool,
    ) -> Result<bool> {
        let mut node = self.node.write().await;
        
        // Only process if we're still a candidate
        if !node.is_candidate() {
            return Ok(false);
        }
        
        let node_id = node.id;
        if let NodeState::Candidate { votes_received, votes_needed, .. } = &mut node.state {
            if vote_granted {
                *votes_received += 1;
                tracing::info!("Node {} received vote from {}, now have {}/{} votes", 
                          node_id, peer_id, *votes_received, *votes_needed);
                
                // Check if we have majority
                if *votes_received >= *votes_needed {
                    self.become_leader(&mut *node).await?;
                    return Ok(true); // Became leader
                }
            } else {
                tracing::info!("Node {} vote rejected by {}", node_id, peer_id);
            }
        }
        
        Ok(false)
    }
    
    /// Check if election timeout has expired
    pub fn is_election_timeout(&self) -> bool {
        if let Some(last_start) = self.last_election_start {
            last_start.elapsed() > self.election_timeout + self.get_jitter()
        } else {
            true // No election started, so timeout
        }
    }
    
    /// Reset the election timeout
    pub fn reset_election_timeout(&mut self) {
        self.last_election_start = Some(Instant::now());
    }
    
    /// Transition to leader state
    async fn become_leader(&self, node: &mut RaftNode) -> Result<()> {
        let peers = self.peers.read().await;
        
        // Initialize follower state for each peer
        let mut followers = HashMap::new();
        for &peer_id in peers.keys() {
            followers.insert(peer_id, super::node::FollowerState {
                next_index: 1, // Will be updated based on actual log
                match_index: 0,
                last_contact: Instant::now(),
                is_active: true,
            });
        }
        
        node.update_state(NodeState::Leader {
            followers,
            last_heartbeat_sent: Instant::now(),
        });
        
        tracing::info!("Node {} became leader", node.id);
        
        // Send initial heartbeats to establish leadership
        self.send_initial_heartbeats(node, &*peers).await?;
        
        Ok(())
    }
    
    /// Send vote requests to all peers
    async fn send_vote_requests(
        &self,
        node: &RaftNode,
        peers: &HashMap<u64, String>,
    ) -> Result<()> {
        // In a full implementation, this would send actual network messages
        // For now, we'll simulate the process
        
        for (&peer_id, address) in peers {
            tracing::debug!("Sending vote request from {} to {} at {}", 
                       node.id, peer_id, address);
            
            // TODO: Send actual RaftMessage::VoteRequest to peer
            // This would use gRPC or HTTP to send the message
        }
        
        Ok(())
    }
    
    /// Send initial heartbeats after becoming leader
    async fn send_initial_heartbeats(
        &self,
        node: &RaftNode,
        peers: &HashMap<u64, String>,
    ) -> Result<()> {
        // Send heartbeat messages to establish leadership
        for (&peer_id, address) in peers {
            tracing::debug!("Sending initial heartbeat from leader {} to {} at {}", 
                       node.id, peer_id, address);
            
            // TODO: Send actual RaftMessage::AppendEntries (heartbeat) to peer
        }
        
        Ok(())
    }
    
    /// Determine if we should grant a vote to a candidate
    fn should_grant_vote(
        &self,
        _node: &RaftNode,
        _candidate_id: u64,
        _term: u64,
        _last_log_index: u64,
        _last_log_term: u64,
    ) -> bool {
        // Basic vote granting logic:
        // 1. Haven't voted for anyone else in this term
        // 2. Candidate's log is at least as up-to-date as ours
        // 3. Term is valid
        
        // In a full implementation, this would check:
        // - Current term and voted_for state
        // - Log comparison (last_log_term and last_log_index)
        // - Whether we've already voted in this term
        
        // For now, implement basic logic
        true // Simplified - grant all votes
    }
    
    /// Get random jitter for election timeout
    fn get_jitter(&self) -> Duration {
        // In production, use proper random number generation
        // For now, use a simple deterministic jitter
        Duration::from_millis(
            (std::ptr::addr_of!(self) as usize % 150) as u64
        )
    }
}

/// Election state tracking
#[derive(Debug, Clone)]
pub struct ElectionState {
    /// Current election term
    pub term: u64,
    
    /// Whether an election is currently in progress
    pub in_progress: bool,
    
    /// When the current election started
    pub election_start: Option<Instant>,
    
    /// Number of votes received
    pub votes_received: u64,
    
    /// Number of votes needed to win
    pub votes_needed: u64,
    
    /// Peers that have voted
    pub voters: Vec<u64>,
}

impl Default for ElectionState {
    fn default() -> Self {
        Self {
            term: 0,
            in_progress: false,
            election_start: None,
            votes_received: 0,
            votes_needed: 0,
            voters: Vec::new(),
        }
    }
}