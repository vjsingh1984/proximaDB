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

//! Migration Task Management
//!
//! Tracks and coordinates data migrations between storage tiers.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tracing::{debug, info};

use super::policy::PerformanceTier;

/// Unique task ID generator
static TASK_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a unique task ID
fn generate_task_id() -> String {
    let id = TASK_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("migration-{}", id)
}

/// Status of a migration task
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationStatus {
    /// Task is queued and waiting
    Pending,
    /// Task is currently executing
    InProgress,
    /// Task completed successfully
    Completed,
    /// Task failed
    Failed,
    /// Task was cancelled
    Cancelled,
}

/// Priority for migration tasks
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum MigrationPriority {
    /// Low priority (background optimization)
    Low = 0,
    /// Normal priority (policy-triggered)
    #[default]
    Normal = 1,
    /// High priority (space pressure)
    High = 2,
    /// Critical priority (immediate action required)
    Critical = 3,
}

/// A migration task for moving data between tiers
#[derive(Debug, Clone)]
pub struct MigrationTask {
    /// Unique task ID
    pub id: String,
    /// Collection containing the item
    pub collection: String,
    /// Item ID to migrate
    pub item_id: String,
    /// Source tier
    pub source_tier: PerformanceTier,
    /// Target tier
    pub target_tier: PerformanceTier,
    /// Estimated size in bytes
    pub estimated_bytes: u64,
    /// Task priority
    pub priority: MigrationPriority,
    /// Creation timestamp
    pub created_at: Instant,
    /// Current status
    pub status: MigrationStatus,
    /// Retry count
    pub retry_count: u32,
    /// Maximum retries
    pub max_retries: u32,
}

impl MigrationTask {
    /// Create a new migration task
    pub fn new(
        collection: String,
        item_id: String,
        source_tier: PerformanceTier,
        target_tier: PerformanceTier,
        estimated_bytes: u64,
    ) -> Self {
        Self {
            id: generate_task_id(),
            collection,
            item_id,
            source_tier,
            target_tier,
            estimated_bytes,
            priority: MigrationPriority::Normal,
            created_at: Instant::now(),
            status: MigrationStatus::Pending,
            retry_count: 0,
            max_retries: 3,
        }
    }

    /// Set task priority
    pub fn with_priority(mut self, priority: MigrationPriority) -> Self {
        self.priority = priority;
        self
    }

    /// Set max retries
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Check if task is a demotion (moving to colder tier)
    pub fn is_demotion(&self) -> bool {
        self.target_tier.cost_factor() < self.source_tier.cost_factor()
    }

    /// Check if task is a promotion (moving to hotter tier)
    pub fn is_promotion(&self) -> bool {
        self.target_tier.cost_factor() > self.source_tier.cost_factor()
    }

    /// Get age of task
    pub fn age(&self) -> Duration {
        self.created_at.elapsed()
    }

    /// Check if task can be retried
    pub fn can_retry(&self) -> bool {
        self.retry_count < self.max_retries
    }
}

/// Result of a completed migration
#[derive(Debug, Clone)]
pub struct MigrationResult {
    /// Task ID
    pub task_id: String,
    /// Collection
    pub collection: String,
    /// Item ID
    pub item_id: String,
    /// Source tier
    pub source_tier: PerformanceTier,
    /// Target tier
    pub target_tier: PerformanceTier,
    /// Whether migration succeeded
    pub success: bool,
    /// Bytes actually migrated
    pub bytes_migrated: u64,
    /// Duration of migration
    pub duration: Duration,
    /// Error message if failed
    pub error: Option<String>,
}

/// Statistics for the migration coordinator
#[derive(Debug, Clone, Default)]
pub struct MigrationCoordinatorStats {
    /// Total tasks submitted
    pub tasks_submitted: u64,
    /// Tasks currently pending
    pub tasks_pending: usize,
    /// Tasks currently in progress
    pub tasks_in_progress: usize,
    /// Tasks completed
    pub tasks_completed: u64,
    /// Tasks failed
    pub tasks_failed: u64,
    /// Tasks cancelled
    pub tasks_cancelled: u64,
    /// Total bytes migrated
    pub bytes_migrated: u64,
    /// Promotions completed
    pub promotions: u64,
    /// Demotions completed
    pub demotions: u64,
}

