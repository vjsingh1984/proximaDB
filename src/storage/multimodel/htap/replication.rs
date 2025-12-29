//! # HTAP Replication Coordinator
//!
//! Manages async replication from OLTP (row store) to OLAP (column store).
//! Uses Change Data Capture (CDC) patterns with LSN tracking.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use tokio::sync::{mpsc, RwLock, Notify};
use tokio::time::interval;
use tracing::{debug, info, warn, error};

/// Configuration for HTAP replication
#[derive(Debug, Clone)]
pub struct ReplicationConfig {
    /// Batch size for replication
    pub batch_size: usize,
    /// Replication interval in milliseconds
    pub replication_interval_ms: u64,
    /// Maximum acceptable lag in milliseconds
    pub max_lag_ms: u64,
    /// Enable parallel replication across tables
    pub parallel_replication: bool,
    /// Number of parallel replication workers
    pub parallel_workers: usize,
    /// Enable compression for replication
    pub compress_batches: bool,
}

impl Default for ReplicationConfig {
    fn default() -> Self {
        Self {
            batch_size: 1000,
            replication_interval_ms: 100, // 100ms
            max_lag_ms: 5000, // 5 seconds
            parallel_replication: true,
            parallel_workers: 4,
            compress_batches: true,
        }
    }
}

/// Statistics for replication monitoring
#[derive(Debug, Default, Clone)]
pub struct ReplicationStats {
    /// Total rows replicated
    pub rows_replicated: u64,
    /// Total batches processed
    pub batches_processed: u64,
    /// Current replication lag in milliseconds
    pub current_lag_ms: u64,
    /// Last successful replication timestamp
    pub last_success_ms: i64,
    /// Number of failures
    pub failures: u64,
    /// Average batch processing time in ms
    pub avg_batch_time_ms: f64,
    /// Per-table replication status
    pub table_status: HashMap<String, TableReplicationStatus>,
}

/// Per-table replication status
#[derive(Debug, Clone)]
pub struct TableReplicationStatus {
    /// Table name
    pub table_name: String,
    /// Last replicated LSN
    pub last_lsn: u64,
    /// Rows replicated for this table
    pub rows_replicated: u64,
    /// Current lag in milliseconds
    pub lag_ms: u64,
    /// Is replication healthy
    pub is_healthy: bool,
}

/// Change record for replication
#[derive(Debug, Clone)]
pub struct ChangeRecord {
    /// Log Sequence Number
    pub lsn: u64,
    /// Table/collection name
    pub table: String,
    /// Operation type
    pub operation: ChangeOperation,
    /// Record data (serialized)
    pub data: Vec<u8>,
    /// Timestamp of the change
    pub timestamp_ms: i64,
}

/// Change operation type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeOperation {
    Insert,
    Update,
    Delete,
}

/// Replication coordinator manages OLTP to OLAP replication
pub struct ReplicationCoordinator {
    /// Configuration
    config: ReplicationConfig,

    /// Current OLTP LSN (source of truth)
    oltp_lsn: AtomicU64,

    /// Current OLAP LSN (replicated up to)
    olap_lsn: AtomicU64,

    /// Is replication running
    is_running: AtomicBool,

    /// Shutdown notification
    shutdown: Arc<Notify>,

    /// Per-table LSN tracking
    table_lsns: RwLock<HashMap<String, u64>>,

    /// Statistics
    stats: RwLock<ReplicationStats>,

    /// Pending changes channel
    pending_tx: Option<mpsc::Sender<ChangeRecord>>,

    /// Last batch processing time
    last_batch_time_ms: AtomicI64,
}

impl ReplicationCoordinator {
    /// Create a new replication coordinator
    pub fn new(config: ReplicationConfig) -> Self {
        Self {
            config,
            oltp_lsn: AtomicU64::new(0),
            olap_lsn: AtomicU64::new(0),
            is_running: AtomicBool::new(false),
            shutdown: Arc::new(Notify::new()),
            table_lsns: RwLock::new(HashMap::new()),
            stats: RwLock::new(ReplicationStats::default()),
            pending_tx: None,
            last_batch_time_ms: AtomicI64::new(0),
        }
    }

    /// Get current replication lag in milliseconds
    pub fn lag_ms(&self) -> u64 {
        let oltp = self.oltp_lsn.load(Ordering::Relaxed);
        let olap = self.olap_lsn.load(Ordering::Relaxed);

        if oltp <= olap {
            return 0;
        }

        // Estimate lag based on LSN difference and processing rate
        let lsn_diff = oltp - olap;
        let batch_time = self.last_batch_time_ms.load(Ordering::Relaxed).max(1) as u64;
        let batches_behind = lsn_diff / self.config.batch_size.max(1) as u64;

        batches_behind * batch_time
    }

    /// Check if replication is healthy (lag within tolerance)
    pub fn is_healthy(&self) -> bool {
        self.lag_ms() <= self.config.max_lag_ms
    }

    /// Get the current OLTP LSN
    pub fn oltp_lsn(&self) -> u64 {
        self.oltp_lsn.load(Ordering::Relaxed)
    }

    /// Get the current OLAP LSN
    pub fn olap_lsn(&self) -> u64 {
        self.olap_lsn.load(Ordering::Relaxed)
    }

    /// Record a new change (called by OLTP after write)
    pub async fn record_change(&self, change: ChangeRecord) -> Result<()> {
        // Update OLTP LSN
        self.oltp_lsn.store(change.lsn, Ordering::Release);

        // Track per-table LSN
        {
            let mut table_lsns = self.table_lsns.write().await;
            table_lsns.insert(change.table.clone(), change.lsn);
        }

        // Queue for replication if running
        if let Some(tx) = &self.pending_tx {
            tx.send(change).await.map_err(|e| anyhow!("Failed to queue change: {}", e))?;
        }

        Ok(())
    }

