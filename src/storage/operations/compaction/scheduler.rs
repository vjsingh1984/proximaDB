//! Compaction Scheduler
//!
//! Priority-based scheduling of compaction operations across collections.
//! Manages concurrent compactions and resource allocation.

use anyhow::Result;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock, Semaphore};
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use super::strategies::{
    CompactionCostEstimate, CompactionExecutionResult, CompactionPlan, CompactionStrategyRegistry,
    FileMetadata,
};

/// Scheduled compaction task with priority ordering
#[derive(Debug, Clone)]
struct ScheduledTask {
    /// Compaction plan to execute
    plan: CompactionPlan,
    /// Cost estimate for prioritization
    cost_estimate: CompactionCostEstimate,
    /// When this task was scheduled
    scheduled_at: Instant,
    /// Number of reschedules (for starvation prevention)
    reschedule_count: u32,
}

impl ScheduledTask {
    fn effective_priority(&self) -> f64 {
        // Increase priority over time to prevent starvation
        let age_secs = self.scheduled_at.elapsed().as_secs_f64();
        let age_bonus = (age_secs / 60.0).min(50.0); // Max 50 bonus after ~50 min
        let reschedule_bonus = self.reschedule_count as f64 * 10.0;

        self.cost_estimate.priority_score + age_bonus + reschedule_bonus
    }
}

impl PartialEq for ScheduledTask {
    fn eq(&self, other: &Self) -> bool {
        self.plan.plan_id == other.plan.plan_id
    }
}

impl Eq for ScheduledTask {}

impl PartialOrd for ScheduledTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScheduledTask {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher priority = should come first in BinaryHeap
        self.effective_priority()
            .partial_cmp(&other.effective_priority())
            .unwrap_or(Ordering::Equal)
    }
}

/// Statistics about scheduler operations
#[derive(Debug, Clone, Default)]
pub struct SchedulerStats {
    pub pending_tasks: usize,
    pub active_compactions: usize,
    pub completed_compactions: u64,
    pub failed_compactions: u64,
    pub total_bytes_compacted: u64,
    pub average_compaction_time: Duration,
}

/// Configuration for the compaction scheduler
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Maximum concurrent compactions
    pub max_concurrent: usize,
    /// Check interval for new compaction needs
    pub check_interval: Duration,
    /// Minimum time between compactions for same collection
    pub min_collection_interval: Duration,
    /// Maximum pending tasks before rejecting new ones
    pub max_pending_tasks: usize,
    /// Enable automatic scheduling
    pub auto_schedule: bool,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 2,
            check_interval: Duration::from_secs(30),
            min_collection_interval: Duration::from_secs(60),
            max_pending_tasks: 100,
            auto_schedule: true,
        }
    }
}

/// Compaction Scheduler with priority queue
///
/// Manages compaction operations across all collections with:
/// - Priority-based scheduling (urgent compactions first)
/// - Concurrency control (limit parallel compactions)
/// - Starvation prevention (age-based priority boost)
/// - Per-collection rate limiting
pub struct CompactionScheduler {
    /// Configuration
    config: SchedulerConfig,
    /// Strategy registry for selecting strategies
    strategy_registry: Arc<CompactionStrategyRegistry>,
    /// Priority queue of pending tasks
    pending_queue: Mutex<BinaryHeap<ScheduledTask>>,
    /// Currently executing tasks
    active_tasks: RwLock<HashMap<String, CompactionPlan>>,
    /// Semaphore for concurrency control
    concurrency_semaphore: Arc<Semaphore>,
    /// Last compaction time per collection
    last_compaction: RwLock<HashMap<String, Instant>>,
    /// Statistics
    stats: RwLock<SchedulerStats>,
    /// Shutdown flag
    shutdown: RwLock<bool>,
}

impl CompactionScheduler {
    /// Create a new scheduler with default configuration
    pub fn new() -> Self {
        Self::with_config(SchedulerConfig::default())
    }

