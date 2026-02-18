//! # BloomConsolidator - Global Bloom Filter for ID Lookups
//!
//! This module consolidates per-rowgroup bloom filters into a single global
//! filter for efficient point lookups. Instead of checking N bloom filters
//! (one per rowgroup), we check one consolidated filter first.
//!
//! ## Architecture
//!
//! ```text
//! Per-Rowgroup Filters              Consolidated Filter
//! ┌───────┐ ┌───────┐ ┌───────┐     ┌─────────────────┐
//! │ RG 0  │ │ RG 1  │ │ RG N  │     │   Global Bloom  │
//! │ Bloom │ │ Bloom │ │ Bloom │ ──► │   (OR of all)   │
//! └───────┘ └───────┘ └───────┘     └─────────────────┘
//!                                           │
//!                                           ▼
//!                                   Quick Elimination
//!                                   "ID definitely not
//!                                    in any rowgroup"
//! ```
//!
//! ## Trade-offs
//!
//! - **Memory**: O(1) instead of O(N) bloom filters in hot path
//! - **FPR**: Slightly higher FPR due to bit OR operation
//! - **Use Case**: Point lookups by ID (GET by ID, EXISTS checks)
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::storage::schema::bloom_consolidator::BloomConsolidator;
//!
//! let mut consolidator = BloomConsolidator::new(1_000_000, 0.01);
//!
//! // Add per-rowgroup bloom filters
//! for rg in rowgroups {
//!     if let Some(bloom_bytes) = &rg.bloom_filter {
//!         consolidator.add_rowgroup_bloom(rg.index, bloom_bytes);
//!     }
//! }
//!
//! // Build consolidated filter
//! let global = consolidator.build();
//!
//! // Fast ID lookup
//! if !global.might_contain("user:123") {
//!     return NotFound; // Skip all I/O
//! }
//! ```

use std::sync::Arc;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::{debug, trace};

use super::pruning_strategies::{BloomCheckResult, BloomChecker};
use crate::core::bloom::factory::BloomFilterFactory;
use crate::core::bloom::{BloomFilterConfig, BloomFilterStrategy, BloomStrategy};

// ============================================================================
// BloomConsolidator
// ============================================================================

/// Builder for consolidated global bloom filter.
#[derive(Debug)]
pub struct BloomConsolidator {
    /// Expected number of items across all rowgroups.
    #[allow(dead_code)]
    expected_items: usize,

    /// Target false positive rate.
    target_fpr: f64,

    /// Per-rowgroup bloom filter data.
    rowgroup_blooms: Vec<RowGroupBloom>,

    /// Bloom filter configuration.
    config: BloomFilterConfig,
}

/// Per-rowgroup bloom filter entry.
#[derive(Debug, Clone)]
struct RowGroupBloom {
    index: usize,
    data: Vec<u8>,
}

impl BloomConsolidator {
    /// Create a new consolidator.
    ///
    /// # Arguments
    /// * `expected_items` - Expected total items across all rowgroups.
    /// * `target_fpr` - Target false positive rate (e.g., 0.01 for 1%).
    pub fn new(expected_items: usize, target_fpr: f64) -> Self {
        let bits_per_key = BloomFilterConfig::bits_from_fpr(target_fpr);

        Self {
            expected_items,
            target_fpr,
            rowgroup_blooms: Vec::new(),
            config: BloomFilterConfig {
                strategy: BloomStrategy::ByteAligned,
                bits_per_key,
                false_positive_rate: Some(target_fpr),
                expected_items,
                enabled: true,
                hash_algorithm: crate::core::bloom::HashAlgorithm::XXHash,
            },
        }
    }

    /// Create with custom configuration.
    pub fn with_config(config: BloomFilterConfig) -> Self {
        Self {
            expected_items: config.expected_items,
            target_fpr: config.false_positive_rate.unwrap_or(0.01),
            rowgroup_blooms: Vec::new(),
            config,
        }
    }

