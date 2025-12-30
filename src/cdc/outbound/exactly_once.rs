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

//! Exactly-once delivery semantics for outbound CDC
//!
//! Provides transactional guarantees through:
//! - Idempotency keys for deduplication
//! - Transaction state tracking
//! - Two-phase commit protocol support

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::cdc::error::{CdcError, CdcResult};
use crate::cdc::event::ChangeEvent;

/// Idempotency key for exactly-once delivery
#[derive(Debug, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdempotencyKey {
    /// Unique identifier
    pub id: String,
    /// Source event LSN
    pub lsn: u64,
    /// Sink identifier
    pub sink_id: String,
    /// Creation timestamp
    pub created_at: u64,
}

impl IdempotencyKey {
    /// Create a new idempotency key from an event and sink
    pub fn from_event(event: &ChangeEvent, sink_id: &str) -> Self {
        Self {
            id: format!("{}-{}-{}", event.id, event.lsn, sink_id),
            lsn: event.lsn,
            sink_id: sink_id.to_string(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        }
    }

    /// Create a key from components
    pub fn new(event_id: impl Into<String>, lsn: u64, sink_id: impl Into<String>) -> Self {
        let sink_str = sink_id.into();
        let event_str = event_id.into();
        Self {
            id: format!("{}-{}-{}", event_str, lsn, sink_str),
            lsn,
            sink_id: sink_str,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        }
    }
}

/// State of a transaction in the exactly-once protocol
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionState {
    /// Transaction has been started
    Pending,
    /// First phase (prepare) completed
    Prepared,
    /// Transaction committed successfully
    Committed,
    /// Transaction aborted/rolled back
    Aborted,
    /// Transaction expired (timed out)
    Expired,
}

/// Transaction record for exactly-once tracking
#[derive(Debug, Clone)]
pub struct TransactionRecord {
    /// Transaction ID
    pub transaction_id: String,
    /// Idempotency key
    pub key: IdempotencyKey,
    /// Current state
    pub state: TransactionState,
    /// Events in this transaction
    pub event_count: usize,
    /// First event LSN
    pub first_lsn: u64,
    /// Last event LSN
    pub last_lsn: u64,
    /// Start time
    pub started_at: Instant,
    /// Completion time
    pub completed_at: Option<Instant>,
    /// Retry count
    pub retries: u32,
    /// Error message if failed
    pub error: Option<String>,
}

impl TransactionRecord {
    /// Create a new transaction record
    pub fn new(key: IdempotencyKey) -> Self {
        Self {
            transaction_id: Uuid::new_v4().to_string(),
            key,
            state: TransactionState::Pending,
            event_count: 0,
            first_lsn: 0,
            last_lsn: 0,
            started_at: Instant::now(),
            completed_at: None,
            retries: 0,
            error: None,
        }
    }

    /// Check if transaction has timed out
    pub fn is_expired(&self, timeout: Duration) -> bool {
        self.started_at.elapsed() > timeout
    }

    /// Get transaction duration
    pub fn duration(&self) -> Duration {
        match self.completed_at {
            Some(completed) => completed.duration_since(self.started_at),
            None => self.started_at.elapsed(),
        }
    }
}

/// Configuration for exactly-once delivery
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExactlyOnceConfig {
    /// Enable exactly-once semantics
    pub enabled: bool,
    /// Transaction timeout
    #[serde(default = "default_transaction_timeout")]
    pub transaction_timeout_ms: u64,
    /// Maximum retries for failed transactions
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// Idempotency key TTL
    #[serde(default = "default_key_ttl")]
    pub key_ttl_ms: u64,
    /// Maximum pending transactions
    #[serde(default = "default_max_pending")]
    pub max_pending_transactions: usize,
    /// Enable two-phase commit
    #[serde(default)]
    pub two_phase_commit: bool,
}

fn default_transaction_timeout() -> u64 {
    30000 // 30 seconds
}

fn default_max_retries() -> u32 {
    3
}

fn default_key_ttl() -> u64 {
    86400000 // 24 hours
}

fn default_max_pending() -> usize {
    1000
}

impl Default for ExactlyOnceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            transaction_timeout_ms: default_transaction_timeout(),
            max_retries: default_max_retries(),
            key_ttl_ms: default_key_ttl(),
            max_pending_transactions: default_max_pending(),
            two_phase_commit: false,
        }
    }
}

/// Manager for exactly-once delivery semantics
pub struct ExactlyOnceManager {
    /// Configuration
    config: ExactlyOnceConfig,
    /// Active transactions by ID
    transactions: RwLock<HashMap<String, TransactionRecord>>,
    /// Completed idempotency keys (for dedup)
    completed_keys: RwLock<HashMap<String, (TransactionState, Instant)>>,
    /// Statistics
    stats: RwLock<ExactlyOnceStats>,
}

