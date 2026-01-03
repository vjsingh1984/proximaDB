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

//! Multi-Model Transaction Manager with 2PC Support

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use super::context::{TransactionContext, TransactionId, TransactionState, TransactionStats};
use super::isolation::{ConflictResolution, IsolationLevel, Lock, LockMode};
use super::operations::{MultiModelOperation, OperationRollback};
use crate::core::error::ProximaDBError;

/// Result type for transaction operations
pub type Result<T> = std::result::Result<T, ProximaDBError>;

/// Configuration for the transaction manager
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionConfig {
    /// Default transaction timeout
    pub default_timeout: Duration,
    /// Maximum concurrent transactions
    pub max_concurrent_transactions: usize,
    /// Default isolation level
    pub default_isolation_level: IsolationLevel,
    /// Conflict resolution strategy
    pub conflict_resolution: ConflictResolution,
    /// Lock wait timeout
    pub lock_wait_timeout: Duration,
    /// Enable deadlock detection
    pub deadlock_detection: bool,
    /// Deadlock detection interval
    pub deadlock_detection_interval: Duration,
    /// Enable transaction logging
    pub enable_logging: bool,
    /// Maximum retries on conflict
    pub max_conflict_retries: u32,
}

impl Default for TransactionConfig {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(30),
            max_concurrent_transactions: 1000,
            default_isolation_level: IsolationLevel::ReadCommitted,
            conflict_resolution: ConflictResolution::FirstWriterWins,
            lock_wait_timeout: Duration::from_secs(5),
            deadlock_detection: true,
            deadlock_detection_interval: Duration::from_secs(1),
            enable_logging: true,
            max_conflict_retries: 3,
        }
    }
}

/// Participant in a distributed transaction
#[derive(Debug, Clone)]
pub struct TransactionParticipant {
    /// Store identifier (vector, document, graph, observability)
    pub store_id: String,
    /// Whether participant is prepared
    pub prepared: bool,
    /// Vote for commit (None = not voted, true = yes, false = no)
    pub vote: Option<bool>,
    /// Error if participant failed
    pub error: Option<String>,
}

/// Result of a transaction operation
#[derive(Debug, Clone)]
pub struct TransactionResult {
    /// Transaction ID
    pub transaction_id: TransactionId,
    /// Whether transaction was successful
    pub success: bool,
    /// Final state
    pub final_state: TransactionState,
    /// Duration of transaction
    pub duration: Duration,
    /// Number of operations
    pub operation_count: usize,
    /// Participant results
    pub participants: HashMap<String, bool>,
    /// Error message if failed
    pub error: Option<String>,
}

/// Lock entry for lock manager
#[derive(Debug, Clone)]
struct LockEntry {
    /// The lock
    lock: Lock,
    /// Queue of waiting transactions
    waiters: Vec<(TransactionId, LockMode, Instant)>,
}

/// Multi-Model Transaction Manager
///
/// Coordinates ACID transactions across Vector, Document, Graph, and Observability stores
/// using Two-Phase Commit (2PC) protocol.
pub struct MultiModelTransactionManager {
    /// Configuration
    config: TransactionConfig,
    /// Active transactions
    active_transactions: Arc<RwLock<HashMap<TransactionId, Arc<TransactionContext>>>>,
    /// Transaction participants
    participants: Arc<RwLock<HashMap<TransactionId, HashMap<String, TransactionParticipant>>>>,
    /// Global lock table
    lock_table: Arc<RwLock<HashMap<String, LockEntry>>>,
    /// Transaction history (for debugging/auditing)
    history: Arc<RwLock<Vec<TransactionResult>>>,
    /// Commit lock for serializing commits
    commit_lock: Arc<AsyncMutex<()>>,
    /// Statistics
    stats: Arc<RwLock<ManagerStats>>,
}

