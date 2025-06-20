// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Raft node implementation

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};

// use crate::core::VectorDBError; // Unused import

/// Individual Raft node in the cluster
#[derive(Debug)]
pub struct RaftNode {
    /// Node identifier
    pub id: u64,

    /// Node address for network communication
    pub address: String,

    /// Current node state
    pub state: NodeState,

    /// Metrics for monitoring
    pub metrics: NodeMetrics,

    /// Last activity timestamp
    pub last_activity: Instant,
}

/// Current state of a Raft node
#[derive(Debug, Clone)]
pub enum NodeState {
    /// Node is a follower
    Follower {
        leader_id: Option<u64>,
        last_heartbeat: Option<Instant>,
    },

    /// Node is a candidate (during election)
    Candidate {
        votes_received: u64,
        votes_needed: u64,
        election_start: Instant,
    },

    /// Node is the leader
    Leader {
        followers: HashMap<u64, FollowerState>,
        last_heartbeat_sent: Instant,
    },

    /// Node is isolated or shutting down
    Isolated,
}

/// State tracking for each follower (used by leader)
#[derive(Debug, Clone)]
pub struct FollowerState {
    /// Next log index to send to this follower
    pub next_index: u64,

    /// Highest log index known to be replicated
    pub match_index: u64,

    /// Last time we sent a message to this follower
    pub last_contact: Instant,

    /// Is this follower currently responding
    pub is_active: bool,
}

/// Node performance and health metrics
#[derive(Debug, Clone, Default)]
pub struct NodeMetrics {
    /// Total number of messages sent
    pub messages_sent: u64,

    /// Total number of messages received
    pub messages_received: u64,

    /// Number of successful proposals
    pub proposals_succeeded: u64,

    /// Number of failed proposals
    pub proposals_failed: u64,

    /// Average proposal latency in milliseconds
    pub avg_proposal_latency_ms: f64,

    /// Number of leadership changes observed
    pub leadership_changes: u64,

    /// Time spent as leader (milliseconds)
    pub time_as_leader_ms: u64,

    /// Time spent as follower (milliseconds)
    pub time_as_follower_ms: u64,

    /// Number of log entries applied
    pub entries_applied: u64,

    /// Current log size
    pub log_size: u64,

    /// Last update timestamp
    pub last_updated: Option<Instant>,
}

impl RaftNode {
    /// Create a new Raft node
    pub fn new(id: u64, address: String) -> Self {
        Self {
            id,
            address,
            state: NodeState::Follower {
                leader_id: None,
                last_heartbeat: None,
            },
            metrics: NodeMetrics::default(),
            last_activity: Instant::now(),
        }
    }

    /// Update node state
    pub fn update_state(&mut self, new_state: NodeState) {
        // Update metrics based on state transitions
        match (&self.state, &new_state) {
            (NodeState::Follower { .. }, NodeState::Leader { .. }) => {
                self.metrics.leadership_changes += 1;
            }
            (NodeState::Leader { .. }, NodeState::Follower { .. }) => {
                // Leadership lost
            }
            (NodeState::Candidate { .. }, NodeState::Leader { .. }) => {
                self.metrics.leadership_changes += 1;
            }
            _ => {}
        }

        self.state = new_state;
        self.last_activity = Instant::now();
        self.metrics.last_updated = Some(Instant::now());
    }

    /// Check if this node is the leader
    pub fn is_leader(&self) -> bool {
        matches!(self.state, NodeState::Leader { .. })
    }

    /// Check if this node is a follower
    pub fn is_follower(&self) -> bool {
        matches!(self.state, NodeState::Follower { .. })
    }

    /// Check if this node is a candidate
    pub fn is_candidate(&self) -> bool {
        matches!(self.state, NodeState::Candidate { .. })
    }

    /// Get the current leader ID if known
    pub fn leader_id(&self) -> Option<u64> {
        match &self.state {
            NodeState::Follower { leader_id, .. } => *leader_id,
            NodeState::Leader { .. } => Some(self.id),
            _ => None,
        }
    }

    /// Record a message sent
    pub fn record_message_sent(&mut self) {
        self.metrics.messages_sent += 1;
        self.last_activity = Instant::now();
    }

    /// Record a message received
    pub fn record_message_received(&mut self) {
        self.metrics.messages_received += 1;
        self.last_activity = Instant::now();
    }

    /// Record a successful proposal
    pub fn record_proposal_success(&mut self, latency_ms: f64) {
        self.metrics.proposals_succeeded += 1;

        // Update rolling average
        let total_proposals = self.metrics.proposals_succeeded + self.metrics.proposals_failed;
        self.metrics.avg_proposal_latency_ms =
            (self.metrics.avg_proposal_latency_ms * (total_proposals - 1) as f64 + latency_ms)
                / total_proposals as f64;
    }

    /// Record a failed proposal
    pub fn record_proposal_failure(&mut self) {
        self.metrics.proposals_failed += 1;
    }

    /// Record log entry application
    pub fn record_entry_applied(&mut self) {
        self.metrics.entries_applied += 1;
    }

    /// Update log size
    pub fn update_log_size(&mut self, size: u64) {
        self.metrics.log_size = size;
    }

    /// Check if the node has been inactive for too long
    pub fn is_inactive(&self, timeout: Duration) -> bool {
        self.last_activity.elapsed() > timeout
    }

    /// Get node health status
    pub fn health_status(&self) -> NodeHealthStatus {
        let _now = Instant::now();
        let inactive_duration = self.last_activity.elapsed();

        let status = if inactive_duration > Duration::from_secs(30) {
            HealthLevel::Critical
        } else if inactive_duration > Duration::from_secs(10) {
            HealthLevel::Warning
        } else {
            HealthLevel::Healthy
        };

        NodeHealthStatus {
            level: status,
            last_activity: self.last_activity,
            state: self.state.clone(),
            metrics: self.metrics.clone(),
            message: self.get_health_message(),
        }
    }

    /// Get a human-readable health message
    fn get_health_message(&self) -> String {
        match &self.state {
            NodeState::Leader { followers, .. } => {
                let active_followers = followers.values().filter(|f| f.is_active).count();
                let total_followers = followers.len();
                format!(
                    "Leader with {}/{} active followers",
                    active_followers, total_followers
                )
            }
            NodeState::Follower {
                leader_id,
                last_heartbeat,
            } => match (leader_id, last_heartbeat) {
                (Some(id), Some(hb)) => {
                    let hb_age = hb.elapsed();
                    format!("Follower of leader {}, last heartbeat {:?} ago", id, hb_age)
                }
                (Some(id), None) => format!("Follower of leader {}, no heartbeat received", id),
                (None, _) => "Follower with no known leader".to_string(),
            },
            NodeState::Candidate {
                votes_received,
                votes_needed,
                election_start,
            } => {
                let election_duration = election_start.elapsed();
                format!(
                    "Candidate with {}/{} votes, election running for {:?}",
                    votes_received, votes_needed, election_duration
                )
            }
            NodeState::Isolated => "Node is isolated".to_string(),
        }
    }
}

/// Node health status
#[derive(Debug, Clone)]
pub struct NodeHealthStatus {
    pub level: HealthLevel,
    pub last_activity: Instant,
    pub state: NodeState,
    pub metrics: NodeMetrics,
    pub message: String,
}

/// Health level enumeration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HealthLevel {
    Healthy,
    Warning,
    Critical,
}

impl Default for NodeState {
    fn default() -> Self {
        NodeState::Follower {
            leader_id: None,
            last_heartbeat: None,
        }
    }
}