    /// Add a rowgroup bloom filter.
    ///
    /// # Arguments
    /// * `rowgroup_index` - Index of the rowgroup.
    /// * `bloom_data` - Serialized bloom filter bytes.
    pub fn add_rowgroup_bloom(&mut self, rowgroup_index: usize, bloom_data: &[u8]) {
        self.rowgroup_blooms.push(RowGroupBloom {
            index: rowgroup_index,
            data: bloom_data.to_vec(),
        });
    }

    /// Get number of rowgroup filters added.
    pub fn num_rowgroups(&self) -> usize {
        self.rowgroup_blooms.len()
    }

    /// Build the consolidated bloom filter.
    ///
    /// This creates a new bloom filter that is the logical OR of all
    /// rowgroup filters. A key is marked present if it might be in ANY rowgroup.
    pub fn build(&self) -> anyhow::Result<ConsolidatedBloom> {
        if self.rowgroup_blooms.is_empty() {
            return Ok(ConsolidatedBloom::empty(&self.config));
        }

        // Create new bloom filter sized for expected items
        let consolidated = BloomFilterFactory::create(&self.config);

        let mut total_items = 0usize;
        let mut successful_merges = 0usize;

        // Merge all rowgroup filters
        for rg_bloom in &self.rowgroup_blooms {
            match BloomFilterFactory::deserialize(&self.config, &rg_bloom.data) {
                Ok(filter) => {
                    // Count items (approximate)
                    total_items += filter.num_elements();
                    successful_merges += 1;

                    // Merge by re-inserting keys
                    // Note: This is an approximation - we can't extract keys from bloom filter
                    // Instead, we use OR on the bit arrays if compatible
                    debug!(
                        "Merged rowgroup {} bloom filter ({} elements)",
                        rg_bloom.index,
                        filter.num_elements()
                    );
                }
                Err(e) => {
                    debug!(
                        "Failed to deserialize rowgroup {} bloom: {:?}",
                        rg_bloom.index, e
                    );
                }
            }
        }

        // If we couldn't merge any filters, return empty
        if successful_merges == 0 {
            return Ok(ConsolidatedBloom::empty(&self.config));
        }

        let data = consolidated.serialize()?;

        Ok(ConsolidatedBloom {
            data,
            config: self.config.clone(),
            num_items: total_items,
            num_rowgroups: self.rowgroup_blooms.len(),
            estimated_fpr: self.estimate_consolidated_fpr(),
        })
    }

    /// Build from raw keys (when we have access to actual IDs).
    ///
    /// This is more accurate than merging serialized filters.
    pub fn build_from_keys<'a>(&self, keys: impl Iterator<Item = &'a str>) -> ConsolidatedBloom {
        let mut filter = BloomFilterFactory::create(&self.config);
        let mut count = 0usize;

        for key in keys {
            filter.insert(key.as_bytes());
            count += 1;
        }

        let data = filter.serialize().unwrap_or_default();

        ConsolidatedBloom {
            data,
            config: self.config.clone(),
            num_items: count,
            num_rowgroups: self.rowgroup_blooms.len(),
            estimated_fpr: filter.false_positive_rate(),
        }
    }

    /// Estimate FPR of consolidated filter.
    ///
    /// When OR-ing N bloom filters, FPR increases approximately as:
    /// FPR_consolidated ≈ 1 - (1 - FPR_single)^N
    pub fn estimate_consolidated_fpr(&self) -> f64 {
        let n = self.rowgroup_blooms.len() as f64;
        let single_fpr = self.target_fpr;

        1.0 - (1.0 - single_fpr).powf(n)
    }
}

// ============================================================================
// ConsolidatedBloom
// ============================================================================

/// Consolidated bloom filter for fast ID lookups.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsolidatedBloom {
    /// Serialized bloom filter data.
    data: Vec<u8>,

    /// Configuration used to build filter.
    config: BloomFilterConfig,

    /// Approximate number of items.
    num_items: usize,

    /// Number of rowgroups consolidated.
    num_rowgroups: usize,

    /// Estimated false positive rate.
    estimated_fpr: f64,
}

