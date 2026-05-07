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

//! # Unified Eviction Policies (TD-042)
//!
//! This module provides unified eviction policies for all cache types in ProximaDB,
//! enabling coordinated eviction decisions across caches based on global memory pressure.
//!
//! ## Architecture
//!
//! ```text
//! Global Memory Pressure
//!         ↓
//! UnifiedEvictionPolicy
//!    ↓    ↓    ↓    ↓
//! Vector Query Metadata Bitmap
//!  Cache Cache   Cache   Cache
//!    ↓    ↓    ↓    ↓
//!  Coordinated Eviction Decisions
//! ```
//!
//! ## Benefits
//!
//! 1. **Coordinated Eviction**: All caches work together under memory pressure
//! 2. **Global Optimization**: Evict from least valuable cache first
//! 3. **Proportional Eviction**: Maintain cache size ratios under pressure
//! 4. **Priority System**: Protect high-priority caches during eviction
//! 5. **Adaptive Policies**: Adjust eviction strategy based on workload

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::Result;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::storage::cache::unified_cache::{CacheId, UnifiedCacheCoordinator};

/// Unified eviction policy for coordinated cache management
pub struct UnifiedEvictionPolicy {
    /// Cache coordinator for accessing all caches
    coordinator: Arc<UnifiedCacheCoordinator>,
    /// Eviction configuration
    config: EvictionConfig,
    /// Current memory usage by cache
    memory_usage: Arc<RwLock<HashMap<CacheId, u64>>>,
    /// Last eviction time
    last_eviction: Arc<RwLock<SystemTime>>,
}

/// Eviction configuration
#[derive(Debug, Clone)]
pub struct EvictionConfig {
    /// Total memory budget in bytes
    pub total_memory_budget: u64,
    /// Memory pressure threshold (0.0 to 1.0)
    pub pressure_threshold: f64,
    /// Target memory usage after eviction (0.0 to 1.0)
    pub target_usage_ratio: f64,
    /// Cache priority for eviction decisions
    pub cache_priorities: HashMap<CacheId, CachePriority>,
    /// Minimum cache sizes (protect critical data)
    pub minimum_cache_sizes: HashMap<CacheId, u64>,
}

impl Default for EvictionConfig {
    fn default() -> Self {
        let mut cache_priorities = HashMap::new();
        cache_priorities.insert(CacheId::VectorData, CachePriority::High);
        cache_priorities.insert(CacheId::QueryResult, CachePriority::Medium);
        cache_priorities.insert(CacheId::Metadata, CachePriority::Critical);
        cache_priorities.insert(CacheId::BitmapFilter, CachePriority::Low);
        cache_priorities.insert(CacheId::IndexNode, CachePriority::High);

        let mut minimum_cache_sizes = HashMap::new();
        minimum_cache_sizes.insert(CacheId::VectorData, 100_000_000); // 100 MB
        minimum_cache_sizes.insert(CacheId::QueryResult, 50_000_000); // 50 MB
        minimum_cache_sizes.insert(CacheId::Metadata, 200_000_000); // 200 MB
        minimum_cache_sizes.insert(CacheId::BitmapFilter, 10_000_000); // 10 MB
        minimum_cache_sizes.insert(CacheId::IndexNode, 50_000_000); // 50 MB

        Self {
            total_memory_budget: 2_000_000_000, // 2 GB default
            pressure_threshold: 0.9,            // 90% usage triggers eviction
            target_usage_ratio: 0.7,            // Evict to 70% usage
            cache_priorities,
            minimum_cache_sizes,
        }
    }
}

/// Cache priority for eviction decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CachePriority {
    /// Critical cache - evict last (metadata)
    Critical = 4,
    /// High priority cache - evict sparingly (vectors, indexes)
    High = 3,
    /// Medium priority cache - normal eviction (queries)
    Medium = 2,
    /// Low priority cache - evict first (filters)
    Low = 1,
}

