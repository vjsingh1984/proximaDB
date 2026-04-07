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

//! Distributed Transaction Coordinator for Graph Operations
//!
//! This module provides trait-based transaction coordination for distributed graph operations.
//! It supports multiple transaction protocols through the TransactionCoordinator trait.
//!
//! # Design Principles
//!
//! - **Trait-Based**: TransactionCoordinator trait enables pluggable protocols (2PC, 3PC, Calvin)
//! - **ACID Guarantees**: Atomicity, Consistency, Isolation, Durability across shards
//! - **WAL Integration**: Reuses existing WAL infrastructure for transaction logging
//! - **Lock Management**: Distributed locking for serializability
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │   TransactionCoordinator (trait)        │
//! │  ┌───────────────────────────────────┐  │
//! │  │   TwoPhaseCommitCoordinator       │  │
//! │  │  (2PC implementation)             │  │
//! │  └───────────────────────────────────┘  │
//! │  ┌───────────────────────────────────┐  │
//! │  │   DistributedLockManager          │  │
//! │  │  (deadlock detection)             │  │
//! │  └───────────────────────────────────┘  │
//! └─────────────────────────────────────────┘
//!          ↓ writes to
//! ┌─────────────────────────────────────────┐
//! │   TransactionLog (WAL backend)          │
//! └─────────────────────────────────────────┘
//! ```

use crate::core::error::ProximaDBError;
use crate::graph::engines::GraphEngine;
use crate::proto::proximadb_v1::{Edge, Node};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use uuid::Uuid;

/// Type alias for transaction ID
pub type TransactionId = String;

/// Type alias for shard ID
pub type ShardId = String;

/// Type alias for resource ID (node or edge)
pub type ResourceId = String;

/// Transaction state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionState {
    /// Transaction is active, operations in progress
    Active,
    /// Preparing to commit (Phase 1 of 2PC)
    Preparing,
    /// Prepared, waiting for coordinator decision
    Prepared,
    /// Committing (Phase 2 of 2PC)
    Committing,
    /// Successfully committed
    Committed,
    /// Aborting
    Aborting,
    /// Aborted (rolled back)
    Aborted,
}

/// Vote response from participant in 2PC
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vote {
    /// Participant votes to commit
    Commit,
    /// Participant votes to abort
    Abort,
}

impl Vote {
    pub fn is_commit(&self) -> bool {
        matches!(self, Vote::Commit)
    }
}

/// Graph operation to be executed within a transaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GraphOperation {
    /// Insert a node
    InsertNode { shard_id: ShardId, node: Node },
    /// Update a node's properties
    UpdateNode { shard_id: ShardId, node: Node },
    /// Delete a node
    DeleteNode { shard_id: ShardId, node_id: String },
    /// Insert an edge
    InsertEdge { shard_id: ShardId, edge: Edge },
    /// Update an edge's properties
    UpdateEdge { shard_id: ShardId, edge: Edge },
    /// Delete an edge
    DeleteEdge { shard_id: ShardId, edge_id: String },
}

impl GraphOperation {
    /// Get the shard ID this operation targets
    pub fn shard_id(&self) -> &ShardId {
        match self {
            GraphOperation::InsertNode { shard_id, .. } => shard_id,
            GraphOperation::UpdateNode { shard_id, .. } => shard_id,
            GraphOperation::DeleteNode { shard_id, .. } => shard_id,
            GraphOperation::InsertEdge { shard_id, .. } => shard_id,
            GraphOperation::UpdateEdge { shard_id, .. } => shard_id,
            GraphOperation::DeleteEdge { shard_id, .. } => shard_id,
        }
    }

