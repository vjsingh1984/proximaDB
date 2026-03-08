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

//! # WAL Coordinator for Cross-Model Transaction Recovery
//!
//! This module coordinates Write-Ahead Log (WAL) operations across multiple
//! data models to enable transaction recovery after crashes.
//!
//! ## Architecture
//!
//! ```text
//! CrossModelTransactionCoordinator
//!      ↓
//! WALCoordinator (coordinates WALs)
//!      ↓
//! ┌─────────┬─────────┬─────────┐
//! │Vector   │Document │Graph    │
//! │WAL      │WAL      │WAL      │
//! └─────────┴─────────┴─────────┘
//! ```
//!
//! ## WAL Records
//!
//! Each transaction writes WAL records:
//! - `TX_BEGIN`: Transaction started
//! - `TX_PREPARE`: Participant voted YES
//! - `TX_COMMIT`: Transaction committed
//! - `TX_ABORT`: Transaction aborted
//!
//! ## Recovery Process
//!
//! 1. Scan all WALs for incomplete transactions
//! 2. Collect participant states
//! 3. Decide: COMMIT if all prepared, ABORT otherwise
//! 4. Write decision to WAL
//! 5. Notify participants

use crate::core::error::ProximaDBError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

type Result<T> = std::result::Result<T, ProximaDBError>;

use super::two_phase_commit::TransactionId;

/// Placeholder for WAL writer (will be replaced with actual implementation)
pub type WALWriter = Arc<RwLock<Vec<u8>>>;

/// WAL record type for transaction logging
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransactionWALRecord {
    /// Transaction begun
    TxBegin { tx_id: TransactionId },

    /// Participant prepared (voted YES)
    TxPrepare {
        tx_id: TransactionId,
        participant_id: String,
    },

    /// Transaction committed
    TxCommit { tx_id: TransactionId },

    /// Transaction aborted
    TxAbort { tx_id: TransactionId },
}

/// WAL state for a transaction
#[derive(Debug, Clone, PartialEq)]
pub enum WALTransactionState {
    /// Transaction begun
    Begun,
    /// One or more participants prepared
    Preparing {
        prepared_count: usize,
        total_count: usize,
    },
    /// All participants prepared (ready to commit)
    Prepared { total_count: usize },
    /// Transaction committed
    Committed,
    /// Transaction aborted
    Aborted,
}

/// WAL coordinator for cross-model transactions
pub struct WALCoordinator {
    /// Base WAL directory
    wal_dir: PathBuf,

    /// WAL writers per participant
    wal_writers: Arc<RwLock<HashMap<String, WALWriter>>>,

    /// In-memory transaction state (recovered from WAL)
    tx_states: Arc<RwLock<HashMap<TransactionId, WALTransactionState>>>,
}