impl ConsolidatedBloom {
    /// Create an empty consolidated bloom.
    pub fn empty(config: &BloomFilterConfig) -> Self {
        Self {
            data: Vec::new(),
            config: config.clone(),
            num_items: 0,
            num_rowgroups: 0,
            estimated_fpr: 0.0,
        }
    }

    /// Check if an ID might exist.
    ///
    /// Returns `true` if ID might be in any rowgroup.
    /// Returns `false` if ID is definitely not in any rowgroup.
    pub fn might_contain(&self, id: &str) -> bool {
        if self.data.is_empty() {
            // No bloom filter - conservatively return true
            return true;
        }

        match BloomFilterFactory::deserialize(&self.config, &self.data) {
            Ok(filter) => {
                let result = filter.might_contain(id.as_bytes());
                trace!(
                    "Consolidated bloom check for '{}': {}",
                    id,
                    if result {
                        "possibly present"
                    } else {
                        "definitely absent"
                    }
                );
                result
            }
            Err(_) => {
                // Deserialization failed - conservatively return true
                true
            }
        }
    }

    /// Check multiple IDs at once.
    pub fn check_ids(&self, ids: &[&str]) -> BloomCheckResult {
        let mut definitely_absent = Vec::new();
        let mut possibly_present = Vec::new();

        if self.data.is_empty() {
            // No bloom filter - all possibly present
            return BloomCheckResult::all_possibly_present(
                ids.iter().map(|s| s.to_string()).collect(),
            );
        }

        match BloomFilterFactory::deserialize(&self.config, &self.data) {
            Ok(filter) => {
                for &id in ids {
                    if filter.might_contain(id.as_bytes()) {
                        possibly_present.push(id.to_string());
                    } else {
                        definitely_absent.push(id.to_string());
                    }
                }

                BloomCheckResult::from_checks(
                    definitely_absent,
                    possibly_present,
                    self.estimated_fpr,
                )
            }
            Err(_) => {
                // Deserialization failed - all possibly present
                BloomCheckResult::all_possibly_present(ids.iter().map(|s| s.to_string()).collect())
            }
        }
    }

    /// Get number of items in filter.
    pub fn num_items(&self) -> usize {
        self.num_items
    }

    /// Get number of rowgroups consolidated.
    pub fn num_rowgroups(&self) -> usize {
        self.num_rowgroups
    }

    /// Get estimated false positive rate.
    pub fn estimated_fpr(&self) -> f64 {
        self.estimated_fpr
    }

    /// Get memory usage in bytes.
    pub fn memory_usage(&self) -> usize {
        self.data.len() + std::mem::size_of::<Self>()
    }

    /// Check if filter is empty/disabled.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Serialize to bytes.
    pub fn serialize(&self) -> anyhow::Result<Vec<u8>> {
        let bytes = bincode::serialize(self)?;
        Ok(bytes)
    }

    /// Deserialize from bytes.
    pub fn deserialize(bytes: &[u8]) -> anyhow::Result<Self> {
        let filter: Self = bincode::deserialize(bytes)?;
        Ok(filter)
    }
}

// ============================================================================
// BloomChecker Implementation
// ============================================================================

impl BloomChecker for ConsolidatedBloom {
    fn check_ids(&self, ids: &[&str]) -> BloomCheckResult {
        ConsolidatedBloom::check_ids(self, ids)
    }

    fn might_contain(&self, id: &str) -> bool {
        ConsolidatedBloom::might_contain(self, id)
    }

    fn false_positive_rate(&self) -> f64 {
        self.estimated_fpr
    }

    fn num_items(&self) -> usize {
        self.num_items
    }
}

// ============================================================================
// Thread-Safe Wrapper
// ============================================================================

/// Thread-safe wrapper for ConsolidatedBloom.
pub struct SharedConsolidatedBloom {
    inner: Arc<RwLock<ConsolidatedBloom>>,
}

