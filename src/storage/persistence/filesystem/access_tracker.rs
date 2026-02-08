//! Access Pattern Tracker for Unified Filesystem
//!
//! Tracks file access patterns to identify hot files, predict future accesses,
//! and optimize caching decisions.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::RwLock;
use tracing::{debug, trace};

use crate::storage::persistence::filesystem::unified::AccessOperation;

/// Tracks access patterns for intelligent caching decisions
pub struct AccessPatternTracker {
    /// Access history per file
    access_history: Arc<DashMap<String, FileAccessHistory>>,

    /// Global access statistics
    global_stats: Arc<RwLock<GlobalAccessStats>>,

    /// Configuration
    hot_threshold: u32,
    window_duration: Duration,
}

/// Access history for a single file
#[derive(Debug, Clone)]
struct FileAccessHistory {
    /// Recent access times
    access_times: VecDeque<Instant>,

    /// Access count
    total_accesses: u64,

    /// Access types
    read_count: u64,
    write_count: u64,
    metadata_count: u64,

    /// Last operation type
    last_operation: AccessOperation,

    /// File size (if known)
    file_size: Option<usize>,
}

/// Global access statistics
#[derive(Debug, Default)]
struct GlobalAccessStats {
    total_files_accessed: usize,
    total_accesses: u64,
    hot_files: Vec<String>,
}

impl AccessPatternTracker {
    /// Create new access pattern tracker
    pub fn new() -> Self {
        Self {
            access_history: Arc::new(DashMap::new()),
            global_stats: Arc::new(RwLock::new(GlobalAccessStats::default())),
            hot_threshold: 5, // File is hot if accessed 5+ times in window
            window_duration: Duration::from_secs(300), // 5 minute window
        }
    }

    /// Record a file access
    pub async fn record(&self, path: &str, operation: AccessOperation) {
        let now = Instant::now();

        // Update file-specific history
        let mut entry = self
            .access_history
            .entry(path.to_string())
            .or_insert_with(|| FileAccessHistory {
                access_times: VecDeque::new(),
                total_accesses: 0,
                read_count: 0,
                write_count: 0,
                metadata_count: 0,
                last_operation: operation.clone(),
                file_size: None,
            });

        // Update counts
        entry.total_accesses += 1;
        match operation {
            AccessOperation::Read | AccessOperation::RangeRead => entry.read_count += 1,
            AccessOperation::Write => entry.write_count += 1,
            AccessOperation::Metadata => entry.metadata_count += 1,
        }

        // Add to access times and clean old entries
        entry.access_times.push_back(now);
        while let Some(front) = entry.access_times.front() {
            if now.duration_since(*front) > self.window_duration {
                entry.access_times.pop_front();
            } else {
                break;
            }
        }

        entry.last_operation = operation;

        // Update global stats
        let mut global = self.global_stats.write().await;
        global.total_accesses += 1;

        // Update hot files list
        if entry.access_times.len() >= self.hot_threshold as usize {
            if !global.hot_files.contains(&path.to_string()) {
                global.hot_files.push(path.to_string());
                debug!("File {} marked as hot", path);
            }
        }

        trace!(
            "Recorded {} access for {}, total: {}",
            match operation {
                AccessOperation::Read => "read",
                AccessOperation::Write => "write",
                AccessOperation::RangeRead => "range read",
                AccessOperation::Metadata => "metadata",
            },
            path,
            entry.total_accesses
        );
    }

    /// Check if a file is hot (frequently accessed)
    pub fn is_hot(&self, path: &str) -> bool {
        if let Some(entry) = self.access_history.get(path) {
            entry.access_times.len() >= self.hot_threshold as usize
        } else {
            false
        }
    }

    /// Get access frequency for a file
    pub fn get_access_frequency(&self, path: &str) -> AccessFrequency {
        if let Some(entry) = self.access_history.get(path) {
            let recent_count = entry.access_times.len();

            if recent_count >= 10 {
                AccessFrequency::VeryHigh
            } else if recent_count >= self.hot_threshold as usize {
                AccessFrequency::High
            } else if recent_count >= 2 {
                AccessFrequency::Medium
            } else if recent_count >= 1 {
                AccessFrequency::Low
            } else {
                AccessFrequency::Cold
            }
        } else {
            AccessFrequency::Cold
        }
    }

    /// Predict if a file will be accessed soon
    pub async fn predict_access(&self, path: &str) -> AccessPrediction {
        if let Some(entry) = self.access_history.get(path) {
            // Simple prediction based on recent access pattern
            let recent_count = entry.access_times.len();

            if recent_count >= 3 {
                // Calculate average time between accesses
                if entry.access_times.len() >= 2 {
                    let mut intervals = Vec::new();
                    for i in 1..entry.access_times.len() {
                        let interval =
                            entry.access_times[i].duration_since(entry.access_times[i - 1]);
                        intervals.push(interval);
                    }

                    let avg_interval = intervals.iter().sum::<Duration>() / intervals.len() as u32;
                    let last_access = entry.access_times.back().unwrap();
                    let time_since_last = Instant::now().duration_since(*last_access);

                    if time_since_last < avg_interval * 2 {
                        return AccessPrediction::Likely(0.8);
                    } else if time_since_last < avg_interval * 4 {
                        return AccessPrediction::Possible(0.5);
                    }
                }

                AccessPrediction::Possible(0.3)
            } else if recent_count >= 1 {
                AccessPrediction::Unlikely(0.2)
            } else {
                AccessPrediction::Unlikely(0.1)
            }
        } else {
            AccessPrediction::Unlikely(0.0)
        }
    }

