//! # Transaction Coordinator
//!
//! High-level transaction coordinator that combines MVCC isolation
//! with 2PC for cross-model distributed transactions.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use tokio::sync::RwLock;
use tracing::{debug, info};
use uuid::Uuid;

use super::isolation::{IsolationLevel, IsolationManager, ReadSnapshot};
use super::two_phase_commit::{
    ParticipantType, TransactionState, TwoPhaseCommitConfig, TwoPhaseCommitProtocol,
    TwoPhaseParticipant,
};

/// Transaction configuration
#[derive(Debug, Clone)]
pub struct TransactionConfig {
    /// Default isolation level
    pub default_isolation: IsolationLevel,
    /// 2PC configuration
    pub two_phase_commit: TwoPhaseCommitConfig,
    /// Enable automatic 2PC for cross-model transactions
    pub auto_2pc: bool,
    /// Transaction timeout
    pub timeout: Duration,
    /// Maximum concurrent transactions
    pub max_concurrent: usize,
}

impl Default for TransactionConfig {
    fn default() -> Self {
        Self {
            default_isolation: IsolationLevel::ReadCommitted,
            two_phase_commit: TwoPhaseCommitConfig::default(),
            auto_2pc: true,
            timeout: Duration::from_secs(300), // 5 minutes
            max_concurrent: 1000,
        }
    }
}

/// Transaction statistics
#[derive(Debug, Clone, Default)]
pub struct TransactionStats {
    /// Total transactions started
    pub total_started: u64,
    /// Total transactions committed
    pub total_committed: u64,
    /// Total transactions aborted
    pub total_aborted: u64,
    /// Total 2PC transactions
    pub total_2pc: u64,
    /// Current active transactions
    pub active_count: u64,
    /// Average transaction duration in milliseconds
    pub avg_duration_ms: f64,
    /// Transactions by isolation level
    pub by_isolation: HashMap<IsolationLevel, u64>,
}

/// A transaction handle
#[derive(Debug)]
pub struct Transaction {
    /// Transaction ID
    pub id: String,
    /// Start time
    pub start_time: Instant,
    /// Isolation level
    pub isolation_level: IsolationLevel,
    /// Read snapshot for MVCC
    pub snapshot: ReadSnapshot,
    /// Stores involved in this transaction
    pub involved_stores: Vec<ParticipantType>,
    /// Is this a distributed (2PC) transaction?
    pub is_distributed: bool,
    /// Current state
    pub state: TransactionState,
    /// Auto-commit mode
    pub auto_commit: bool,
}

impl Transaction {
    /// Create a new transaction
    pub fn new(isolation_level: IsolationLevel) -> Self {
        let id = Uuid::new_v4().to_string();
        let snapshot = ReadSnapshot::new(id.clone());

        Self {
            id,
            start_time: Instant::now(),
            isolation_level,
            snapshot,
            involved_stores: Vec::new(),
            is_distributed: false,
            state: TransactionState::Active,
            auto_commit: false,
        }
    }

    /// Add a store to involved stores
    pub fn involve_store(&mut self, store: ParticipantType) {
        if !self.involved_stores.contains(&store) {
            self.involved_stores.push(store);
            // Multiple stores means distributed transaction
            if self.involved_stores.len() > 1 {
                self.is_distributed = true;
            }
        }
    }

    /// Get transaction duration
    pub fn duration(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Check if transaction is active
    pub fn is_active(&self) -> bool {
        self.state == TransactionState::Active
    }
}

/// Transaction coordinator manages transactions across all stores
pub struct TransactionCoordinator {
    /// Configuration
    config: TransactionConfig,
    /// Isolation manager for MVCC
    isolation_manager: Arc<IsolationManager>,
    /// 2PC protocol for distributed transactions
    two_phase_commit: Arc<TwoPhaseCommitProtocol>,
    /// Active transactions
    transactions: RwLock<HashMap<String, Transaction>>,
    /// Statistics
    stats: RwLock<TransactionStats>,
}

impl TransactionCoordinator {
    /// Create a new transaction coordinator
    pub fn new(config: TransactionConfig) -> Self {
        Self {
            isolation_manager: Arc::new(IsolationManager::new(config.default_isolation)),
            two_phase_commit: Arc::new(TwoPhaseCommitProtocol::new(
                config.two_phase_commit.clone(),
            )),
            config,
            transactions: RwLock::new(HashMap::new()),
            stats: RwLock::new(TransactionStats::default()),
        }
    }

