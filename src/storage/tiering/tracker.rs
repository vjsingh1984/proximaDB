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

//! Access Pattern Tracker
//!
//! Tracks access patterns for data items to inform tiering decisions.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

/// An access event for tracking
#[derive(Debug, Clone)]
pub struct AccessEvent {
    /// Item ID
    pub item_id: String,
    /// Collection name
    pub collection: String,
    /// Timestamp of access
    pub timestamp: Instant,
    /// Type of access
    pub access_type: AccessType,
    /// Bytes read/written
    pub bytes: u64,
}

/// Type of data access
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessType {
    /// Read operation
    Read,
    /// Write operation
    Write,
    /// Scan operation
    Scan,
}

/// Tracked access pattern for an item
#[derive(Debug, Clone)]
pub struct AccessPattern {
    /// Total number of accesses
    pub access_count: u64,
    /// Read count
    pub read_count: u64,
    /// Write count
    pub write_count: u64,
    /// Total bytes accessed
    pub total_bytes: u64,
    /// First access time
    pub first_access: Instant,
    /// Last access time
    pub last_access: Instant,
    /// Average interval between accesses (if multiple)
    pub avg_access_interval: Option<Duration>,
}

impl AccessPattern {
    /// Create new pattern with first access
    pub fn new(timestamp: Instant, access_type: AccessType, bytes: u64) -> Self {
        let (read_count, write_count) = match access_type {
            AccessType::Read | AccessType::Scan => (1, 0),
            AccessType::Write => (0, 1),
        };

        Self {
            access_count: 1,
            read_count,
            write_count,
            total_bytes: bytes,
            first_access: timestamp,
            last_access: timestamp,
            avg_access_interval: None,
        }
    }

    /// Record a new access
    pub fn record_access(&mut self, timestamp: Instant, access_type: AccessType, bytes: u64) {
        // Update counts
        self.access_count += 1;
        match access_type {
            AccessType::Read | AccessType::Scan => self.read_count += 1,
            AccessType::Write => self.write_count += 1,
        }
        self.total_bytes += bytes;

        // Update average interval
        let interval = timestamp.duration_since(self.last_access);
        self.avg_access_interval = Some(match self.avg_access_interval {
            Some(avg) => {
                // Exponential moving average
                let alpha = 0.3;
                Duration::from_secs_f64(
                    avg.as_secs_f64() * (1.0 - alpha) + interval.as_secs_f64() * alpha,
                )
            }
            None => interval,
        });

        self.last_access = timestamp;
    }

    /// Get time since last access
    pub fn time_since_last_access(&self) -> Duration {
        self.last_access.elapsed()
    }

    /// Calculate "hotness" score (higher = more active)
    pub fn hotness_score(&self) -> f64 {
        let recency_factor = 1.0 / (1.0 + self.time_since_last_access().as_secs_f64() / 3600.0);
        let frequency_factor = (self.access_count as f64).ln_1p();
        recency_factor * frequency_factor
    }
}

/// Tracks access patterns across collections
pub struct AccessTracker {
    /// Access patterns by (collection, item_id)
    patterns: Arc<RwLock<HashMap<(String, String), AccessPattern>>>,
    /// Configuration
    config: AccessTrackerConfig,
    /// Stats
    stats: Arc<RwLock<TrackerStats>>,
}

/// Configuration for access tracker
#[derive(Debug, Clone)]
pub struct AccessTrackerConfig {
    /// Maximum number of items to track
    pub max_tracked_items: usize,
    /// Eviction interval for old patterns
    pub eviction_interval: Duration,
    /// Age threshold for eviction
    pub eviction_age: Duration,
    /// Enable detailed byte tracking
    pub track_bytes: bool,
}

impl Default for AccessTrackerConfig {
    fn default() -> Self {
        Self {
            max_tracked_items: 100_000,
            eviction_interval: Duration::from_secs(300), // 5 minutes
            eviction_age: Duration::from_secs(7 * 24 * 3600), // 7 days
            track_bytes: true,
        }
    }
}

/// Tracker statistics
#[derive(Debug, Clone, Default)]
pub struct TrackerStats {
    /// Total events recorded
    pub total_events: u64,
    /// Total items tracked
    pub tracked_items: usize,
    /// Events dropped due to capacity
    pub dropped_events: u64,
    /// Last eviction time
    pub last_eviction: Option<Instant>,
    /// Items evicted in last cycle
    pub last_eviction_count: usize,
}

impl AccessTracker {
    /// Create a new access tracker
    pub fn new(config: AccessTrackerConfig) -> Self {
        Self {
            patterns: Arc::new(RwLock::new(HashMap::new())),
            config,
            stats: Arc::new(RwLock::new(TrackerStats::default())),
        }
    }

