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

//! # Cross-Model Transaction Coordinator
//!
//! This module provides the main coordinator for ACID transactions across
//! multiple data models (vector, document, graph, time-series).
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────┐
//! │ CrossModelTransactionCoordinator        │
//! ├──────────────────────────────────────────┤
//! │  ┌─────────────┐  ┌─────────────────┐  │
//! │  │TwoPhaseCommit│  │ WALCoordinator   │  │
//! │  │ (2PC protocol)│  │ (WAL recovery)  │  │
//! │  └─────────────┘  └─────────────────┘  │
//! └──────────────────────────────────────────┘
//!           ↓                    ↓
//! ┌────────────────┐  ┌────────────────┐
//! │Vector Engine   │  │Vector WAL      │
//! │Document Engine │  │Document WAL    │
//! │Graph Engine    │  │Graph WAL       │
//! │TimeSeries      │  │TimeSeries WAL  │
//! └────────────────┘  └────────────────┘
//! ```
//!
//! ## Transaction Flow
//!
//! 1. **Begin**: Generate transaction ID, write to WAL
//! 2. **Enlist**: Add participants (engines) to transaction
//! 3. **Prepare**: Two-phase commit prepare phase
//! 4. **Commit/Rollback**: Finalize transaction
//! 5. **WAL Replay**: Recovery on restart

use proximadb_kernel::error::ProximaDBError;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use super::two_phase_commit::{
    TransactionId, TransactionParticipant, TransactionState, TwoPhaseCommit,
};
use super::wal_coordinator::WALCoordinator;

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Backwards-compat aliases.
pub type TransactionConfig = CoordinatorTransactionConfig;
pub type TransactionStats = CoordinatorTransactionStats;

/// Configuration for cross-model transaction coordinator
#[derive(Debug, Clone)]
pub struct CoordinatorTransactionConfig {
    /// WAL directory for transaction logs
    pub wal_dir: PathBuf,

    /// Transaction timeout in seconds
    pub timeout_secs: u64,

    /// Enable auto-recovery on startup
    pub enable_recovery: bool,

    /// Cleanup interval for completed transactions (seconds)
    pub cleanup_interval_secs: u64,
}

impl Default for CoordinatorTransactionConfig {
    fn default() -> Self {
        Self {
            wal_dir: PathBuf::from("/tmp/proximadb/transactions"),
            timeout_secs: 30,
            enable_recovery: true,
            cleanup_interval_secs: 300, // 5 minutes
        }
    }
}

/// Transaction statistics
#[derive(Debug, Clone, Default)]
pub struct CoordinatorTransactionStats {
    /// Total transactions started
    pub total_transactions: u64,
    /// Committed transactions
    pub committed_transactions: u64,
    /// Aborted transactions
    pub aborted_transactions: u64,
    /// Active transactions
    pub active_transactions: u64,
    /// Recovered transactions
    pub recovered_transactions: u64,
}

/// Cross-model transaction coordinator
pub struct CrossModelTransactionCoordinator {
    /// Configuration
    config: CoordinatorTransactionConfig,

    /// Two-phase commit coordinator
    two_phase_commit: TwoPhaseCommit,

    /// WAL coordinator
    wal_coordinator: WALCoordinator,

    /// Transaction participants
    participants: Arc<RwLock<HashMap<String, Arc<dyn TransactionParticipant>>>>,

    /// Transaction statistics
    stats: Arc<RwLock<CoordinatorTransactionStats>>,
}