/// Eviction result
#[derive(Debug, Clone)]
pub struct EvictionResult {
    /// Total bytes evicted
    pub bytes_evicted: u64,
    /// Entries evicted by cache
    pub entries_evicted: HashMap<CacheId, usize>,
    /// Time taken for eviction
    pub eviction_duration_ms: u64,
}

impl UnifiedEvictionPolicy {
    /// Create new unified eviction policy
    pub fn new(coordinator: Arc<UnifiedCacheCoordinator>, config: EvictionConfig) -> Self {
        Self {
            coordinator,
            config,
            memory_usage: Arc::new(RwLock::new(HashMap::new())),
            last_eviction: Arc::new(RwLock::new(SystemTime::UNIX_EPOCH)),
        }
    }

    /// Create with default configuration
    pub fn with_default(coordinator: Arc<UnifiedCacheCoordinator>) -> Self {
        Self::new(coordinator, EvictionConfig::default())
    }

    /// Check and handle memory pressure
    ///
    /// # Arguments
    ///
    /// * `force` - Force eviction even if under threshold
    ///
    /// # Returns
    ///
    /// Eviction result if eviction occurred, None if not needed
    ///
    /// # Example
    ///
    /// ```ignore
    /// if let Some(result) = eviction_policy.check_memory_pressure(false).await? {
    ///     println!("Evicted {} bytes", result.bytes_evicted);
    /// }
    /// ```
    pub async fn check_memory_pressure(
        &self,
        force: bool,
    ) -> Result<Option<EvictionResult>, VectorDBError> {
        // Update memory usage from all caches
        self.update_memory_usage().await?;

        // Calculate total memory usage
        let total_usage = self.calculate_total_usage().await;

        // Check if under pressure
        let usage_ratio = total_usage as f64 / self.config.total_memory_budget as f64;
        let under_pressure = usage_ratio > self.config.pressure_threshold || force;

        if !under_pressure {
            debug!(
                "Memory pressure OK: {:.1}% used ({}/{} bytes)",
                usage_ratio * 100.0,
                total_usage,
                self.config.total_memory_budget
            );
            return Ok(None);
        }

        warn!(
            "Memory pressure detected: {:.1}% used ({}/{} bytes)",
            usage_ratio * 100.0,
            total_usage,
            self.config.total_memory_budget
        );

        // Perform coordinated eviction
        let result = self.evict_to_target().await?;

        Ok(Some(result))
    }

    /// Update memory usage from all caches
    async fn update_memory_usage(&self) -> Result<(), VectorDBError> {
        let stats = self.coordinator.get_all_stats().await;

        let mut usage = self.memory_usage.write().await;
        for (cache_id, cache_stats) in stats {
            usage.insert(cache_id, cache_stats.memory_usage_bytes);
        }

        Ok(())
    }

    /// Calculate total memory usage across all caches
    async fn calculate_total_usage(&self) -> u64 {
        let usage = self.memory_usage.read().await;
        usage.values().sum()
    }

