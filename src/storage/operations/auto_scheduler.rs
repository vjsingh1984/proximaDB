//! Automatic Operations Scheduler
//!
//! Proactively schedules and coordinates storage operations like compaction,
//! flush, and optimization based on workload patterns, resource availability,
//! and configured policies.
//!
//! # Architecture
//!
//! The AutoScheduler runs as a background service that:
//! 1. Monitors system metrics (disk usage, memory pressure, I/O load)
//! 2. Analyzes workload patterns to identify optimal operation windows
//! 3. Schedules operations with priority-based queuing
//! 4. Coordinates with existing coordinators (Compaction, Flush)
//! 5. Provides adaptive policies based on system state
//!
//! # Usage
//!
//! ```ignore
//! let scheduler = AutoScheduler::new(config, compaction_coordinator, flush_coordinator);
//! scheduler.start().await?;
//!
//! // Scheduler runs in background, automatically triggering operations
//! // Can be paused/resumed for maintenance windows
//! scheduler.pause().await;
//! scheduler.resume().await;
//!
//! // Graceful shutdown
//! scheduler.stop().await?;
//! ```

use anyhow::Result;
use chrono::{DateTime, Duration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, broadcast, mpsc};
use tokio::time::interval;
use tracing::{debug, info, warn};

/// Type of scheduled operation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OperationType {
    /// Compaction operation
    Compaction,
    /// Flush from memtable to storage
    Flush,
    /// Index optimization (AXIS updates)
    IndexOptimization,
    /// Statistics collection
    StatsCollection,
    /// Backup operation
    Backup,
    /// Health check
    HealthCheck,
}

impl std::fmt::Display for OperationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OperationType::Compaction => write!(f, "compaction"),
            OperationType::Flush => write!(f, "flush"),
            OperationType::IndexOptimization => write!(f, "index_optimization"),
            OperationType::StatsCollection => write!(f, "stats_collection"),
            OperationType::Backup => write!(f, "backup"),
            OperationType::HealthCheck => write!(f, "health_check"),
        }
    }
}

/// Priority level for scheduled operations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OperationPriority {
    /// Critical - must run immediately
    Critical = 0,
    /// High - run as soon as possible
    High = 1,
    /// Normal - run when convenient
    Normal = 2,
    /// Low - run during idle periods
    Low = 3,
    /// Background - run only when system is idle
    Background = 4,
}

impl Ord for OperationPriority {
    fn cmp(&self, other: &Self) -> Ordering {
        // Lower number = higher priority
        (*other as u8).cmp(&(*self as u8))
    }
}

impl PartialOrd for OperationPriority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Status of a scheduled operation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationStatus {
    /// Waiting to be executed
    Pending,
    /// Currently running
    Running,
    /// Completed successfully
    Completed,
    /// Failed
    Failed,
    /// Canceled
    Canceled,
    /// Skipped (conditions not met)
    Skipped,
}

/// A scheduled operation
#[derive(Debug, Clone)]
pub struct ScheduledOperation {
    /// Unique operation ID
    pub id: String,
    /// Type of operation
    pub operation_type: OperationType,
    /// Priority
    pub priority: OperationPriority,
    /// Target collection (if applicable)
    pub collection_id: Option<String>,
    /// When this operation was scheduled
    pub scheduled_at: DateTime<Utc>,
    /// When this operation should run (earliest)
    pub run_after: DateTime<Utc>,
    /// Deadline (if any)
    pub deadline: Option<DateTime<Utc>>,
    /// Current status
    pub status: OperationStatus,
    /// Number of retry attempts
    pub retry_count: u32,
    /// Maximum retries allowed
    pub max_retries: u32,
    /// Additional context/parameters
    pub context: HashMap<String, String>,
}

impl Eq for ScheduledOperation {}

