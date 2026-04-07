//! Prefetch Engine for Unified Filesystem
//!
//! Intelligently prefetches files based on access patterns to improve
//! read performance and reduce latency.

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::RwLock;
use tokio::time::sleep;
use tracing::debug;

use super::access_tracker::{AccessPatternTracker, AccessPrediction};

/// Prefetch engine for predictive file loading
pub struct PrefetchEngine {
    /// Whether prefetching is enabled
    enabled: bool,

    /// Files currently being prefetched
    prefetching: Arc<DashMap<String, PrefetchStatus>>,

    /// Prefetch queue
    queue: Arc<RwLock<Vec<PrefetchRequest>>>,

    /// Statistics
    stats: Arc<PrefetchStats>,
}

/// Prefetch request
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct PrefetchRequest {
    path: String,
    priority: PrefetchPriority,
    predicted_probability: f64,
}

/// Prefetch priority
#[derive(Debug, Clone, PartialEq, PartialOrd)]
#[allow(dead_code)]
enum PrefetchPriority {
    High = 3,
    Medium = 2,
    Low = 1,
}

/// Prefetch status
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum PrefetchStatus {
    InProgress,
    Completed,
    Failed(String),
}

/// Prefetch statistics
#[derive(Debug, Default)]
struct PrefetchStats {
    total_prefetches: std::sync::atomic::AtomicU64,
    successful_prefetches: std::sync::atomic::AtomicU64,
    useful_prefetches: std::sync::atomic::AtomicU64, // Actually used after prefetch
    wasted_prefetches: std::sync::atomic::AtomicU64, // Prefetched but not used
}