    /// Register a 2PC participant
    pub async fn register_participant(&self, participant: Arc<dyn TwoPhaseParticipant>) {
        self.two_phase_commit
            .register_participant(participant)
            .await;
    }

    /// Begin a new transaction
    pub async fn begin(&self, isolation_level: Option<IsolationLevel>) -> Result<String> {
        let level = isolation_level.unwrap_or(self.config.default_isolation);

        // Check concurrent transaction limit
        {
            let transactions = self.transactions.read().await;
            if transactions.len() >= self.config.max_concurrent {
                return Err(anyhow!("Maximum concurrent transactions exceeded"));
            }
        }

        let mut transaction = Transaction::new(level);
        let txn_id = transaction.id.clone();

        // Initialize MVCC snapshot
        let snapshot = self
            .isolation_manager
            .begin_transaction(&txn_id, Some(level))
            .await;
        transaction.snapshot = snapshot;

        // Store transaction
        {
            let mut transactions = self.transactions.write().await;
            transactions.insert(txn_id.clone(), transaction);
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_started += 1;
            stats.active_count += 1;
            *stats.by_isolation.entry(level).or_insert(0) += 1;
        }

        debug!("Transaction {} started with isolation {:?}", txn_id, level);
        Ok(txn_id)
    }

    /// Begin a transaction with auto-commit mode
    pub async fn begin_auto_commit(
        &self,
        isolation_level: Option<IsolationLevel>,
    ) -> Result<String> {
        let txn_id = self.begin(isolation_level).await?;

        {
            let mut transactions = self.transactions.write().await;
            if let Some(txn) = transactions.get_mut(&txn_id) {
                txn.auto_commit = true;
            }
        }

        Ok(txn_id)
    }

    /// Register store involvement in a transaction
    pub async fn involve_store(&self, txn_id: &str, store: ParticipantType) -> Result<()> {
        let mut transactions = self.transactions.write().await;
        let transaction = transactions
            .get_mut(txn_id)
            .ok_or_else(|| anyhow!("Transaction {} not found", txn_id))?;

        if !transaction.is_active() {
            return Err(anyhow!("Transaction {} is not active", txn_id));
        }

        let was_distributed = transaction.is_distributed;
        transaction.involve_store(store);

        // If transaction became distributed, initialize 2PC
        if !was_distributed && transaction.is_distributed && self.config.auto_2pc {
            drop(transactions);
            self.two_phase_commit.begin(txn_id).await?;
            for store_type in self.get_involved_stores(txn_id).await? {
                self.two_phase_commit.enlist(txn_id, store_type).await?;
            }

            let mut stats = self.stats.write().await;
            stats.total_2pc += 1;
        }

        Ok(())
    }

    /// Record a write operation
    pub async fn record_write(
        &self,
        txn_id: &str,
        store_type: &str,
        record_id: &str,
    ) -> Result<()> {
        {
            let transactions = self.transactions.read().await;
            if !transactions.contains_key(txn_id) {
                return Err(anyhow!("Transaction {} not found", txn_id));
            }
        }

        self.isolation_manager
            .record_write(txn_id, store_type, record_id)
            .await;
        Ok(())
    }

    /// Record a delete operation
    pub async fn record_delete(
        &self,
        txn_id: &str,
        store_type: &str,
        record_id: &str,
    ) -> Result<()> {
        {
            let transactions = self.transactions.read().await;
            if !transactions.contains_key(txn_id) {
                return Err(anyhow!("Transaction {} not found", txn_id));
            }
        }

        self.isolation_manager
            .record_delete(txn_id, store_type, record_id)
            .await;
        Ok(())
    }

    /// Commit a transaction
    pub async fn commit(&self, txn_id: &str) -> Result<()> {
        let (is_distributed, duration) = {
            let transactions = self.transactions.read().await;
            let transaction = transactions
                .get(txn_id)
                .ok_or_else(|| anyhow!("Transaction {} not found", txn_id))?;

            if !transaction.is_active() {
                return Err(anyhow!("Transaction {} is not active", txn_id));
            }

            (transaction.is_distributed, transaction.duration())
        };

        if is_distributed {
            // Use 2PC for distributed transactions
            let prepared = self.two_phase_commit.prepare(txn_id).await?;

            if prepared {
                self.two_phase_commit.commit(txn_id).await?;
            } else {
                self.two_phase_commit.abort(txn_id).await?;
                return Err(anyhow!(
                    "Transaction {} aborted during prepare phase",
                    txn_id
                ));
            }
        }

        // Commit in isolation manager
        self.isolation_manager
            .commit_transaction(txn_id)
            .await
            .map_err(|e| anyhow!(e))?;

        // Update transaction state
        {
            let mut transactions = self.transactions.write().await;
            if let Some(txn) = transactions.get_mut(txn_id) {
                txn.state = TransactionState::Committed;
            }
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_committed += 1;
            stats.active_count = stats.active_count.saturating_sub(1);

            // Update average duration
            let new_avg = (stats.avg_duration_ms * (stats.total_committed - 1) as f64
                + duration.as_millis() as f64)
                / stats.total_committed as f64;
            stats.avg_duration_ms = new_avg;
        }

        info!(
            "Transaction {} committed (duration: {:?})",
            txn_id, duration
        );
        Ok(())
    }

