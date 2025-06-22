// // Copyright 2025 ProximaDB
// //
// // Licensed under the Apache License, Version 2.0 (the "License");
// // you may not use this file except in compliance with the License.
// 
// //! Raft log replication implementation
// 
// use std::collections::HashMap;
// use std::sync::Arc;
// use std::time::{Duration, Instant};
// use tokio::sync::RwLock;
// 
// use crate::core::VectorDBError;
// 
// type Result<T> = std::result::Result<T, VectorDBError>;
// type ProximaDBError = VectorDBError;
// use super::message::LogEntry;
// use super::{RaftMessage, RaftNode};
// 
// /// Log replication manager for Raft leader
// pub struct LogReplication {
//     /// Current node (must be leader)
//     node: Arc<RwLock<RaftNode>>,
// 
//     /// Peer nodes for replication
//     peers: Arc<RwLock<HashMap<u64, String>>>,
// 
//     /// Replication state for each follower
//     follower_states: Arc<RwLock<HashMap<u64, FollowerReplicationState>>>,
// 
//     /// Heartbeat interval
//     heartbeat_interval: Duration,
// 
//     /// Last heartbeat sent time
//     last_heartbeat: Instant,
// 
//     /// Maximum batch size for log entries
//     max_batch_size: usize,
// }
// 
// /// Replication state for a specific follower
// #[derive(Debug, Clone)]
// pub struct FollowerReplicationState {
//     /// Next log index to send to this follower
//     pub next_index: u64,
// 
//     /// Highest log index known to be replicated on follower
//     pub match_index: u64,
// 
//     /// Last time we sent entries to this follower
//     pub last_sent: Instant,
// 
//     /// Number of in-flight append entries requests
//     pub in_flight_requests: u32,
// 
//     /// Whether this follower is currently reachable
//     pub is_reachable: bool,
// 
//     /// Retry count for failed replications
//     pub retry_count: u32,
// 
//     /// Last known term from this follower
//     pub last_known_term: u64,
// }
// 
// impl LogReplication {
//     /// Create a new log replication manager
//     pub fn new(
//         node: Arc<RwLock<RaftNode>>,
//         peers: Arc<RwLock<HashMap<u64, String>>>,
//         heartbeat_interval: Duration,
//     ) -> Self {
//         Self {
//             node,
//             peers,
//             follower_states: Arc::new(RwLock::new(HashMap::new())),
//             heartbeat_interval,
//             last_heartbeat: Instant::now(),
//             max_batch_size: 100, // Maximum 100 entries per batch
//         }
//     }
// 
//     /// Initialize replication for a new cluster configuration
//     pub async fn initialize_replication(&self) -> Result<()> {
//         let node = self.node.read().await;
//         let peers = self.peers.read().await;
// 
//         if !node.is_leader() {
//             return Err(ProximaDBError::Consensus(
//                 crate::core::ConsensusError::Raft(
//                     "Cannot initialize replication - not a leader".to_string(),
//                 ),
//             ));
//         }
// 
//         let mut follower_states = self.follower_states.write().await;
//         follower_states.clear();
// 
//         // Initialize state for each follower
//         for &peer_id in peers.keys() {
//             follower_states.insert(
//                 peer_id,
//                 FollowerReplicationState {
//                     next_index: 1, // Will be updated based on actual log state
//                     match_index: 0,
//                     last_sent: Instant::now(),
//                     in_flight_requests: 0,
//                     is_reachable: true,
//                     retry_count: 0,
//                     last_known_term: 0,
//                 },
//             );
//         }
// 
//         tracing::info!("Initialized replication for {} followers", peers.len());
//         Ok(())
//     }
// 
//     /// Replicate log entries to all followers
//     pub async fn replicate_entries(&self, entries: &[LogEntry]) -> Result<()> {
//         let node = self.node.read().await;
//         let peers = self.peers.read().await;
// 
//         if !node.is_leader() {
//             return Err(ProximaDBError::Consensus(
//                 crate::core::ConsensusError::Raft("Cannot replicate - not a leader".to_string()),
//             ));
//         }
// 
//         // Send entries to each follower
//         for (&peer_id, address) in peers.iter() {
//             self.send_entries_to_follower(peer_id, address, entries)
//                 .await?;
//         }
// 
//         Ok(())
//     }
// 
//     /// Send heartbeat messages to maintain leadership
//     pub async fn send_heartbeats(&mut self) -> Result<()> {
//         let now = Instant::now();
// 
//         if now.duration_since(self.last_heartbeat) < self.heartbeat_interval {
//             return Ok(());
//         }
// 
//         let node = self.node.read().await;
//         let peers = self.peers.read().await;
// 
//         if !node.is_leader() {
//             return Ok(());
//         }
// 
//         // Send empty append entries (heartbeats) to all followers
//         for (&peer_id, address) in peers.iter() {
//             self.send_heartbeat_to_follower(peer_id, address).await?;
//         }
// 
//         self.last_heartbeat = now;
//         tracing::debug!("Sent heartbeats to {} followers", peers.len());
// 
//         Ok(())
//     }
// 
//     /// Handle append entries response from a follower
//     pub async fn handle_append_response(
//         &self,
//         follower_id: u64,
//         term: u64,
//         success: bool,
//         match_index: Option<u64>,
//     ) -> Result<()> {
//         let mut follower_states = self.follower_states.write().await;
// 
//         if let Some(state) = follower_states.get_mut(&follower_id) {
//             state.in_flight_requests = state.in_flight_requests.saturating_sub(1);
//             state.last_known_term = term;
//             state.is_reachable = true;
// 
//             if success {
//                 // Update replication progress
//                 if let Some(match_idx) = match_index {
//                     state.match_index = match_idx;
//                     state.next_index = match_idx + 1;
//                 }
//                 state.retry_count = 0;
// 
//                 tracing::debug!(
//                     "Replication to follower {} succeeded, match_index: {:?}",
//                     follower_id,
//                     match_index
//                 );
//             } else {
//                 // Replication failed, backtrack
//                 state.next_index = state.next_index.saturating_sub(1);
//                 state.retry_count += 1;
// 
//                 tracing::warn!(
//                     "Replication to follower {} failed, backing off to index {}",
//                     follower_id,
//                     state.next_index
//                 );
// 
//                 // If too many failures, mark as unreachable
//                 if state.retry_count > 5 {
//                     state.is_reachable = false;
//                     tracing::error!(
//                         "Follower {} marked as unreachable after {} failures",
//                         follower_id,
//                         state.retry_count
//                     );
//                 }
//             }
//         }
// 
//         Ok(())
//     }
// 
//     /// Check if a log index has been replicated to majority
//     pub async fn is_replicated_to_majority(&self, log_index: u64) -> bool {
//         let follower_states = self.follower_states.read().await;
//         let total_nodes = follower_states.len() + 1; // +1 for leader
//         let majority_count = (total_nodes / 2) + 1;
// 
//         // Count nodes that have replicated up to log_index
//         let mut replicated_count = 1; // Leader always has the entry
// 
//         for state in follower_states.values() {
//             if state.match_index >= log_index {
//                 replicated_count += 1;
//             }
//         }
// 
//         replicated_count >= majority_count
//     }
// 
//     /// Get replication status for all followers
//     pub async fn get_replication_status(&self) -> HashMap<u64, ReplicationStatus> {
//         let follower_states = self.follower_states.read().await;
//         let mut status = HashMap::new();
// 
//         for (&follower_id, state) in follower_states.iter() {
//             status.insert(
//                 follower_id,
//                 ReplicationStatus {
//                     follower_id,
//                     next_index: state.next_index,
//                     match_index: state.match_index,
//                     is_reachable: state.is_reachable,
//                     in_flight_requests: state.in_flight_requests,
//                     retry_count: state.retry_count,
//                     lag: self.calculate_lag(state).await,
//                     last_contact: state.last_sent,
//                 },
//             );
//         }
// 
//         status
//     }
// 
//     /// Send log entries to a specific follower
//     async fn send_entries_to_follower(
//         &self,
//         follower_id: u64,
//         address: &str,
//         entries: &[LogEntry],
//     ) -> Result<()> {
//         let mut follower_states = self.follower_states.write().await;
// 
//         if let Some(state) = follower_states.get_mut(&follower_id) {
//             if !state.is_reachable {
//                 return Ok(()); // Skip unreachable followers
//             }
// 
//             // Limit batch size
//             let batch_size = std::cmp::min(entries.len(), self.max_batch_size);
//             let batch = &entries[..batch_size];
// 
//             // Create append entries message
//             let _message = RaftMessage::AppendEntries {
//                 term: 1,      // TODO: Get actual term from node
//                 leader_id: 0, // TODO: Get leader ID from node
//                 prev_log_index: state.next_index.saturating_sub(1),
//                 prev_log_term: 0, // TODO: Get from log
//                 entries: batch.to_vec(),
//                 leader_commit: 0, // TODO: Get commit index
//             };
// 
//             // Update state
//             state.in_flight_requests += 1;
//             state.last_sent = Instant::now();
// 
//             tracing::debug!(
//                 "Sending {} entries to follower {} at {}",
//                 batch.len(),
//                 follower_id,
//                 address
//             );
// 
//             // TODO: Send actual message over network
//             // This would use gRPC or HTTP to send the message
//         }
// 
//         Ok(())
//     }
// 
//     /// Send heartbeat to a specific follower
//     async fn send_heartbeat_to_follower(&self, follower_id: u64, address: &str) -> Result<()> {
//         // Send empty append entries as heartbeat
//         self.send_entries_to_follower(follower_id, address, &[])
//             .await
//     }
// 
//     /// Calculate replication lag for a follower
//     async fn calculate_lag(&self, state: &FollowerReplicationState) -> u64 {
//         // In a full implementation, this would compare against the current log index
//         // For now, use a simple heuristic
//         let current_time = Instant::now();
//         let time_since_last_contact = current_time.duration_since(state.last_sent);
// 
//         if time_since_last_contact > Duration::from_secs(1) {
//             time_since_last_contact.as_millis() as u64
//         } else {
//             0
//         }
//     }
// }
// 
// /// Replication status for a follower
// #[derive(Debug, Clone)]
// pub struct ReplicationStatus {
//     pub follower_id: u64,
//     pub next_index: u64,
//     pub match_index: u64,
//     pub is_reachable: bool,
//     pub in_flight_requests: u32,
//     pub retry_count: u32,
//     pub lag: u64, // Milliseconds
//     pub last_contact: Instant,
// }
// 
// /// Replication metrics
// #[derive(Debug, Clone, Default)]
// pub struct ReplicationMetrics {
//     /// Total entries replicated
//     pub entries_replicated: u64,
// 
//     /// Total replication failures
//     pub replication_failures: u64,
// 
//     /// Average replication latency
//     pub avg_replication_latency_ms: f64,
// 
//     /// Number of currently unreachable followers
//     pub unreachable_followers: u32,
// 
//     /// Heartbeats sent
//     pub heartbeats_sent: u64,
// }