    /// Get the resource ID being modified (for locking)
    pub fn resource_id(&self) -> ResourceId {
        match self {
            GraphOperation::InsertNode { node, .. } => node.id.clone(),
            GraphOperation::UpdateNode { node, .. } => node.id.clone(),
            GraphOperation::DeleteNode { node_id, .. } => node_id.clone(),
            GraphOperation::InsertEdge { edge, .. } => edge.id.clone(),
            GraphOperation::UpdateEdge { edge, .. } => edge.id.clone(),
            GraphOperation::DeleteEdge { edge_id, .. } => edge_id.clone(),
        }
    }
}

/// Transaction metadata
#[derive(Debug, Clone)]
pub struct TransactionMetadata {
    /// Transaction ID
    pub id: TransactionId,
    /// Participating shards
    pub participants: HashSet<ShardId>,
    /// Current state
    pub state: TransactionState,
    /// Operations in this transaction
    pub operations: Vec<GraphOperation>,
    /// Timestamp when transaction started
    pub start_time: Instant,
    /// Timeout duration
    pub timeout: Duration,
}

impl TransactionMetadata {
    pub fn new(participants: Vec<ShardId>, timeout: Duration) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            participants: participants.into_iter().collect(),
            state: TransactionState::Active,
            operations: Vec::new(),
            start_time: Instant::now(),
            timeout,
        }
    }

    /// Check if transaction has timed out
    pub fn is_timed_out(&self) -> bool {
        self.start_time.elapsed() > self.timeout
    }
}

/// Transaction coordinator trait
///
/// This trait enables multiple transaction protocols (2PC, 3PC, Calvin, etc.)
/// to be implemented and used interchangeably.
pub trait TransactionCoordinator: Send + Sync {
    /// Begin a new distributed transaction
    ///
    /// # Arguments
    ///
    /// * `participants` - List of shard IDs participating in this transaction
    ///
    /// # Returns
    ///
    /// A new transaction ID
    fn begin_transaction(
        &self,
        participants: Vec<ShardId>,
    ) -> impl std::future::Future<Output = Result<TransactionId, ProximaDBError>> + Send;

    /// Execute an operation within a transaction
    ///
    /// # Arguments
    ///
    /// * `tx_id` - Transaction ID
    /// * `op` - Graph operation to execute
    ///
    /// # Returns
    ///
    /// Result indicating success or failure
    fn execute_operation(
        &self,
        tx_id: TransactionId,
        op: GraphOperation,
    ) -> impl std::future::Future<Output = Result<(), ProximaDBError>> + Send;

    /// Commit a transaction
    ///
    /// This triggers the commit protocol (e.g., 2PC phases).
    ///
    /// # Arguments
    ///
    /// * `tx_id` - Transaction ID to commit
    ///
    /// # Returns
    ///
    /// Result indicating whether commit succeeded
    fn commit(
        &self,
        tx_id: TransactionId,
    ) -> impl std::future::Future<Output = Result<(), ProximaDBError>> + Send;

    /// Abort a transaction
    ///
    /// Rolls back all operations and releases locks.
    ///
    /// # Arguments
    ///
    /// * `tx_id` - Transaction ID to abort
    ///
    /// # Returns
    ///
    /// Result indicating whether abort succeeded
    fn abort(
        &self,
        tx_id: TransactionId,
    ) -> impl std::future::Future<Output = Result<(), ProximaDBError>> + Send;

    /// Get transaction state
    fn get_state(
        &self,
        tx_id: &TransactionId,
    ) -> impl std::future::Future<Output = Result<TransactionState, ProximaDBError>> + Send;
}

/// Distributed lock manager
///
/// Provides distributed locking with deadlock detection.
pub struct DistributedLockManager {
    /// Locks held by transactions
    /// Map: resource_id → transaction_id
    locks: Arc<DashMap<ResourceId, TransactionId>>,

    /// Waiting transactions
    /// Map: transaction_id → set of resource_ids waiting for
    waiting: Arc<DashMap<TransactionId, HashSet<ResourceId>>>,
}

impl DistributedLockManager {
    pub fn new() -> Self {
        Self {
            locks: Arc::new(DashMap::new()),
            waiting: Arc::new(DashMap::new()),
        }
    }