/// Statistics for exactly-once processing
#[derive(Debug, Clone, Default)]
pub struct ExactlyOnceStats {
    /// Total transactions started
    pub transactions_started: u64,
    /// Transactions committed
    pub transactions_committed: u64,
    /// Transactions aborted
    pub transactions_aborted: u64,
    /// Transactions expired
    pub transactions_expired: u64,
    /// Duplicate events detected
    pub duplicates_detected: u64,
    /// Total retries
    pub total_retries: u64,
}

impl ExactlyOnceManager {
    /// Create a new exactly-once manager
    pub fn new(config: ExactlyOnceConfig) -> Self {
        Self {
            config,
            transactions: RwLock::new(HashMap::new()),
            completed_keys: RwLock::new(HashMap::new()),
            stats: RwLock::new(ExactlyOnceStats::default()),
        }
    }

    /// Create with default configuration
    pub fn with_defaults() -> Self {
        Self::new(ExactlyOnceConfig::default())
    }

    /// Check if an event has already been processed
    pub fn is_duplicate(&self, key: &IdempotencyKey) -> bool {
        let completed = self.completed_keys.read().unwrap();
        if let Some((state, time)) = completed.get(&key.id) {
            // Check if key is still valid (not expired)
            if time.elapsed() < Duration::from_millis(self.config.key_ttl_ms) {
                if *state == TransactionState::Committed {
                    self.stats.write().unwrap().duplicates_detected += 1;
                    return true;
                }
            }
        }
        false
    }

    /// Begin a new transaction
    pub fn begin_transaction(&self, key: IdempotencyKey) -> CdcResult<String> {
        // Check for duplicates
        if self.is_duplicate(&key) {
            return Err(CdcError::Duplicate(format!(
                "Event with key {} already processed",
                key.id
            )));
        }

        // Check capacity
        let transactions = self.transactions.read().unwrap();
        if transactions.len() >= self.config.max_pending_transactions {
            return Err(CdcError::Capacity(
                "Too many pending transactions".to_string(),
            ));
        }
        drop(transactions);

        // Create new transaction
        let record = TransactionRecord::new(key);
        let txn_id = record.transaction_id.clone();

        self.transactions
            .write()
            .unwrap()
            .insert(txn_id.clone(), record);
        self.stats.write().unwrap().transactions_started += 1;

        Ok(txn_id)
    }

    /// Update transaction with event info
    pub fn add_event(&self, txn_id: &str, lsn: u64) -> CdcResult<()> {
        let mut transactions = self.transactions.write().unwrap();
        let record = transactions
            .get_mut(txn_id)
            .ok_or_else(|| CdcError::NotFound(format!("Transaction {} not found", txn_id)))?;

        if record.state != TransactionState::Pending {
            return Err(CdcError::InvalidState(format!(
                "Transaction {} is not pending",
                txn_id
            )));
        }

        record.event_count += 1;
        if record.first_lsn == 0 {
            record.first_lsn = lsn;
        }
        record.last_lsn = lsn;

        Ok(())
    }

    /// Prepare transaction (first phase of 2PC)
    pub fn prepare(&self, txn_id: &str) -> CdcResult<()> {
        let mut transactions = self.transactions.write().unwrap();
        let record = transactions
            .get_mut(txn_id)
            .ok_or_else(|| CdcError::NotFound(format!("Transaction {} not found", txn_id)))?;

        if record.state != TransactionState::Pending {
            return Err(CdcError::InvalidState(format!(
                "Transaction {} cannot be prepared from state {:?}",
                txn_id, record.state
            )));
        }

        // Check timeout
        if record.is_expired(Duration::from_millis(self.config.transaction_timeout_ms)) {
            record.state = TransactionState::Expired;
            self.stats.write().unwrap().transactions_expired += 1;
            return Err(CdcError::Timeout("Transaction timed out".to_string()));
        }

        record.state = TransactionState::Prepared;
        Ok(())
    }

    /// Commit transaction
    pub fn commit(&self, txn_id: &str) -> CdcResult<()> {
        let mut transactions = self.transactions.write().unwrap();
        let record = transactions
            .get_mut(txn_id)
            .ok_or_else(|| CdcError::NotFound(format!("Transaction {} not found", txn_id)))?;

        // Allow commit from Pending (1PC) or Prepared (2PC)
        if record.state != TransactionState::Pending
            && record.state != TransactionState::Prepared
        {
            return Err(CdcError::InvalidState(format!(
                "Transaction {} cannot be committed from state {:?}",
                txn_id, record.state
            )));
        }

        // Check timeout
        if record.is_expired(Duration::from_millis(self.config.transaction_timeout_ms)) {
            record.state = TransactionState::Expired;
            self.stats.write().unwrap().transactions_expired += 1;
            return Err(CdcError::Timeout("Transaction timed out".to_string()));
        }

        record.state = TransactionState::Committed;
        record.completed_at = Some(Instant::now());

        // Record completed key
        let key_id = record.key.id.clone();
        drop(transactions);

        self.completed_keys
            .write()
            .unwrap()
            .insert(key_id, (TransactionState::Committed, Instant::now()));
        self.stats.write().unwrap().transactions_committed += 1;

        // Clean up transaction
        self.transactions.write().unwrap().remove(txn_id);

        Ok(())
    }