/// Coordinates migration tasks
pub struct MigrationCoordinator {
    /// Maximum concurrent migrations
    max_concurrent: usize,
    /// Pending tasks queue (priority-ordered)
    pending: Arc<RwLock<VecDeque<MigrationTask>>>,
    /// In-progress tasks
    in_progress: Arc<RwLock<HashMap<String, MigrationTask>>>,
    /// Completed tasks (recent)
    completed: Arc<RwLock<VecDeque<MigrationResult>>>,
    /// Maximum completed history
    max_history: usize,
    /// Statistics
    stats: Arc<RwLock<MigrationCoordinatorStats>>,
}

impl MigrationCoordinator {
    /// Create a new migration coordinator
    pub fn new(max_concurrent: usize) -> Self {
        Self {
            max_concurrent,
            pending: Arc::new(RwLock::new(VecDeque::new())),
            in_progress: Arc::new(RwLock::new(HashMap::new())),
            completed: Arc::new(RwLock::new(VecDeque::new())),
            max_history: 1000,
            stats: Arc::new(RwLock::new(MigrationCoordinatorStats::default())),
        }
    }

    /// Submit a migration task
    pub async fn submit(&self, task: MigrationTask) {
        let mut pending = self.pending.write().await;

        // Insert in priority order (higher priority first)
        let insert_pos = pending
            .iter()
            .position(|t| t.priority < task.priority)
            .unwrap_or(pending.len());

        pending.insert(insert_pos, task);

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.tasks_submitted += 1;
            stats.tasks_pending = pending.len();
        }