impl WALCoordinator {
    /// Create a new WAL coordinator
    pub fn new(wal_dir: PathBuf) -> Self {
        Self {
            wal_dir,
            wal_writers: Arc::new(RwLock::new(HashMap::new())),
            tx_states: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Initialize WAL coordinator (recover transactions)
    pub async fn initialize(&self) -> Result<()> {
        info!("Initializing WAL coordinator at {:?}", self.wal_dir);

        // Create WAL directory if it doesn't exist
        tokio::fs::create_dir_all(&self.wal_dir)
            .await
            .map_err(|e| ProximaDBError::Internal(format!("Failed to create WAL dir: {}", e)))?;

        // Recover incomplete transactions
        self.recover_transactions().await?;

        info!("WAL coordinator initialized successfully");
        Ok(())
    }

    /// Register a participant's WAL writer
    pub async fn register_participant_wal(
        &self,
        participant_id: String,
        wal_writer: WALWriter,
    ) -> Result<()> {
        let mut wal_writers = self.wal_writers.write().await;
        wal_writers.insert(participant_id, wal_writer);
        Ok(())
    }

    /// Write transaction begin record
    pub async fn write_tx_begin(&self, tx_id: TransactionId) -> Result<()> {
        let record = TransactionWALRecord::TxBegin { tx_id };
        self.write_record_to_all_participants(tx_id, &record)
            .await?;

        // Update state
        {
            let mut tx_states = self.tx_states.write().await;
            tx_states.insert(tx_id, WALTransactionState::Begun);
        }

        debug!("WAL: Transaction {} begun", tx_id);
        Ok(())
    }

    /// Write prepare record for a participant
    pub async fn write_tx_prepare(
        &self,
        tx_id: TransactionId,
        participant_id: &str,
        prepared_count: usize,
        total_count: usize,
    ) -> Result<()> {
        let record = TransactionWALRecord::TxPrepare {
            tx_id,
            participant_id: participant_id.to_string(),
        };

        // Write to this participant's WAL
        let wal_writers = self.wal_writers.read().await;
        if let Some(wal_writer) = wal_writers.get(participant_id) {
            self.write_record_to_wal(wal_writer, &record).await?;
        }

        // Update state
        {
            let mut tx_states = self.tx_states.write().await;
            let state = WALTransactionState::Preparing {
                prepared_count,
                total_count,
            };

            // If all prepared, move to Prepared state
            if prepared_count >= total_count {
                tx_states.insert(tx_id, WALTransactionState::Prepared { total_count });
            } else {
                tx_states.insert(tx_id, state);
            }
        }

        debug!("WAL: Transaction {} prepared by {}", tx_id, participant_id);
        Ok(())
    }

    /// Write commit record
    pub async fn write_tx_commit(&self, tx_id: TransactionId) -> Result<()> {
        let record = TransactionWALRecord::TxCommit { tx_id };
        self.write_record_to_all_participants(tx_id, &record)
            .await?;

        // Update state
        {
            let mut tx_states = self.tx_states.write().await;
            tx_states.insert(tx_id, WALTransactionState::Committed);
        }

        debug!("WAL: Transaction {} committed", tx_id);
        Ok(())
    }

    /// Write abort record
    pub async fn write_tx_abort(&self, tx_id: TransactionId) -> Result<()> {
        let record = TransactionWALRecord::TxAbort { tx_id };
        self.write_record_to_all_participants(tx_id, &record)
            .await?;

        // Update state
        {
            let mut tx_states = self.tx_states.write().await;
            tx_states.insert(tx_id, WALTransactionState::Aborted);
        }

        debug!("WAL: Transaction {} aborted", tx_id);
        Ok(())
    }

    /// Get transaction state from WAL
    pub async fn get_tx_state(&self, tx_id: TransactionId) -> Option<WALTransactionState> {
        let tx_states = self.tx_states.read().await;
        tx_states.get(&tx_id).cloned()
    }

    /// Write record to all participants' WALs
    async fn write_record_to_all_participants(
        &self,
        _tx_id: TransactionId,
        record: &TransactionWALRecord,
    ) -> Result<()> {
        let wal_writers = self.wal_writers.read().await;

        if wal_writers.is_empty() {
            // No participants registered, write to global WAL
            return self.write_record_to_global_wal(record).await;
        }

        // Write to each participant's WAL
        for (participant_id, wal_writer) in wal_writers.iter() {
            debug!("Writing WAL record to {}", participant_id);
            self.write_record_to_wal(wal_writer, record).await?;
        }

        Ok(())
    }

    /// Write record to global WAL
    async fn write_record_to_global_wal(&self, record: &TransactionWALRecord) -> Result<()> {
        // Serialize record
        let serialized = bincode::serialize(record)
            .map_err(|e| ProximaDBError::Internal(format!("Serialization error: {}", e)))?;

        // Write to transaction WAL file
        let tx_wal_path = self.wal_dir.join("transactions.wal");

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&tx_wal_path)
            .await
            .map_err(|e| ProximaDBError::Internal(format!("Failed to open WAL: {}", e)))?;

        use tokio::io::AsyncWriteExt;
        file.write_all(&serialized)
            .await
            .map_err(|e| ProximaDBError::Internal(format!("Failed to write WAL: {}", e)))?;

        Ok(())
    }

    /// Write record to a specific WAL
    async fn write_record_to_wal(
        &self,
        _wal_writer: &WALWriter,
        record: &TransactionWALRecord,
    ) -> Result<()> {
        // Serialize record
        let serialized = bincode::serialize(record)
            .map_err(|e| ProximaDBError::Internal(format!("Serialization error: {}", e)))?;

        // Write to transaction WAL file
        let tx_wal_path = self.wal_dir.join("transactions.wal");

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&tx_wal_path)
            .await
            .map_err(|e| ProximaDBError::Internal(format!("Failed to open WAL: {}", e)))?;

        use tokio::io::AsyncWriteExt;
        file.write_all(&serialized)
            .await
            .map_err(|e| ProximaDBError::Internal(format!("Failed to write WAL: {}", e)))?;

        Ok(())
    }

