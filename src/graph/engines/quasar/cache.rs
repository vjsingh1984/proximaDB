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

//! # QUASAR Access Pattern Cache Module
//!
//! Tracks access patterns for nodes and edges to make intelligent tiering decisions.
//! Uses LRU eviction policy with access frequency and recency tracking.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{Duration, Instant};
use crate::storage::cache::orchestrator::{CacheStatsProvider, UsageStats};

/// Tracks access patterns for intelligent hot/cold tiering
#[derive(Debug)]
pub struct AccessPatternCache {
    /// Access information for each item
    access_data: Arc<RwLock<HashMap<String, AccessInfo>>>,
    /// LRU ordering for eviction
    lru_order: Arc<RwLock<VecDeque<String>>>,
    /// Maximum number of items to track
    max_size: usize,
    /// Cache statistics
    stats: Arc<RwLock<AccessStats>>,
}

/// Access information for a single item
#[derive(Debug, Clone)]
pub struct AccessInfo {
    /// When this item was last accessed
    pub last_access: Instant,
    /// How many times this item has been accessed
    pub access_count: u32,
    /// First time this item was accessed
    pub first_access: Instant,
    /// Access frequency (accesses per minute)
    pub access_frequency: f64,
    /// Recent access pattern (last 10 accesses)
    pub recent_accesses: VecDeque<Instant>,
}

/// Statistics for access pattern tracking
#[derive(Debug, Default, Clone)]
pub struct AccessStats {
    pub total_accesses: u64,
    pub unique_items_tracked: u64,
    pub cache_evictions: u64,
    pub hot_candidates: u64,
    pub cold_candidates: u64,
    pub average_access_frequency: f64,
}

impl AccessPatternCache {
    /// Create a new access pattern cache
    pub fn new(max_size: usize) -> Self {
        Self {
            access_data: Arc::new(RwLock::new(HashMap::new())),
            lru_order: Arc::new(RwLock::new(VecDeque::new())),
            max_size,
            stats: Arc::new(RwLock::new(AccessStats::default())),
        }
    }

    /// Record an access for an item
    pub async fn record_access(&self, item_id: &str, access_time: Instant) {
        let mut access_data = self.access_data.write().await;
        let mut lru_order = self.lru_order.write().await;
        let mut stats = self.stats.write().await;

        // Update statistics
        stats.total_accesses += 1;

        // Check if item already exists
        if let Some(access_info) = access_data.get_mut(item_id) {
            // Update existing access info
            access_info.last_access = access_time;
            access_info.access_count += 1;

            // Update recent accesses (keep last 10)
            access_info.recent_accesses.push_back(access_time);
            if access_info.recent_accesses.len() > 10 {
                access_info.recent_accesses.pop_front();
            }

            // Recalculate access frequency
            access_info.access_frequency = self.calculate_access_frequency(access_info);

            // Move to end of LRU (most recently used)
            lru_order.retain(|id| id != item_id);
            lru_order.push_back(item_id.to_string());
        } else {
            // New item
            let access_info = AccessInfo {
                last_access: access_time,
                access_count: 1,
                first_access: access_time,
                access_frequency: 0.0, // Will be calculated after more accesses
                recent_accesses: {
                    let mut deque = VecDeque::new();
                    deque.push_back(access_time);
                    deque
                },
            };

            access_data.insert(item_id.to_string(), access_info);
            lru_order.push_back(item_id.to_string());

            stats.unique_items_tracked += 1;

            // Check if we need to evict old items
            if access_data.len() > self.max_size {
                if let Some(oldest_item) = lru_order.pop_front() {
                    access_data.remove(&oldest_item);
                    stats.cache_evictions += 1;
                    stats.unique_items_tracked -= 1;
                }
            }
        }

        // Update average access frequency
        self.update_average_access_frequency(&mut stats, &access_data)
            .await;
    }

    /// Calculate access frequency for an item (accesses per minute)
    fn calculate_access_frequency(&self, access_info: &AccessInfo) -> f64 {
        if access_info.access_count <= 1 {
            return 0.0;
        }

        let time_span = access_info
            .last_access
            .duration_since(access_info.first_access);
        let time_span_minutes = time_span.as_secs_f64() / 60.0;

        if time_span_minutes < 0.01 {
            // Very short time span, use recent accesses
            return access_info.access_count as f64 * 60.0; // Assume all in one minute
        }

        access_info.access_count as f64 / time_span_minutes
    }