    /// Rollback a transaction
    pub async fn rollback(&self, txn_id: &str) -> Result<()> {
        let is_distributed = {
            let transactions = self.transactions.read().await;
            let transaction = transactions
                .get(txn_id)
                .ok_or_else(|| anyhow!("Transaction {} not found", txn_id))?;

            transaction.is_distributed
        };

        if is_distributed {
            // Use 2PC abort
            self.two_phase_commit.abort(txn_id).await?;
        }

        // Abort in isolation manager
        self.isolation_manager.abort_transaction(txn_id).await;

        // Update transaction state
        {
            let mut transactions = self.transactions.write().await;
            if let Some(txn) = transactions.get_mut(txn_id) {
                txn.state = TransactionState::Aborted;
            }
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_aborted += 1;
            stats.active_count = stats.active_count.saturating_sub(1);
        }

        info!("Transaction {} rolled back", txn_id);
        Ok(())
    }

    /// Get transaction snapshot
    pub async fn get_snapshot(&self, txn_id: &str) -> Option<ReadSnapshot> {
        let transactions = self.transactions.read().await;
        transactions.get(txn_id).map(|t| t.snapshot.clone())
    }

    /// Get transaction state
    pub async fn get_state(&self, txn_id: &str) -> Option<TransactionState> {
        let transactions = self.transactions.read().await;
        transactions.get(txn_id).map(|t| t.state)
    }

    /// Get involved stores for a transaction
    pub async fn get_involved_stores(&self, txn_id: &str) -> Result<Vec<ParticipantType>> {
        let transactions = self.transactions.read().await;
        transactions
            .get(txn_id)
            .map(|t| t.involved_stores.clone())
            .ok_or_else(|| anyhow!("Transaction {} not found", txn_id))
    }

    /// Check if transaction is distributed
    pub async fn is_distributed(&self, txn_id: &str) -> bool {
        let transactions = self.transactions.read().await;
        transactions.get(txn_id).is_some_and(|t| t.is_distributed)
    }

    /// Get statistics
    pub async fn stats(&self) -> TransactionStats {
        let stats = self.stats.read().await;
        stats.clone()
    }

    /// Cleanup completed transactions
    pub async fn cleanup_completed(&self, max_age: Duration) {
        let mut transactions = self.transactions.write().await;
        transactions.retain(|_, txn| txn.is_active() || txn.duration() < max_age);

        // Also cleanup in 2PC
        self.two_phase_commit.cleanup_completed(max_age).await;
    }

    /// Get active transaction count
    pub async fn active_count(&self) -> usize {
        let transactions = self.transactions.read().await;
        transactions.values().filter(|t| t.is_active()).count()
    }

    /// Get configuration
    pub fn config(&self) -> &TransactionConfig {
        &self.config
    }

    /// Get isolation manager
    pub fn isolation_manager(&self) -> Arc<IsolationManager> {
        Arc::clone(&self.isolation_manager)
    }

