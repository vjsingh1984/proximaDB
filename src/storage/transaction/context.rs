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

//! Transaction Context for Multi-Model Operations

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use super::isolation::{IsolationLevel, Lock, LockMode};
use super::operations::{MultiModelOperation, OperationRollback};

/// Unique transaction identifier
pub type TransactionId = String;

/// Transaction state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransactionState {
    /// Transaction is active and accepting operations
    Active,
    /// Transaction is preparing for commit (2PC phase 1)
    Preparing,
    /// Transaction is prepared and ready to commit
    Prepared,
    /// Transaction is committing (2PC phase 2)
    Committing,
    /// Transaction has been committed
    Committed,
    /// Transaction is rolling back
    RollingBack,
    /// Transaction has been aborted
    Aborted,
    /// Transaction has timed out
    TimedOut,
}

impl TransactionState {
    /// Check if transaction is still active (can accept operations)
    pub fn is_active(&self) -> bool {
        matches!(self, TransactionState::Active)
    }

    /// Check if transaction has ended (committed or aborted)
    pub fn is_ended(&self) -> bool {
        matches!(
            self,
            TransactionState::Committed | TransactionState::Aborted | TransactionState::TimedOut
        )
    }

    /// Check if transaction can be rolled back
    pub fn can_rollback(&self) -> bool {
        matches!(
            self,
            TransactionState::Active
                | TransactionState::Preparing
                | TransactionState::Prepared
                | TransactionState::RollingBack
        )
    }
}

/// Type of operation performed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationType {
    /// Read operation
    Read,
    /// Write operation (insert, update, delete)
    Write,
    /// Schema modification (create, alter, drop)
    Schema,
}

/// A single operation within a transaction
#[derive(Debug, Clone)]
pub struct TransactionOperation {
    /// Sequence number within transaction
    pub sequence: u64,
    /// Operation type
    pub op_type: OperationType,
    /// The actual operation
    pub operation: MultiModelOperation,
    /// When operation was added
    pub timestamp: Instant,
    /// Resources affected by this operation
    pub affected_resources: Vec<String>,
    /// Rollback information
    pub rollback: Option<OperationRollback>,
}

impl TransactionOperation {
    /// Create a new transaction operation
    pub fn new(sequence: u64, operation: MultiModelOperation) -> Self {
        let op_type = if operation.is_write() {
            OperationType::Write
        } else {
            OperationType::Read
        };

        let affected_resources = vec![operation.target().to_string()];

        Self {
            sequence,
            op_type,
            operation,
            timestamp: Instant::now(),
            affected_resources,
            rollback: None,
        }
    }

    /// Set rollback information
    pub fn with_rollback(mut self, rollback: OperationRollback) -> Self {
        self.rollback = Some(rollback);
        self
    }
}

/// Read set for tracking reads in a transaction
#[derive(Debug, Clone, Default)]
pub struct ReadSet {
    /// Collection/graph -> set of IDs read
    entries: HashMap<String, HashSet<String>>,
    /// Version numbers observed
    versions: HashMap<String, u64>,
}

impl ReadSet {
    /// Add a read entry
    pub fn add(&mut self, container: &str, id: &str, version: u64) {
        self.entries
            .entry(container.to_string())
            .or_default()
            .insert(id.to_string());
        self.versions.insert(format!("{}:{}", container, id), version);
    }

    /// Check if a read conflicts with a write
    pub fn conflicts_with(&self, container: &str, id: &str, new_version: u64) -> bool {
        let key = format!("{}:{}", container, id);
        if let Some(&observed_version) = self.versions.get(&key) {
            new_version > observed_version
        } else {
            false
        }
    }

    /// Get all entries
    pub fn entries(&self) -> &HashMap<String, HashSet<String>> {
        &self.entries
    }
}

/// Write set for tracking writes in a transaction
#[derive(Debug, Clone, Default)]
pub struct WriteSet {
    /// Collection/graph -> set of IDs written
    entries: HashMap<String, HashSet<String>>,
    /// Operations pending commit
    operations: Vec<TransactionOperation>,
}