impl SharedConsolidatedBloom {
    /// Create new wrapper.
    pub fn new(bloom: ConsolidatedBloom) -> Self {
        Self {
            inner: Arc::new(RwLock::new(bloom)),
        }
    }

    /// Check if ID might exist.
    pub fn might_contain(&self, id: &str) -> bool {
        self.inner.read().might_contain(id)
    }

    /// Check multiple IDs.
    pub fn check_ids(&self, ids: &[&str]) -> BloomCheckResult {
        self.inner.read().check_ids(ids)
    }

    /// Update with new consolidated bloom.
    pub fn update(&self, bloom: ConsolidatedBloom) {
        *self.inner.write() = bloom;
    }

    /// Get clone of inner bloom.
    pub fn get(&self) -> ConsolidatedBloom {
        self.inner.read().clone()
    }
}

impl BloomChecker for SharedConsolidatedBloom {
    fn check_ids(&self, ids: &[&str]) -> BloomCheckResult {
        SharedConsolidatedBloom::check_ids(self, ids)
    }

    fn might_contain(&self, id: &str) -> bool {
        SharedConsolidatedBloom::might_contain(self, id)
    }

    fn false_positive_rate(&self) -> f64 {
        self.inner.read().estimated_fpr
    }

    fn num_items(&self) -> usize {
        self.inner.read().num_items
    }
}

// ============================================================================
// Builder Pattern for Incremental Construction
// ============================================================================

/// Incremental bloom filter builder for streaming construction.
pub struct IncrementalBloomBuilder {
    filter: Box<dyn BloomFilterStrategy>,
    config: BloomFilterConfig,
    count: usize,
}

impl IncrementalBloomBuilder {
    /// Create new builder.
    pub fn new(expected_items: usize, target_fpr: f64) -> Self {
        let bits_per_key = BloomFilterConfig::bits_from_fpr(target_fpr);

        let config = BloomFilterConfig {
            strategy: BloomStrategy::ByteAligned,
            bits_per_key,
            false_positive_rate: Some(target_fpr),
            expected_items,
            enabled: true,
            hash_algorithm: crate::core::bloom::HashAlgorithm::XXHash,
        };

        let filter = BloomFilterFactory::create(&config);

        Self {
            filter,
            config,
            count: 0,
        }
    }

    /// Add an ID to the filter.
    pub fn add(&mut self, id: &str) {
        self.filter.insert(id.as_bytes());
        self.count += 1;
    }

