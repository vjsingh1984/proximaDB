//! # Two-Phase Commit (2PC) Protocol
//!
//! Implements distributed transaction coordination using 2PC for
//! cross-model transactions involving multiple storage engines.
//!
//! ## Protocol Flow
//!
//! ```text
//! Coordinator                 Participants (Vector, Document, Graph, RDBMS)
//!     │                                    │
//!     │── PREPARE ────────────────────────>│
//!     │                                    │
//!     │<─ VOTE (YES/NO) ──────────────────│
//!     │                                    │
//!     │   [If all YES]                     │
//!     │── COMMIT ─────────────────────────>│
//!     │                                    │
//!     │   [If any NO]                      │
//!     │── ABORT ──────────────────────────>│
//!     │                                    │
//!     │<─ ACK ────────────────────────────│
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Result};
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// Transaction state in 2PC
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionState {
    /// Transaction started, not yet prepared
    Active,
    /// Prepare phase initiated
    Preparing,
    /// All participants voted YES
    Prepared,
    /// Commit phase initiated
    Committing,
    /// Transaction committed
    Committed,
    /// Abort phase initiated
    Aborting,
    /// Transaction aborted
    Aborted,
    /// Unknown state (for recovery)
    Unknown,
}

impl TransactionState {
    /// Check if transaction is in a terminal state
    pub fn is_terminal(&self) -> bool {
        matches!(self, TransactionState::Committed | TransactionState::Aborted)
    }

    /// Check if transaction can be aborted
    pub fn can_abort(&self) -> bool {
        matches!(
            self,
            TransactionState::Active
                | TransactionState::Preparing
                | TransactionState::Prepared
                | TransactionState::Aborting
        )
    }
}

/// Result of prepare phase
#[derive(Debug, Clone)]
pub enum PrepareResult {
    /// Participant votes YES
    Yes,
    /// Participant votes NO with reason
    No(String),
    /// Participant timed out
    Timeout,
    /// Error during prepare
    Error(String),
}

impl PrepareResult {
    /// Check if the result is a YES vote
    pub fn is_yes(&self) -> bool {
        matches!(self, PrepareResult::Yes)
    }
}

/// Result of commit/abort phase
#[derive(Debug, Clone)]
pub enum CommitResult {
    /// Operation successful
    Success,
    /// Operation failed with reason
    Failed(String),
    /// Operation timed out
    Timeout,
}

impl CommitResult {
    /// Check if the result is success
    pub fn is_success(&self) -> bool {
        matches!(self, CommitResult::Success)
    }
}

/// Participant type in 2PC
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParticipantType {
    Vector,
    Document,
    Graph,
    RDBMS,
    Observability,
}

impl ParticipantType {
    /// Get display name
    pub fn name(&self) -> &'static str {
        match self {
            ParticipantType::Vector => "vector",
            ParticipantType::Document => "document",
            ParticipantType::Graph => "graph",
            ParticipantType::RDBMS => "rdbms",
            ParticipantType::Observability => "observability",
        }
    }
}

/// Participant state in a transaction
#[derive(Debug, Clone)]
pub struct ParticipantState {
    /// Participant type
    pub participant_type: ParticipantType,
    /// Current state
    pub state: TransactionState,
    /// Prepare result (if prepared)
    pub prepare_result: Option<PrepareResult>,
    /// Commit result (if committed/aborted)
    pub commit_result: Option<CommitResult>,
    /// Time of last state change
    pub last_updated: Instant,
}

impl ParticipantState {
    /// Create new participant state
    pub fn new(participant_type: ParticipantType) -> Self {
        Self {
            participant_type,
            state: TransactionState::Active,
            prepare_result: None,
            commit_result: None,
            last_updated: Instant::now(),
        }
    }
}

/// 2PC transaction record
#[derive(Debug)]
pub struct TwoPhaseTransaction {
    /// Transaction ID
    pub transaction_id: String,
    /// Global state
    pub state: TransactionState,
    /// Participant states
    pub participants: HashMap<ParticipantType, ParticipantState>,
    /// Transaction start time
    pub start_time: Instant,
    /// Prepare deadline
    pub prepare_timeout: Duration,
    /// Commit deadline
    pub commit_timeout: Duration,
    /// Transaction log for recovery
    pub log: Vec<(Instant, String)>,
}