    /// Abort transaction
    pub fn abort(&self, txn_id: &str, error: Option<String>) -> CdcResult<()> {
        let mut transactions = self.transactions.write().unwrap();
        let record = transactions
            .get_mut(txn_id)
            .ok_or_else(|| CdcError::NotFound(format!("Transaction {} not found", txn_id)))?;

        if record.state == TransactionState::Committed {
            return Err(CdcError::InvalidState(
                "Cannot abort committed transaction".to_string(),
            ));
        }

        record.state = TransactionState::Aborted;
        record.completed_at = Some(Instant::now());
        record.error = error;

        // Record completed key (as aborted)
        let key_id = record.key.id.clone();
        drop(transactions);

        self.completed_keys
            .write()
            .unwrap()
            .insert(key_id, (TransactionState::Aborted, Instant::now()));
        self.stats.write().unwrap().transactions_aborted += 1;

        // Clean up transaction
        self.transactions.write().unwrap().remove(txn_id);

        Ok(())
    }

    /// Retry a failed transaction
    pub fn retry(&self, key: IdempotencyKey) -> CdcResult<String> {
        // Check retry limit
        let completed = self.completed_keys.read().unwrap();
        if let Some((TransactionState::Aborted, _)) = completed.get(&key.id) {
            // Allow retry for aborted transactions
            drop(completed);
            self.completed_keys.write().unwrap().remove(&key.id);
            self.stats.write().unwrap().total_retries += 1;
            return self.begin_transaction(key);
        }

        Err(CdcError::InvalidState(
            "Only aborted transactions can be retried".to_string(),
        ))
    }

    /// Get transaction state
    pub fn get_state(&self, txn_id: &str) -> Option<TransactionState> {
        self.transactions
            .read()
            .unwrap()
            .get(txn_id)
            .map(|r| r.state)
    }

    /// Get transaction record
    pub fn get_transaction(&self, txn_id: &str) -> Option<TransactionRecord> {
        self.transactions.read().unwrap().get(txn_id).cloned()
    }

    /// Get pending transaction count
    pub fn pending_count(&self) -> usize {
        self.transactions.read().unwrap().len()
    }

    /// Clean up expired transactions
    pub fn cleanup_expired(&self) -> usize {
        let timeout = Duration::from_millis(self.config.transaction_timeout_ms);
        let key_ttl = Duration::from_millis(self.config.key_ttl_ms);
        let mut expired_count = 0;

        // Expire pending transactions
        let mut transactions = self.transactions.write().unwrap();
        let expired_ids: Vec<String> = transactions
            .iter()
            .filter(|(_, r)| r.is_expired(timeout))
            .map(|(id, _)| id.clone())
            .collect();

        for id in expired_ids {
            if let Some(mut record) = transactions.remove(&id) {
                record.state = TransactionState::Expired;
                expired_count += 1;
            }
        }
        drop(transactions);

        // Clean up old completed keys
        let mut completed = self.completed_keys.write().unwrap();
        completed.retain(|_, (_, time)| time.elapsed() < key_ttl);

        self.stats.write().unwrap().transactions_expired += expired_count as u64;
        expired_count
    }

    /// Get statistics
    pub fn stats(&self) -> ExactlyOnceStats {
        self.stats.read().unwrap().clone()
    }