    /// Get files that are frequently accessed together
    pub async fn get_correlated_files(&self, path: &str) -> Vec<String> {
        // Track files accessed within a short time window
        let _now = Instant::now();
        let window = Duration::from_secs(10); // 10 second correlation window

        // Find recent accesses to the given file
        if let Some(entry) = self.access_history.get(path) {
            if let Some(last_access) = entry.access_times.back() {
                // Find other files accessed around the same time
                let mut correlated = Vec::new();

                for other_entry in self.access_history.iter() {
                    if other_entry.key() == path {
                        continue;
                    }

                    // Check if this file was accessed near our target file
                    for access_time in &other_entry.value().access_times {
                        if access_time.duration_since(*last_access) < window
                            || last_access.duration_since(*access_time) < window
                        {
                            correlated.push(other_entry.key().clone());
                            break;
                        }
                    }
                }

                // Sort by access frequency
                correlated.sort_by_key(|k| {
                    self.access_history
                        .get(k)
                        .map(|e| std::cmp::Reverse(e.total_accesses))
                        .unwrap_or(std::cmp::Reverse(0))
                });

                // Return top 5 correlated files
                correlated.truncate(5);
                return correlated;
            }
        }

        vec![]
    }

    /// Analyze access patterns for a collection
    pub async fn analyze_collection_patterns(
        &self,
        collection_prefix: &str,
    ) -> CollectionAccessPattern {
        let mut total_accesses = 0u64;
        let mut total_reads = 0u64;
        let mut total_writes = 0u64;
        let mut file_count = 0usize;

        for entry in self.access_history.iter() {
            if entry.key().starts_with(collection_prefix) {
                file_count += 1;
                total_accesses += entry.value().total_accesses;
                total_reads += entry.value().read_count;
                total_writes += entry.value().write_count;
            }
        }

        CollectionAccessPattern {
            collection_prefix: collection_prefix.to_string(),
            total_accesses,
            read_write_ratio: if total_writes > 0 {
                total_reads as f64 / total_writes as f64
            } else {
                f64::INFINITY
            },
            file_count,
            is_read_heavy: total_reads > total_writes * 10,
            is_write_heavy: total_writes > total_reads * 10,
        }
    }

    /// Clean up old access records
    pub async fn cleanup(&self) {
        let now = Instant::now();
        let mut to_remove = Vec::new();

        // Find entries with no recent accesses
        for entry in self.access_history.iter() {
            if let Some(last) = entry.value().access_times.back() {
                if now.duration_since(*last) > Duration::from_secs(3600) {
                    to_remove.push(entry.key().clone());
                }
            }
        }

        // Remove stale entries
        for key in to_remove {
            self.access_history.remove(&key);
        }

        // Update global stats
        let mut global = self.global_stats.write().await;
        global.hot_files.retain(|path| self.is_hot(path));
    }

    /// Get access statistics
    pub async fn get_stats(&self) -> AccessPatternStats {
        let global = self.global_stats.read().await;

        AccessPatternStats {
            total_files: self.access_history.len(),
            total_accesses: global.total_accesses,
            hot_files: global.hot_files.clone(),
        }
    }
}

/// Access frequency classification
#[derive(Debug, Clone, PartialEq)]
pub enum AccessFrequency {
    VeryHigh,
    High,
    Medium,
    Low,
    Cold,
}

/// Access prediction
#[derive(Debug, Clone)]
pub enum AccessPrediction {
    Likely(f64),   // > 70% probability
    Possible(f64), // 30-70% probability
    Unlikely(f64), // < 30% probability
}

/// Access pattern statistics
#[derive(Debug, Clone)]
pub struct AccessPatternStats {
    pub total_files: usize,
    pub total_accesses: u64,
    pub hot_files: Vec<String>,
}

/// Collection-specific access pattern
#[derive(Debug, Clone)]
pub struct CollectionAccessPattern {
    pub collection_prefix: String,
    pub total_accesses: u64,
    pub read_write_ratio: f64,
    pub file_count: usize,
    pub is_read_heavy: bool,
    pub is_write_heavy: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hot_file_detection() {
        let tracker = AccessPatternTracker::new();

        // Record multiple accesses
        for _ in 0..6 {
            tracker
                .record("hot_file.parquet", AccessOperation::Read)
                .await;
        }

        assert!(tracker.is_hot("hot_file.parquet"));
        assert!(!tracker.is_hot("cold_file.parquet"));
    }

    #[tokio::test]
    async fn test_access_frequency() {
        let tracker = AccessPatternTracker::new();

        tracker.record("file1.parquet", AccessOperation::Read).await;
        assert_eq!(
            tracker.get_access_frequency("file1.parquet"),
            AccessFrequency::Low
        );

        for _ in 0..5 {
            tracker.record("file2.parquet", AccessOperation::Read).await;
        }
        assert_eq!(
            tracker.get_access_frequency("file2.parquet"),
            AccessFrequency::High
        );
    }
}