    /// Record an access event
    pub async fn record(&self, event: AccessEvent) {
        let key = (event.collection.clone(), event.item_id.clone());

        let mut patterns = self.patterns.write().await;

        // Check capacity
        if patterns.len() >= self.config.max_tracked_items {
            let mut stats = self.stats.write().await;
            stats.dropped_events += 1;
            return;
        }

        // Update or create pattern
        if let Some(pattern) = patterns.get_mut(&key) {
            pattern.record_access(event.timestamp, event.access_type, event.bytes);
        } else {
            patterns.insert(
                key,
                AccessPattern::new(event.timestamp, event.access_type, event.bytes),
            );
        }

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.total_events += 1;
            stats.tracked_items = patterns.len();
        }
    }

    /// Get access pattern for an item
    pub async fn get_pattern(&self, collection: &str, item_id: &str) -> Option<AccessPattern> {
        let patterns = self.patterns.read().await;
        patterns
            .get(&(collection.to_string(), item_id.to_string()))
            .cloned()
    }

    /// Get all patterns for a collection
    pub async fn get_collection_patterns(&self, collection: &str) -> Vec<(String, AccessPattern)> {
        let patterns = self.patterns.read().await;
        patterns
            .iter()
            .filter(|((col, _), _)| col == collection)
            .map(|((_, id), pattern)| (id.clone(), pattern.clone()))
            .collect()
    }

    /// Get hottest items across all collections
    pub async fn get_hottest(&self, limit: usize) -> Vec<(String, String, AccessPattern)> {
        let patterns = self.patterns.read().await;

        let mut items: Vec<_> = patterns
            .iter()
            .map(|((col, id), pattern)| (col.clone(), id.clone(), pattern.clone()))
            .collect();

        items.sort_by(|a, b| {
            b.2.hotness_score()
                .partial_cmp(&a.2.hotness_score())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        items.into_iter().take(limit).collect()
    }

    /// Get coldest items across all collections
    pub async fn get_coldest(&self, limit: usize) -> Vec<(String, String, AccessPattern)> {
        let patterns = self.patterns.read().await;

        let mut items: Vec<_> = patterns
            .iter()
            .map(|((col, id), pattern)| (col.clone(), id.clone(), pattern.clone()))
            .collect();

        items.sort_by(|a, b| {
            a.2.hotness_score()
                .partial_cmp(&b.2.hotness_score())
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        items.into_iter().take(limit).collect()
    }

    /// Evict old patterns
    pub async fn evict_old(&self) -> usize {
        let now = Instant::now();
        let mut patterns = self.patterns.write().await;

        let before = patterns.len();
        patterns.retain(|(_, _), pattern| {
            now.duration_since(pattern.last_access) < self.config.eviction_age
        });
        let evicted = before - patterns.len();

        // Update stats
        {
            let mut stats = self.stats.write().await;
            stats.last_eviction = Some(now);
            stats.last_eviction_count = evicted;
            stats.tracked_items = patterns.len();
        }

        evicted
    }

    /// Get tracker stats
    pub async fn get_stats(&self) -> TrackerStats {
        self.stats.read().await.clone()
    }

    /// Clear all patterns
    pub async fn clear(&self) {
        let mut patterns = self.patterns.write().await;
        patterns.clear();

        let mut stats = self.stats.write().await;
        stats.tracked_items = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_access_pattern_new() {
        let pattern = AccessPattern::new(Instant::now(), AccessType::Read, 1024);
        assert_eq!(pattern.access_count, 1);
        assert_eq!(pattern.read_count, 1);
        assert_eq!(pattern.write_count, 0);
        assert_eq!(pattern.total_bytes, 1024);
    }

    #[test]
    fn test_access_pattern_record() {
        let mut pattern = AccessPattern::new(Instant::now(), AccessType::Read, 1024);
        std::thread::sleep(Duration::from_millis(10));
        pattern.record_access(Instant::now(), AccessType::Write, 2048);

        assert_eq!(pattern.access_count, 2);
        assert_eq!(pattern.read_count, 1);
        assert_eq!(pattern.write_count, 1);
        assert_eq!(pattern.total_bytes, 3072);
        assert!(pattern.avg_access_interval.is_some());
    }

    #[test]
    fn test_hotness_score() {
        let pattern = AccessPattern::new(Instant::now(), AccessType::Read, 1024);
        let score = pattern.hotness_score();
        assert!(score > 0.0);
    }

    #[tokio::test]
    async fn test_tracker_record_and_get() {
        let tracker = AccessTracker::new(AccessTrackerConfig::default());

        let event = AccessEvent {
            item_id: "item1".to_string(),
            collection: "test".to_string(),
            timestamp: Instant::now(),
            access_type: AccessType::Read,
            bytes: 1024,
        };

        tracker.record(event).await;

        let pattern = tracker.get_pattern("test", "item1").await;
        assert!(pattern.is_some());
        assert_eq!(pattern.unwrap().access_count, 1);
    }

    #[tokio::test]
    async fn test_tracker_get_hottest() {
        let tracker = AccessTracker::new(AccessTrackerConfig::default());

        // Add multiple items with different access patterns
        for i in 0..5 {
            for _ in 0..i + 1 {
                tracker
                    .record(AccessEvent {
                        item_id: format!("item{}", i),
                        collection: "test".to_string(),
                        timestamp: Instant::now(),
                        access_type: AccessType::Read,
                        bytes: 1024,
                    })
                    .await;
            }
        }

        let hottest = tracker.get_hottest(3).await;
        assert_eq!(hottest.len(), 3);
        // item4 should be hottest (5 accesses)
        assert_eq!(hottest[0].1, "item4");
    }

    #[tokio::test]
    async fn test_tracker_stats() {
        let tracker = AccessTracker::new(AccessTrackerConfig::default());

        tracker
            .record(AccessEvent {
                item_id: "item1".to_string(),
                collection: "test".to_string(),
                timestamp: Instant::now(),
                access_type: AccessType::Read,
                bytes: 1024,
            })
            .await;

        let stats = tracker.get_stats().await;
        assert_eq!(stats.total_events, 1);
        assert_eq!(stats.tracked_items, 1);
    }
}