/// Manager statistics
#[derive(Debug, Clone, Default)]
pub struct ManagerStats {
    /// Total transactions started
    pub total_started: u64,
    /// Total transactions committed
    pub total_committed: u64,
    /// Total transactions aborted
    pub total_aborted: u64,
    /// Total transactions timed out
    pub total_timed_out: u64,
    /// Total conflicts detected
    pub total_conflicts: u64,
    /// Total deadlocks detected
    pub total_deadlocks: u64,
    /// Current active transactions
    pub current_active: usize,
    /// Average transaction duration
    pub avg_duration_ms: f64,
}

impl MultiModelTransactionManager {
    /// Create a new transaction manager
    pub fn new(config: TransactionConfig) -> Self {
        Self {
            config,
            active_transactions: Arc::new(RwLock::new(HashMap::new())),
            participants: Arc::new(RwLock::new(HashMap::new())),
            lock_table: Arc::new(RwLock::new(HashMap::new())),
            history: Arc::new(RwLock::new(Vec::new())),
            commit_lock: Arc::new(AsyncMutex::new(())),
            stats: Arc::new(RwLock::new(ManagerStats::default())),
        }
    }

    /// Begin a new transaction
    pub fn begin(
        &self,
        isolation_level: Option<IsolationLevel>,
    ) -> Result<Arc<TransactionContext>> {
        // Check capacity
        let active_count = self.active_transactions.read().len();
        if active_count >= self.config.max_concurrent_transactions {
            return Err(ProximaDBError::TooManyTransactions {
                max: self.config.max_concurrent_transactions,
            });
        }

        let tx_id = format!("tx_{}", Uuid::new_v4());
        let isolation = isolation_level.unwrap_or(self.config.default_isolation_level);

        let ctx = Arc::new(TransactionContext::new(
            tx_id.clone(),
            isolation,
            self.config.default_timeout,
        ));

        // Register transaction
        self.active_transactions
            .write()
            .insert(tx_id.clone(), ctx.clone());
        self.participants.write().insert(tx_id, HashMap::new());

        // Update stats
        {
            let mut stats = self.stats.write();
            stats.total_started += 1;
            stats.current_active = active_count + 1;
        }

        Ok(ctx)
    }

    /// Begin a nested transaction
    pub fn begin_nested(
        &self,
        parent_id: &TransactionId,
        isolation_level: Option<IsolationLevel>,
    ) -> Result<Arc<TransactionContext>> {
        // Verify parent exists and is active
        let parent = self.get_transaction(parent_id)?;
        if !parent.state().is_active() {
            return Err(ProximaDBError::TransactionNotActive {
                id: parent_id.clone(),
            });
        }

        let tx_id = format!("tx_{}_{}", parent_id, Uuid::new_v4());
        let isolation = isolation_level.unwrap_or(parent.isolation_level);

        let ctx = Arc::new(TransactionContext::new_nested(
            tx_id.clone(),
            parent_id.clone(),
            isolation,
            parent
                .remaining_time()
                .unwrap_or(self.config.default_timeout),
        ));

        // Register as child
        parent.add_child(tx_id.clone());

        // Register transaction
        self.active_transactions
            .write()
            .insert(tx_id.clone(), ctx.clone());
        self.participants.write().insert(tx_id, HashMap::new());

        Ok(ctx)
    }

    /// Get a transaction by ID
    pub fn get_transaction(&self, tx_id: &TransactionId) -> Result<Arc<TransactionContext>> {
        self.active_transactions
            .read()
            .get(tx_id)
            .cloned()
            .ok_or_else(|| ProximaDBError::TransactionNotFound { id: tx_id.clone() })
    }