    /// Create a new scheduler with custom configuration
    pub fn with_config(config: SchedulerConfig) -> Self {
        let max_concurrent = config.max_concurrent;
        Self {
            config,
            strategy_registry: Arc::new(CompactionStrategyRegistry::new()),
            pending_queue: Mutex::new(BinaryHeap::new()),
            active_tasks: RwLock::new(HashMap::new()),
            concurrency_semaphore: Arc::new(Semaphore::new(max_concurrent)),
            last_compaction: RwLock::new(HashMap::new()),
            stats: RwLock::new(SchedulerStats::default()),
            shutdown: RwLock::new(false),
        }
    }

    /// Schedule a compaction plan
    pub async fn schedule(&self, plan: CompactionPlan) -> Result<()> {
        // Check if we're at capacity
        let queue = self.pending_queue.lock().await;
        if queue.len() >= self.config.max_pending_tasks {
            return Err(anyhow::anyhow!(
                "Scheduler at capacity ({} pending tasks)",
                self.config.max_pending_tasks
            ));
        }
        drop(queue);

        // Check collection rate limit
        let last_compaction = self.last_compaction.read().await;
        if let Some(last) = last_compaction.get(&plan.collection_id) {
            if last.elapsed() < self.config.min_collection_interval {
                return Err(anyhow::anyhow!(
                    "Collection {} was compacted recently, waiting for cooldown",
                    plan.collection_id
                ));
            }
        }
        drop(last_compaction);

        // Calculate cost estimate (find by strategy name or engine compatibility)
        let strategy = self
            .strategy_registry
            .find(&plan.strategy_name)
            .ok_or_else(|| anyhow::anyhow!("No strategy found for {}", plan.strategy_name))?;

        let cost_estimate = strategy.estimate_cost(&plan);
        let priority_score = cost_estimate.priority_score;

        let task = ScheduledTask {
            plan: plan.clone(),
            cost_estimate,
            scheduled_at: Instant::now(),
            reschedule_count: 0,
        };

        let mut queue = self.pending_queue.lock().await;
        queue.push(task);

        info!(
            "Scheduled compaction {} for collection {} (priority: {:.1})",
            plan.plan_id, plan.collection_id, priority_score
        );

        // Update stats
        let mut stats = self.stats.write().await;
        stats.pending_tasks = queue.len();

        Ok(())
    }

    /// Check collections and automatically schedule compactions
    pub async fn check_and_schedule(
        &self,
        collection_id: &str,
        engine_name: &str,
        files: &[FileMetadata],
    ) -> Result<Option<String>> {
        // Use registry to find best plan
        let plan = self
            .strategy_registry
            .select_best_plan(collection_id, engine_name, files)
            .await?;

        match plan {
            Some(p) => {
                let plan_id = p.plan_id.clone();
                self.schedule(p).await?;
                Ok(Some(plan_id))
            }
            None => Ok(None),
        }
    }

    /// Execute the next available task
    pub async fn execute_next<F, Fut>(
        &self,
        executor: F,
    ) -> Result<Option<CompactionExecutionResult>>
    where
        F: FnOnce(CompactionPlan) -> Fut,
        Fut: std::future::Future<Output = Result<CompactionExecutionResult>>,
    {
        // Acquire concurrency permit
        let permit = self.concurrency_semaphore.try_acquire();
        if permit.is_err() {
            debug!("No available compaction slot, waiting...");
            return Ok(None);
        }
        let _permit = permit.unwrap();

        // Get next task from queue
        let task = {
            let mut queue = self.pending_queue.lock().await;
            queue.pop()
        };

        let task = match task {
            Some(t) => t,
            None => return Ok(None),
        };

        let plan_id = task.plan.plan_id.clone();
        let collection_id = task.plan.collection_id.clone();

        // Mark as active
        {
            let mut active = self.active_tasks.write().await;
            active.insert(plan_id.clone(), task.plan.clone());
        }

        info!(
            "Executing compaction {} for collection {} ({} files)",
            plan_id,
            collection_id,
            task.plan.input_files.len()
        );

        let start_time = Instant::now();

        // Execute the compaction
        let result = executor(task.plan).await;

        // Update tracking
        {
            let mut active = self.active_tasks.write().await;
            active.remove(&plan_id);
        }

        {
            let mut last = self.last_compaction.write().await;
            last.insert(collection_id.clone(), Instant::now());
        }

        // Update stats
        let duration = start_time.elapsed();
        {
            let mut stats = self.stats.write().await;
            stats.active_compactions = self.active_tasks.read().await.len();
            stats.pending_tasks = self.pending_queue.lock().await.len();

            match &result {
                Ok(r) => {
                    stats.completed_compactions += 1;
                    stats.total_bytes_compacted += r.bytes_freed;
                    // Update average (simple moving average)
                    let n = stats.completed_compactions as f64;
                    let prev_avg = stats.average_compaction_time.as_secs_f64();
                    let new_avg = prev_avg + (duration.as_secs_f64() - prev_avg) / n;
                    stats.average_compaction_time = Duration::from_secs_f64(new_avg);
                }
                Err(_) => {
                    stats.failed_compactions += 1;
                }
            }
        }

        match result {
            Ok(r) => {
                info!(
                    "Compaction {} completed in {:?}, freed {} bytes",
                    plan_id, duration, r.bytes_freed
                );
                Ok(Some(r))
            }
            Err(e) => {
                error!("Compaction {} failed: {}", plan_id, e);
                Err(e)
            }
        }
    }