impl TwoPhaseTransaction {
    /// Create a new 2PC transaction
    pub fn new(transaction_id: String, prepare_timeout: Duration, commit_timeout: Duration) -> Self {
        Self {
            transaction_id,
            state: TransactionState::Active,
            participants: HashMap::new(),
            start_time: Instant::now(),
            prepare_timeout,
            commit_timeout,
            log: vec![(Instant::now(), "Transaction created".to_string())],
        }
    }

    /// Add a participant
    pub fn add_participant(&mut self, participant_type: ParticipantType) {
        if !self.participants.contains_key(&participant_type) {
            self.participants.insert(
                participant_type,
                ParticipantState::new(participant_type),
            );
            self.log(format!("Added participant: {:?}", participant_type));
        }
    }

    /// Log an event
    pub fn log(&mut self, message: String) {
        self.log.push((Instant::now(), message));
    }

    /// Check if all participants voted YES
    pub fn all_prepared(&self) -> bool {
        !self.participants.is_empty()
            && self.participants.values().all(|p| {
                p.prepare_result.as_ref().is_some_and(|r| r.is_yes())
            })
    }

    /// Check if any participant voted NO
    pub fn any_rejected(&self) -> bool {
        self.participants.values().any(|p| {
            matches!(
                &p.prepare_result,
                Some(PrepareResult::No(_)) | Some(PrepareResult::Error(_)) | Some(PrepareResult::Timeout)
            )
        })
    }

    /// Get participant types
    pub fn participant_types(&self) -> Vec<ParticipantType> {
        self.participants.keys().cloned().collect()
    }

    /// Check if timed out during prepare
    pub fn is_prepare_timeout(&self) -> bool {
        self.start_time.elapsed() > self.prepare_timeout
    }

    /// Check if timed out during commit
    pub fn is_commit_timeout(&self) -> bool {
        self.start_time.elapsed() > self.prepare_timeout + self.commit_timeout
    }
}

/// Configuration for 2PC protocol
#[derive(Debug, Clone)]
pub struct TwoPhaseCommitConfig {
    /// Timeout for prepare phase
    pub prepare_timeout: Duration,
    /// Timeout for commit phase
    pub commit_timeout: Duration,
    /// Maximum retries for commit/abort
    pub max_retries: usize,
    /// Delay between retries
    pub retry_delay: Duration,
    /// Enable write-ahead logging for recovery
    pub wal_enabled: bool,
}

impl Default for TwoPhaseCommitConfig {
    fn default() -> Self {
        Self {
            prepare_timeout: Duration::from_secs(30),
            commit_timeout: Duration::from_secs(60),
            max_retries: 3,
            retry_delay: Duration::from_millis(100),
            wal_enabled: true,
        }
    }
}

/// Trait for 2PC participants
#[async_trait::async_trait]
pub trait TwoPhaseParticipant: Send + Sync {
    /// Prepare phase - vote YES or NO
    async fn prepare(&self, transaction_id: &str) -> PrepareResult;

    /// Commit phase - commit the transaction
    async fn commit(&self, transaction_id: &str) -> CommitResult;

    /// Abort phase - rollback the transaction
    async fn abort(&self, transaction_id: &str) -> CommitResult;

    /// Get participant type
    fn participant_type(&self) -> ParticipantType;
}

/// Two-Phase Commit Protocol coordinator
pub struct TwoPhaseCommitProtocol {
    /// Configuration
    config: TwoPhaseCommitConfig,
    /// Active transactions
    transactions: RwLock<HashMap<String, TwoPhaseTransaction>>,
    /// Registered participants
    participants: RwLock<HashMap<ParticipantType, Arc<dyn TwoPhaseParticipant>>>,
    /// Statistics
    stats: RwLock<TwoPhaseCommitStats>,
}

/// Statistics for 2PC operations
#[derive(Debug, Clone, Default)]
pub struct TwoPhaseCommitStats {
    /// Total transactions started
    pub total_started: u64,
    /// Total transactions committed
    pub total_committed: u64,
    /// Total transactions aborted
    pub total_aborted: u64,
    /// Total prepare timeouts
    pub prepare_timeouts: u64,
    /// Total commit timeouts
    pub commit_timeouts: u64,
    /// Average prepare time in milliseconds
    pub avg_prepare_time_ms: f64,
    /// Average commit time in milliseconds
    pub avg_commit_time_ms: f64,
}