impl CrossModelTransactionCoordinator {
    /// Create a new cross-model transaction coordinator
    pub fn new(config: CoordinatorTransactionConfig) -> Self {
        let two_phase_commit = TwoPhaseCommit::new(config.timeout_secs);
        let wal_coordinator = WALCoordinator::new(config.wal_dir.clone());

        Self {
            config,
            two_phase_commit,
            wal_coordinator,
            participants: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(CoordinatorTransactionStats::default())),
        }
    }

    /// Initialize the coordinator (recover transactions)
    pub async fn initialize(&self) -> Result<()> {
        info!("Initializing cross-model transaction coordinator");

        // Initialize WAL coordinator
        self.wal_coordinator.initialize().await?;

        // Recover incomplete transactions if enabled
        if self.config.enable_recovery {
            self.recover_transactions().await?;
        }

        // Start background cleanup task
        self.start_cleanup_task().await;

        info!("Cross-model transaction coordinator initialized");
        Ok(())
    }

    /// Register a transaction participant (storage engine)
    pub async fn register_participant(
        &self,
        participant: Arc<dyn TransactionParticipant>,
    ) -> Result<()> {
        let id = participant.participant_id().to_string();
        info!("Registering transaction participant: {}", id);

        // Register with 2PC coordinator
        self.two_phase_commit
            .register_participant(participant.clone())
            .await?;

        // Store locally
        let mut participants = self.participants.write().await;
        participants.insert(id.clone(), participant);

        debug!("Participant {} registered", id);
        Ok(())
    }

    /// Begin a new transaction
    pub async fn begin_transaction(&self) -> Result<TransactionId> {
        // Begin transaction with 2PC
        let tx_id = self.two_phase_commit.begin().await?;

        // Write to WAL
        self.wal_coordinator.write_tx_begin(tx_id).await?;

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_transactions += 1;
            stats.active_transactions += 1;
        }

        debug!("Transaction {} begun", tx_id);
        Ok(tx_id)
    }

    /// Commit a transaction
    ///
    /// # Arguments
    ///
    /// * `tx_id` - Transaction ID
    /// * `participant_ids` - IDs of participants to involve in commit
    ///
    /// # Returns
    ///
    /// Ok(()) if transaction committed successfully
    pub async fn commit_transaction(
        &self,
        tx_id: TransactionId,
        participant_ids: &[String],
    ) -> Result<()> {
        info!(
            "Committing transaction {} with {} participants",
            tx_id,
            participant_ids.len()
        );

        self.ensure_durable_participants(participant_ids).await?;

        let prepare_result = self.two_phase_commit.prepare(tx_id, participant_ids).await;
        let result = match prepare_result {
            Ok(()) => {
                let wal_prepare_result: Result<()> = async {
                    for (index, participant_id) in participant_ids.iter().enumerate() {
                        self.wal_coordinator
                            .write_tx_prepare(
                                tx_id,
                                participant_id,
                                index + 1,
                                participant_ids.len(),
                            )
                            .await?;
                    }

                    Ok(())
                }
                .await;

                if let Err(e) = wal_prepare_result {
                    Err(e)
                } else {
                    self.two_phase_commit
                        .commit_prepared(tx_id, participant_ids)
                        .await
                }
            }
            Err(e) => {
                self.two_phase_commit
                    .abort(tx_id, participant_ids)
                    .await
                    .ok();
                Err(ProximaDBError::TransactionAborted(format!(
                    "Prepare phase failed: {}",
                    e
                )))
            }
        };

        match &result {
            Ok(()) => {
                // Write commit to WAL
                self.wal_coordinator.write_tx_commit(tx_id).await?;

                // Update stats
                {
                    let mut stats = self.stats.write().await;
                    stats.committed_transactions += 1;
                    stats.active_transactions = stats.active_transactions.saturating_sub(1);
                }

                info!("Transaction {} committed", tx_id);
            }
            Err(e) => {
                self.two_phase_commit
                    .abort(tx_id, participant_ids)
                    .await
                    .ok();

                // Write abort to WAL
                self.wal_coordinator.write_tx_abort(tx_id).await?;

                // Update stats
                {
                    let mut stats = self.stats.write().await;
                    stats.aborted_transactions += 1;
                    stats.active_transactions = stats.active_transactions.saturating_sub(1);
                }

                warn!("Transaction {} aborted: {}", tx_id, e);
            }
        }

        result
    }

    /// Rollback a transaction
    pub async fn rollback_transaction(
        &self,
        tx_id: TransactionId,
        participant_ids: &[String],
    ) -> Result<()> {
        warn!("Rolling back transaction {}", tx_id);

        // Abort with 2PC
        self.two_phase_commit.abort(tx_id, participant_ids).await?;

        // Write abort to WAL
        self.wal_coordinator.write_tx_abort(tx_id).await?;

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.aborted_transactions += 1;
            stats.active_transactions = stats.active_transactions.saturating_sub(1);
        }

        debug!("Transaction {} rolled back", tx_id);
        Ok(())
    }

    /// Get transaction state
    pub async fn get_transaction_state(&self, tx_id: TransactionId) -> Option<TransactionState> {
        self.two_phase_commit.get_state(tx_id).await
    }

    /// Get transaction statistics
    pub async fn get_stats(&self) -> CoordinatorTransactionStats {
        self.stats.read().await.clone()
    }

    async fn ensure_durable_participants(&self, participant_ids: &[String]) -> Result<()> {
        let participants = self.participants.read().await;

        for participant_id in participant_ids {
            let participant = participants.get(participant_id).ok_or_else(|| {
                ProximaDBError::Internal(format!("Participant not found: {}", participant_id))
            })?;

            if !participant.supports_durable_commit() {
                return Err(ProximaDBError::NotImplemented(format!(
                    "Transaction participant '{}' is buffer-only; live engine-backed transaction commits are not wired yet",
                    participant_id
                )));
            }
        }

        Ok(())
    }

    /// Recover incomplete transactions from WAL
    async fn recover_transactions(&self) -> Result<()> {
        info!("Recovering incomplete transactions");

        let incomplete = self.wal_coordinator.get_incomplete_transactions().await;

        if incomplete.is_empty() {
            info!("No incomplete transactions to recover");
            return Ok(());
        }

        warn!(
            "Found {} incomplete transactions, attempting recovery",
            incomplete.len()
        );

        let mut recovered = 0;
        for tx_id in incomplete {
            debug!("Attempting recovery for transaction {}", tx_id);

            // Get WAL state
            if let Some(wal_state) = self.wal_coordinator.get_tx_state(tx_id).await {
                match wal_state {
                    super::wal_coordinator::WALTransactionState::Prepared {
                        participant_ids,
                        ..
                    } => {
                        // All participants prepared, can commit
                        info!("Transaction {} is prepared, committing", tx_id);

                        if let Err(e) = self.ensure_durable_participants(&participant_ids).await {
                            warn!(
                                "Prepared transaction {} cannot be replayed durably: {}. Aborting instead.",
                                tx_id, e
                            );
                            match self.two_phase_commit.abort(tx_id, &participant_ids).await {
                                Ok(_) => debug!(
                                    "Successfully aborted transaction {} during recovery",
                                    tx_id
                                ),
                                Err(err) => error!(
                                    "Failed to abort transaction {} during recovery: {}",
                                    tx_id, err
                                ),
                            }
                            self.wal_coordinator.write_tx_abort(tx_id).await.ok();
                            continue;
                        }

                        self.two_phase_commit
                            .mark_prepared_for_recovery(tx_id)
                            .await;

                        if let Err(e) = self
                            .two_phase_commit
                            .commit_prepared(tx_id, &participant_ids)
                            .await
                        {
                            warn!("Failed to commit prepared tx {}: {}", tx_id, e);
                            match self.two_phase_commit.abort(tx_id, &participant_ids).await {
                                Ok(_) => debug!(
                                    "Successfully aborted transaction {} after commit failure",
                                    tx_id
                                ),
                                Err(err) => error!(
                                    "Failed to abort transaction {} after commit failure: {}",
                                    tx_id, err
                                ),
                            }
                        } else {
                            self.wal_coordinator.write_tx_commit(tx_id).await.ok();
                            recovered += 1;
                        }
                    }
                    super::wal_coordinator::WALTransactionState::Preparing {
                        participant_ids,
                        ..
                    } => {
                        warn!("Transaction {} only partially prepared, aborting", tx_id);

                        match self.two_phase_commit.abort(tx_id, &participant_ids).await {
                            Ok(_) => debug!(
                                "Successfully aborted partially prepared transaction {}",
                                tx_id
                            ),
                            Err(err) => error!(
                                "Failed to abort partially prepared transaction {}: {}",
                                tx_id, err
                            ),
                        }
                        self.wal_coordinator.write_tx_abort(tx_id).await.ok();
                    }
                    _ => {
                        // Not prepared, must abort
                        warn!("Transaction {} not prepared, aborting", tx_id);
                        // Update TwoPhaseCommit state before writing to WAL
                        self.two_phase_commit
                            .set_state(tx_id, TransactionState::Aborted)
                            .await;
                        self.wal_coordinator.write_tx_abort(tx_id).await.ok();
                    }
                }
            }
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.recovered_transactions = recovered;
        }

        info!("Transaction recovery completed: {} recovered", recovered);
        Ok(())
    }

    /// Start background cleanup task
    async fn start_cleanup_task(&self) {
        let two_phase_commit = self.two_phase_commit.clone();
        let wal_coordinator = self.wal_coordinator.clone();
        let cleanup_interval_secs = self.config.cleanup_interval_secs;

        tokio::spawn(async move {
            let period = tokio::time::Duration::from_secs(cleanup_interval_secs);
            // Skip the immediate first tick `interval` emits. cleanup_old_transactions
            // GCs committed/aborted transactions, so firing at t=0 can race with — and
            // discard — state that callers still query right after commit. Start one
            // full period out.
            let mut interval =
                tokio::time::interval_at(tokio::time::Instant::now() + period, period);

            loop {
                interval.tick().await;

                // Cleanup old 2PC transactions
                two_phase_commit
                    .cleanup_old_transactions(cleanup_interval_secs)
                    .await;

                // Cleanup completed WAL transactions
                wal_coordinator.cleanup_completed_transactions().await.ok();

                debug!("Background cleanup completed");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::{Vote, WALTransactionState, participants::VectorEngineParticipant};
    use async_trait::async_trait;
    use std::sync::Arc;

    struct DurableMockParticipant {
        id: String,
    }

    #[async_trait]
    impl TransactionParticipant for DurableMockParticipant {
        async fn prepare(&self, _tx_id: TransactionId) -> Result<Vote> {
            Ok(Vote::Yes)
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
            true
        }

        fn supports_durable_commit(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn test_coordinator_creation() {
        let config = CoordinatorTransactionConfig::default();
        let coordinator = CrossModelTransactionCoordinator::new(config);

        assert_eq!(coordinator.config.timeout_secs, 30);
    }

    #[tokio::test]
    async fn test_coordinator_initialize() {
        let config = CoordinatorTransactionConfig {
            wal_dir: PathBuf::from("/tmp/test_tx_coordinator"),
            ..Default::default()
        };
        let coordinator = CrossModelTransactionCoordinator::new(config);

        let result = coordinator.initialize().await;
        assert!(result.is_ok());

        // Cleanup
        let _ = tokio::fs::remove_dir_all("/tmp/test_tx_coordinator").await;
    }

    #[tokio::test]
    async fn test_begin_transaction() {
        let config = CoordinatorTransactionConfig {
            wal_dir: PathBuf::from("/tmp/test_tx_begin"),
            ..Default::default()
        };
        let coordinator = CrossModelTransactionCoordinator::new(config.clone());

        coordinator.initialize().await.unwrap();

        let tx_id = coordinator.begin_transaction().await.unwrap();
        assert!(tx_id > 0);

        let state = coordinator.get_transaction_state(tx_id).await;
        assert_eq!(state, Some(TransactionState::Initialized));

        let stats = coordinator.get_stats().await;
        assert_eq!(stats.total_transactions, 1);
        assert_eq!(stats.active_transactions, 1);

        // Cleanup
        let _ = tokio::fs::remove_dir_all(config.wal_dir).await;
    }

    #[tokio::test]
    async fn test_commit_rejects_buffer_only_participants() {
        let config = CoordinatorTransactionConfig {
            wal_dir: PathBuf::from("/tmp/test_tx_buffer_only"),
            ..Default::default()
        };
        let coordinator = CrossModelTransactionCoordinator::new(config.clone());

        coordinator.initialize().await.unwrap();

        let participant =
            Arc::new(VectorEngineParticipant::new("products")) as Arc<dyn TransactionParticipant>;
        coordinator.register_participant(participant).await.unwrap();

        let tx_id = coordinator.begin_transaction().await.unwrap();
        let err = coordinator
            .commit_transaction(tx_id, &["vector:products".to_string()])
            .await
            .expect_err("buffer-only participants should be rejected");

        assert!(matches!(err, ProximaDBError::NotImplemented(_)));

        // Cleanup
        let _ = tokio::fs::remove_dir_all(config.wal_dir).await;
    }

    #[tokio::test]
    async fn test_commit_writes_prepare_and_commit_wal_for_durable_participant() {
        let config = CoordinatorTransactionConfig {
            wal_dir: PathBuf::from("/tmp/test_tx_durable_commit"),
            ..Default::default()
        };
        let coordinator = CrossModelTransactionCoordinator::new(config.clone());

        coordinator.initialize().await.unwrap();

        let participant = Arc::new(DurableMockParticipant {
            id: "vector:products".to_string(),
        }) as Arc<dyn TransactionParticipant>;
        coordinator.register_participant(participant).await.unwrap();

        let tx_id = coordinator.begin_transaction().await.unwrap();
        coordinator
            .commit_transaction(tx_id, &["vector:products".to_string()])
            .await
            .unwrap();

        let wal_state = coordinator.wal_coordinator.get_tx_state(tx_id).await;
        assert_eq!(wal_state, Some(WALTransactionState::Committed));

        let stats = coordinator.get_stats().await;
        assert_eq!(stats.committed_transactions, 1);
        assert_eq!(stats.active_transactions, 0);

        // Cleanup
        let _ = tokio::fs::remove_dir_all(config.wal_dir).await;
    }

    #[tokio::test]
    async fn test_recovery_aborts_prepared_tx_for_buffer_only_participants() {
        let config = CoordinatorTransactionConfig {
            wal_dir: PathBuf::from("/tmp/test_tx_recovery_abort"),
            enable_recovery: false,
            ..Default::default()
        };
        let coordinator = CrossModelTransactionCoordinator::new(config.clone());

        coordinator.initialize().await.unwrap();

        let participant =
            Arc::new(VectorEngineParticipant::new("products")) as Arc<dyn TransactionParticipant>;
        coordinator.register_participant(participant).await.unwrap();

        let tx_id = 424242;
        coordinator
            .wal_coordinator
            .write_tx_begin(tx_id)
            .await
            .unwrap();
        coordinator
            .wal_coordinator
            .write_tx_prepare(tx_id, "vector:products", 1, 1)
            .await
            .unwrap();

        coordinator.recover_transactions().await.unwrap();

        let wal_state = coordinator.wal_coordinator.get_tx_state(tx_id).await;
        assert_eq!(wal_state, Some(WALTransactionState::Aborted));

        let tx_state = coordinator.get_transaction_state(tx_id).await;
        assert_eq!(tx_state, Some(TransactionState::Aborted));

        // Cleanup
        let _ = tokio::fs::remove_dir_all(config.wal_dir).await;
    }
}