    /// Run the scheduler loop
    pub async fn run_scheduler_loop<F, Fut>(&self, executor: F)
    where
        F: Fn(CompactionPlan) -> Fut + Clone,
        Fut: std::future::Future<Output = Result<CompactionExecutionResult>>,
    {
        let mut check_interval = interval(self.config.check_interval);

        info!(
            "Compaction scheduler started (max_concurrent: {}, check_interval: {:?})",
            self.config.max_concurrent, self.config.check_interval
        );

        loop {
            check_interval.tick().await;

            // Check shutdown flag
            if *self.shutdown.read().await {
                info!("Compaction scheduler shutting down");
                break;
            }

            // Try to execute pending tasks
            let executor_clone = executor.clone();
            match self.execute_next(executor_clone).await {
                Ok(Some(result)) => {
                    debug!(
                        "Scheduler processed compaction, freed {} bytes",
                        result.bytes_freed
                    );
                }
                Ok(None) => {
                    debug!("No pending compactions");
                }
                Err(e) => {
                    warn!("Scheduler encountered error: {}", e);
                }
            }
        }
    }

    /// Get current scheduler statistics
    pub async fn get_stats(&self) -> SchedulerStats {
        self.stats.read().await.clone()
    }

    /// Get pending task count
    pub async fn pending_count(&self) -> usize {
        self.pending_queue.lock().await.len()
    }

    /// Get active compaction count
    pub async fn active_count(&self) -> usize {
        self.active_tasks.read().await.len()
    }

    /// Cancel a pending compaction
    pub async fn cancel(&self, plan_id: &str) -> bool {
        let mut queue = self.pending_queue.lock().await;
        let original_len = queue.len();

        // Rebuild queue without the cancelled task
        let tasks: Vec<ScheduledTask> = queue
            .drain()
            .filter(|t| t.plan.plan_id != plan_id)
            .collect();

        for task in tasks {
            queue.push(task);
        }

        let cancelled = queue.len() < original_len;
        if cancelled {
            info!("Cancelled compaction {}", plan_id);
        }
        cancelled
    }

    /// Signal scheduler to shutdown
    pub async fn shutdown(&self) {
        let mut shutdown = self.shutdown.write().await;
        *shutdown = true;
        info!("Compaction scheduler shutdown requested");
    }

    /// Drain all pending tasks (for testing/shutdown)
    pub async fn drain_pending(&self) -> Vec<CompactionPlan> {
        let mut queue = self.pending_queue.lock().await;
        queue.drain().map(|t| t.plan).collect()
    }
}