impl TwoPhaseCommitProtocol {
    /// Create a new 2PC protocol coordinator
    pub fn new(config: TwoPhaseCommitConfig) -> Self {
        Self {
            config,
            transactions: RwLock::new(HashMap::new()),
            participants: RwLock::new(HashMap::new()),
            stats: RwLock::new(TwoPhaseCommitStats::default()),
        }
    }

    /// Register a participant
    pub async fn register_participant(&self, participant: Arc<dyn TwoPhaseParticipant>) {
        let mut participants = self.participants.write().await;
        participants.insert(participant.participant_type(), participant);
    }

    /// Begin a new 2PC transaction
    pub async fn begin(&self, transaction_id: &str) -> Result<()> {
        let transaction = TwoPhaseTransaction::new(
            transaction_id.to_string(),
            self.config.prepare_timeout,
            self.config.commit_timeout,
        );

        let mut transactions = self.transactions.write().await;
        if transactions.contains_key(transaction_id) {
            return Err(anyhow!("Transaction {} already exists", transaction_id));
        }
        transactions.insert(transaction_id.to_string(), transaction);

        {
            let mut stats = self.stats.write().await;
            stats.total_started += 1;
        }

        debug!("2PC transaction {} started", transaction_id);
        Ok(())
    }

    /// Enlist a participant in the transaction
    pub async fn enlist(
        &self,
        transaction_id: &str,
        participant_type: ParticipantType,
    ) -> Result<()> {
        let mut transactions = self.transactions.write().await;
        let transaction = transactions
            .get_mut(transaction_id)
            .ok_or_else(|| anyhow!("Transaction {} not found", transaction_id))?;

        transaction.add_participant(participant_type);
        Ok(())
    }

    /// Execute prepare phase
    pub async fn prepare(&self, transaction_id: &str) -> Result<bool> {
        // Update state to preparing
        {
            let mut transactions = self.transactions.write().await;
            if let Some(txn) = transactions.get_mut(transaction_id) {
                txn.state = TransactionState::Preparing;
                txn.log("Starting prepare phase".to_string());
            }
        }

        let participant_types = {
            let transactions = self.transactions.read().await;
            transactions
                .get(transaction_id)
                .map(|t| t.participant_types())
                .unwrap_or_default()
        };

        let participants = self.participants.read().await;

        // Send prepare to all participants
        let mut all_yes = true;
        for ptype in participant_types {
            if let Some(participant) = participants.get(&ptype) {
                let result = participant.prepare(transaction_id).await;
                let is_yes = result.is_yes();

                // Update participant state
                {
                    let mut transactions = self.transactions.write().await;
                    if let Some(txn) = transactions.get_mut(transaction_id) {
                        if let Some(pstate) = txn.participants.get_mut(&ptype) {
                            pstate.prepare_result = Some(result.clone());
                            pstate.last_updated = Instant::now();
                        }
                        txn.log(format!("Participant {:?} voted {:?}", ptype, result));
                    }
                }

                if !is_yes {
                    all_yes = false;
                    warn!("Participant {:?} voted NO for transaction {}", ptype, transaction_id);
                }
            } else {
                all_yes = false;
                error!("Participant {:?} not registered", ptype);
            }
        }

        // Update global state
        {
            let mut transactions = self.transactions.write().await;
            if let Some(txn) = transactions.get_mut(transaction_id) {
                txn.state = if all_yes {
                    TransactionState::Prepared
                } else {
                    TransactionState::Aborting
                };
            }
        }

        debug!(
            "2PC transaction {} prepare phase complete: all_yes={}",
            transaction_id, all_yes
        );
        Ok(all_yes)
    }