    /// Reset statistics
    pub fn reset_stats(&self) {
        *self.stats.write().unwrap() = ExactlyOnceStats::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::event::{Operation, SourceInfo};

    fn create_test_event(lsn: u64) -> ChangeEvent {
        let mut event = ChangeEvent::new(
            SourceInfo::proximadb("testdb", "test_server"),
            Operation::Insert,
            "products",
            format!("prod_{}", lsn),
        );
        event.lsn = lsn;
        event
    }

    #[test]
    fn test_idempotency_key_creation() {
        let event = create_test_event(100);
        let key = IdempotencyKey::from_event(&event, "kafka_sink");

        assert!(key.id.contains("kafka_sink"));
        assert_eq!(key.lsn, 100);
        assert_eq!(key.sink_id, "kafka_sink");
    }

    #[test]
    fn test_transaction_lifecycle() {
        let manager = ExactlyOnceManager::with_defaults();
        let key = IdempotencyKey::new("event_1", 100, "sink_1");

        // Begin transaction
        let txn_id = manager.begin_transaction(key).unwrap();
        assert_eq!(manager.get_state(&txn_id), Some(TransactionState::Pending));

        // Add events
        manager.add_event(&txn_id, 100).unwrap();
        manager.add_event(&txn_id, 101).unwrap();

        // Commit
        manager.commit(&txn_id).unwrap();
        assert!(manager.get_state(&txn_id).is_none()); // Removed after commit
    }

    #[test]
    fn test_two_phase_commit() {
        let config = ExactlyOnceConfig {
            two_phase_commit: true,
            ..Default::default()
        };
        let manager = ExactlyOnceManager::new(config);
        let key = IdempotencyKey::new("event_1", 100, "sink_1");

        // Begin
        let txn_id = manager.begin_transaction(key).unwrap();

        // Prepare (phase 1)
        manager.prepare(&txn_id).unwrap();
        assert_eq!(manager.get_state(&txn_id), Some(TransactionState::Prepared));

        // Commit (phase 2)
        manager.commit(&txn_id).unwrap();
    }

    #[test]
    fn test_duplicate_detection() {
        let manager = ExactlyOnceManager::with_defaults();
        let key = IdempotencyKey::new("event_1", 100, "sink_1");

        // First transaction
        let txn_id = manager.begin_transaction(key.clone()).unwrap();
        manager.commit(&txn_id).unwrap();

        // Duplicate should be detected
        assert!(manager.is_duplicate(&key));

        // Second transaction with same key should fail
        let result = manager.begin_transaction(key);
        assert!(result.is_err());
    }

    #[test]
    fn test_abort_and_retry() {
        let manager = ExactlyOnceManager::with_defaults();
        let key = IdempotencyKey::new("event_1", 100, "sink_1");

        // Begin and abort
        let txn_id = manager.begin_transaction(key.clone()).unwrap();
        manager
            .abort(&txn_id, Some("Test error".to_string()))
            .unwrap();

        // Should be able to retry
        let key2 = IdempotencyKey::new("event_1", 100, "sink_1");
        let new_txn_id = manager.retry(key2).unwrap();
        assert_ne!(txn_id, new_txn_id);

        // Commit retry
        manager.commit(&new_txn_id).unwrap();
    }

    #[test]
    fn test_transaction_states() {
        assert_eq!(TransactionState::Pending, TransactionState::Pending);
        assert_ne!(TransactionState::Pending, TransactionState::Committed);

        // Serialize/deserialize
        let state = TransactionState::Committed;
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, "\"committed\"");
    }

    #[test]
    fn test_stats_tracking() {
        let manager = ExactlyOnceManager::with_defaults();

        // Start and commit
        let key1 = IdempotencyKey::new("event_1", 100, "sink_1");
        let txn1 = manager.begin_transaction(key1).unwrap();
        manager.commit(&txn1).unwrap();

        // Start and abort
        let key2 = IdempotencyKey::new("event_2", 200, "sink_1");
        let txn2 = manager.begin_transaction(key2).unwrap();
        manager.abort(&txn2, None).unwrap();

        let stats = manager.stats();
        assert_eq!(stats.transactions_started, 2);
        assert_eq!(stats.transactions_committed, 1);
        assert_eq!(stats.transactions_aborted, 1);
    }

    #[test]
    fn test_capacity_limit() {
        let config = ExactlyOnceConfig {
            max_pending_transactions: 2,
            ..Default::default()
        };
        let manager = ExactlyOnceManager::new(config);

        // Fill up capacity
        let _txn1 = manager
            .begin_transaction(IdempotencyKey::new("e1", 1, "s"))
            .unwrap();
        let _txn2 = manager
            .begin_transaction(IdempotencyKey::new("e2", 2, "s"))
            .unwrap();

        // Third should fail
        let result = manager.begin_transaction(IdempotencyKey::new("e3", 3, "s"));
        assert!(result.is_err());
    }

    #[test]
    fn test_cleanup_expired() {
        let config = ExactlyOnceConfig {
            transaction_timeout_ms: 1, // 1ms timeout
            key_ttl_ms: 1,
            ..Default::default()
        };
        let manager = ExactlyOnceManager::new(config);

        // Create transaction
        let _txn = manager
            .begin_transaction(IdempotencyKey::new("e1", 1, "s"))
            .unwrap();

        // Wait for expiration
        std::thread::sleep(Duration::from_millis(10));

        // Cleanup should expire the transaction
        let expired = manager.cleanup_expired();
        assert_eq!(expired, 1);
        assert_eq!(manager.pending_count(), 0);
    }
}