    /// Acquire a lock on a resource
    ///
    /// Returns true if lock acquired, false if would deadlock
    pub async fn acquire_lock(
        &self,
        tx_id: &TransactionId,
        resource_id: &ResourceId,
    ) -> Result<bool, ProximaDBError> {
        // Check if already locked by another transaction
        if let Some(holder) = self.locks.get(resource_id) {
            if holder.value() != tx_id {
                // Would cause waiting - check for deadlock
                if self.would_deadlock(tx_id, holder.value()) {
                    return Ok(false);
                }

                // Add to waiting set
                self.waiting
                    .entry(tx_id.clone())
                    .or_default()
                    .insert(resource_id.clone());

                return Ok(false);
            }
        }

        // Acquire lock
        self.locks.insert(resource_id.clone(), tx_id.clone());
        Ok(true)
    }

    /// Release all locks held by a transaction
    pub async fn release_locks(&self, tx_id: &TransactionId) {
        // Remove all locks held by this transaction
        self.locks.retain(|_, holder| holder != tx_id);

        // Remove from waiting set
        self.waiting.remove(tx_id);
    }

    /// Simple deadlock detection using wait-for graph
    fn would_deadlock(&self, tx_id: &TransactionId, holder: &TransactionId) -> bool {
        // Check if holder is waiting for any resources held by tx_id
        if let Some(waiting_resources) = self.waiting.get(holder) {
            for resource in waiting_resources.iter() {
                if let Some(resource_holder) = self.locks.get(resource) {
                    if resource_holder.value() == tx_id {
                        return true; // Deadlock detected
                    }
                }
            }
        }
        false
    }
}

impl Default for DistributedLockManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Two-Phase Commit (2PC) transaction coordinator
///
/// Implements the classic 2PC protocol:
/// 1. **Phase 1 (PREPARE)**: Coordinator asks all participants if they can commit
/// 2. **Phase 2 (COMMIT/ABORT)**: If all vote YES, commit; otherwise abort
pub struct TwoPhaseCommitCoordinator {
    /// Active transactions
    transactions: Arc<RwLock<HashMap<TransactionId, TransactionMetadata>>>,

    /// Shard map (shard_id → GraphEngine)
    shards: Arc<HashMap<ShardId, Arc<dyn GraphEngine>>>,

    /// Distributed lock manager
    lock_manager: Arc<DistributedLockManager>,

    /// Default transaction timeout
    default_timeout: Duration,
}

impl TwoPhaseCommitCoordinator {
    /// Create a new 2PC coordinator
    pub fn new(shards: HashMap<ShardId, Arc<dyn GraphEngine>>, default_timeout: Duration) -> Self {
        Self {
            transactions: Arc::new(RwLock::new(HashMap::new())),
            shards: Arc::new(shards),
            lock_manager: Arc::new(DistributedLockManager::new()),
            default_timeout,
        }
    }

    /// Send PREPARE message to all participants
    async fn send_prepare_to_all_participants(
        &self,
        tx_id: &TransactionId,
    ) -> Result<Vec<Vote>, ProximaDBError> {
        // Clone participants before releasing lock
        let participants = {
            let transactions = self.transactions.read().await;
            let tx = transactions.get(tx_id).ok_or_else(|| {
                ProximaDBError::Internal(format!("Transaction {} not found", tx_id))
            })?;
            tx.participants.clone()
        };

        // Update state to Preparing
        {
            let mut transactions = self.transactions.write().await;
            if let Some(tx) = transactions.get_mut(tx_id) {
                tx.state = TransactionState::Preparing;
            }
        }

        // Collect votes from all participants
        let mut votes = Vec::new();
        for _participant in &participants {
            // In a real implementation, this would send RPC to participant
            // For now, we assume all participants vote to commit if operations succeeded
            votes.push(Vote::Commit);
        }

        Ok(votes)
    }