    /// Execute commit phase
    pub async fn commit(&self, transaction_id: &str) -> Result<()> {
        // Update state to committing
        {
            let mut transactions = self.transactions.write().await;
            if let Some(txn) = transactions.get_mut(transaction_id) {
                if txn.state != TransactionState::Prepared {
                    return Err(anyhow!(
                        "Cannot commit transaction {} in state {:?}",
                        transaction_id,
                        txn.state
                    ));
                }
                txn.state = TransactionState::Committing;
                txn.log("Starting commit phase".to_string());
            }
        }

        let participant_types = {
            let transactions = self.transactions.read().await;
            transactions
                .get(transaction_id)
                .map(|t| t.participant_types())
                .unwrap_or_default()
        };

        let participants = self.participants.read().await;

        // Send commit to all participants
        let mut all_success = true;
        for ptype in participant_types {
            if let Some(participant) = participants.get(&ptype) {
                let mut result = CommitResult::Failed("Not attempted".to_string());

                // Retry commit
                for attempt in 0..=self.config.max_retries {
                    result = participant.commit(transaction_id).await;
                    if result.is_success() {
                        break;
                    }
                    if attempt < self.config.max_retries {
                        tokio::time::sleep(self.config.retry_delay).await;
                    }
                }

                // Update participant state
                {
                    let mut transactions = self.transactions.write().await;
                    if let Some(txn) = transactions.get_mut(transaction_id) {
                        if let Some(pstate) = txn.participants.get_mut(&ptype) {
                            pstate.commit_result = Some(result.clone());
                            pstate.last_updated = Instant::now();
                        }
                        txn.log(format!("Participant {:?} commit: {:?}", ptype, result));
                    }
                }

                if !result.is_success() {
                    all_success = false;
                    error!(
                        "Participant {:?} failed to commit transaction {}: {:?}",
                        ptype, transaction_id, result
                    );
                }
            }
        }

        // Update global state
        {
            let mut transactions = self.transactions.write().await;
            if let Some(txn) = transactions.get_mut(transaction_id) {
                txn.state = TransactionState::Committed;
            }
        }

        {
            let mut stats = self.stats.write().await;
            stats.total_committed += 1;
        }

        if !all_success {
            return Err(anyhow!(
                "Some participants failed to commit transaction {}",
                transaction_id
            ));
        }

        info!("2PC transaction {} committed", transaction_id);
        Ok(())
    }

    /// Execute abort phase
    pub async fn abort(&self, transaction_id: &str) -> Result<()> {
        // Update state to aborting
        {
            let mut transactions = self.transactions.write().await;
            if let Some(txn) = transactions.get_mut(transaction_id) {
                if !txn.state.can_abort() {
                    return Err(anyhow!(
                        "Cannot abort transaction {} in state {:?}",
                        transaction_id,
                        txn.state
                    ));
                }
                txn.state = TransactionState::Aborting;
                txn.log("Starting abort phase".to_string());
            }
        }

        let participant_types = {
            let transactions = self.transactions.read().await;
            transactions
                .get(transaction_id)
                .map(|t| t.participant_types())
                .unwrap_or_default()
        };

        let participants = self.participants.read().await;

        // Send abort to all participants
        for ptype in participant_types {
            if let Some(participant) = participants.get(&ptype) {
                let mut result = CommitResult::Failed("Not attempted".to_string());

                // Retry abort
                for attempt in 0..=self.config.max_retries {
                    result = participant.abort(transaction_id).await;
                    if result.is_success() {
                        break;
                    }
                    if attempt < self.config.max_retries {
                        tokio::time::sleep(self.config.retry_delay).await;
                    }
                }

                // Update participant state
                {
                    let mut transactions = self.transactions.write().await;
                    if let Some(txn) = transactions.get_mut(transaction_id) {
                        if let Some(pstate) = txn.participants.get_mut(&ptype) {
                            pstate.commit_result = Some(result.clone());
                            pstate.last_updated = Instant::now();
                        }
                        txn.log(format!("Participant {:?} abort: {:?}", ptype, result));
                    }
                }
            }
        }

        // Update global state
        {
            let mut transactions = self.transactions.write().await;
            if let Some(txn) = transactions.get_mut(transaction_id) {
                txn.state = TransactionState::Aborted;
            }
        }

        {
            let mut stats = self.stats.write().await;
            stats.total_aborted += 1;
        }

        info!("2PC transaction {} aborted", transaction_id);
        Ok(())
    }

    /// Get transaction state
    pub async fn get_state(&self, transaction_id: &str) -> Option<TransactionState> {
        let transactions = self.transactions.read().await;
        transactions.get(transaction_id).map(|t| t.state)
    }

    /// Get statistics
    pub async fn stats(&self) -> TwoPhaseCommitStats {
        let stats = self.stats.read().await;
        stats.clone()
    }