impl Default for CompactionScheduler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::strategies::{CompactionParameters, FileMetadata};
    use super::*;

    fn create_test_plan(id: &str, priority: f64) -> CompactionPlan {
        CompactionPlan {
            plan_id: id.to_string(),
            collection_id: "test_collection".to_string(),
            input_files: vec![FileMetadata::new("f1", "/path/f1.sst", 10 * 1024 * 1024)],
            target_level: 1,
            estimated_output_size: 10 * 1024 * 1024,
            priority,
            strategy_name: "leveled".to_string(),
            parameters: CompactionParameters::default(),
        }
    }

    #[tokio::test]
    async fn test_scheduler_creation() {
        let scheduler = CompactionScheduler::new();

        assert_eq!(scheduler.pending_count().await, 0);
        assert_eq!(scheduler.active_count().await, 0);
    }

    #[tokio::test]
    async fn test_schedule_and_priority() {
        let scheduler = CompactionScheduler::new();

        // Schedule low priority first
        let low_priority = create_test_plan("low", 10.0);
        scheduler.schedule(low_priority).await.unwrap();

        // Schedule high priority second
        let high_priority = create_test_plan("high", 100.0);
        scheduler.schedule(high_priority).await.unwrap();

        assert_eq!(scheduler.pending_count().await, 2);

        // Execute should return high priority first
        let result = scheduler
            .execute_next(|plan| async move {
                Ok(CompactionExecutionResult {
                    plan_id: plan.plan_id,
                    files_removed: vec![],
                    files_created: vec![],
                    bytes_freed: 1000,
                    duration: Duration::from_secs(1),
                    success: true,
                    error_message: None,
                })
            })
            .await
            .unwrap();

        assert!(result.is_some());
        assert_eq!(result.unwrap().plan_id, "high");
    }

    #[tokio::test]
    async fn test_cancel_pending() {
        let scheduler = CompactionScheduler::new();

        let plan = create_test_plan("to_cancel", 50.0);
        scheduler.schedule(plan).await.unwrap();

        assert_eq!(scheduler.pending_count().await, 1);

        let cancelled = scheduler.cancel("to_cancel").await;
        assert!(cancelled);

        assert_eq!(scheduler.pending_count().await, 0);
    }

    #[tokio::test]
    async fn test_stats_tracking() {
        let scheduler = CompactionScheduler::new();

        let plan = create_test_plan("stats_test", 50.0);
        scheduler.schedule(plan).await.unwrap();

        scheduler
            .execute_next(|plan| async move {
                Ok(CompactionExecutionResult {
                    plan_id: plan.plan_id,
                    files_removed: vec![],
                    files_created: vec![],
                    bytes_freed: 5000,
                    duration: Duration::from_millis(100),
                    success: true,
                    error_message: None,
                })
            })
            .await
            .unwrap();

        let stats = scheduler.get_stats().await;
        assert_eq!(stats.completed_compactions, 1);
        assert_eq!(stats.total_bytes_compacted, 5000);
    }

    #[tokio::test]
    async fn test_collection_rate_limit() {
        let config = SchedulerConfig {
            min_collection_interval: Duration::from_secs(60),
            ..Default::default()
        };
        let scheduler = CompactionScheduler::with_config(config);

        // Schedule and execute first compaction
        let plan1 = create_test_plan("first", 50.0);
        scheduler.schedule(plan1).await.unwrap();
        scheduler
            .execute_next(|plan| async move {
                Ok(CompactionExecutionResult {
                    plan_id: plan.plan_id,
                    files_removed: vec![],
                    files_created: vec![],
                    bytes_freed: 1000,
                    duration: Duration::from_secs(1),
                    success: true,
                    error_message: None,
                })
            })
            .await
            .unwrap();

        // Try to schedule another for same collection - should fail due to rate limit
        let plan2 = create_test_plan("second", 50.0);
        let result = scheduler.schedule(plan2).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_drain_pending() {
        let scheduler = CompactionScheduler::new();

        scheduler
            .schedule(create_test_plan("p1", 50.0))
            .await
            .unwrap();
        scheduler
            .schedule(create_test_plan("p2", 60.0))
            .await
            .unwrap();
        scheduler
            .schedule(create_test_plan("p3", 70.0))
            .await
            .unwrap();

        assert_eq!(scheduler.pending_count().await, 3);

        let drained = scheduler.drain_pending().await;
        assert_eq!(drained.len(), 3);
        assert_eq!(scheduler.pending_count().await, 0);
    }
}