impl PartialEq for ScheduledOperation {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Ord for ScheduledOperation {
    fn cmp(&self, other: &Self) -> Ordering {
        // First by priority (higher priority first)
        match self.priority.cmp(&other.priority) {
            Ordering::Equal => {
                // Then by run_after time (earlier first)
                other.run_after.cmp(&self.run_after)
            }
            other_order => other_order,
        }
    }
}

impl PartialOrd for ScheduledOperation {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Configuration for the auto scheduler
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoSchedulerConfig {
    /// Enable the scheduler
    pub enabled: bool,
    /// Check interval in seconds
    pub check_interval_secs: u64,
    /// Maximum concurrent operations
    pub max_concurrent_operations: usize,
    /// Maximum queue size
    pub max_queue_size: usize,
    /// Compaction scheduling policy
    pub compaction_policy: CompactionPolicy,
    /// Flush scheduling policy
    pub flush_policy: FlushPolicy,
    /// Enable workload analysis
    pub enable_workload_analysis: bool,
    /// Idle detection threshold (ops/sec)
    pub idle_threshold_ops_per_sec: f64,
    /// Low activity hours (0-23) for background operations
    pub low_activity_hours: Vec<u8>,
}

impl Default for AutoSchedulerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            check_interval_secs: 30,
            max_concurrent_operations: 4,
            max_queue_size: 1000,
            compaction_policy: CompactionPolicy::default(),
            flush_policy: FlushPolicy::default(),
            enable_workload_analysis: true,
            idle_threshold_ops_per_sec: 10.0,
            low_activity_hours: vec![2, 3, 4, 5], // 2am-6am
        }
    }
}

/// Policy for scheduling compaction operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionPolicy {
    /// Minimum interval between compactions (seconds)
    pub min_interval_secs: u64,
    /// File count threshold to trigger compaction
    pub file_count_threshold: usize,
    /// Size threshold to trigger compaction (bytes)
    pub size_threshold_bytes: u64,
    /// Prefer low-activity hours
    pub prefer_low_activity: bool,
    /// Maximum compaction duration before escalating priority
    pub max_pending_duration_secs: u64,
}

impl Default for CompactionPolicy {
    fn default() -> Self {
        Self {
            min_interval_secs: 300, // 5 minutes
            file_count_threshold: 10,
            size_threshold_bytes: 500 * 1024 * 1024, // 500MB
            prefer_low_activity: true,
            max_pending_duration_secs: 3600, // 1 hour
        }
    }
}

/// Policy for scheduling flush operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlushPolicy {
    /// Memory threshold to trigger flush (percentage)
    pub memory_threshold_percent: f64,
    /// Time threshold to trigger flush (seconds since last flush)
    pub time_threshold_secs: u64,
    /// Entry count threshold
    pub entry_count_threshold: u64,
    /// Force flush if WAL size exceeds this (bytes)
    pub max_wal_size_bytes: u64,
}

impl Default for FlushPolicy {
    fn default() -> Self {
        Self {
            memory_threshold_percent: 80.0,
            time_threshold_secs: 300, // 5 minutes
            entry_count_threshold: 100_000,
            max_wal_size_bytes: 1024 * 1024 * 1024, // 1GB
        }
    }
}

/// System metrics for decision making
#[derive(Debug, Clone, Default)]
pub struct SystemMetrics {
    /// Current operations per second
    pub ops_per_second: f64,
    /// Memory usage percentage
    pub memory_usage_percent: f64,
    /// Disk usage percentage
    pub disk_usage_percent: f64,
    /// Active I/O operations
    pub active_io_operations: u64,
    /// CPU usage percentage
    pub cpu_usage_percent: f64,
    /// Timestamp of metrics collection
    pub collected_at: DateTime<Utc>,
}

/// Workload pattern analysis
#[derive(Debug, Clone)]
pub struct WorkloadAnalysis {
    /// Average ops/sec over the analysis window
    pub avg_ops_per_sec: f64,
    /// Peak ops/sec
    pub peak_ops_per_sec: f64,
    /// Is system currently idle
    pub is_idle: bool,
    /// Is current time in low-activity window
    pub is_low_activity_window: bool,
    /// Recommended operation priority adjustment
    pub priority_adjustment: i8,
    /// Analysis timestamp
    pub analyzed_at: DateTime<Utc>,
}

/// Scheduler statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SchedulerStats {
    /// Total operations scheduled
    pub total_scheduled: u64,
    /// Operations completed successfully
    pub completed: u64,
    /// Operations failed
    pub failed: u64,
    /// Operations skipped
    pub skipped: u64,
    /// Operations canceled
    pub canceled: u64,
    /// Operations currently running
    pub running: u64,
    /// Operations pending
    pub pending: u64,
    /// Average operation duration (ms)
    pub avg_duration_ms: f64,
    /// Scheduler uptime (seconds)
    pub uptime_secs: u64,
}

