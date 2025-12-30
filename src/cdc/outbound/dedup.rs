/*
 * Copyright 2025 Vijaykumar Singh
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

//! Deduplication for CDC events

use std::collections::{HashMap, VecDeque};
use std::sync::RwLock;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::cdc::event::ChangeEvent;

/// Deduplication strategy
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DeduplicationStrategy {
    /// No deduplication
    None,
    /// Deduplicate by event ID
    #[default]
    ByEventId,
    /// Deduplicate by LSN
    ByLsn,
    /// Deduplicate by key (collection + key)
    ByKey,
    /// Deduplicate by content hash
    ByHash,
}

/// Cache for deduplication
pub struct DeduplicationCache {
    /// Maximum cache size
    max_size: usize,
    /// Strategy to use
    strategy: DeduplicationStrategy,
    /// Seen event IDs (LRU-ordered)
    seen_ids: RwLock<VecDeque<String>>,
    /// ID to timestamp mapping
    id_times: RwLock<HashMap<String, Instant>>,
    /// TTL for entries
    ttl: Duration,
    /// Statistics
    stats: RwLock<DeduplicationStats>,
}

/// Statistics for deduplication
#[derive(Debug, Clone, Default)]
pub struct DeduplicationStats {
    /// Number of events checked
    pub checked: u64,
    /// Number of duplicates found
    pub duplicates: u64,
    /// Number of unique events
    pub unique: u64,
    /// Number of evictions (due to cache size)
    pub evictions: u64,
    /// Number of expirations (due to TTL)
    pub expirations: u64,
}

impl DeduplicationCache {
    /// Create a new deduplication cache
    pub fn new(max_size: usize) -> Self {
        Self {
            max_size,
            strategy: DeduplicationStrategy::ByEventId,
            seen_ids: RwLock::new(VecDeque::with_capacity(max_size)),
            id_times: RwLock::new(HashMap::with_capacity(max_size)),
            ttl: Duration::from_secs(3600), // 1 hour default
            stats: RwLock::new(DeduplicationStats::default()),
        }
    }

    /// Set the deduplication strategy
    pub fn with_strategy(mut self, strategy: DeduplicationStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Set the TTL for cache entries
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Check if an event is a duplicate
    pub fn is_duplicate(&self, event: &ChangeEvent) -> bool {
        if matches!(self.strategy, DeduplicationStrategy::None) {
            return false;
        }

        let key = self.get_key(event);
        let mut stats = self.stats.write().unwrap();
        stats.checked += 1;

        // Check if seen
        let id_times = self.id_times.read().unwrap();
        if let Some(time) = id_times.get(&key) {
            // Check TTL
            if time.elapsed() < self.ttl {
                stats.duplicates += 1;
                return true;
            }
        }

        stats.unique += 1;
        false
    }

    /// Mark an event as seen
    pub fn mark_seen(&self, event: &ChangeEvent) {
        if matches!(self.strategy, DeduplicationStrategy::None) {
            return;
        }

        let key = self.get_key(event);
        let now = Instant::now();

        // Evict if necessary
        self.evict_if_needed();

        // Add to seen
        let mut seen_ids = self.seen_ids.write().unwrap();
        let mut id_times = self.id_times.write().unwrap();

        if !id_times.contains_key(&key) {
            seen_ids.push_back(key.clone());
        }
        id_times.insert(key, now);
    }

    /// Check and mark in one operation
    pub fn check_and_mark(&self, event: &ChangeEvent) -> bool {
        if self.is_duplicate(event) {
            true
        } else {
            self.mark_seen(event);
            false
        }
    }

    /// Get the deduplication key for an event
    fn get_key(&self, event: &ChangeEvent) -> String {
        match self.strategy {
            DeduplicationStrategy::None => String::new(),
            DeduplicationStrategy::ByEventId => event.id.to_string(),
            DeduplicationStrategy::ByLsn => format!("lsn:{}", event.lsn),
            DeduplicationStrategy::ByKey => {
                format!("{}:{}", event.collection, event.key)
            }
            DeduplicationStrategy::ByHash => {
                // Simple hash of event content
                let content = format!(
                    "{}-{}-{}-{}",
                    event.collection,
                    event.key,
                    event.operation,
                    event.lsn
                );
                format!("hash:{:x}", simple_hash(&content))
            }
        }
    }

    /// Evict old entries if cache is full
    fn evict_if_needed(&self) {
        let mut seen_ids = self.seen_ids.write().unwrap();
        let mut id_times = self.id_times.write().unwrap();
        let mut stats = self.stats.write().unwrap();
        let now = Instant::now();

        // First, remove expired entries
        let mut to_remove = Vec::new();
        for (key, time) in id_times.iter() {
            if now.duration_since(*time) > self.ttl {
                to_remove.push(key.clone());
                stats.expirations += 1;
            }
        }
        for key in to_remove {
            id_times.remove(&key);
            seen_ids.retain(|k| k != &key);
        }

        // Then, evict oldest if still over capacity
        while seen_ids.len() >= self.max_size {
            if let Some(oldest) = seen_ids.pop_front() {
                id_times.remove(&oldest);
                stats.evictions += 1;
            }
        }
    }

    /// Clear the cache
    pub fn clear(&self) {
        self.seen_ids.write().unwrap().clear();
        self.id_times.write().unwrap().clear();
    }

    /// Get cache size
    pub fn size(&self) -> usize {
        self.seen_ids.read().unwrap().len()
    }

    /// Get statistics
    pub fn stats(&self) -> DeduplicationStats {
        self.stats.read().unwrap().clone()
    }

    /// Get the duplicate rate
    pub fn duplicate_rate(&self) -> f64 {
        let stats = self.stats.read().unwrap();
        if stats.checked == 0 {
            0.0
        } else {
            stats.duplicates as f64 / stats.checked as f64
        }
    }
}

/// Simple hash function (FNV-1a)
fn simple_hash(input: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in input.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdc::event::{Operation, SourceInfo};

    fn create_test_event(id: u64) -> ChangeEvent {
        let mut event = ChangeEvent::new(
            SourceInfo::proximadb("testdb", "server"),
            Operation::Insert,
            "products",
            format!("prod_{}", id),
        );
        event.lsn = id;
        event
    }

    #[test]
    fn test_dedup_cache_creation() {
        let cache = DeduplicationCache::new(1000);
        assert_eq!(cache.size(), 0);
    }

    #[test]
    fn test_dedup_no_strategy() {
        let cache = DeduplicationCache::new(100)
            .with_strategy(DeduplicationStrategy::None);

        let event = create_test_event(1);

        assert!(!cache.is_duplicate(&event));
        cache.mark_seen(&event);
        assert!(!cache.is_duplicate(&event)); // Still not duplicate with None strategy
    }

    #[test]
    fn test_dedup_by_event_id() {
        let cache = DeduplicationCache::new(100)
            .with_strategy(DeduplicationStrategy::ByEventId);

        let event1 = create_test_event(1);
        let event2 = create_test_event(2);

        assert!(!cache.is_duplicate(&event1));
        cache.mark_seen(&event1);
        assert!(cache.is_duplicate(&event1));

        assert!(!cache.is_duplicate(&event2));
        cache.mark_seen(&event2);
        assert!(cache.is_duplicate(&event2));
    }

    #[test]
    fn test_dedup_by_lsn() {
        let cache = DeduplicationCache::new(100)
            .with_strategy(DeduplicationStrategy::ByLsn);

        let event1 = create_test_event(100);
        let mut event2 = create_test_event(100);
        event2.key = "different_key".to_string();

        // Same LSN = duplicate even with different key
        cache.mark_seen(&event1);
        assert!(cache.is_duplicate(&event2));
    }

    #[test]
    fn test_dedup_by_key() {
        let cache = DeduplicationCache::new(100)
            .with_strategy(DeduplicationStrategy::ByKey);

        let event1 = create_test_event(1);
        let mut event2 = create_test_event(2);
        event2.collection = event1.collection.clone();
        event2.key = event1.key.clone();

        // Same collection + key = duplicate
        cache.mark_seen(&event1);
        assert!(cache.is_duplicate(&event2));
    }

    #[test]
    fn test_check_and_mark() {
        let cache = DeduplicationCache::new(100);

        let event = create_test_event(1);

        // First call: not duplicate, marks as seen
        assert!(!cache.check_and_mark(&event));

        // Second call: is duplicate
        assert!(cache.check_and_mark(&event));
    }

    #[test]
    fn test_eviction() {
        let cache = DeduplicationCache::new(3);

        for i in 1..=5 {
            let event = create_test_event(i);
            cache.mark_seen(&event);
        }

        // Should have evicted oldest entries
        assert!(cache.size() <= 3);

        let stats = cache.stats();
        assert!(stats.evictions > 0);
    }

    #[test]
    fn test_stats() {
        let cache = DeduplicationCache::new(100);

        let event = create_test_event(1);

        cache.check_and_mark(&event);
        cache.check_and_mark(&event);
        cache.check_and_mark(&event);

        let stats = cache.stats();
        assert_eq!(stats.checked, 3);
        assert_eq!(stats.unique, 1);
        assert_eq!(stats.duplicates, 2);
    }

    #[test]
    fn test_duplicate_rate() {
        let cache = DeduplicationCache::new(100);

        let event = create_test_event(1);

        // 1 unique, 3 duplicates = 4 total, 75% duplicate rate
        cache.check_and_mark(&event);
        cache.check_and_mark(&event);
        cache.check_and_mark(&event);
        cache.check_and_mark(&event);

        assert!((cache.duplicate_rate() - 0.75).abs() < 0.01);
    }

    #[test]
    fn test_clear() {
        let cache = DeduplicationCache::new(100);

        for i in 1..=10 {
            cache.mark_seen(&create_test_event(i));
        }

        assert_eq!(cache.size(), 10);
        cache.clear();
        assert_eq!(cache.size(), 0);
    }

    #[test]
    fn test_simple_hash() {
        let h1 = simple_hash("test");
        let h2 = simple_hash("test");
        let h3 = simple_hash("other");

        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }
}