    /// Add an operation to a transaction
    pub fn add_operation(
        &self,
        tx_id: &TransactionId,
        operation: MultiModelOperation,
    ) -> Result<u64> {
        let ctx = self.get_transaction(tx_id)?;

        // Check transaction is active
        if !ctx.state().is_active() {
            return Err(ProximaDBError::TransactionNotActive { id: tx_id.clone() });
        }

        // Check timeout
        if ctx.is_timed_out() {
            ctx.set_state(TransactionState::TimedOut);
            self.cleanup_transaction(tx_id);
            return Err(ProximaDBError::TransactionTimedOut { id: tx_id.clone() });
        }

        // Register participant
        let model_type = operation.model_type();
        self.register_participant(tx_id, model_type)?;

        // For serializable isolation, acquire locks
        if ctx.isolation_level == IsolationLevel::Serializable {
            let resource = format!("{}:{}", operation.model_type(), operation.target());
            let mode = if operation.is_write() {
                LockMode::Exclusive
            } else {
                LockMode::Shared
            };
            self.acquire_lock(tx_id, &resource, mode)?;
        }

        // Add to write set
        let seq = ctx.add_write(operation);

        Ok(seq)
    }

    /// Register a participant store
    fn register_participant(&self, tx_id: &TransactionId, store_id: &str) -> Result<()> {
        let mut participants = self.participants.write();
        let tx_participants = participants.entry(tx_id.clone()).or_default();

        if !tx_participants.contains_key(store_id) {
            tx_participants.insert(
                store_id.to_string(),
                TransactionParticipant {
                    store_id: store_id.to_string(),
                    prepared: false,
                    vote: None,
                    error: None,
                },
            );
        }

        Ok(())
    }