impl WriteSet {
    /// Add a write entry
    pub fn add(&mut self, container: &str, id: &str, operation: TransactionOperation) {
        self.entries
            .entry(container.to_string())
            .or_default()
            .insert(id.to_string());
        self.operations.push(operation);
    }

    /// Check if a write conflicts with another transaction's write
    pub fn conflicts_with(&self, container: &str, id: &str) -> bool {
        self.entries
            .get(container)
            .map(|ids| ids.contains(id))
            .unwrap_or(false)
    }

    /// Get all entries
    pub fn entries(&self) -> &HashMap<String, HashSet<String>> {
        &self.entries
    }

    /// Get operations
    pub fn operations(&self) -> &[TransactionOperation] {
        &self.operations
    }

    /// Take operations for commit
    pub fn take_operations(&mut self) -> Vec<TransactionOperation> {
        std::mem::take(&mut self.operations)
    }
}

/// Transaction context holding all state for a multi-model transaction
#[derive(Debug)]
pub struct TransactionContext {
    /// Unique transaction ID
    pub id: TransactionId,
    /// Current state
    state: Arc<RwLock<TransactionState>>,
    /// Isolation level
    pub isolation_level: IsolationLevel,
    /// When transaction started
    pub started_at: Instant,
    /// Transaction timeout
    pub timeout: Duration,
    /// Read set
    read_set: Arc<RwLock<ReadSet>>,
    /// Write set
    write_set: Arc<RwLock<WriteSet>>,
    /// Locks held by this transaction
    locks: Arc<RwLock<Vec<Lock>>>,
    /// Savepoints within transaction
    savepoints: Arc<RwLock<HashMap<String, u64>>>,
    /// Operation sequence counter
    sequence_counter: Arc<RwLock<u64>>,
    /// Parent transaction (for nested transactions)
    parent: Option<TransactionId>,
    /// Child transactions (for nested transactions)
    children: Arc<RwLock<Vec<TransactionId>>>,
    /// Participant stores involved in this transaction
    participants: Arc<RwLock<HashSet<String>>>,
}