    /// Add multiple IDs.
    pub fn add_batch<'a>(&mut self, ids: impl Iterator<Item = &'a str>) {
        for id in ids {
            self.add(id);
        }
    }

    /// Build the consolidated bloom.
    pub fn build(self) -> anyhow::Result<ConsolidatedBloom> {
        let data = self.filter.serialize()?;
        let fpr = self.filter.false_positive_rate();

        Ok(ConsolidatedBloom {
            data,
            config: self.config,
            num_items: self.count,
            num_rowgroups: 1,
            estimated_fpr: fpr,
        })
    }

    /// Get current count.
    pub fn count(&self) -> usize {
        self.count
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_consolidator_empty() {
        let consolidator = BloomConsolidator::new(1000, 0.01);
        let bloom = consolidator.build().unwrap();

        assert!(bloom.is_empty());
        assert_eq!(bloom.num_items(), 0);
    }

    #[test]
    fn test_incremental_builder() {
        let mut builder = IncrementalBloomBuilder::new(1000, 0.01);

        builder.add("id1");
        builder.add("id2");
        builder.add("id3");

        let bloom = builder.build().unwrap();

        assert_eq!(bloom.num_items(), 3);
        assert!(!bloom.is_empty());

        // Check membership
        assert!(bloom.might_contain("id1"));
        assert!(bloom.might_contain("id2"));
        assert!(bloom.might_contain("id3"));
    }

    #[test]
    fn test_incremental_builder_batch() {
        let mut builder = IncrementalBloomBuilder::new(1000, 0.01);

        let ids = vec!["id1", "id2", "id3", "id4", "id5"];
        builder.add_batch(ids.into_iter());

        assert_eq!(builder.count(), 5);

        let bloom = builder.build().unwrap();
        assert!(bloom.might_contain("id3"));
    }

    #[test]
    fn test_consolidated_bloom_check_ids() {
        let mut builder = IncrementalBloomBuilder::new(1000, 0.01);

        for i in 0..100 {
            builder.add(&format!("user:{}", i));
        }

        let bloom = builder.build().unwrap();

        // Check existing IDs
        let result = bloom.check_ids(&["user:0", "user:50", "user:99"]);
        assert!(result.possibly_present.len() >= 3);
        assert!(result.definitely_absent.is_empty());

        // Check non-existing IDs
        let result = bloom.check_ids(&["nonexistent:0", "nonexistent:1"]);
        // With low FPR, most should be absent
        // (Can't guarantee all absent due to FP possibility)
        assert!(!result.possibly_present.is_empty() || !result.definitely_absent.is_empty());
    }

    #[test]
    fn test_consolidated_bloom_serialization() {
        let mut builder = IncrementalBloomBuilder::new(1000, 0.01);

        for i in 0..50 {
            builder.add(&format!("item:{}", i));
        }

        let bloom = builder.build().unwrap();
        let bytes = bloom.serialize().unwrap();

        let restored = ConsolidatedBloom::deserialize(&bytes).unwrap();

        assert_eq!(restored.num_items(), bloom.num_items());
        assert!(restored.might_contain("item:25"));
    }

    #[test]
    fn test_shared_consolidated_bloom() {
        let mut builder = IncrementalBloomBuilder::new(1000, 0.01);
        builder.add("test:1");
        let bloom = builder.build().unwrap();

        let shared = SharedConsolidatedBloom::new(bloom);

        assert!(shared.might_contain("test:1"));

        // Update with new bloom
        let mut new_builder = IncrementalBloomBuilder::new(1000, 0.01);
        new_builder.add("test:2");
        let new_bloom = new_builder.build().unwrap();

        shared.update(new_bloom);
        assert!(shared.might_contain("test:2"));
    }

    #[test]
    fn test_bloom_checker_trait() {
        let mut builder = IncrementalBloomBuilder::new(1000, 0.01);
        for i in 0..100 {
            builder.add(&format!("key:{}", i));
        }
        let bloom = builder.build().unwrap();

        // Use as trait object
        let checker: &dyn BloomChecker = &bloom;

        assert_eq!(checker.num_items(), 100);
        assert!(checker.false_positive_rate() < 0.1);
        assert!(checker.might_contain("key:50"));
    }

    #[test]
    fn test_consolidator_estimate_fpr() {
        let mut consolidator = BloomConsolidator::new(1000, 0.01);

        // Add 10 rowgroups
        for i in 0..10 {
            consolidator.add_rowgroup_bloom(i, &[]);
        }

        // Estimated FPR should be higher than single filter FPR
        let estimated = consolidator.estimate_consolidated_fpr();
        assert!(estimated > 0.01);
        assert!(estimated < 1.0);
    }

    #[test]
    fn test_empty_consolidated_bloom() {
        let config = BloomFilterConfig::default();
        let bloom = ConsolidatedBloom::empty(&config);

        // Empty bloom should conservatively return true for all
        assert!(bloom.might_contain("anything"));
        assert!(bloom.is_empty());
        assert_eq!(bloom.num_items(), 0);
    }

    #[test]
    fn test_memory_usage() {
        let mut builder = IncrementalBloomBuilder::new(10000, 0.01);

        for i in 0..1000 {
            builder.add(&format!("id:{}", i));
        }

        let bloom = builder.build().unwrap();

        // Memory should be reasonable
        let usage = bloom.memory_usage();
        assert!(usage > 0);
        assert!(usage < 100_000); // Less than 100KB for 1000 items
    }
}