impl PrefetchEngine {
    /// Create new prefetch engine
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            prefetching: Arc::new(DashMap::new()),
            queue: Arc::new(RwLock::new(Vec::new())),
            stats: Arc::new(PrefetchStats::default()),
        }
    }

    /// Check if prefetching should occur based on access patterns
    pub async fn maybe_prefetch(&self, path: &str, tracker: &AccessPatternTracker) {
        if !self.enabled {
            return;
        }

        // Check if already prefetching this file
        if self.prefetching.contains_key(path) {
            return;
        }

        // Get access prediction
        let prediction = tracker.predict_access(path).await;

        // Determine if we should prefetch
        let should_prefetch = match &prediction {
            AccessPrediction::Likely(p) => *p > 0.7,
            AccessPrediction::Possible(p) => *p > 0.5 && self.has_capacity().await,
            AccessPrediction::Unlikely(_) => false,
        };

        if should_prefetch {
            let priority = match prediction {
                AccessPrediction::Likely(_) => PrefetchPriority::High,
                AccessPrediction::Possible(_) => PrefetchPriority::Medium,
                AccessPrediction::Unlikely(_) => PrefetchPriority::Low,
            };

            let probability = match prediction {
                AccessPrediction::Likely(p)
                | AccessPrediction::Possible(p)
                | AccessPrediction::Unlikely(p) => p,
            };

            self.queue_prefetch(path, priority, probability).await;
        }

        // Also check for correlated files
        let correlated = tracker.get_correlated_files(path).await;
        for correlated_path in correlated {
            if !self.prefetching.contains_key(&correlated_path) {
                self.queue_prefetch(&correlated_path, PrefetchPriority::Low, 0.4)
                    .await;
            }
        }
    }

    /// Queue a file for prefetching
    async fn queue_prefetch(&self, path: &str, priority: PrefetchPriority, probability: f64) {
        let mut queue = self.queue.write().await;

        // Check if already in queue
        if queue.iter().any(|r| r.path == path) {
            return;
        }

        debug!(
            "Queuing {} for prefetch (priority: {:?}, prob: {:.2})",
            path, priority, probability
        );

        queue.push(PrefetchRequest {
            path: path.to_string(),
            priority,
            predicted_probability: probability,
        });

        // Sort by priority (highest first)
        queue.sort_by(|a, b| {
            b.priority
                .partial_cmp(&a.priority)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Limit queue size
        if queue.len() > 100 {
            queue.truncate(50);
        }
    }

    /// Process prefetch queue (should be called periodically)
    pub async fn process_queue(&self) -> Vec<String> {
        if !self.enabled {
            return vec![];
        }

        let mut queue = self.queue.write().await;
        let mut to_prefetch = Vec::new();

        // Sort queue by priority (high priority first)
        queue.sort_by(|a, b| {
            b.priority
                .partial_cmp(&a.priority)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take up to 5 items from queue (highest priority first)
        while to_prefetch.len() < 5 && !queue.is_empty() {
            let request = queue.remove(0); // Remove from front (highest priority)
            // Mark as in progress
            self.prefetching
                .insert(request.path.clone(), PrefetchStatus::InProgress);
            to_prefetch.push(request.path);

            self.stats
                .total_prefetches
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        to_prefetch
    }

    /// Mark prefetch as completed
    pub async fn mark_completed(&self, path: &str, success: bool) {
        if success {
            self.prefetching
                .insert(path.to_string(), PrefetchStatus::Completed);
            self.stats
                .successful_prefetches
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        } else {
            self.prefetching.insert(
                path.to_string(),
                PrefetchStatus::Failed("Failed to prefetch".to_string()),
            );
        }

        // Clean up old entries after some time
        let path = path.to_string();
        let prefetching = self.prefetching.clone();
        tokio::spawn(async move {
            sleep(Duration::from_secs(300)).await;
            prefetching.remove(&path);
        });
    }

    /// Check if a file was prefetched (mark as useful if it was)
    pub fn was_prefetched(&self, path: &str) -> bool {
        if let Some(entry) = self.prefetching.get(path)
            && matches!(entry.value(), PrefetchStatus::Completed)
        {
            self.stats
                .useful_prefetches
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return true;
        }
        false
    }

    /// Check if we have capacity for more prefetches
    async fn has_capacity(&self) -> bool {
        // Limit concurrent prefetches
        let in_progress = self
            .prefetching
            .iter()
            .filter(|entry| matches!(entry.value(), PrefetchStatus::InProgress))
            .count();

        in_progress < 10
    }

    /// Clean up wasted prefetches
    pub async fn cleanup(&self) {
        let mut wasted = 0;

        // Find completed prefetches that weren't used
        let to_remove: Vec<String> = self
            .prefetching
            .iter()
            .filter_map(|entry| {
                if matches!(entry.value(), PrefetchStatus::Completed) {
                    wasted += 1;
                    Some(entry.key().clone())
                } else {
                    None
                }
            })
            .collect();

        for key in to_remove {
            self.prefetching.remove(&key);
        }

        if wasted > 0 {
            self.stats
                .wasted_prefetches
                .fetch_add(wasted, std::sync::atomic::Ordering::Relaxed);
            debug!("Cleaned up {} wasted prefetches", wasted);
        }
    }

    /// Get prefetch statistics
    pub fn stats(&self) -> PrefetchStatistics {
        PrefetchStatistics {
            total_prefetches: self
                .stats
                .total_prefetches
                .load(std::sync::atomic::Ordering::Relaxed),
            successful_prefetches: self
                .stats
                .successful_prefetches
                .load(std::sync::atomic::Ordering::Relaxed),
            useful_prefetches: self
                .stats
                .useful_prefetches
                .load(std::sync::atomic::Ordering::Relaxed),
            wasted_prefetches: self
                .stats
                .wasted_prefetches
                .load(std::sync::atomic::Ordering::Relaxed),
            queue_size: 0, // Will be updated when needed
        }
    }

    /// Enable or disable prefetching
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            // Clear queue if disabled
            let queue = self.queue.try_write();
            if let Ok(mut q) = queue {
                q.clear();
            }
        }
    }
}

/// Public prefetch statistics
#[derive(Debug, Clone)]
pub struct PrefetchStatistics {
    pub total_prefetches: u64,
    pub successful_prefetches: u64,
    pub useful_prefetches: u64,
    pub wasted_prefetches: u64,
    pub queue_size: usize,
}

impl PrefetchStatistics {
    /// Calculate effectiveness ratio
    pub fn effectiveness_ratio(&self) -> f64 {
        if self.successful_prefetches == 0 {
            0.0
        } else {
            self.useful_prefetches as f64 / self.successful_prefetches as f64
        }
    }

    /// Calculate waste ratio
    pub fn waste_ratio(&self) -> f64 {
        if self.successful_prefetches == 0 {
            0.0
        } else {
            self.wasted_prefetches as f64 / self.successful_prefetches as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_prefetch_queue() {
        let engine = PrefetchEngine::new(true);

        // Queue multiple items with different priorities
        engine
            .queue_prefetch("file1.parquet", PrefetchPriority::Low, 0.3)
            .await;
        engine
            .queue_prefetch("file2.parquet", PrefetchPriority::High, 0.9)
            .await;
        engine
            .queue_prefetch("file3.parquet", PrefetchPriority::Medium, 0.6)
            .await;

        let to_prefetch = engine.process_queue().await;

        // Should process high priority first
        assert!(!to_prefetch.is_empty());
        assert_eq!(to_prefetch[0], "file2.parquet");
    }

    #[tokio::test]
    async fn test_prefetch_tracking() {
        let engine = PrefetchEngine::new(true);

        engine.mark_completed("file1.parquet", true).await;
        assert!(engine.was_prefetched("file1.parquet"));
        assert!(!engine.was_prefetched("file2.parquet"));
    }
}