impl TransactionContext {
    /// Create a new transaction context
    pub fn new(id: TransactionId, isolation_level: IsolationLevel, timeout: Duration) -> Self {
        Self {
            id,
            state: Arc::new(RwLock::new(TransactionState::Active)),
            isolation_level,
            started_at: Instant::now(),
            timeout,
            read_set: Arc::new(RwLock::new(ReadSet::default())),
            write_set: Arc::new(RwLock::new(WriteSet::default())),
            locks: Arc::new(RwLock::new(Vec::new())),
            savepoints: Arc::new(RwLock::new(HashMap::new())),
            sequence_counter: Arc::new(RwLock::new(0)),
            parent: None,
            children: Arc::new(RwLock::new(Vec::new())),
            participants: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Create a nested transaction
    pub fn new_nested(
        id: TransactionId,
        parent: TransactionId,
        isolation_level: IsolationLevel,
        timeout: Duration,
    ) -> Self {
        let mut ctx = Self::new(id, isolation_level, timeout);
        ctx.parent = Some(parent);
        ctx
    }

    /// Get current state
    pub fn state(&self) -> TransactionState {
        *self.state.read()
    }

    /// Set transaction state
    pub fn set_state(&self, new_state: TransactionState) {
        *self.state.write() = new_state;
    }

    /// Check if transaction has timed out
    pub fn is_timed_out(&self) -> bool {
        self.started_at.elapsed() > self.timeout
    }

    /// Get remaining time before timeout
    pub fn remaining_time(&self) -> Option<Duration> {
        let elapsed = self.started_at.elapsed();
        if elapsed >= self.timeout {
            None
        } else {
            Some(self.timeout - elapsed)
        }
    }

    /// Add a read to the read set
    pub fn add_read(&self, container: &str, id: &str, version: u64) {
        self.read_set.write().add(container, id, version);
    }

    /// Add a write to the write set
    pub fn add_write(&self, operation: MultiModelOperation) -> u64 {
        let mut counter = self.sequence_counter.write();
        *counter += 1;
        let seq = *counter;

        let target = operation.target().to_string();
        let ids = Self::extract_ids(&operation);
        let tx_op = TransactionOperation::new(seq, operation);

        let mut write_set = self.write_set.write();
        for id in ids {
            write_set.add(&target, &id, tx_op.clone());
        }

        // Register participant store
        let model_type = tx_op.operation.model_type();
        self.participants.write().insert(model_type.to_string());

        seq
    }

    /// Extract IDs affected by an operation
    fn extract_ids(operation: &MultiModelOperation) -> Vec<String> {
        match operation {
            MultiModelOperation::Vector(op) => op.affected_ids().iter().map(|s| s.clone()).collect(),
            MultiModelOperation::Document(_) => vec!["*".to_string()], // Filter-based
            MultiModelOperation::Graph(op) => {
                op.affected_node_ids().iter().map(|s| s.to_string()).collect()
            }
            MultiModelOperation::Observability(_) => vec!["*".to_string()], // Batch
        }
    }

    /// Get read set
    pub fn read_set(&self) -> ReadSet {
        self.read_set.read().clone()
    }

    /// Get write set
    pub fn write_set(&self) -> WriteSet {
        self.write_set.read().clone()
    }

    /// Create a savepoint
    pub fn savepoint(&self, name: &str) {
        let current_seq = *self.sequence_counter.read();
        self.savepoints.write().insert(name.to_string(), current_seq);
    }

    /// Rollback to a savepoint
    pub fn rollback_to_savepoint(&self, name: &str) -> Option<Vec<TransactionOperation>> {
        let seq = self.savepoints.read().get(name).copied()?;

        let mut write_set = self.write_set.write();
        let operations = write_set.take_operations();

        // Keep operations before savepoint, return those after
        let (keep, rollback): (Vec<_>, Vec<_>) =
            operations.into_iter().partition(|op| op.sequence <= seq);

        // Restore kept operations
        for op in keep {
            let target = op.operation.target().to_string();
            let ids = Self::extract_ids(&op.operation);
            for id in ids {
                write_set.add(&target, &id, op.clone());
            }
        }

        Some(rollback)
    }

    /// Acquire a lock
    pub fn acquire_lock(&self, resource_id: &str, mode: LockMode) -> Lock {
        let lock = Lock {
            transaction_id: self.id.clone(),
            resource_id: resource_id.to_string(),
            mode,
            acquired_at: Instant::now(),
        };
        self.locks.write().push(lock.clone());
        lock
    }

    /// Release all locks
    pub fn release_locks(&self) -> Vec<Lock> {
        std::mem::take(&mut *self.locks.write())
    }

    /// Get all locks
    pub fn locks(&self) -> Vec<Lock> {
        self.locks.read().clone()
    }

    /// Check for conflicts with another transaction
    pub fn conflicts_with(&self, other: &TransactionContext) -> bool {
        // Check write-write conflicts
        let our_writes = self.write_set.read();
        let their_writes = other.write_set.read();

        for (container, our_ids) in our_writes.entries() {
            if let Some(their_ids) = their_writes.entries().get(container) {
                for id in our_ids {
                    if their_ids.contains(id) {
                        return true;
                    }
                }
            }
        }

        // For higher isolation levels, also check read-write conflicts
        if self.isolation_level.prevents_non_repeatable_reads() {
            let our_reads = self.read_set.read();
            for (container, our_ids) in our_reads.entries() {
                if let Some(their_ids) = their_writes.entries().get(container) {
                    for id in our_ids {
                        if their_ids.contains(id) {
                            return true;
                        }
                    }
                }
            }
        }

        false
    }

    /// Get participant stores
    pub fn participants(&self) -> HashSet<String> {
        self.participants.read().clone()
    }

    /// Add a child transaction
    pub fn add_child(&self, child_id: TransactionId) {
        self.children.write().push(child_id);
    }

    /// Get child transactions
    pub fn children(&self) -> Vec<TransactionId> {
        self.children.read().clone()
    }

    /// Get parent transaction
    pub fn parent(&self) -> Option<&TransactionId> {
        self.parent.as_ref()
    }

    /// Get transaction statistics
    pub fn stats(&self) -> TransactionStats {
        TransactionStats {
            id: self.id.clone(),
            state: self.state(),
            isolation_level: self.isolation_level,
            duration: self.started_at.elapsed(),
            read_count: self.read_set.read().entries.values().map(|s| s.len()).sum(),
            write_count: self.write_set.read().operations.len(),
            lock_count: self.locks.read().len(),
            participant_count: self.participants.read().len(),
        }
    }
}

/// Transaction statistics
#[derive(Debug, Clone)]
pub struct TransactionStats {
    /// Transaction ID
    pub id: TransactionId,
    /// Current state
    pub state: TransactionState,
    /// Isolation level
    pub isolation_level: IsolationLevel,
    /// Duration since start
    pub duration: Duration,
    /// Number of reads
    pub read_count: usize,
    /// Number of writes
    pub write_count: usize,
    /// Number of locks held
    pub lock_count: usize,
    /// Number of participant stores
    pub participant_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::transaction::operations::VectorOperation;
    use std::collections::HashMap;

    #[test]
    fn test_transaction_context_creation() {
        let ctx = TransactionContext::new(
            "tx_001".to_string(),
            IsolationLevel::ReadCommitted,
            Duration::from_secs(30),
        );

        assert_eq!(ctx.id, "tx_001");
        assert_eq!(ctx.state(), TransactionState::Active);
        assert_eq!(ctx.isolation_level, IsolationLevel::ReadCommitted);
    }

    #[test]
    fn test_transaction_state_transitions() {
        let ctx = TransactionContext::new(
            "tx_002".to_string(),
            IsolationLevel::Serializable,
            Duration::from_secs(30),
        );

        assert!(ctx.state().is_active());
        assert!(!ctx.state().is_ended());

        ctx.set_state(TransactionState::Preparing);
        assert!(!ctx.state().is_active());
        assert!(ctx.state().can_rollback());

        ctx.set_state(TransactionState::Committed);
        assert!(ctx.state().is_ended());
        assert!(!ctx.state().can_rollback());
    }

    #[test]
    fn test_read_write_sets() {
        let ctx = TransactionContext::new(
            "tx_003".to_string(),
            IsolationLevel::RepeatableRead,
            Duration::from_secs(30),
        );

        // Add reads
        ctx.add_read("collection1", "id1", 1);
        ctx.add_read("collection1", "id2", 1);
        ctx.add_read("collection2", "id3", 2);

        let read_set = ctx.read_set();
        assert_eq!(read_set.entries().get("collection1").unwrap().len(), 2);
        assert_eq!(read_set.entries().get("collection2").unwrap().len(), 1);

        // Add writes
        let op = MultiModelOperation::Vector(VectorOperation::Insert {
            collection: "collection1".to_string(),
            ids: vec!["new1".to_string()],
            vectors: vec![vec![0.1, 0.2]],
            metadata: vec![HashMap::new()],
        });
        ctx.add_write(op);

        let write_set = ctx.write_set();
        assert_eq!(write_set.operations().len(), 1);
    }

    #[test]
    fn test_savepoints() {
        let ctx = TransactionContext::new(
            "tx_004".to_string(),
            IsolationLevel::ReadCommitted,
            Duration::from_secs(30),
        );

        // Add some writes
        let op1 = MultiModelOperation::Vector(VectorOperation::Insert {
            collection: "col".to_string(),
            ids: vec!["id1".to_string()],
            vectors: vec![vec![0.1]],
            metadata: vec![HashMap::new()],
        });
        ctx.add_write(op1);

        // Create savepoint
        ctx.savepoint("sp1");

        // Add more writes
        let op2 = MultiModelOperation::Vector(VectorOperation::Insert {
            collection: "col".to_string(),
            ids: vec!["id2".to_string()],
            vectors: vec![vec![0.2]],
            metadata: vec![HashMap::new()],
        });
        ctx.add_write(op2);

        // Verify we have 2 operations
        assert_eq!(ctx.write_set().operations().len(), 2);

        // Rollback to savepoint
        let rolled_back = ctx.rollback_to_savepoint("sp1").unwrap();
        assert_eq!(rolled_back.len(), 1);
        assert_eq!(ctx.write_set().operations().len(), 1);
    }

    #[test]
    fn test_conflict_detection() {
        let ctx1 = TransactionContext::new(
            "tx_005".to_string(),
            IsolationLevel::RepeatableRead,
            Duration::from_secs(30),
        );

        let ctx2 = TransactionContext::new(
            "tx_006".to_string(),
            IsolationLevel::RepeatableRead,
            Duration::from_secs(30),
        );

        // Both write to same collection/id
        let op1 = MultiModelOperation::Vector(VectorOperation::Update {
            collection: "shared".to_string(),
            ids: vec!["same_id".to_string()],
            vectors: Some(vec![vec![0.1]]),
            metadata: None,
        });
        ctx1.add_write(op1);

        let op2 = MultiModelOperation::Vector(VectorOperation::Update {
            collection: "shared".to_string(),
            ids: vec!["same_id".to_string()],
            vectors: Some(vec![vec![0.2]]),
            metadata: None,
        });
        ctx2.add_write(op2);

        assert!(ctx1.conflicts_with(&ctx2));
    }

    #[test]
    fn test_locks() {
        let ctx = TransactionContext::new(
            "tx_007".to_string(),
            IsolationLevel::Serializable,
            Duration::from_secs(30),
        );

        let lock1 = ctx.acquire_lock("resource1", LockMode::Shared);
        let lock2 = ctx.acquire_lock("resource2", LockMode::Exclusive);

        assert_eq!(ctx.locks().len(), 2);
        assert_eq!(lock1.resource_id, "resource1");
        assert_eq!(lock2.mode, LockMode::Exclusive);

        let released = ctx.release_locks();
        assert_eq!(released.len(), 2);
        assert_eq!(ctx.locks().len(), 0);
    }

    #[test]
    fn test_timeout() {
        let ctx = TransactionContext::new(
            "tx_008".to_string(),
            IsolationLevel::ReadCommitted,
            Duration::from_millis(10),
        );

        assert!(!ctx.is_timed_out());
        assert!(ctx.remaining_time().is_some());

        std::thread::sleep(Duration::from_millis(15));

        assert!(ctx.is_timed_out());
        assert!(ctx.remaining_time().is_none());
    }

    #[test]
    fn test_nested_transactions() {
        let parent = TransactionContext::new(
            "tx_parent".to_string(),
            IsolationLevel::Serializable,
            Duration::from_secs(30),
        );

        let child = TransactionContext::new_nested(
            "tx_child".to_string(),
            "tx_parent".to_string(),
            IsolationLevel::Serializable,
            Duration::from_secs(30),
        );

        parent.add_child("tx_child".to_string());

        assert_eq!(child.parent(), Some(&"tx_parent".to_string()));
        assert_eq!(parent.children().len(), 1);
    }

    #[test]
    fn test_stats() {
        let ctx = TransactionContext::new(
            "tx_009".to_string(),
            IsolationLevel::Snapshot,
            Duration::from_secs(30),
        );

        ctx.add_read("col", "id1", 1);
        let op = MultiModelOperation::Vector(VectorOperation::Insert {
            collection: "col".to_string(),
            ids: vec!["id2".to_string()],
            vectors: vec![vec![0.1]],
            metadata: vec![HashMap::new()],
        });
        ctx.add_write(op);
        ctx.acquire_lock("resource", LockMode::Shared);

        let stats = ctx.stats();
        assert_eq!(stats.id, "tx_009");
        assert_eq!(stats.read_count, 1);
        assert_eq!(stats.write_count, 1);
        assert_eq!(stats.lock_count, 1);
        assert_eq!(stats.participant_count, 1);
    }
}