    /// Advance OLTP LSN without recording a change (for LSN-only updates)
    pub fn advance_oltp_lsn(&self, lsn: u64) {
        self.oltp_lsn.fetch_max(lsn, Ordering::Release);
    }

    /// Mark batch as replicated (called after successful OLAP write)
    pub async fn mark_replicated(&self, lsn: u64, rows: u64) -> Result<()> {
        // Update OLAP LSN
        self.olap_lsn.fetch_max(lsn, Ordering::Release);

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.rows_replicated += rows;
            stats.batches_processed += 1;
            stats.current_lag_ms = self.lag_ms();
            stats.last_success_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
        }

        debug!("Replicated batch up to LSN {} ({} rows)", lsn, rows);
        Ok(())
    }

    /// Record a replication failure
    pub async fn record_failure(&self, error: &str) {
        warn!("Replication failure: {}", error);
        let mut stats = self.stats.write().await;
        stats.failures += 1;
    }

    /// Get replication statistics
    pub async fn stats(&self) -> ReplicationStats {
        let stats = self.stats.read().await;
        let mut result = stats.clone();
        result.current_lag_ms = self.lag_ms();
        result
    }

    /// Get per-table replication status
    pub async fn table_status(&self, table: &str) -> Option<TableReplicationStatus> {
        let table_lsns = self.table_lsns.read().await;

        table_lsns.get(table).map(|&lsn| {
            TableReplicationStatus {
                table_name: table.to_string(),
                last_lsn: lsn,
                rows_replicated: 0, // Would need per-table tracking
                lag_ms: self.lag_ms(),
                is_healthy: self.is_healthy(),
            }
        })
    }

    /// Check if we should use OLAP for a query on a table
    pub async fn can_use_olap(&self, table: &str, require_fresh: bool) -> bool {
        if !self.is_healthy() && require_fresh {
            return false;
        }

        // Check if table has been replicated at all
        let table_lsns = self.table_lsns.read().await;
        table_lsns.get(table).map(|&lsn| lsn > 0).unwrap_or(false)
    }

    /// Wait for OLAP to catch up to a specific LSN
    pub async fn wait_for_lsn(&self, target_lsn: u64, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;

        while Instant::now() < deadline {
            if self.olap_lsn() >= target_lsn {
                return Ok(true);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        Ok(false)
    }

    /// Get configuration
    pub fn config(&self) -> &ReplicationConfig {
        &self.config
    }

    /// Check if replication is running
    pub fn is_running(&self) -> bool {
        self.is_running.load(Ordering::Relaxed)
    }

    /// Stop replication
    pub fn stop(&self) {
        self.is_running.store(false, Ordering::Release);
        self.shutdown.notify_waiters();
        info!("Replication coordinator stopped");
    }
}

impl Default for ReplicationCoordinator {
    fn default() -> Self {
        Self::new(ReplicationConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replication_config_default() {
        let config = ReplicationConfig::default();
        assert_eq!(config.batch_size, 1000);
        assert_eq!(config.replication_interval_ms, 100);
        assert!(config.parallel_replication);
    }

    #[tokio::test]
    async fn test_coordinator_creation() {
        let coordinator = ReplicationCoordinator::new(ReplicationConfig::default());
        assert_eq!(coordinator.oltp_lsn(), 0);
        assert_eq!(coordinator.olap_lsn(), 0);
        assert!(coordinator.is_healthy());
    }

    #[tokio::test]
    async fn test_record_change() {
        let coordinator = ReplicationCoordinator::new(ReplicationConfig::default());

        let change = ChangeRecord {
            lsn: 100,
            table: "users".to_string(),
            operation: ChangeOperation::Insert,
            data: vec![1, 2, 3],
            timestamp_ms: 12345,
        };

        coordinator.record_change(change).await.unwrap();
        assert_eq!(coordinator.oltp_lsn(), 100);
    }

    #[tokio::test]
    async fn test_mark_replicated() {
        let coordinator = ReplicationCoordinator::new(ReplicationConfig::default());

        // Record a change
        coordinator.advance_oltp_lsn(100);

        // Mark as replicated
        coordinator.mark_replicated(100, 50).await.unwrap();

        let stats = coordinator.stats().await;
        assert_eq!(stats.rows_replicated, 50);
        assert_eq!(stats.batches_processed, 1);
        assert_eq!(coordinator.olap_lsn(), 100);
    }

    #[tokio::test]
    async fn test_lag_calculation() {
        let coordinator = ReplicationCoordinator::new(ReplicationConfig::default());

        // No lag when caught up
        assert_eq!(coordinator.lag_ms(), 0);

        // Advance OLTP ahead
        coordinator.advance_oltp_lsn(10000);

        // Should show lag
        assert!(coordinator.oltp_lsn() > coordinator.olap_lsn());
    }

    #[tokio::test]
    async fn test_can_use_olap() {
        let coordinator = ReplicationCoordinator::new(ReplicationConfig::default());

        // Initially can't use OLAP (no data replicated)
        assert!(!coordinator.can_use_olap("users", true).await);

        // Record and replicate a change
        let change = ChangeRecord {
            lsn: 1,
            table: "users".to_string(),
            operation: ChangeOperation::Insert,
            data: vec![],
            timestamp_ms: 0,
        };
        coordinator.record_change(change).await.unwrap();
        coordinator.mark_replicated(1, 1).await.unwrap();

        // Now can use OLAP
        assert!(coordinator.can_use_olap("users", true).await);
    }
}