/// Result of an operation execution
#[derive(Debug)]
pub struct OperationResult {
    /// Operation ID
    pub operation_id: String,
    /// Whether it succeeded
    pub success: bool,
    /// Duration in milliseconds
    pub duration_ms: u64,
    /// Error message if failed
    pub error: Option<String>,
    /// Output data
    pub output: HashMap<String, String>,
}

/// Automatic Operations Scheduler
pub struct AutoScheduler {
    /// Configuration
    config: AutoSchedulerConfig,
    /// Priority queue of pending operations
    pending_queue: Arc<Mutex<BinaryHeap<ScheduledOperation>>>,
    /// Currently running operations
    running_operations: Arc<RwLock<HashMap<String, ScheduledOperation>>>,
    /// Completed operations (recent history)
    completed_history: Arc<RwLock<VecDeque<ScheduledOperation>>>,
    /// Scheduler statistics
    stats: Arc<RwLock<SchedulerStats>>,
    /// Current system metrics
    system_metrics: Arc<RwLock<SystemMetrics>>,
    /// Recent metrics for workload analysis
    metrics_history: Arc<RwLock<VecDeque<SystemMetrics>>>,
    /// Workload analysis result
    workload_analysis: Arc<RwLock<WorkloadAnalysis>>,
    /// Scheduler state (running/paused)
    #[allow(dead_code)]
    is_running: Arc<RwLock<bool>>,
    /// Shutdown signal sender
    shutdown_tx: broadcast::Sender<()>,
    /// Operation completion channel
    completion_tx: mpsc::Sender<OperationResult>,
    completion_rx: Arc<Mutex<mpsc::Receiver<OperationResult>>>,
    /// Start time for uptime calculation
    started_at: Arc<RwLock<Option<DateTime<Utc>>>>,
    /// Next operation ID
    next_operation_id: Arc<Mutex<u64>>,
}

impl AutoScheduler {
    /// Create a new auto scheduler
    pub fn new(config: AutoSchedulerConfig) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        let (completion_tx, completion_rx) = mpsc::channel(100);