    /// Evict entries to reach target memory usage
    ///
    /// # Strategy
    ///
    /// 1. Sort caches by priority (low priority first)
    /// 2. Calculate eviction target
    /// 3. Evict from low-priority caches first
    /// 4. Respect minimum cache sizes
    /// 5. Stop when target usage reached
    async fn evict_to_target(&self) -> Result<EvictionResult, VectorDBError> {
        let start = std::time::Instant::now();
        let total_usage = self.calculate_total_usage().await;
        let target_usage =
            (self.config.total_memory_budget as f64 * self.config.target_usage_ratio) as u64;

        // Calculate bytes to evict
        let bytes_to_evict = total_usage.saturating_sub(target_usage);

        info!(
            "Evicting to target: {} bytes (current: {}, target: {})",
            bytes_to_evict, total_usage, target_usage
        );

        let mut entries_evicted: HashMap<CacheId, usize> = HashMap::new();
        let mut total_evicted = 0u64;

        // Sort caches by priority (low priority first)
        let mut caches_by_priority: Vec<_> = self
            .config
            .cache_priorities
            .iter()
            .map(|(cache_id, priority)| (*cache_id, *priority))
            .collect();
        caches_by_priority.sort_by_key(|(_, priority)| *priority);

        // Evict from low-priority caches first
        for (cache_id, _priority) in caches_by_priority {
            if total_evicted >= bytes_to_evict {
                break;
            }

            // Check minimum cache size
            let current_usage = {
                let usage = self.memory_usage.read().await;
                usage.get(&cache_id).copied().unwrap_or(0)
            };

            let min_size = self
                .config
                .minimum_cache_sizes
                .get(&cache_id)
                .copied()
                .unwrap_or(0);

            if current_usage <= min_size {
                debug!("Cache {:?} at minimum size, skipping eviction", cache_id);
                continue;
            }

            // Calculate evictable bytes from this cache
            let evictable = current_usage.saturating_sub(min_size);
            let to_evict = std::cmp::min(evictable, bytes_to_evict - total_evicted);

            if to_evict == 0 {
                continue;
            }

            // Perform eviction from this cache
            // TODO: Implement actual eviction from cache
            // For now, just update tracking
            let entries = self.estimate_entries_to_evict(cache_id, to_evict);

            debug!(
                "Evicting from cache {:?}: ~{} bytes ({} entries)",
                cache_id, to_evict, entries
            );

            entries_evicted.insert(cache_id, entries);
            total_evicted += to_evict;

            // Update memory usage tracking
            let mut usage = self.memory_usage.write().await;
            if let Some(usage_value) = usage.get_mut(&cache_id) {
                *usage_value = usage_value.saturating_sub(to_evict);
            }
        }

        let duration = start.elapsed();

        // Update last eviction time
        let mut last_eviction = self.last_eviction.write().await;
        *last_eviction = SystemTime::now();

        warn!(
            "Eviction complete: {} bytes from {:?} caches in {:?}",
            total_evicted,
            entries_evicted.len(),
            duration
        );

        Ok(EvictionResult {
            bytes_evicted: total_evicted,
            entries_evicted,
            eviction_duration_ms: duration.as_millis() as u64,
        })
    }

    /// Estimate number of entries to evict to reach byte target
    ///
    /// # Arguments
    ///
    /// * `cache_id` - Cache to evict from
    /// * `bytes_to_evict` - Target bytes to evict
    ///
    /// # Returns
    ///
    /// Estimated number of entries to evict
    fn estimate_entries_to_evict(&self, cache_id: CacheId, bytes_to_evict: u64) -> usize {
        // Estimate average entry size by cache type
        let avg_entry_size = match cache_id {
            CacheId::VectorData => 1000,    // 1 KB per vector
            CacheId::QueryResult => 5000,   // 5 KB per query result
            CacheId::Metadata => 500,       // 500 B per metadata entry
            CacheId::BitmapFilter => 10000, // 10 KB per bitmap
            CacheId::IndexNode => 200,      // 200 B per index node
        };

        (bytes_to_evict / avg_entry_size) as usize
    }

    /// Get current memory usage by cache
    pub async fn get_memory_usage(&self) -> HashMap<CacheId, u64> {
        let usage = self.memory_usage.read().await;
        usage.clone()
    }

    /// Get memory pressure status
    pub async fn get_pressure_status(&self) -> PressureStatus {
        let total_usage = self.calculate_total_usage().await;
        let usage_ratio = total_usage as f64 / self.config.total_memory_budget as f64;

        if usage_ratio < 0.7 {
            PressureStatus::Healthy
        } else if usage_ratio < 0.9 {
            PressureStatus::Moderate
        } else {
            PressureStatus::Critical
        }
    }