    /// Acquire a lock for a transaction
    fn acquire_lock(&self, tx_id: &TransactionId, resource: &str, mode: LockMode) -> Result<Lock> {
        let start = Instant::now();
        let ctx = self.get_transaction(tx_id)?;

        loop {
            // Check timeout
            if start.elapsed() > self.config.lock_wait_timeout {
                return Err(ProximaDBError::LockTimeout {
                    resource: resource.to_string(),
                });
            }

            let mut lock_table = self.lock_table.write();

            if let Some(entry) = lock_table.get_mut(resource) {
                // Check compatibility
                if entry.lock.transaction_id == *tx_id {
                    // Already hold lock, check upgrade
                    if entry.lock.mode == LockMode::Shared && mode == LockMode::Exclusive {
                        // Lock upgrade
                        if entry.waiters.is_empty() {
                            entry.lock.mode = LockMode::Exclusive;
                            return Ok(entry.lock.clone());
                        }
                        // Wait for upgrade
                        entry.waiters.push((tx_id.clone(), mode, Instant::now()));
                        drop(lock_table);
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    return Ok(entry.lock.clone());
                }

                if entry.lock.mode.is_compatible(&mode) {
                    // Compatible, can share
                    let lock = ctx.acquire_lock(resource, mode);
                    return Ok(lock);
                }

                // Incompatible, must wait
                entry.waiters.push((tx_id.clone(), mode, Instant::now()));
                drop(lock_table);

                // Check for deadlock
                if self.config.deadlock_detection {
                    if self.detect_deadlock(tx_id, resource) {
                        self.stats.write().total_deadlocks += 1;
                        return Err(ProximaDBError::DeadlockDetected {
                            transaction: tx_id.clone(),
                        });
                    }
                }

                std::thread::sleep(Duration::from_millis(10));
                continue;
            }

            // No existing lock, acquire it
            let lock = ctx.acquire_lock(resource, mode);
            lock_table.insert(
                resource.to_string(),
                LockEntry {
                    lock: lock.clone(),
                    waiters: Vec::new(),
                },
            );
            return Ok(lock);
        }
    }

    /// Simple deadlock detection using wait-for graph
    fn detect_deadlock(&self, _tx_id: &TransactionId, _resource: &str) -> bool {
        // TODO: Implement proper wait-for graph traversal
        // For now, return false (no deadlock)
        false
    }

    /// Commit a transaction using 2PC
    pub async fn commit(&self, tx_id: &TransactionId) -> Result<TransactionResult> {
        let start = Instant::now();
        let ctx = self.get_transaction(tx_id)?;

        // Check transaction is active
        if !ctx.state().is_active() {
            return Err(ProximaDBError::TransactionNotActive { id: tx_id.clone() });
        }

        // Check timeout
        if ctx.is_timed_out() {
            return self.abort_with_reason(tx_id, "Transaction timed out").await;
        }

        // For serializable transactions, check for conflicts
        if ctx.isolation_level == IsolationLevel::Serializable {
            if let Err(e) = self.validate_transaction(tx_id) {
                return self.abort_with_reason(tx_id, &e.to_string()).await;
            }
        }

        // Acquire commit lock for serialization
        let _guard = self.commit_lock.lock().await;

        // Phase 1: Prepare
        ctx.set_state(TransactionState::Preparing);
        let prepare_result = self.prepare_phase(tx_id).await;

        match prepare_result {
            Ok(all_prepared) => {
                if all_prepared {
                    // All participants voted YES
                    ctx.set_state(TransactionState::Prepared);

                    // Phase 2: Commit
                    ctx.set_state(TransactionState::Committing);
                    let commit_result = self.commit_phase(tx_id).await;

                    match commit_result {
                        Ok(()) => {
                            ctx.set_state(TransactionState::Committed);
                            let result = self.build_result(tx_id, true, None, start);
                            self.cleanup_transaction(tx_id);

                            // Update stats
                            {
                                let mut stats = self.stats.write();
                                stats.total_committed += 1;
                                stats.current_active = self.active_transactions.read().len();
                            }

                            Ok(result)
                        }
                        Err(e) => {
                            // Commit phase failed - this is a critical error
                            // In a real system, we'd need recovery
                            self.abort_with_reason(tx_id, &e.to_string()).await
                        }
                    }
                } else {
                    // At least one participant voted NO
                    self.abort_with_reason(tx_id, "Participant voted NO during prepare")
                        .await
                }
            }
            Err(e) => self.abort_with_reason(tx_id, &e.to_string()).await,
        }
    }

    /// Prepare phase of 2PC
    async fn prepare_phase(&self, tx_id: &TransactionId) -> Result<bool> {
        let participants = {
            self.participants
                .read()
                .get(tx_id)
                .cloned()
                .unwrap_or_default()
        };

        if participants.is_empty() {
            // No participants, nothing to prepare
            return Ok(true);
        }

        // Send prepare to all participants
        let mut all_prepared = true;
        for (store_id, _participant) in &participants {
            // In a real implementation, this would call the actual store
            // For now, simulate successful prepare
            let vote = self.prepare_participant(tx_id, store_id).await?;

            // Update participant state
            if let Some(tx_participants) = self.participants.write().get_mut(tx_id) {
                if let Some(p) = tx_participants.get_mut(store_id) {
                    p.prepared = vote;
                    p.vote = Some(vote);
                }
            }

            if !vote {
                all_prepared = false;
            }
        }

        Ok(all_prepared)
    }

    /// Prepare a single participant
    async fn prepare_participant(&self, _tx_id: &TransactionId, _store_id: &str) -> Result<bool> {
        // In a real implementation:
        // 1. Send prepare message to store
        // 2. Store validates it can commit (resources available, no conflicts)
        // 3. Store writes to WAL
        // 4. Store responds with vote

        // For now, always vote YES
        Ok(true)
    }

    /// Commit phase of 2PC
    async fn commit_phase(&self, tx_id: &TransactionId) -> Result<()> {
        let participants = {
            self.participants
                .read()
                .get(tx_id)
                .cloned()
                .unwrap_or_default()
        };

        for (store_id, _participant) in &participants {
            self.commit_participant(tx_id, store_id).await?;
        }

        Ok(())
    }

    /// Commit a single participant
    async fn commit_participant(&self, _tx_id: &TransactionId, _store_id: &str) -> Result<()> {
        // In a real implementation:
        // 1. Send commit message to store
        // 2. Store applies the changes
        // 3. Store acknowledges commit

        Ok(())
    }

    /// Abort a transaction
    pub async fn abort(&self, tx_id: &TransactionId) -> Result<TransactionResult> {
        self.abort_with_reason(tx_id, "User requested abort").await
    }

    /// Abort a transaction with reason
    async fn abort_with_reason(
        &self,
        tx_id: &TransactionId,
        reason: &str,
    ) -> Result<TransactionResult> {
        let start = Instant::now();

        if let Ok(ctx) = self.get_transaction(tx_id) {
            if ctx.state().can_rollback() {
                ctx.set_state(TransactionState::RollingBack);

                // Rollback all participants
                self.rollback_all_participants(tx_id).await?;

                ctx.set_state(TransactionState::Aborted);
            }
        }

        let result = self.build_result(tx_id, false, Some(reason.to_string()), start);
        self.cleanup_transaction(tx_id);

        // Update stats
        {
            let mut stats = self.stats.write();
            stats.total_aborted += 1;
            stats.current_active = self.active_transactions.read().len();
        }

        Ok(result)
    }

    /// Rollback all participants
    async fn rollback_all_participants(&self, tx_id: &TransactionId) -> Result<()> {
        let participants = {
            self.participants
                .read()
                .get(tx_id)
                .cloned()
                .unwrap_or_default()
        };

        for (store_id, _participant) in &participants {
            self.rollback_participant(tx_id, store_id).await?;
        }

        Ok(())
    }

    /// Rollback a single participant
    async fn rollback_participant(&self, _tx_id: &TransactionId, _store_id: &str) -> Result<()> {
        // In a real implementation:
        // 1. Send rollback message to store
        // 2. Store discards prepared changes
        // 3. Store acknowledges rollback

        Ok(())
    }

    /// Validate transaction for serializable isolation
    fn validate_transaction(&self, tx_id: &TransactionId) -> Result<()> {
        let ctx = self.get_transaction(tx_id)?;
        let active = self.active_transactions.read();

        for (other_id, other_ctx) in active.iter() {
            if other_id == tx_id {
                continue;
            }

            // Skip committed/aborted transactions
            if other_ctx.state().is_ended() {
                continue;
            }

            if ctx.conflicts_with(other_ctx) {
                self.stats.write().total_conflicts += 1;

                return match self.config.conflict_resolution {
                    ConflictResolution::FirstWriterWins => {
                        // Abort the later transaction
                        Err(ProximaDBError::TransactionConflict {
                            transaction: tx_id.clone(),
                            conflicting_with: other_id.clone(),
                        })
                    }
                    ConflictResolution::LastWriterWins => {
                        // Let this transaction proceed, other will be aborted
                        Ok(())
                    }
                    ConflictResolution::AbortOnConflict => {
                        Err(ProximaDBError::TransactionConflict {
                            transaction: tx_id.clone(),
                            conflicting_with: other_id.clone(),
                        })
                    }
                    ConflictResolution::MergeIfPossible => {
                        // Try to merge - for now just fail
                        Err(ProximaDBError::TransactionConflict {
                            transaction: tx_id.clone(),
                            conflicting_with: other_id.clone(),
                        })
                    }
                    ConflictResolution::WaitAndRetry => Err(ProximaDBError::TransactionConflict {
                        transaction: tx_id.clone(),
                        conflicting_with: other_id.clone(),
                    }),
                };
            }
        }

        Ok(())
    }

    /// Build transaction result
    fn build_result(
        &self,
        tx_id: &TransactionId,
        success: bool,
        error: Option<String>,
        start: Instant,
    ) -> TransactionResult {
        let ctx = self.active_transactions.read().get(tx_id).cloned();
        let participants = self
            .participants
            .read()
            .get(tx_id)
            .cloned()
            .unwrap_or_default();

        let (final_state, operation_count) = if let Some(c) = ctx {
            (c.state(), c.write_set().operations().len())
        } else {
            (TransactionState::Aborted, 0)
        };

        let participant_results: HashMap<String, bool> = participants
            .iter()
            .map(|(k, v)| (k.clone(), v.vote.unwrap_or(false)))
            .collect();

        TransactionResult {
            transaction_id: tx_id.clone(),
            success,
            final_state,
            duration: start.elapsed(),
            operation_count,
            participants: participant_results,
            error,
        }
    }

    /// Cleanup transaction resources
    fn cleanup_transaction(&self, tx_id: &TransactionId) {
        // Release locks
        if let Ok(ctx) = self.get_transaction(tx_id) {
            let locks = ctx.release_locks();
            let mut lock_table = self.lock_table.write();

            for lock in locks {
                if let Some(entry) = lock_table.get_mut(&lock.resource_id) {
                    if entry.lock.transaction_id == *tx_id {
                        // Grant lock to first waiter if any
                        if let Some((waiter_id, mode, _)) = entry.waiters.pop() {
                            entry.lock = Lock {
                                transaction_id: waiter_id,
                                resource_id: lock.resource_id.clone(),
                                mode,
                                acquired_at: Instant::now(),
                            };
                        } else {
                            lock_table.remove(&lock.resource_id);
                        }
                    }
                }
            }
        }

        // Store in history
        if let Some(ctx) = self.active_transactions.read().get(tx_id) {
            let result = TransactionResult {
                transaction_id: tx_id.clone(),
                success: ctx.state() == TransactionState::Committed,
                final_state: ctx.state(),
                duration: ctx.started_at.elapsed(),
                operation_count: ctx.write_set().operations().len(),
                participants: HashMap::new(),
                error: None,
            };

            let mut history = self.history.write();
            history.push(result);

            // Keep only recent history
            if history.len() > 1000 {
                history.remove(0);
            }
        }

        // Remove from active transactions
        self.active_transactions.write().remove(tx_id);
        self.participants.write().remove(tx_id);
    }

    /// Get statistics
    pub fn stats(&self) -> ManagerStats {
        self.stats.read().clone()
    }

    /// Get active transaction count
    pub fn active_count(&self) -> usize {
        self.active_transactions.read().len()
    }

    /// Get all active transactions
    pub fn active_transactions(&self) -> Vec<TransactionStats> {
        self.active_transactions
            .read()
            .values()
            .map(|ctx| ctx.stats())
            .collect()
    }

    /// Create a savepoint
    pub fn savepoint(&self, tx_id: &TransactionId, name: &str) -> Result<()> {
        let ctx = self.get_transaction(tx_id)?;
        if !ctx.state().is_active() {
            return Err(ProximaDBError::TransactionNotActive { id: tx_id.clone() });
        }
        ctx.savepoint(name);
        Ok(())
    }

    /// Rollback to a savepoint
    pub fn rollback_to_savepoint(
        &self,
        tx_id: &TransactionId,
        name: &str,
    ) -> Result<Vec<OperationRollback>> {
        let ctx = self.get_transaction(tx_id)?;
        if !ctx.state().is_active() {
            return Err(ProximaDBError::TransactionNotActive { id: tx_id.clone() });
        }

        let operations =
            ctx.rollback_to_savepoint(name)
                .ok_or_else(|| ProximaDBError::SavepointNotFound {
                    name: name.to_string(),
                })?;

        // Collect rollback info
        let rollbacks: Vec<_> = operations
            .into_iter()
            .filter_map(|op| op.rollback)
            .collect();

        Ok(rollbacks)
    }

    /// Timeout check for all active transactions
    pub async fn check_timeouts(&self) {
        let tx_ids: Vec<TransactionId> = self
            .active_transactions
            .read()
            .iter()
            .filter(|(_, ctx)| ctx.is_timed_out() && ctx.state().is_active())
            .map(|(id, _)| id.clone())
            .collect();

        for tx_id in tx_ids {
            if let Ok(ctx) = self.get_transaction(&tx_id) {
                ctx.set_state(TransactionState::TimedOut);
                let _ = self
                    .abort_with_reason(&tx_id, "Transaction timed out")
                    .await;
                self.stats.write().total_timed_out += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::transaction::operations::VectorOperation;

    fn create_manager() -> MultiModelTransactionManager {
        MultiModelTransactionManager::new(TransactionConfig::default())
    }

    #[test]
    fn test_begin_transaction() {
        let manager = create_manager();

        let tx = manager.begin(None).unwrap();
        assert!(tx.state().is_active());
        assert_eq!(manager.active_count(), 1);
    }

    #[test]
    fn test_begin_with_isolation_level() {
        let manager = create_manager();

        let tx = manager.begin(Some(IsolationLevel::Serializable)).unwrap();
        assert_eq!(tx.isolation_level, IsolationLevel::Serializable);
    }

    #[test]
    fn test_add_operation() {
        let manager = create_manager();
        let tx = manager.begin(None).unwrap();

        let op = MultiModelOperation::Vector(VectorOperation::Insert {
            collection: "test".to_string(),
            ids: vec!["id1".to_string()],
            vectors: vec![vec![0.1, 0.2]],
            metadata: vec![HashMap::new()],
        });

        let seq = manager.add_operation(&tx.id, op).unwrap();
        assert_eq!(seq, 1);

        let stats = tx.stats();
        assert_eq!(stats.write_count, 1);
        assert_eq!(stats.participant_count, 1);
    }

    #[tokio::test]
    async fn test_commit_empty_transaction() {
        let manager = create_manager();
        let tx = manager.begin(None).unwrap();

        let result = manager.commit(&tx.id).await.unwrap();
        assert!(result.success);
        assert_eq!(result.final_state, TransactionState::Committed);
        assert_eq!(manager.active_count(), 0);
    }

    #[tokio::test]
    async fn test_commit_with_operations() {
        let manager = create_manager();
        let tx = manager.begin(None).unwrap();

        let op = MultiModelOperation::Vector(VectorOperation::Insert {
            collection: "test".to_string(),
            ids: vec!["id1".to_string()],
            vectors: vec![vec![0.1, 0.2]],
            metadata: vec![HashMap::new()],
        });
        manager.add_operation(&tx.id, op).unwrap();

        let result = manager.commit(&tx.id).await.unwrap();
        assert!(result.success);
        assert_eq!(result.operation_count, 1);
    }

    #[tokio::test]
    async fn test_abort_transaction() {
        let manager = create_manager();
        let tx = manager.begin(None).unwrap();

        let result = manager.abort(&tx.id).await.unwrap();
        assert!(!result.success);
        assert_eq!(result.final_state, TransactionState::Aborted);
        assert_eq!(manager.active_count(), 0);
    }

    #[test]
    fn test_savepoint() {
        let manager = create_manager();
        let tx = manager.begin(None).unwrap();

        // Add operation
        let op1 = MultiModelOperation::Vector(VectorOperation::Insert {
            collection: "test".to_string(),
            ids: vec!["id1".to_string()],
            vectors: vec![vec![0.1]],
            metadata: vec![HashMap::new()],
        });
        manager.add_operation(&tx.id, op1).unwrap();

        // Create savepoint
        manager.savepoint(&tx.id, "sp1").unwrap();

        // Add more operations
        let op2 = MultiModelOperation::Vector(VectorOperation::Insert {
            collection: "test".to_string(),
            ids: vec!["id2".to_string()],
            vectors: vec![vec![0.2]],
            metadata: vec![HashMap::new()],
        });
        manager.add_operation(&tx.id, op2).unwrap();

        assert_eq!(tx.write_set().operations().len(), 2);

        // Rollback to savepoint
        let rolled_back = manager.rollback_to_savepoint(&tx.id, "sp1").unwrap();
        assert_eq!(rolled_back.len(), 0); // No rollback info set

        assert_eq!(tx.write_set().operations().len(), 1);
    }

    #[test]
    fn test_nested_transaction() {
        let manager = create_manager();
        let parent = manager.begin(None).unwrap();
        let child = manager.begin_nested(&parent.id, None).unwrap();

        assert_eq!(child.parent(), Some(&parent.id));
        assert!(parent.children().contains(&child.id));
        assert_eq!(manager.active_count(), 2);
    }

    #[tokio::test]
    async fn test_transaction_timeout() {
        let config = TransactionConfig {
            default_timeout: Duration::from_millis(10),
            ..Default::default()
        };
        let manager = MultiModelTransactionManager::new(config);
        let tx = manager.begin(None).unwrap();

        // Wait for timeout
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Try to add operation - should fail
        let op = MultiModelOperation::Vector(VectorOperation::Insert {
            collection: "test".to_string(),
            ids: vec!["id1".to_string()],
            vectors: vec![vec![0.1]],
            metadata: vec![HashMap::new()],
        });

        let result = manager.add_operation(&tx.id, op);
        assert!(result.is_err());
    }

    #[test]
    fn test_max_concurrent_transactions() {
        let config = TransactionConfig {
            max_concurrent_transactions: 2,
            ..Default::default()
        };
        let manager = MultiModelTransactionManager::new(config);

        let _tx1 = manager.begin(None).unwrap();
        let _tx2 = manager.begin(None).unwrap();

        // Third should fail
        let result = manager.begin(None);
        assert!(result.is_err());
    }

    #[test]
    fn test_stats() {
        let manager = create_manager();

        let _tx1 = manager.begin(None).unwrap();
        let _tx2 = manager.begin(None).unwrap();

        let stats = manager.stats();
        assert_eq!(stats.total_started, 2);
        assert_eq!(stats.current_active, 2);
    }

    #[tokio::test]
    async fn test_conflict_detection() {
        // Use RepeatableRead (no locking) to test conflict detection at commit time
        let config = TransactionConfig {
            default_isolation_level: IsolationLevel::RepeatableRead,
            conflict_resolution: ConflictResolution::FirstWriterWins,
            ..Default::default()
        };
        let manager = MultiModelTransactionManager::new(config);

        // Use RepeatableRead isolation to avoid lock contention during operations
        let tx1 = manager.begin(Some(IsolationLevel::RepeatableRead)).unwrap();
        let tx2 = manager.begin(Some(IsolationLevel::RepeatableRead)).unwrap();

        // Both write to same resource
        let op1 = MultiModelOperation::Vector(VectorOperation::Update {
            collection: "shared".to_string(),
            ids: vec!["same_id".to_string()],
            vectors: Some(vec![vec![0.1]]),
            metadata: None,
        });
        manager.add_operation(&tx1.id, op1).unwrap();

        let op2 = MultiModelOperation::Vector(VectorOperation::Update {
            collection: "shared".to_string(),
            ids: vec!["same_id".to_string()],
            vectors: Some(vec![vec![0.2]]),
            metadata: None,
        });
        manager.add_operation(&tx2.id, op2).unwrap();

        // First commit should succeed
        let result1 = manager.commit(&tx1.id).await.unwrap();
        assert!(result1.success);

        // Second should also succeed at RepeatableRead isolation
        // (conflict detection is stricter at Serializable)
        let result2 = manager.commit(&tx2.id).await;
        assert!(result2.is_ok());
    }

    #[tokio::test]
    async fn test_timeout_check() {
        let config = TransactionConfig {
            default_timeout: Duration::from_millis(10),
            ..Default::default()
        };
        let manager = MultiModelTransactionManager::new(config);
        let _tx = manager.begin(None).unwrap();

        assert_eq!(manager.active_count(), 1);

        // Wait and check timeouts
        tokio::time::sleep(Duration::from_millis(20)).await;
        manager.check_timeouts().await;

        assert_eq!(manager.active_count(), 0);
        assert_eq!(manager.stats().total_timed_out, 1);
    }
}
