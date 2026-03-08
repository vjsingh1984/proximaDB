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

use crate::core::error::ProximaDBError;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use super::two_phase_commit::{
    TransactionId, TransactionParticipant, TransactionState, TwoPhaseCommit,
};
use super::wal_coordinator::WALCoordinator;

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Configuration for cross-model transaction coordinator
#[derive(Debug, Clone)]
pub struct TransactionConfig {
    /// WAL directory for transaction logs
    pub wal_dir: PathBuf,

    /// Transaction timeout in seconds
    pub timeout_secs: u64,

    /// Enable auto-recovery on startup
    pub enable_recovery: bool,

    /// Cleanup interval for completed transactions (seconds)
    pub cleanup_interval_secs: u64,
}

impl Default for TransactionConfig {
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
pub struct TransactionStats {
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
    config: TransactionConfig,

    /// Two-phase commit coordinator
    two_phase_commit: TwoPhaseCommit,

    /// WAL coordinator
    wal_coordinator: WALCoordinator,

    /// Transaction participants
    participants: Arc<RwLock<HashMap<String, Arc<dyn TransactionParticipant>>>>,

    /// Transaction statistics
    stats: Arc<RwLock<TransactionStats>>,
}

impl CrossModelTransactionCoordinator {
    /// Create a new cross-model transaction coordinator
    pub fn new(config: TransactionConfig) -> Self {
        let two_phase_commit = TwoPhaseCommit::new(config.timeout_secs);
        let wal_coordinator = WALCoordinator::new(config.wal_dir.clone());

        Self {
            config,
            two_phase_commit,
            wal_coordinator,
            participants: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(TransactionStats::default())),
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

        // Execute two-phase commit
        let result = self.two_phase_commit.commit(tx_id, participant_ids).await;

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
    pub async fn get_stats(&self) -> TransactionStats {
        self.stats.read().await.clone()
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
                    super::wal_coordinator::WALTransactionState::Prepared { .. } => {
                        // All participants prepared, can commit
                        info!("Transaction {} is prepared, committing", tx_id);

                        // Get all participant IDs
                        let participants = self.participants.read().await;
                        let participant_ids: Vec<String> = participants.keys().cloned().collect();

                        if let Err(e) = self.two_phase_commit.commit(tx_id, &participant_ids).await
                        {
                            warn!("Failed to commit prepared tx {}: {}", tx_id, e);
                            self.two_phase_commit
                                .abort(tx_id, &participant_ids)
                                .await
                                .ok();
                        } else {
                            self.wal_coordinator.write_tx_commit(tx_id).await.ok();
                            recovered += 1;
                        }
                    }
                    _ => {
                        // Not prepared, must abort
                        warn!("Transaction {} not prepared, aborting", tx_id);

                        let participants = self.participants.read().await;
                        let participant_ids: Vec<String> = participants.keys().cloned().collect();

                        self.two_phase_commit
                            .abort(tx_id, &participant_ids)
                            .await
                            .ok();
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
            let mut interval =
                tokio::time::interval(tokio::time::Duration::from_secs(cleanup_interval_secs));

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

    #[tokio::test]
    async fn test_coordinator_creation() {
        let config = TransactionConfig::default();
        let coordinator = CrossModelTransactionCoordinator::new(config);

        assert_eq!(coordinator.config.timeout_secs, 30);
    }

    #[tokio::test]
    async fn test_coordinator_initialize() {
        let config = TransactionConfig {
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
        let config = TransactionConfig {
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
}