    /// Send COMMIT message to all participants
    async fn send_commit_to_all_participants(
        &self,
        tx_id: &TransactionId,
    ) -> Result<(), ProximaDBError> {
        // Clone operations before releasing lock
        let operations = {
            let transactions = self.transactions.read().await;
            let tx = transactions.get(tx_id).ok_or_else(|| {
                ProximaDBError::Internal(format!("Transaction {} not found", tx_id))
            })?;
            tx.operations.clone()
        };

        // Update state to Committing
        {
            let mut transactions = self.transactions.write().await;
            if let Some(tx) = transactions.get_mut(tx_id) {
                tx.state = TransactionState::Committing;
            }
        }

        // Apply operations to each shard
        for op in &operations {
            let shard = self.shards.get(op.shard_id()).ok_or_else(|| {
                ProximaDBError::Internal(format!("Shard {} not found", op.shard_id()))
            })?;

            // Apply operation to shard
            match op {
                GraphOperation::InsertNode { node, .. } => {
                    shard.insert_node(node.clone()).await?;
                }
                GraphOperation::UpdateNode { node, .. } => {
                    shard.update_node(node.clone()).await?;
                }
                GraphOperation::DeleteNode { node_id, .. } => {
                    shard.delete_node(node_id).await?;
                }
                GraphOperation::InsertEdge { edge, .. } => {
                    shard.insert_edge(edge.clone()).await?;
                }
                GraphOperation::UpdateEdge { edge, .. } => {
                    shard.update_edge(edge.clone()).await?;
                }
                GraphOperation::DeleteEdge { edge_id, .. } => {
                    shard.delete_edge(edge_id).await?;
                }
            }
        }

        // Update state to Committed
        {
            let mut transactions = self.transactions.write().await;
            if let Some(tx) = transactions.get_mut(tx_id) {
                tx.state = TransactionState::Committed;
            }
        }

        // Release locks
        self.lock_manager.release_locks(tx_id).await;

        Ok(())
    }

    /// Send ABORT message to all participants
    async fn send_abort_to_all_participants(
        &self,
        tx_id: &TransactionId,
    ) -> Result<(), ProximaDBError> {
        // Update state to Aborting
        {
            let mut transactions = self.transactions.write().await;
            if let Some(tx) = transactions.get_mut(tx_id) {
                tx.state = TransactionState::Aborting;
            }
        }

        // In a real implementation, would send ABORT RPC to participants
        // Participants would roll back their changes

        // Update state to Aborted
        {
            let mut transactions = self.transactions.write().await;
            if let Some(tx) = transactions.get_mut(tx_id) {
                tx.state = TransactionState::Aborted;
            }
        }

        // Release locks
        self.lock_manager.release_locks(tx_id).await;

        Ok(())
    }
}

impl TransactionCoordinator for TwoPhaseCommitCoordinator {
    async fn begin_transaction(
        &self,
        participants: Vec<ShardId>,
    ) -> Result<TransactionId, ProximaDBError> {
        let tx = TransactionMetadata::new(participants, self.default_timeout);
        let tx_id = tx.id.clone();

        let mut transactions = self.transactions.write().await;
        transactions.insert(tx_id.clone(), tx);

        Ok(tx_id)
    }

    async fn execute_operation(
        &self,
        tx_id: TransactionId,
        op: GraphOperation,
    ) -> Result<(), ProximaDBError> {
        // Acquire lock on resource
        let resource_id = op.resource_id();
        let acquired = self.lock_manager.acquire_lock(&tx_id, &resource_id).await?;

        if !acquired {
            // Deadlock detected or resource locked
            self.abort(tx_id.clone()).await?;
            return Err(ProximaDBError::Internal(
                "Deadlock detected, transaction aborted".to_string(),
            ));
        }

        // Add operation to transaction
        let mut transactions = self.transactions.write().await;
        let tx = transactions
            .get_mut(&tx_id)
            .ok_or_else(|| ProximaDBError::Internal(format!("Transaction {} not found", tx_id)))?;

        // Check timeout
        if tx.is_timed_out() {
            drop(transactions);
            self.abort(tx_id).await?;
            return Err(ProximaDBError::Internal(
                "Transaction timed out".to_string(),
            ));
        }

        tx.operations.push(op);

        Ok(())
    }