    /// Cleanup completed transactions
    pub async fn cleanup_completed(&self, max_age: Duration) {
        let mut transactions = self.transactions.write().await;
        transactions.retain(|_, txn| {
            !txn.state.is_terminal() || txn.start_time.elapsed() < max_age
        });
    }

    /// Get configuration
    pub fn config(&self) -> &TwoPhaseCommitConfig {
        &self.config
    }
}

impl Default for TwoPhaseCommitProtocol {
    fn default() -> Self {
        Self::new(TwoPhaseCommitConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transaction_state_terminal() {
        assert!(!TransactionState::Active.is_terminal());
        assert!(!TransactionState::Preparing.is_terminal());
        assert!(TransactionState::Committed.is_terminal());
        assert!(TransactionState::Aborted.is_terminal());
    }

    #[test]
    fn test_transaction_state_can_abort() {
        assert!(TransactionState::Active.can_abort());
        assert!(TransactionState::Preparing.can_abort());
        assert!(TransactionState::Prepared.can_abort());
        assert!(!TransactionState::Committed.can_abort());
    }

    #[test]
    fn test_prepare_result() {
        assert!(PrepareResult::Yes.is_yes());
        assert!(!PrepareResult::No("reason".to_string()).is_yes());
        assert!(!PrepareResult::Timeout.is_yes());
    }

    #[test]
    fn test_commit_result() {
        assert!(CommitResult::Success.is_success());
        assert!(!CommitResult::Failed("reason".to_string()).is_success());
        assert!(!CommitResult::Timeout.is_success());
    }

    #[test]
    fn test_two_phase_transaction_participants() {
        let mut txn = TwoPhaseTransaction::new(
            "txn1".to_string(),
            Duration::from_secs(30),
            Duration::from_secs(60),
        );

        txn.add_participant(ParticipantType::Vector);
        txn.add_participant(ParticipantType::Document);

        assert_eq!(txn.participants.len(), 2);
        assert!(!txn.all_prepared());
    }

    #[test]
    fn test_two_phase_transaction_voting() {
        let mut txn = TwoPhaseTransaction::new(
            "txn1".to_string(),
            Duration::from_secs(30),
            Duration::from_secs(60),
        );

        txn.add_participant(ParticipantType::Vector);
        txn.add_participant(ParticipantType::Document);

        // Simulate YES votes
        txn.participants.get_mut(&ParticipantType::Vector).unwrap().prepare_result =
            Some(PrepareResult::Yes);
        txn.participants.get_mut(&ParticipantType::Document).unwrap().prepare_result =
            Some(PrepareResult::Yes);

        assert!(txn.all_prepared());
        assert!(!txn.any_rejected());

        // Simulate a NO vote
        txn.participants.get_mut(&ParticipantType::Vector).unwrap().prepare_result =
            Some(PrepareResult::No("Resource conflict".to_string()));

        assert!(!txn.all_prepared());
        assert!(txn.any_rejected());
    }

    #[test]
    fn test_config_default() {
        let config = TwoPhaseCommitConfig::default();
        assert_eq!(config.prepare_timeout, Duration::from_secs(30));
        assert_eq!(config.commit_timeout, Duration::from_secs(60));
        assert_eq!(config.max_retries, 3);
    }

    #[tokio::test]
    async fn test_protocol_begin() {
        let protocol = TwoPhaseCommitProtocol::new(TwoPhaseCommitConfig::default());

        protocol.begin("txn1").await.unwrap();

        let state = protocol.get_state("txn1").await;
        assert_eq!(state, Some(TransactionState::Active));

        // Duplicate should fail
        let result = protocol.begin("txn1").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_protocol_enlist() {
        let protocol = TwoPhaseCommitProtocol::new(TwoPhaseCommitConfig::default());

        protocol.begin("txn1").await.unwrap();
        protocol.enlist("txn1", ParticipantType::Vector).await.unwrap();
        protocol.enlist("txn1", ParticipantType::Document).await.unwrap();

        // Enlist to non-existent transaction should fail
        let result = protocol.enlist("txn2", ParticipantType::Graph).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_stats() {
        let protocol = TwoPhaseCommitProtocol::new(TwoPhaseCommitConfig::default());

        protocol.begin("txn1").await.unwrap();

        let stats = protocol.stats().await;
        assert_eq!(stats.total_started, 1);
        assert_eq!(stats.total_committed, 0);
        assert_eq!(stats.total_aborted, 0);
    }
}