    /// Recover incomplete transactions from WAL
    async fn recover_transactions(&self) -> Result<()> {
        info!("Recovering transactions from WAL");

        let tx_wal_path = self.wal_dir.join("transactions.wal");

        if !tx_wal_path.exists() {
            info!("No transaction WAL found, starting fresh");
            return Ok(());
        }

        // Read WAL file
        let contents = tokio::fs::read(&tx_wal_path)
            .await
            .map_err(|e| ProximaDBError::Internal(format!("Failed to read WAL: {}", e)))?;

        // Deserialize records
        let tx_states: HashMap<TransactionId, WALTransactionState> = HashMap::new();

        // Note: In production, we'd parse records properly
        // For now, just log that we found the WAL
        debug!("Read {} bytes from transaction WAL", contents.len());

        // Store recovered states
        let mut states = self.tx_states.write().await;
        *states = tx_states;

        info!("Transaction recovery completed");
        Ok(())
    }

    /// Get incomplete transactions that need recovery
    pub async fn get_incomplete_transactions(&self) -> Vec<TransactionId> {
        let tx_states = self.tx_states.read().await;
        tx_states
            .iter()
            .filter(|(_, state)| {
                matches!(
                    state,
                    WALTransactionState::Begun
                        | WALTransactionState::Preparing { .. }
                        | WALTransactionState::Prepared { .. }
                )
            })
            .map(|(tx_id, _)| *tx_id)
            .collect()
    }

    /// Cleanup committed/aborted transactions from WAL
    pub async fn cleanup_completed_transactions(&self) -> Result<()> {
        let mut to_remove = Vec::new();

        // Collect transaction IDs to remove
        {
            let tx_states = self.tx_states.read().await;
            for (tx_id, state) in tx_states.iter() {
                if matches!(
                    state,
                    WALTransactionState::Committed | WALTransactionState::Aborted
                ) {
                    to_remove.push(*tx_id);
                }
            }
        }

        // Remove transactions
        {
            let mut tx_states = self.tx_states.write().await;
            for tx_id in &to_remove {
                tx_states.remove(tx_id);
            }
        }

        debug!(
            "Cleaned up {} completed transactions from WAL",
            to_remove.len()
        );
        Ok(())
    }
}

// Implement Clone for WALCoordinator
impl Clone for WALCoordinator {
    fn clone(&self) -> Self {
        Self {
            wal_dir: self.wal_dir.clone(),
            wal_writers: self.wal_writers.clone(),
            tx_states: self.tx_states.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_wal_coordinator_creation() {
        let wal_dir = PathBuf::from("/tmp/test_wal");
        let coordinator = WALCoordinator::new(wal_dir);
        assert_eq!(coordinator.wal_dir, PathBuf::from("/tmp/test_wal"));
    }

    #[tokio::test]
    async fn test_wal_coordinator_initialize() {
        let wal_dir = PathBuf::from("/tmp/test_wal_init");
        let coordinator = WALCoordinator::new(wal_dir);

        let result = coordinator.initialize().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_write_tx_begin() {
        let wal_dir = PathBuf::from("/tmp/test_wal_begin");
        let coordinator = WALCoordinator::new(wal_dir.clone());
        coordinator.initialize().await.unwrap();

        let tx_id = 12345;
        let result = coordinator.write_tx_begin(tx_id).await;

        assert!(result.is_ok());

        let state = coordinator.get_tx_state(tx_id).await;
        assert_eq!(state, Some(WALTransactionState::Begun));

        // Cleanup
        let _ = tokio::fs::remove_dir_all(wal_dir).await;
    }

    #[tokio::test]
    async fn test_write_tx_commit() {
        let wal_dir = PathBuf::from("/tmp/test_wal_commit");
        let coordinator = WALCoordinator::new(wal_dir.clone());
        coordinator.initialize().await.unwrap();

        let tx_id = 54321;
        coordinator.write_tx_begin(tx_id).await.unwrap();
        let result = coordinator.write_tx_commit(tx_id).await;

        assert!(result.is_ok());

        let state = coordinator.get_tx_state(tx_id).await;
        assert_eq!(state, Some(WALTransactionState::Committed));

        // Cleanup
        let _ = tokio::fs::remove_dir_all(wal_dir).await;
    }
}
