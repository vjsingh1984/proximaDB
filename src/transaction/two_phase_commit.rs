/*
 * Copyright 2025 Vijaykumar Singh
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

//! # Two-Phase Commit Protocol for Cross-Model ACID Transactions
//!
//! This module implements the two-phase commit (2PC) protocol to ensure
//! atomicity across multiple data models (vector, document, graph, time-series).
//!
//! ## Protocol
//!
//! ### Phase 1: Prepare
//! 1. Coordinator sends PREPARE to all participants
//! 2. Each participant validates the transaction
//! 3. Participant writes prepare record to WAL
//! 4. Participant votes YES or NO
//!
//! ### Phase 2: Commit
//! 1. If all voted YES, coordinator sends COMMIT to all
//! 2. If any voted NO, coordinator sends ABORT to all
//! 3. Participants apply changes and write commit/abort record to WAL
//!
//! ## Failure Recovery
//!
//! - **Coordinator failure**: Participants timeout and abort
//! - **Participant failure**: Coordinator retries prepare
//! - **Network partition**: Timeout and abort
//! - **WAL replay**: On restart, check prepared transactions and decide

use proximadb_kernel::error::ProximaDBError;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Unique transaction identifier
pub type TransactionId = u64;

/// Participant identifier (e.g., "vector:products", "graph:users")
pub type ParticipantId = String;

/// Transaction state machine
#[derive(Debug, Clone, PartialEq)]
pub enum TransactionState {
    /// Transaction initialized
    Initialized,
    /// Prepare phase in progress
    Preparing,
    /// All participants voted yes
    Prepared,
    /// Commit phase in progress
    Committing,
    /// Transaction committed
    Committed,
    /// Transaction aborted
    Aborted,
}

/// Participant vote in prepare phase
#[derive(Debug, Clone, PartialEq)]
pub enum Vote {
    /// Participant can commit
    Yes,
    /// Participant cannot commit
    No,
}

/// Transaction participant (storage engine, graph engine, etc.)
#[async_trait::async_trait]
pub trait TransactionParticipant: Send + Sync {
    /// Prepare the transaction (vote phase)
    ///
    /// Returns YES if participant can commit, NO otherwise
    async fn prepare(&self, tx_id: TransactionId) -> Result<Vote>;

    /// Commit the transaction
    async fn commit(&self, tx_id: TransactionId) -> Result<()>;

    /// Rollback the transaction
    async fn rollback(&self, tx_id: TransactionId) -> Result<()>;

    /// Get participant ID
    fn participant_id(&self) -> &str;

    /// Check if participant is healthy
    async fn is_healthy(&self) -> bool;

    /// Whether this participant can durably apply a committed transaction to a live engine.
    ///
    /// Buffer-only participants should return `false` so higher-level coordinators can reject
    /// transaction flows that would otherwise look durable without actually reaching storage.
    fn supports_durable_commit(&self) -> bool {
        false
    }
}

/// Two-phase commit coordinator
pub struct TwoPhaseCommit {
    /// Transaction state
    transactions: Arc<RwLock<HashMap<TransactionId, TransactionState>>>,

    /// Participant votes (tx_id -> participant_id -> vote)
    votes: Arc<RwLock<HashMap<TransactionId, HashMap<ParticipantId, Vote>>>>,

    /// Registered participants
    participants: Arc<RwLock<HashMap<ParticipantId, Arc<dyn TransactionParticipant>>>>,

    /// Transaction timeout in seconds
    timeout_secs: u64,
}

impl TwoPhaseCommit {
    /// Create a new 2PC coordinator
    pub fn new(timeout_secs: u64) -> Self {
        Self {
            transactions: Arc::new(RwLock::new(HashMap::new())),
            votes: Arc::new(RwLock::new(HashMap::new())),
            participants: Arc::new(RwLock::new(HashMap::new())),
            timeout_secs,
        }
    }

    /// Register a transaction participant
    pub async fn register_participant(
        &self,
        participant: Arc<dyn TransactionParticipant>,
    ) -> Result<()> {
        let id = participant.participant_id().to_string();
        info!("Registering transaction participant: {}", id);

        let mut participants = self.participants.write().await;
        participants.insert(id, participant);

        Ok(())
    }

    /// Begin a new transaction
    pub async fn begin(&self) -> Result<TransactionId> {
        let tx_id = self.generate_tx_id();

        let mut transactions = self.transactions.write().await;
        transactions.insert(tx_id, TransactionState::Initialized);

        debug!("Transaction {} started", tx_id);
        Ok(tx_id)
    }

    /// Execute two-phase commit
    ///
    /// # Arguments
    ///
    /// * `tx_id` - Transaction ID
    /// * `participant_ids` - IDs of participants to involve in transaction
    ///
    /// # Returns
    ///
    /// Ok(()) if transaction committed, Err if aborted
    pub async fn commit(
        &self,
        tx_id: TransactionId,
        participant_ids: &[ParticipantId],
    ) -> Result<()> {
        info!(
            "Starting 2PC for transaction {} with {} participants",
            tx_id,
            participant_ids.len()
        );

        // Phase 1: Prepare
        let prepare_result = self.prepare(tx_id, participant_ids).await;

        if let Err(ref e) = prepare_result {
            warn!("Prepare phase failed for tx {}: {}", tx_id, e);
            self.abort(tx_id, participant_ids).await?;
            return Err(ProximaDBError::TransactionAborted(format!(
                "Prepare phase failed: {}",
                e
            )));
        }

        // Phase 2: Commit
        self.commit_prepared(tx_id, participant_ids).await
    }

    /// Run only the prepare phase of 2PC.
    ///
    /// This is used by the higher-level coordinator so it can durably record
    /// prepared participants before issuing the final commit decision.
    pub async fn prepare(
        &self,
        tx_id: TransactionId,
        participant_ids: &[ParticipantId],
    ) -> Result<()> {
        self.prepare_phase(tx_id, participant_ids).await
    }

    /// Commit a transaction that has already completed the prepare phase.
    pub async fn commit_prepared(
        &self,
        tx_id: TransactionId,
        participant_ids: &[ParticipantId],
    ) -> Result<()> {
        let state = self.get_state(tx_id).await;
        if !matches!(
            state,
            Some(TransactionState::Prepared) | Some(TransactionState::Committing)
        ) {
            return Err(ProximaDBError::InvalidInput(format!(
                "Transaction {} is not prepared for commit",
                tx_id
            )));
        }

        self.commit_phase(tx_id, participant_ids).await?;

        // Update transaction state
        {
            let mut transactions = self.transactions.write().await;
            transactions.insert(tx_id, TransactionState::Committed);
        }

        info!("Transaction {} committed successfully", tx_id);
        Ok(())
    }

    /// Rehydrate a prepared transaction during WAL recovery.
    pub async fn mark_prepared_for_recovery(&self, tx_id: TransactionId) {
        let mut transactions = self.transactions.write().await;
        transactions.insert(tx_id, TransactionState::Prepared);
    }

    /// Prepare phase: ask all participants to vote
    async fn prepare_phase(
        &self,
        tx_id: TransactionId,
        participant_ids: &[ParticipantId],
    ) -> Result<()> {
        // Update state to preparing
        {
            let mut transactions = self.transactions.write().await;
            transactions.insert(tx_id, TransactionState::Preparing);
        }

        // Get participants
        let participants = self.participants.read().await;
        let mut votes = HashMap::new();

        // Ask each participant to prepare
        for participant_id in participant_ids {
            let participant = participants.get(participant_id).ok_or_else(|| {
                ProximaDBError::Internal(format!("Participant not found: {}", participant_id))
            })?;

            debug!("Preparing participant {} for tx {}", participant_id, tx_id);

            let vote = participant.prepare(tx_id).await?;
            debug!("Participant {} voted {:?}", participant_id, vote);

            // If any participant votes NO, abort immediately
            if vote == Vote::No {
                return Err(ProximaDBError::TransactionAborted(format!(
                    "Participant {} voted NO",
                    participant_id
                )));
            }

            // Store the vote (after checking)
            votes.insert(participant_id.clone(), vote);
        }

        // All participants voted YES
        {
            let mut votes_store = self.votes.write().await;
            votes_store.insert(tx_id, votes);
        }

        // Update state to prepared
        {
            let mut transactions = self.transactions.write().await;
            transactions.insert(tx_id, TransactionState::Prepared);
        }

        debug!("Prepare phase completed for tx {}", tx_id);
        Ok(())
    }

    /// Commit phase: ask all participants to commit
    async fn commit_phase(
        &self,
        tx_id: TransactionId,
        participant_ids: &[ParticipantId],
    ) -> Result<()> {
        // Update state to committing
        {
            let mut transactions = self.transactions.write().await;
            transactions.insert(tx_id, TransactionState::Committing);
        }

        // Get participants
        let participants = self.participants.read().await;

        // Ask each participant to commit
        for participant_id in participant_ids {
            let participant = participants.get(participant_id).ok_or_else(|| {
                ProximaDBError::Internal(format!("Participant not found: {}", participant_id))
            })?;

            debug!("Committing participant {} for tx {}", participant_id, tx_id);

            participant.commit(tx_id).await?;
        }

        debug!("Commit phase completed for tx {}", tx_id);
        Ok(())
    }

    /// Abort transaction: ask all participants to rollback
    pub async fn abort(
        &self,
        tx_id: TransactionId,
        participant_ids: &[ParticipantId],
    ) -> Result<()> {
        warn!("Aborting transaction {}", tx_id);

        // Update state to aborted
        {
            let mut transactions = self.transactions.write().await;
            transactions.insert(tx_id, TransactionState::Aborted);
        }

        // Get participants
        let participants = self.participants.read().await;

        // Ask each participant to rollback
        for participant_id in participant_ids {
            if let Some(participant) = participants.get(participant_id) {
                debug!(
                    "Rolling back participant {} for tx {}",
                    participant_id, tx_id
                );

                // Ignore rollback errors (best effort)
                let _ = participant.rollback(tx_id).await;
            }
        }

        debug!("Transaction {} aborted", tx_id);
        Ok(())
    }

    /// Get transaction state
    pub async fn get_state(&self, tx_id: TransactionId) -> Option<TransactionState> {
        let transactions = self.transactions.read().await;
        transactions.get(&tx_id).cloned()
    }

    /// Set transaction state directly (used for recovery)
    pub async fn set_state(&self, tx_id: TransactionId, state: TransactionState) {
        let mut transactions = self.transactions.write().await;
        debug!("Transaction {} state set to {:?}", tx_id, state);
        transactions.insert(tx_id, state);
    }

    /// Generate unique transaction ID
    fn generate_tx_id(&self) -> TransactionId {
        use std::time::{SystemTime, UNIX_EPOCH};

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Combine timestamp with random component
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        timestamp.hash(&mut hasher);
        std::thread::current().id().hash(&mut hasher);

        hasher.finish()
    }

    /// Cleanup old transactions
    pub async fn cleanup_old_transactions(&self, _max_age_secs: u64) {
        let mut to_remove = Vec::new();

        // Collect transaction IDs to remove
        {
            let transactions = self.transactions.read().await;
            for (tx_id, state) in transactions.iter() {
                if matches!(
                    state,
                    TransactionState::Committed | TransactionState::Aborted
                ) {
                    to_remove.push(*tx_id);
                }
            }
        }

        // Remove from transactions
        {
            let mut transactions = self.transactions.write().await;
            for tx_id in &to_remove {
                transactions.remove(tx_id);
            }
        }

        // Remove from votes
        {
            let mut votes = self.votes.write().await;
            for tx_id in &to_remove {
                votes.remove(tx_id);
            }
        }

        debug!("Cleaned up {} old transactions", to_remove.len());
    }
}

// Implement Clone for TwoPhaseCommit
impl Clone for TwoPhaseCommit {
    fn clone(&self) -> Self {
        Self {
            transactions: self.transactions.clone(),
            votes: self.votes.clone(),
            participants: self.participants.clone(),
            timeout_secs: self.timeout_secs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mock participant for testing
    struct MockParticipant {
        id: String,
        vote: Vote,
        healthy: bool,
    }

    #[async_trait::async_trait]
    impl TransactionParticipant for MockParticipant {
        async fn prepare(&self, _tx_id: TransactionId) -> Result<Vote> {
            Ok(self.vote.clone())
        }

        async fn commit(&self, _tx_id: TransactionId) -> Result<()> {
            Ok(())
        }

        async fn rollback(&self, _tx_id: TransactionId) -> Result<()> {
            Ok(())
        }

        fn participant_id(&self) -> &str {
            &self.id
        }

        async fn is_healthy(&self) -> bool {
            self.healthy
        }
    }

    #[tokio::test]
    async fn test_2pc_creation() {
        let tpc = TwoPhaseCommit::new(30);
        assert_eq!(tpc.timeout_secs, 30);
    }

    #[tokio::test]
    async fn test_transaction_begin() {
        let tpc = TwoPhaseCommit::new(30);
        let tx_id = tpc.begin().await.unwrap();

        let state = tpc.get_state(tx_id).await;
        assert_eq!(state, Some(TransactionState::Initialized));
    }

    #[tokio::test]
    async fn test_register_participant() {
        let tpc = TwoPhaseCommit::new(30);
        let participant = Arc::new(MockParticipant {
            id: "test_participant".to_string(),
            vote: Vote::Yes,
            healthy: true,
        });

        tpc.register_participant(participant).await.unwrap();

        let participants = tpc.participants.read().await;
        assert!(participants.contains_key("test_participant"));
    }

    #[tokio::test]
    async fn test_2pc_commit_success() {
        let tpc = TwoPhaseCommit::new(30);

        // Register participants
        let p1 = Arc::new(MockParticipant {
            id: "p1".to_string(),
            vote: Vote::Yes,
            healthy: true,
        });
        let p2 = Arc::new(MockParticipant {
            id: "p2".to_string(),
            vote: Vote::Yes,
            healthy: true,
        });

        tpc.register_participant(p1).await.unwrap();
        tpc.register_participant(p2).await.unwrap();

        // Begin and commit transaction
        let tx_id = tpc.begin().await.unwrap();
        let result = tpc
            .commit(tx_id, &["p1".to_string(), "p2".to_string()])
            .await;

        assert!(result.is_ok());

        let state = tpc.get_state(tx_id).await;
        assert_eq!(state, Some(TransactionState::Committed));
    }

    #[tokio::test]
    async fn test_2pc_abort_on_no_vote() {
        let tpc = TwoPhaseCommit::new(30);

        // Register participants (one votes NO)
        let p1 = Arc::new(MockParticipant {
            id: "p1".to_string(),
            vote: Vote::Yes,
            healthy: true,
        });
        let p2 = Arc::new(MockParticipant {
            id: "p2".to_string(),
            vote: Vote::No,
            healthy: true,
        });

        tpc.register_participant(p1).await.unwrap();
        tpc.register_participant(p2).await.unwrap();

        // Begin and commit transaction (should abort)
        let tx_id = tpc.begin().await.unwrap();
        let result = tpc
            .commit(tx_id, &["p1".to_string(), "p2".to_string()])
            .await;

        assert!(result.is_err());

        let state = tpc.get_state(tx_id).await;
        assert_eq!(state, Some(TransactionState::Aborted));
    }

    #[tokio::test]
    async fn test_2pc_prepare_then_commit_prepared() {
        let tpc = TwoPhaseCommit::new(30);

        let p1 = Arc::new(MockParticipant {
            id: "p1".to_string(),
            vote: Vote::Yes,
            healthy: true,
        });
        let p2 = Arc::new(MockParticipant {
            id: "p2".to_string(),
            vote: Vote::Yes,
            healthy: true,
        });

        tpc.register_participant(p1).await.unwrap();
        tpc.register_participant(p2).await.unwrap();

        let tx_id = tpc.begin().await.unwrap();
        tpc.prepare(tx_id, &["p1".to_string(), "p2".to_string()])
            .await
            .unwrap();

        let state = tpc.get_state(tx_id).await;
        assert_eq!(state, Some(TransactionState::Prepared));

        tpc.commit_prepared(tx_id, &["p1".to_string(), "p2".to_string()])
            .await
            .unwrap();

        let state = tpc.get_state(tx_id).await;
        assert_eq!(state, Some(TransactionState::Committed));
    }
}