        debug!("Migration task submitted, pending: {}", pending.len());
    }

    /// Get next task to execute (if under concurrency limit)
    pub async fn next(&self) -> Option<MigrationTask> {
        let in_progress = self.in_progress.read().await;
        if in_progress.len() >= self.max_concurrent {
            return None;
        }
        drop(in_progress);

        let mut pending = self.pending.write().await;
        let task = pending.pop_front()?;

        // Move to in-progress
        {
            let mut in_progress = self.in_progress.write().await;
            let mut task = task.clone();
            task.status = MigrationStatus::InProgress;
            in_progress.insert(task.id.clone(), task.clone());

            let mut stats = self.stats.write().await;
            stats.tasks_pending = pending.len();
            stats.tasks_in_progress = in_progress.len();
        }

        Some(task)
    }

    /// Mark a task as completed
    pub async fn complete(&self, task_id: &str, result: MigrationResult) {
        // Remove from in-progress
        {
            let mut in_progress = self.in_progress.write().await;
            in_progress.remove(task_id);
        }

        // Add to completed history
        {
            let mut completed = self.completed.write().await;
            completed.push_front(result.clone());
            while completed.len() > self.max_history {
                completed.pop_back();
            }
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            let in_progress = self.in_progress.read().await;
            stats.tasks_in_progress = in_progress.len();

            if result.success {
                stats.tasks_completed += 1;
                stats.bytes_migrated += result.bytes_migrated;

                if result.target_tier.cost_factor() > result.source_tier.cost_factor() {
                    stats.promotions += 1;
                } else {
                    stats.demotions += 1;
                }
            } else {
                stats.tasks_failed += 1;
            }
        }

        info!(
            "Migration {} completed: success={}, bytes={}",
            task_id, result.success, result.bytes_migrated
        );
    }

    /// Retry a failed task
    pub async fn retry(&self, task_id: &str) -> bool {
        // Find in completed (failed tasks)
        let task_to_retry = {
            let completed = self.completed.read().await;
            completed
                .iter()
                .find(|r| r.task_id == task_id && !r.success)
                .map(|r| {
                    MigrationTask::new(
                        r.collection.clone(),
                        r.item_id.clone(),
                        r.source_tier,
                        r.target_tier,
                        r.bytes_migrated,
                    )
                })
        };

        if let Some(task) = task_to_retry {
            self.submit(task).await;
            true
        } else {
            false
        }
    }

    /// Cancel a pending task
    pub async fn cancel(&self, task_id: &str) -> bool {
        let mut pending = self.pending.write().await;
        let len_before = pending.len();
        pending.retain(|t| t.id != task_id);

        if pending.len() < len_before {
            let mut stats = self.stats.write().await;
            stats.tasks_cancelled += 1;
            stats.tasks_pending = pending.len();
            true
        } else {
            false
        }
    }

    /// Get all pending tasks
    pub async fn get_pending(&self) -> Vec<MigrationTask> {
        self.pending.read().await.iter().cloned().collect()
    }

    /// Get all in-progress tasks
    pub async fn get_in_progress(&self) -> Vec<MigrationTask> {
        self.in_progress.read().await.values().cloned().collect()
    }

    /// Get recent completed tasks
    pub async fn get_completed(&self, limit: usize) -> Vec<MigrationResult> {
        self.completed
            .read()
            .await
            .iter()
            .take(limit)
            .cloned()
            .collect()
    }

    /// Get statistics
    pub async fn get_stats(&self) -> MigrationCoordinatorStats {
        self.stats.read().await.clone()
    }

    /// Clear all pending tasks
    pub async fn clear_pending(&self) -> usize {
        let mut pending = self.pending.write().await;
        let count = pending.len();
        pending.clear();

        let mut stats = self.stats.write().await;
        stats.tasks_cancelled += count as u64;
        stats.tasks_pending = 0;

        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_creation() {
        let task = MigrationTask::new(
            "test_collection".to_string(),
            "item_1".to_string(),
            PerformanceTier::Hot,
            PerformanceTier::Cold,
            1024,
        );

        assert!(task.id.starts_with("migration-"));
        assert_eq!(task.collection, "test_collection");
        assert_eq!(task.source_tier, PerformanceTier::Hot);
        assert_eq!(task.target_tier, PerformanceTier::Cold);
        assert!(task.is_demotion());
        assert!(!task.is_promotion());
    }

    #[test]
    fn test_task_priority() {
        let low = MigrationTask::new(
            "c".to_string(),
            "i".to_string(),
            PerformanceTier::Warm,
            PerformanceTier::Cold,
            100,
        )
        .with_priority(MigrationPriority::Low);

        let high = MigrationTask::new(
            "c".to_string(),
            "i".to_string(),
            PerformanceTier::Warm,
            PerformanceTier::Cold,
            100,
        )
        .with_priority(MigrationPriority::High);

        assert!(high.priority > low.priority);
    }

    #[test]
    fn test_task_retry() {
        let mut task = MigrationTask::new(
            "c".to_string(),
            "i".to_string(),
            PerformanceTier::Warm,
            PerformanceTier::Cold,
            100,
        )
        .with_max_retries(2);

        assert!(task.can_retry());
        task.retry_count = 1;
        assert!(task.can_retry());
        task.retry_count = 2;
        assert!(!task.can_retry());
    }

    #[tokio::test]
    async fn test_coordinator_submit() {
        let coordinator = MigrationCoordinator::new(2);

        let task = MigrationTask::new(
            "test".to_string(),
            "item1".to_string(),
            PerformanceTier::Hot,
            PerformanceTier::Warm,
            1024,
        );

        coordinator.submit(task).await;

        let pending = coordinator.get_pending().await;
        assert_eq!(pending.len(), 1);

        let stats = coordinator.get_stats().await;
        assert_eq!(stats.tasks_submitted, 1);
        assert_eq!(stats.tasks_pending, 1);
    }

    #[tokio::test]
    async fn test_coordinator_priority_ordering() {
        let coordinator = MigrationCoordinator::new(2);

        let low = MigrationTask::new(
            "c".to_string(),
            "low".to_string(),
            PerformanceTier::Warm,
            PerformanceTier::Cold,
            100,
        )
        .with_priority(MigrationPriority::Low);

        let high = MigrationTask::new(
            "c".to_string(),
            "high".to_string(),
            PerformanceTier::Warm,
            PerformanceTier::Cold,
            100,
        )
        .with_priority(MigrationPriority::High);

        let normal = MigrationTask::new(
            "c".to_string(),
            "normal".to_string(),
            PerformanceTier::Warm,
            PerformanceTier::Cold,
            100,
        )
        .with_priority(MigrationPriority::Normal);

        // Submit in random order
        coordinator.submit(low).await;
        coordinator.submit(high).await;
        coordinator.submit(normal).await;

        let pending = coordinator.get_pending().await;
        assert_eq!(pending[0].item_id, "high");
        assert_eq!(pending[1].item_id, "normal");
        assert_eq!(pending[2].item_id, "low");
    }

    #[tokio::test]
    async fn test_coordinator_next() {
        let coordinator = MigrationCoordinator::new(1);

        let task1 = MigrationTask::new(
            "c".to_string(),
            "item1".to_string(),
            PerformanceTier::Warm,
            PerformanceTier::Cold,
            100,
        );

        let task2 = MigrationTask::new(
            "c".to_string(),
            "item2".to_string(),
            PerformanceTier::Warm,
            PerformanceTier::Cold,
            100,
        );

        coordinator.submit(task1).await;
        coordinator.submit(task2).await;

        // Get first task
        let next = coordinator.next().await;
        assert!(next.is_some());
        assert_eq!(next.unwrap().item_id, "item1");

        // Concurrency limit reached
        let next = coordinator.next().await;
        assert!(next.is_none());
    }

    #[tokio::test]
    async fn test_coordinator_complete() {
        let coordinator = MigrationCoordinator::new(2);

        let task = MigrationTask::new(
            "test".to_string(),
            "item1".to_string(),
            PerformanceTier::Hot,
            PerformanceTier::Warm,
            1024,
        );
        let task_id = task.id.clone();

        coordinator.submit(task).await;
        let _ = coordinator.next().await;

        let result = MigrationResult {
            task_id: task_id.clone(),
            collection: "test".to_string(),
            item_id: "item1".to_string(),
            source_tier: PerformanceTier::Hot,
            target_tier: PerformanceTier::Warm,
            success: true,
            bytes_migrated: 1024,
            duration: Duration::from_millis(100),
            error: None,
        };

        coordinator.complete(&task_id, result).await;

        let stats = coordinator.get_stats().await;
        assert_eq!(stats.tasks_completed, 1);
        assert_eq!(stats.bytes_migrated, 1024);
        assert_eq!(stats.demotions, 1);
    }

    #[tokio::test]
    async fn test_coordinator_cancel() {
        let coordinator = MigrationCoordinator::new(2);

        let task = MigrationTask::new(
            "test".to_string(),
            "item1".to_string(),
            PerformanceTier::Hot,
            PerformanceTier::Warm,
            1024,
        );
        let task_id = task.id.clone();

        coordinator.submit(task).await;

        assert!(coordinator.cancel(&task_id).await);
        assert!(!coordinator.cancel(&task_id).await); // Already cancelled

        let pending = coordinator.get_pending().await;
        assert!(pending.is_empty());
    }

    #[tokio::test]
    async fn test_clear_pending() {
        let coordinator = MigrationCoordinator::new(2);

        for i in 0..5 {
            let task = MigrationTask::new(
                "test".to_string(),
                format!("item{}", i),
                PerformanceTier::Hot,
                PerformanceTier::Warm,
                100,
            );
            coordinator.submit(task).await;
        }

        let cleared = coordinator.clear_pending().await;
        assert_eq!(cleared, 5);

        let pending = coordinator.get_pending().await;
        assert!(pending.is_empty());
    }
}