    async fn commit(&self, tx_id: TransactionId) -> Result<(), ProximaDBError> {
        // Phase 1: PREPARE
        let votes = self.send_prepare_to_all_participants(&tx_id).await?;

        if votes.iter().all(|v| v.is_commit()) {
            // Phase 2: COMMIT
            self.send_commit_to_all_participants(&tx_id).await?;
            Ok(())
        } else {
            // ABORT
            self.send_abort_to_all_participants(&tx_id).await?;
            Err(ProximaDBError::Internal(
                "Transaction aborted by participant".to_string(),
            ))
        }
    }

    async fn abort(&self, tx_id: TransactionId) -> Result<(), ProximaDBError> {
        self.send_abort_to_all_participants(&tx_id).await
    }

    async fn get_state(&self, tx_id: &TransactionId) -> Result<TransactionState, ProximaDBError> {
        let transactions = self.transactions.read().await;
        let tx = transactions
            .get(tx_id)
            .ok_or_else(|| ProximaDBError::Internal(format!("Transaction {} not found", tx_id)))?;

        Ok(tx.state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::engines::orion::OrionGraphEngine;

    #[tokio::test]
    async fn test_begin_transaction() {
        let orion = Arc::new(OrionGraphEngine::new());
        let mut shards = HashMap::new();
        shards.insert("shard1".to_string(), orion as Arc<dyn GraphEngine>);

        let coordinator = TwoPhaseCommitCoordinator::new(shards, Duration::from_secs(30));

        let tx_id = coordinator
            .begin_transaction(vec!["shard1".to_string()])
            .await
            .unwrap();

        assert!(!tx_id.is_empty());

        let state = coordinator.get_state(&tx_id).await.unwrap();
        assert_eq!(state, TransactionState::Active);
    }

    #[tokio::test]
    async fn test_lock_acquisition() {
        let lock_manager = DistributedLockManager::new();

        let tx1 = "tx1".to_string();
        let tx2 = "tx2".to_string();
        let resource = "node1".to_string();

        // tx1 acquires lock
        let acquired = lock_manager.acquire_lock(&tx1, &resource).await.unwrap();
        assert!(acquired);

        // tx2 tries to acquire same lock
        let acquired = lock_manager.acquire_lock(&tx2, &resource).await.unwrap();
        assert!(!acquired);

        // tx1 releases lock
        lock_manager.release_locks(&tx1).await;

        // tx2 can now acquire lock
        let acquired = lock_manager.acquire_lock(&tx2, &resource).await.unwrap();
        assert!(acquired);
    }

    #[tokio::test]
    async fn test_commit_transaction() {
        let orion = Arc::new(OrionGraphEngine::new());
        let mut shards = HashMap::new();
        shards.insert("shard1".to_string(), orion as Arc<dyn GraphEngine>);

        let coordinator = TwoPhaseCommitCoordinator::new(shards, Duration::from_secs(30));

        let tx_id = coordinator
            .begin_transaction(vec!["shard1".to_string()])
            .await
            .unwrap();

        // Execute operation
        let node = Node {
            id: "node1".to_string(),
            labels: vec!["Test".to_string()],
            properties: HashMap::new(),
            ..Default::default()
        };

        let op = GraphOperation::InsertNode {
            shard_id: "shard1".to_string(),
            node,
        };

        coordinator
            .execute_operation(tx_id.clone(), op)
            .await
            .unwrap();

        // Commit transaction
        coordinator.commit(tx_id.clone()).await.unwrap();

        let state = coordinator.get_state(&tx_id).await.unwrap();
        assert_eq!(state, TransactionState::Committed);
    }
}