    /// Get 2PC protocol
    pub fn two_phase_commit(&self) -> Arc<TwoPhaseCommitProtocol> {
        Arc::clone(&self.two_phase_commit)
    }
}

impl Default for TransactionCoordinator {
    fn default() -> Self {
        Self::new(TransactionConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_config_default() {
        let config = TransactionConfig::default();
        assert_eq!(config.default_isolation, IsolationLevel::ReadCommitted);
        assert!(config.auto_2pc);
        assert_eq!(config.timeout, Duration::from_secs(300));
    }

    #[test]
    fn test_transaction_creation() {
        let txn = Transaction::new(IsolationLevel::ReadCommitted);
        assert!(!txn.id.is_empty());
        assert!(txn.is_active());
        assert!(!txn.is_distributed);
        assert!(txn.involved_stores.is_empty());
    }

    #[test]
    fn test_transaction_involve_store() {
        let mut txn = Transaction::new(IsolationLevel::ReadCommitted);

        txn.involve_store(ParticipantType::Vector);
        assert_eq!(txn.involved_stores.len(), 1);
        assert!(!txn.is_distributed);

        txn.involve_store(ParticipantType::Document);
        assert_eq!(txn.involved_stores.len(), 2);
        assert!(txn.is_distributed);

        // Adding same store again shouldn't change anything
        txn.involve_store(ParticipantType::Vector);
        assert_eq!(txn.involved_stores.len(), 2);
    }

    #[tokio::test]
    async fn test_coordinator_begin() {
        let coordinator = TransactionCoordinator::new(TransactionConfig::default());

        let txn_id = coordinator.begin(None).await.unwrap();
        assert!(!txn_id.is_empty());

        let state = coordinator.get_state(&txn_id).await;
        assert_eq!(state, Some(TransactionState::Active));

        let stats = coordinator.stats().await;
        assert_eq!(stats.total_started, 1);
        assert_eq!(stats.active_count, 1);
    }

    #[tokio::test]
    async fn test_coordinator_commit() {
        let coordinator = TransactionCoordinator::new(TransactionConfig::default());

        let txn_id = coordinator.begin(None).await.unwrap();

        // Single store - no 2PC needed
        coordinator
            .involve_store(&txn_id, ParticipantType::Vector)
            .await
            .unwrap();
        coordinator
            .record_write(&txn_id, "vector", "vec1")
            .await
            .unwrap();

        coordinator.commit(&txn_id).await.unwrap();

        let state = coordinator.get_state(&txn_id).await;
        assert_eq!(state, Some(TransactionState::Committed));

        let stats = coordinator.stats().await;
        assert_eq!(stats.total_committed, 1);
    }

    #[tokio::test]
    async fn test_coordinator_rollback() {
        let coordinator = TransactionCoordinator::new(TransactionConfig::default());

        let txn_id = coordinator.begin(None).await.unwrap();
        coordinator
            .involve_store(&txn_id, ParticipantType::Vector)
            .await
            .unwrap();

        coordinator.rollback(&txn_id).await.unwrap();

        let state = coordinator.get_state(&txn_id).await;
        assert_eq!(state, Some(TransactionState::Aborted));

        let stats = coordinator.stats().await;
        assert_eq!(stats.total_aborted, 1);
    }

    #[tokio::test]
    async fn test_coordinator_distributed_detection() {
        let config = TransactionConfig {
            auto_2pc: false, // Disable auto 2PC for this test
            ..Default::default()
        };
        let coordinator = TransactionCoordinator::new(config);

        let txn_id = coordinator.begin(None).await.unwrap();

        coordinator
            .involve_store(&txn_id, ParticipantType::Vector)
            .await
            .unwrap();
        assert!(!coordinator.is_distributed(&txn_id).await);

        coordinator
            .involve_store(&txn_id, ParticipantType::Document)
            .await
            .unwrap();
        assert!(coordinator.is_distributed(&txn_id).await);
    }

    #[tokio::test]
    async fn test_coordinator_max_concurrent() {
        let config = TransactionConfig {
            max_concurrent: 2,
            ..Default::default()
        };
        let coordinator = TransactionCoordinator::new(config);

        // First two should succeed
        coordinator.begin(None).await.unwrap();
        coordinator.begin(None).await.unwrap();

        // Third should fail
        let result = coordinator.begin(None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_coordinator_isolation_levels() {
        let coordinator = TransactionCoordinator::new(TransactionConfig::default());

        let _txn1 = coordinator
            .begin(Some(IsolationLevel::ReadCommitted))
            .await
            .unwrap();
        let _txn2 = coordinator
            .begin(Some(IsolationLevel::Serializable))
            .await
            .unwrap();

        let stats = coordinator.stats().await;
        assert_eq!(
            stats.by_isolation.get(&IsolationLevel::ReadCommitted),
            Some(&1)
        );
        assert_eq!(
            stats.by_isolation.get(&IsolationLevel::Serializable),
            Some(&1)
        );
    }

    #[tokio::test]
    async fn test_coordinator_cleanup() {
        let coordinator = TransactionCoordinator::new(TransactionConfig::default());

        let txn_id = coordinator.begin(None).await.unwrap();
        coordinator.commit(&txn_id).await.unwrap();

        // Should still be there (within max_age)
        assert!(coordinator.get_state(&txn_id).await.is_some());

        // Cleanup with very short max_age
        coordinator.cleanup_completed(Duration::from_nanos(1)).await;

        // Should be gone now
        assert!(coordinator.get_state(&txn_id).await.is_none());
    }
}