    /// Check if an item should be promoted to hot tier
    pub async fn should_promote(&self, item_id: &str, promotion_threshold: Duration) -> bool {
        let access_data = self.access_data.read().await;

        if let Some(access_info) = access_data.get(item_id) {
            let time_since_access = Instant::now().duration_since(access_info.last_access);

            // Promote if accessed recently or has high frequency
            time_since_access < promotion_threshold || access_info.access_frequency > 1.0
        } else {
            false
        }
    }

    /// Check if an item should be demoted to cold tier
    pub async fn should_demote(&self, item_id: &str, demotion_threshold: Duration) -> bool {
        let access_data = self.access_data.read().await;

        if let Some(access_info) = access_data.get(item_id) {
            let time_since_access = Instant::now().duration_since(access_info.last_access);

            // Demote if not accessed recently and has low frequency
            time_since_access > demotion_threshold && access_info.access_frequency < 0.1
        } else {
            // No access info means it's a candidate for demotion
            true
        }
    }

    /// Get access info for an item
    pub async fn get_access_info(&self, item_id: &str) -> Option<AccessInfo> {
        let access_data = self.access_data.read().await;
        access_data.get(item_id).cloned()
    }

    /// Get all access patterns (for migration decisions)
    pub async fn get_all_access_patterns(&self) -> HashMap<String, AccessInfo> {
        let access_data = self.access_data.read().await;
        access_data.clone()
    }