        Self {
            config,
            pending_queue: Arc::new(Mutex::new(BinaryHeap::new())),
            running_operations: Arc::new(RwLock::new(HashMap::new())),
            completed_history: Arc::new(RwLock::new(VecDeque::with_capacity(1000))),
            stats: Arc::new(RwLock::new(SchedulerStats::default())),
            system_metrics: Arc::new(RwLock::new(SystemMetrics::default())),
            metrics_history: Arc::new(RwLock::new(VecDeque::with_capacity(60))), // 30min at 30s interval
            workload_analysis: Arc::new(RwLock::new(WorkloadAnalysis {
                avg_ops_per_sec: 0.0,
                peak_ops_per_sec: 0.0,
                is_idle: true,
                is_low_activity_window: false,
                priority_adjustment: 0,
                analyzed_at: Utc::now(),
            })),
            is_running: Arc::new(RwLock::new(false)),
            shutdown_tx,
            completion_tx,
            completion_rx: Arc::new(Mutex::new(completion_rx)),
            started_at: Arc::new(RwLock::new(None)),
            next_operation_id: Arc::new(Mutex::new(1)),
        }
    }

    /// Start the scheduler
    pub async fn start(&self) -> Result<()> {
        if !self.config.enabled {
            info!("⏰ AutoScheduler: Disabled by configuration");
            return Ok(());
        }

        {
            let mut running = self.is_running.write().await;
            if *running {
                return Ok(());
            }
            *running = true;
        }

        *self.started_at.write().await = Some(Utc::now());

        info!(
            "⏰ AutoScheduler: Starting with check_interval={}s, max_concurrent={}",
            self.config.check_interval_secs, self.config.max_concurrent_operations
        );

        // Start the main scheduler loop
        self.run_scheduler_loop().await;

        Ok(())
    }

    /// Stop the scheduler gracefully
    pub async fn stop(&self) -> Result<()> {
        info!("⏰ AutoScheduler: Stopping...");

        *self.is_running.write().await = false;

        // Signal shutdown to all tasks
        let _ = self.shutdown_tx.send(());

        // Wait for running operations to complete (with timeout)
        let timeout = tokio::time::Duration::from_secs(30);
        let start = std::time::Instant::now();

        while start.elapsed() < timeout {
            let running = self.running_operations.read().await;
            if running.is_empty() {
                break;
            }
            drop(running);
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        info!("⏰ AutoScheduler: Stopped");
        Ok(())
    }

    /// Pause the scheduler (stops scheduling new operations)
    pub async fn pause(&self) {
        info!("⏰ AutoScheduler: Paused");
        *self.is_running.write().await = false;
    }

    /// Resume the scheduler
    pub async fn resume(&self) {
        info!("⏰ AutoScheduler: Resumed");
        *self.is_running.write().await = true;
    }

    /// Schedule an operation
    pub async fn schedule(&self, operation: ScheduledOperation) -> Result<String> {
        let queue_len = self.pending_queue.lock().await.len();
        if queue_len >= self.config.max_queue_size {
            return Err(anyhow::anyhow!(
                "Scheduler queue is full ({}/{})",
                queue_len,
                self.config.max_queue_size
            ));
        }

        let id = operation.id.clone();

        self.pending_queue.lock().await.push(operation);

        {
            let mut stats = self.stats.write().await;
            stats.total_scheduled += 1;
            stats.pending = self.pending_queue.lock().await.len() as u64;
        }

        debug!("⏰ Scheduled operation: {}", id);
        Ok(id)
    }

    /// Schedule a compaction operation
    pub async fn schedule_compaction(
        &self,
        collection_id: &str,
        priority: OperationPriority,
    ) -> Result<String> {
        let operation = self
            .create_operation(
                OperationType::Compaction,
                priority,
                Some(collection_id.to_string()),
            )
            .await?;

        self.schedule(operation).await
    }

    /// Schedule a flush operation
    pub async fn schedule_flush(
        &self,
        collection_id: &str,
        priority: OperationPriority,
    ) -> Result<String> {
        let operation = self
            .create_operation(
                OperationType::Flush,
                priority,
                Some(collection_id.to_string()),
            )
            .await?;

        self.schedule(operation).await
    }

    /// Get current scheduler statistics
    pub async fn get_stats(&self) -> SchedulerStats {
        let mut stats = self.stats.read().await.clone();

        // Update dynamic stats
        if let Some(started) = *self.started_at.read().await {
            stats.uptime_secs = (Utc::now() - started).num_seconds() as u64;
        }
        stats.running = self.running_operations.read().await.len() as u64;
        stats.pending = self.pending_queue.lock().await.len() as u64;

        stats
    }

    /// Get current workload analysis
    pub async fn get_workload_analysis(&self) -> WorkloadAnalysis {
        self.workload_analysis.read().await.clone()
    }

    /// Update system metrics (called externally or by monitoring)
    pub async fn update_metrics(&self, metrics: SystemMetrics) {
        // Store current metrics
        *self.system_metrics.write().await = metrics.clone();

        // Add to history for analysis
        let mut history = self.metrics_history.write().await;
        history.push_back(metrics);
        if history.len() > 60 {
            history.pop_front();
        }
        drop(history);

        // Update workload analysis
        self.analyze_workload().await;
    }

    /// Cancel a pending operation
    pub async fn cancel(&self, operation_id: &str) -> Result<bool> {
        let mut queue = self.pending_queue.lock().await;

        // Rebuild queue without the canceled operation
        let mut new_queue = BinaryHeap::new();
        let mut found = false;

        while let Some(op) = queue.pop() {
            if op.id == operation_id {
                found = true;
                let mut stats = self.stats.write().await;
                stats.canceled += 1;
            } else {
                new_queue.push(op);
            }
        }

        *queue = new_queue;
        Ok(found)
    }

    // Private methods

    async fn run_scheduler_loop(&self) {
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let mut check_interval = interval(tokio::time::Duration::from_secs(
            self.config.check_interval_secs,
        ));

        loop {
            tokio::select! {
                _ = shutdown_rx.recv() => {
                    debug!("⏰ AutoScheduler: Received shutdown signal");
                    break;
                }
                _ = check_interval.tick() => {
                    if *self.is_running.read().await {
                        self.process_pending_operations().await;
                        self.check_scheduled_triggers().await;
                    }
                }
            }
        }
    }

    async fn process_pending_operations(&self) {
        let running_count = self.running_operations.read().await.len();
        if running_count >= self.config.max_concurrent_operations {
            return;
        }

        let available_slots = self.config.max_concurrent_operations - running_count;
        let now = Utc::now();

        for _ in 0..available_slots {
            let operation = {
                let mut queue = self.pending_queue.lock().await;

                // Find the highest priority operation that's ready to run

                queue
                    .iter()
                    .position(|op| op.run_after <= now)
                    .map(|_| {
                        // Remove and return the operation
                        // Note: This is inefficient, but BinaryHeap doesn't support removal by index
                        let mut temp = BinaryHeap::new();
                        let mut found = None;

                        while let Some(op) = queue.pop() {
                            if found.is_none() && op.run_after <= now {
                                found = Some(op);
                            } else {
                                temp.push(op);
                            }
                        }

                        *queue = temp;
                        found
                    })
                    .flatten()
            };

            if let Some(mut op) = operation {
                op.status = OperationStatus::Running;

                // Add to running operations
                self.running_operations
                    .write()
                    .await
                    .insert(op.id.clone(), op.clone());

                // Execute the operation asynchronously
                let completion_tx = self.completion_tx.clone();
                let op_id = op.id.clone();
                let op_type = op.operation_type;
                let collection_id = op.collection_id.clone();

                tokio::spawn(async move {
                    let start = std::time::Instant::now();

                    // Execute the operation
                    let result = Self::execute_operation(op_type, collection_id.as_deref()).await;

                    let duration_ms = start.elapsed().as_millis() as u64;

                    let op_result = OperationResult {
                        operation_id: op_id,
                        success: result.is_ok(),
                        duration_ms,
                        error: result.err().map(|e| e.to_string()),
                        output: HashMap::new(),
                    };

                    let _ = completion_tx.send(op_result).await;
                });

                {
                    let mut stats = self.stats.write().await;
                    stats.running += 1;
                    stats.pending = stats.pending.saturating_sub(1);
                }
            } else {
                break;
            }
        }

        // Process completion results
        self.process_completions().await;
    }

    async fn process_completions(&self) {
        let mut rx = self.completion_rx.lock().await;

        while let Ok(result) = rx.try_recv() {
            // Remove from running
            if let Some(op) = self
                .running_operations
                .write()
                .await
                .remove(&result.operation_id)
            {
                let mut completed_op = op;
                completed_op.status = if result.success {
                    OperationStatus::Completed
                } else {
                    OperationStatus::Failed
                };

                // Add to history
                let mut history = self.completed_history.write().await;
                history.push_back(completed_op);
                if history.len() > 1000 {
                    history.pop_front();
                }

                // Update stats
                let mut stats = self.stats.write().await;
                stats.running = stats.running.saturating_sub(1);
                if result.success {
                    stats.completed += 1;
                } else {
                    stats.failed += 1;
                }

                // Update average duration
                let total_ops = stats.completed + stats.failed;
                stats.avg_duration_ms = (stats.avg_duration_ms * (total_ops - 1) as f64
                    + result.duration_ms as f64)
                    / total_ops as f64;
            }
        }
    }

    async fn check_scheduled_triggers(&self) {
        let analysis = self.workload_analysis.read().await.clone();
        let metrics = self.system_metrics.read().await.clone();

        // Check flush triggers
        if self.should_trigger_flush(&metrics).await
            && let Err(e) = self
                .schedule_flush("__global__", OperationPriority::Normal)
                .await
        {
            warn!("⏰ Failed to schedule flush: {}", e);
        }

        // Check compaction triggers (prefer idle/low-activity periods)
        if (analysis.is_idle || analysis.is_low_activity_window)
            && self.should_trigger_compaction(&metrics).await
            && let Err(e) = self
                .schedule_compaction("__global__", OperationPriority::Low)
                .await
        {
            warn!("⏰ Failed to schedule compaction: {}", e);
        }

        // Schedule periodic health checks
        self.schedule_periodic_operations().await;
    }

    async fn should_trigger_flush(&self, metrics: &SystemMetrics) -> bool {
        metrics.memory_usage_percent >= self.config.flush_policy.memory_threshold_percent
    }

    async fn should_trigger_compaction(&self, _metrics: &SystemMetrics) -> bool {
        // This would integrate with CompactionCoordinator to check actual conditions
        // For now, return false to let the existing coordinator handle it
        false
    }

    async fn schedule_periodic_operations(&self) {
        // Schedule periodic health checks (every 5 minutes)
        let now = Utc::now();
        let last_health_check = self
            .get_last_operation_time(OperationType::HealthCheck)
            .await;

        if now - last_health_check > Duration::minutes(5)
            && let Ok(operation) = self
                .create_operation(
                    OperationType::HealthCheck,
                    OperationPriority::Background,
                    None,
                )
                .await
        {
            let _ = self.schedule(operation).await;
        }

        // Schedule periodic stats collection (every minute)
        let last_stats = self
            .get_last_operation_time(OperationType::StatsCollection)
            .await;

        if now - last_stats > Duration::minutes(1)
            && let Ok(operation) = self
                .create_operation(
                    OperationType::StatsCollection,
                    OperationPriority::Background,
                    None,
                )
                .await
        {
            let _ = self.schedule(operation).await;
        }
    }

    async fn get_last_operation_time(&self, op_type: OperationType) -> DateTime<Utc> {
        let history = self.completed_history.read().await;

        history
            .iter()
            .rev()
            .find(|op| op.operation_type == op_type)
            .map_or_else(|| Utc::now() - Duration::hours(1), |op| op.scheduled_at)
    }

    async fn analyze_workload(&self) {
        let history = self.metrics_history.read().await;

        if history.is_empty() {
            return;
        }

        let avg_ops: f64 =
            history.iter().map(|m| m.ops_per_second).sum::<f64>() / history.len() as f64;
        let peak_ops = history
            .iter()
            .map(|m| m.ops_per_second)
            .fold(0.0f64, |a, b| a.max(b));

        let is_idle = avg_ops < self.config.idle_threshold_ops_per_sec;

        let current_hour = Utc::now().hour() as u8;
        let is_low_activity = self.config.low_activity_hours.contains(&current_hour);

        let priority_adjustment: i8 = if is_idle {
            1
        } else if is_low_activity {
            0
        } else {
            -1
        };

        *self.workload_analysis.write().await = WorkloadAnalysis {
            avg_ops_per_sec: avg_ops,
            peak_ops_per_sec: peak_ops,
            is_idle,
            is_low_activity_window: is_low_activity,
            priority_adjustment,
            analyzed_at: Utc::now(),
        };
    }

    async fn create_operation(
        &self,
        operation_type: OperationType,
        priority: OperationPriority,
        collection_id: Option<String>,
    ) -> Result<ScheduledOperation> {
        let id = {
            let mut next_id = self.next_operation_id.lock().await;
            let id = *next_id;
            *next_id += 1;
            format!("op_{:08}_{}", id, operation_type)
        };

        Ok(ScheduledOperation {
            id,
            operation_type,
            priority,
            collection_id,
            scheduled_at: Utc::now(),
            run_after: Utc::now(),
            deadline: None,
            status: OperationStatus::Pending,
            retry_count: 0,
            max_retries: 3,
            context: HashMap::new(),
        })
    }

    async fn execute_operation(
        operation_type: OperationType,
        _collection_id: Option<&str>,
    ) -> Result<()> {
        debug!("⏰ Executing operation: {}", operation_type);

        // This is where we would integrate with the actual coordinators
        // For now, just simulate the operation
        match operation_type {
            OperationType::Compaction => {
                // Would call: compaction_coordinator.trigger_compaction(collection_id).await
                debug!("⏰ Compaction operation executed");
            }
            OperationType::Flush => {
                // Would call: flush_coordinator.trigger_flush(collection_id).await
                debug!("⏰ Flush operation executed");
            }
            OperationType::IndexOptimization => {
                // Would call: axis_manager.optimize(collection_id).await
                debug!("⏰ Index optimization executed");
            }
            OperationType::StatsCollection => {
                // Collect and store system statistics
                debug!("⏰ Stats collection executed");
            }
            OperationType::Backup => {
                // Would call: backup_coordinator.create_incremental().await
                debug!("⏰ Backup operation executed");
            }
            OperationType::HealthCheck => {
                // Run health checks
                debug!("⏰ Health check executed");
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_scheduler_config_default() {
        let config = AutoSchedulerConfig::default();
        assert!(config.enabled);
        assert_eq!(config.check_interval_secs, 30);
        assert_eq!(config.max_concurrent_operations, 4);
        assert_eq!(config.max_queue_size, 1000);
        assert!(config.enable_workload_analysis);
    }

    #[test]
    fn test_compaction_policy_default() {
        let policy = CompactionPolicy::default();
        assert_eq!(policy.min_interval_secs, 300);
        assert_eq!(policy.file_count_threshold, 10);
        assert!(policy.prefer_low_activity);
    }

    #[test]
    fn test_flush_policy_default() {
        let policy = FlushPolicy::default();
        assert_eq!(policy.memory_threshold_percent, 80.0);
        assert_eq!(policy.time_threshold_secs, 300);
    }

    #[test]
    fn test_operation_priority_ordering() {
        assert!(OperationPriority::Critical > OperationPriority::High);
        assert!(OperationPriority::High > OperationPriority::Normal);
        assert!(OperationPriority::Normal > OperationPriority::Low);
        assert!(OperationPriority::Low > OperationPriority::Background);
    }

    #[test]
    fn test_scheduled_operation_ordering() {
        let now = Utc::now();

        let op1 = ScheduledOperation {
            id: "op1".to_string(),
            operation_type: OperationType::Compaction,
            priority: OperationPriority::High,
            collection_id: None,
            scheduled_at: now,
            run_after: now,
            deadline: None,
            status: OperationStatus::Pending,
            retry_count: 0,
            max_retries: 3,
            context: HashMap::new(),
        };

        let op2 = ScheduledOperation {
            id: "op2".to_string(),
            operation_type: OperationType::Flush,
            priority: OperationPriority::Normal,
            collection_id: None,
            scheduled_at: now,
            run_after: now,
            deadline: None,
            status: OperationStatus::Pending,
            retry_count: 0,
            max_retries: 3,
            context: HashMap::new(),
        };

        // Higher priority should be "greater"
        assert!(op1 > op2);
    }

    #[test]
    fn test_operation_type_display() {
        assert_eq!(OperationType::Compaction.to_string(), "compaction");
        assert_eq!(OperationType::Flush.to_string(), "flush");
        assert_eq!(OperationType::Backup.to_string(), "backup");
    }

    #[tokio::test]
    async fn test_scheduler_creation() {
        let config = AutoSchedulerConfig::default();
        let scheduler = AutoScheduler::new(config);

        let stats = scheduler.get_stats().await;
        assert_eq!(stats.total_scheduled, 0);
        assert_eq!(stats.running, 0);
        assert_eq!(stats.pending, 0);
    }

    #[tokio::test]
    async fn test_schedule_operation() {
        let config = AutoSchedulerConfig::default();
        let scheduler = AutoScheduler::new(config);

        let op = scheduler
            .create_operation(
                OperationType::Compaction,
                OperationPriority::Normal,
                Some("test_collection".to_string()),
            )
            .await
            .unwrap();

        let result = scheduler.schedule(op).await;
        assert!(result.is_ok());

        let stats = scheduler.get_stats().await;
        assert_eq!(stats.total_scheduled, 1);
        assert_eq!(stats.pending, 1);
    }

    #[tokio::test]
    async fn test_cancel_operation() {
        let config = AutoSchedulerConfig::default();
        let scheduler = AutoScheduler::new(config);

        let op = scheduler
            .create_operation(OperationType::Compaction, OperationPriority::Normal, None)
            .await
            .unwrap();

        let op_id = op.id.clone();
        scheduler.schedule(op).await.unwrap();

        let canceled = scheduler.cancel(&op_id).await.unwrap();
        assert!(canceled);

        let stats = scheduler.get_stats().await;
        assert_eq!(stats.canceled, 1);
    }
}