    /// Get cache priority
    pub fn get_cache_priority(&self, cache_id: CacheId) -> CachePriority {
        self.config
            .cache_priorities
            .get(&cache_id)
            .copied()
            .unwrap_or(CachePriority::Medium)
    }

    /// Update cache priority
    pub fn set_cache_priority(&mut self, cache_id: CacheId, priority: CachePriority) {
        self.config.cache_priorities.insert(cache_id, priority);
    }
}

/// Memory pressure status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureStatus {
    /// Memory usage is healthy (< 70%)
    Healthy,
    /// Memory usage is moderate (70-90%)
    Moderate,
    /// Memory usage is critical (> 90%)
    Critical,
}

/// Re-export VectorDBError for use in this module
pub use crate::core::error::VectorDBError;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::cache::unified_cache::UnifiedCacheCoordinator;

    #[test]
    fn test_eviction_config_default() {
        let config = EvictionConfig::default();
        assert_eq!(config.total_memory_budget, 2_000_000_000);
        assert_eq!(config.pressure_threshold, 0.9);
        assert_eq!(config.target_usage_ratio, 0.7);
    }

    #[test]
    fn test_cache_priority_ordering() {
        assert!(CachePriority::Critical > CachePriority::High);
        assert!(CachePriority::High > CachePriority::Medium);
        assert!(CachePriority::Medium > CachePriority::Low);
    }

    #[tokio::test]
    async fn test_unified_eviction_policy_new() {
        let coordinator = Arc::new(UnifiedCacheCoordinator::new());
        let policy = UnifiedEvictionPolicy::with_default(coordinator);

        assert_eq!(
            policy.get_cache_priority(CacheId::Metadata),
            CachePriority::Critical
        );
        assert_eq!(
            policy.get_cache_priority(CacheId::VectorData),
            CachePriority::High
        );
    }

    #[tokio::test]
    async fn test_pressure_status() {
        let coordinator = Arc::new(UnifiedCacheCoordinator::new());
        let policy = UnifiedEvictionPolicy::with_default(coordinator);

        // Initially should be healthy (no usage)
        let status = policy.get_pressure_status().await;
        assert_eq!(status, PressureStatus::Healthy);
    }

    #[test]
    fn test_estimate_entries_to_evict() {
        let coordinator = Arc::new(UnifiedCacheCoordinator::new());
        let policy = UnifiedEvictionPolicy::with_default(coordinator);

        let entries = policy.estimate_entries_to_evict(CacheId::VectorData, 100_000);
        assert_eq!(entries, 100); // 100 KB / 1 KB per vector

        let entries = policy.estimate_entries_to_evict(CacheId::QueryResult, 50_000);
        assert_eq!(entries, 10); // 50 KB / 5 KB per query
    }

    #[tokio::test]
    async fn test_set_cache_priority() {
        let coordinator = Arc::new(UnifiedCacheCoordinator::new());
        let mut policy = UnifiedEvictionPolicy::with_default(coordinator);

        policy.set_cache_priority(CacheId::VectorData, CachePriority::Critical);
        assert_eq!(
            policy.get_cache_priority(CacheId::VectorData),
            CachePriority::Critical
        );
    }

    #[tokio::test]
    async fn test_check_memory_pressure_no_pressure() {
        let coordinator = Arc::new(UnifiedCacheCoordinator::new());
        let policy = UnifiedEvictionPolicy::with_default(coordinator);

        // No memory usage, should not trigger eviction
        let result = policy.check_memory_pressure(false).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_check_memory_pressure_forced() {
        let coordinator = Arc::new(UnifiedCacheCoordinator::new());
        let policy = UnifiedEvictionPolicy::with_default(coordinator);

        // Force eviction even without pressure
        let result = policy.check_memory_pressure(true).await.unwrap();
        // Should return Some result even though no eviction actually occurred
        assert!(result.is_some());
    }
}