    /// Get hot candidates (frequently accessed items)
    pub async fn get_hot_candidates(&self, min_frequency: f64) -> Vec<String> {
        let access_data = self.access_data.read().await;

        access_data
            .iter()
            .filter(|(_, info)| info.access_frequency >= min_frequency)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Get cold candidates (infrequently accessed items)
    pub async fn get_cold_candidates(&self, max_frequency: f64, max_age: Duration) -> Vec<String> {
        let access_data = self.access_data.read().await;
        let now = Instant::now();

        access_data
            .iter()
            .filter(|(_, info)| {
                let age = now.duration_since(info.last_access);
                info.access_frequency <= max_frequency && age >= max_age
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Update average access frequency statistic
    async fn update_average_access_frequency(
        &self,
        stats: &mut AccessStats,
        access_data: &HashMap<String, AccessInfo>,
    ) {
        if access_data.is_empty() {
            stats.average_access_frequency = 0.0;
            return;
        }

        let total_frequency: f64 = access_data.values().map(|info| info.access_frequency).sum();

        stats.average_access_frequency = total_frequency / access_data.len() as f64;

        // Count hot and cold candidates
        stats.hot_candidates = access_data
            .values()
            .filter(|info| info.access_frequency > stats.average_access_frequency)
            .count() as u64;

        stats.cold_candidates = access_data
            .values()
            .filter(|info| info.access_frequency < stats.average_access_frequency * 0.5)
            .count() as u64;
    }

    /// Get access pattern statistics
    pub async fn get_stats(&self) -> AccessStats {
        let stats = self.stats.read().await;
        stats.clone()
    }

    /// Clear old access data
    pub async fn cleanup_old_data(&self, max_age: Duration) {
        let mut access_data = self.access_data.write().await;
        let mut lru_order = self.lru_order.write().await;
        let mut stats = self.stats.write().await;

        let now = Instant::now();
        let mut items_to_remove = Vec::new();

        for (item_id, access_info) in access_data.iter() {
            let age = now.duration_since(access_info.last_access);
            if age > max_age {
                items_to_remove.push(item_id.clone());
            }
        }

        for item_id in items_to_remove {
            access_data.remove(&item_id);
            lru_order.retain(|id| id != &item_id);
            stats.unique_items_tracked -= 1;
        }
    }

    /// Get access pattern summary for an item
    pub async fn get_access_summary(&self, item_id: &str) -> Option<AccessSummary> {
        let access_data = self.access_data.read().await;

        if let Some(access_info) = access_data.get(item_id) {
            let now = Instant::now();
            let age = now.duration_since(access_info.first_access);
            let recency = now.duration_since(access_info.last_access);

            Some(AccessSummary {
                item_id: item_id.to_string(),
                total_accesses: access_info.access_count,
                access_frequency: access_info.access_frequency,
                age_seconds: age.as_secs(),
                recency_seconds: recency.as_secs(),
                tier_recommendation: self.recommend_tier(access_info, recency),
            })
        } else {
            None
        }
    }

    /// Recommend tier for an item based on access patterns
    fn recommend_tier(&self, access_info: &AccessInfo, recency: Duration) -> TierRecommendation {
        // High frequency or recent access -> Hot
        if access_info.access_frequency > 1.0 || recency < Duration::from_secs(300) {
            TierRecommendation::Hot
        }
        // Very low frequency and old -> Cold
        else if access_info.access_frequency < 0.01 && recency > Duration::from_secs(3600) {
            TierRecommendation::Cold
        }
        // In between -> Keep current
        else {
            TierRecommendation::Current
        }
    }

    /// Batch record multiple accesses (for performance)
    pub async fn batch_record_accesses(&self, accesses: Vec<(String, Instant)>) {
        for (item_id, access_time) in accesses {
            self.record_access(&item_id, access_time).await;
        }
    }

    /// Get access pattern heatmap (for visualization)
    pub async fn get_access_heatmap(&self, time_buckets: usize) -> AccessHeatmap {
        let access_data = self.access_data.read().await;
        let now = Instant::now();

        let mut buckets = vec![0u32; time_buckets];
        let bucket_duration = Duration::from_secs(3600); // 1 hour buckets

        for access_info in access_data.values() {
            // Place each recent access in appropriate time bucket
            for &access_time in &access_info.recent_accesses {
                let age = now.duration_since(access_time);
                let bucket_index = (age.as_secs() / bucket_duration.as_secs()) as usize;

                if bucket_index < time_buckets {
                    buckets[bucket_index] += 1;
                }
            }
        }

        AccessHeatmap {
            buckets,
            bucket_duration_seconds: bucket_duration.as_secs(),
            total_items: access_data.len(),
        }
    }
}

/// Orchestrator provider exposing QUASAR access pattern cache stats
pub struct QuasarAccessCacheStatsProvider {
    cache: Arc<AccessPatternCache>,
}

impl QuasarAccessCacheStatsProvider {
    pub fn new(cache: Arc<AccessPatternCache>) -> Self { Self { cache } }
}

impl CacheStatsProvider for QuasarAccessCacheStatsProvider {
    fn snapshot(&self) -> UsageStats {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let stats = handle.block_on(self.cache.stats.read());
            // Approximate access frequency via average_access_frequency (per minute)
            let access_frequency = stats.average_access_frequency.max(0.0);
            // Hit rate unknown in this cache; report 0.0 for now
            let hit_rate = 0.0;
            // Average entry size not applicable; use small default
            let avg_entry_size = 1024;
            return UsageStats {
                hit_rate,
                avg_entry_size,
                access_frequency,
                last_rebalance: std::time::SystemTime::now(),
            };
        }
        UsageStats { hit_rate: 0.0, avg_entry_size: 1024, access_frequency: 0.0, last_rebalance: std::time::SystemTime::now() }
    }
}

/// Summary of access patterns for an item
#[derive(Debug, Clone)]
pub struct AccessSummary {
    pub item_id: String,
    pub total_accesses: u32,
    pub access_frequency: f64,
    pub age_seconds: u64,
    pub recency_seconds: u64,
    pub tier_recommendation: TierRecommendation,
}

/// Tier recommendation based on access patterns
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierRecommendation {
    Hot,
    Cold,
    Current,
}

/// Access pattern heatmap for visualization
#[derive(Debug, Clone)]
pub struct AccessHeatmap {
    pub buckets: Vec<u32>,
    pub bucket_duration_seconds: u64,
    pub total_items: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{Duration, sleep};

    #[tokio::test]
    async fn test_cache_creation() {
        let cache = AccessPatternCache::new(100);
        let stats = cache.get_stats().await;

        assert_eq!(stats.total_accesses, 0);
        assert_eq!(stats.unique_items_tracked, 0);
    }

    #[tokio::test]
    async fn test_record_access() {
        let cache = AccessPatternCache::new(100);
        let now = Instant::now();

        cache.record_access("item1", now).await;

        let stats = cache.get_stats().await;
        assert_eq!(stats.total_accesses, 1);
        assert_eq!(stats.unique_items_tracked, 1);

        let access_info = cache.get_access_info("item1").await.unwrap();
        assert_eq!(access_info.access_count, 1);
        assert_eq!(access_info.last_access, now);
    }

    #[tokio::test]
    async fn test_multiple_accesses() {
        let cache = AccessPatternCache::new(100);
        let now = Instant::now();

        // Record multiple accesses
        cache.record_access("item1", now).await;
        cache
            .record_access("item1", now + Duration::from_secs(1))
            .await;
        cache
            .record_access("item1", now + Duration::from_secs(2))
            .await;

        let access_info = cache.get_access_info("item1").await.unwrap();
        assert_eq!(access_info.access_count, 3);
        assert_eq!(access_info.recent_accesses.len(), 3);
        assert!(access_info.access_frequency > 0.0);
    }

    #[tokio::test]
    async fn test_lru_eviction() {
        let cache = AccessPatternCache::new(2); // Small cache
        let now = Instant::now();

        // Add 3 items (should evict first one)
        cache.record_access("item1", now).await;
        cache
            .record_access("item2", now + Duration::from_secs(1))
            .await;
        cache
            .record_access("item3", now + Duration::from_secs(2))
            .await;

        let stats = cache.get_stats().await;
        assert_eq!(stats.unique_items_tracked, 2);
        assert_eq!(stats.cache_evictions, 1);

        // item1 should be evicted
        assert!(cache.get_access_info("item1").await.is_none());
        assert!(cache.get_access_info("item2").await.is_some());
        assert!(cache.get_access_info("item3").await.is_some());
    }

    #[tokio::test]
    async fn test_access_frequency_calculation() {
        let cache = AccessPatternCache::new(100);
        let now = Instant::now();

        // Record accesses over time
        cache.record_access("frequent_item", now).await;
        cache
            .record_access("frequent_item", now + Duration::from_secs(30))
            .await;
        cache
            .record_access("frequent_item", now + Duration::from_secs(60))
            .await;

        let access_info = cache.get_access_info("frequent_item").await.unwrap();

        // Should have frequency > 1 access per minute
        assert!(access_info.access_frequency > 1.0);
    }

    #[tokio::test]
    async fn test_promotion_decision() {
        let cache = AccessPatternCache::new(100);
        let now = Instant::now();

        // Recently accessed item
        cache.record_access("recent_item", now).await;

        let should_promote = cache
            .should_promote("recent_item", Duration::from_secs(300))
            .await;

        assert!(should_promote);

        // Old item
        cache
            .record_access("old_item", now - Duration::from_secs(7200))
            .await;

        let should_not_promote = cache
            .should_promote("old_item", Duration::from_secs(300))
            .await;

        assert!(!should_not_promote);
    }

    #[tokio::test]
    async fn test_demotion_decision() {
        let cache = AccessPatternCache::new(100);
        let old_time = Instant::now() - Duration::from_secs(7200);

        // Old, infrequent item
        cache.record_access("cold_item", old_time).await;

        let should_demote = cache
            .should_demote("cold_item", Duration::from_secs(3600))
            .await;

        assert!(should_demote);
    }

    #[tokio::test]
    async fn test_access_summary() {
        let cache = AccessPatternCache::new(100);
        let now = Instant::now();

        cache.record_access("test_item", now).await;
        cache
            .record_access("test_item", now + Duration::from_secs(10))
            .await;

        let summary = cache.get_access_summary("test_item").await.unwrap();

        assert_eq!(summary.item_id, "test_item");
        assert_eq!(summary.total_accesses, 2);
        assert!(summary.access_frequency > 0.0);
        assert!(summary.recency_seconds < 30); // Recent access
    }

    #[tokio::test]
    async fn test_batch_record_accesses() {
        let cache = AccessPatternCache::new(100);
        let now = Instant::now();

        let accesses = vec![
            ("item1".to_string(), now),
            ("item2".to_string(), now + Duration::from_secs(1)),
            ("item1".to_string(), now + Duration::from_secs(2)),
        ];

        cache.batch_record_accesses(accesses).await;

        let stats = cache.get_stats().await;
        assert_eq!(stats.total_accesses, 3);
        assert_eq!(stats.unique_items_tracked, 2);

        let item1_info = cache.get_access_info("item1").await.unwrap();
        assert_eq!(item1_info.access_count, 2);
    }
}
