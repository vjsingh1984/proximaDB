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

//! Range Coalescer for Smart I/O Layer
//!
//! Merges adjacent/nearby byte ranges to reduce I/O operations.
//! This is particularly beneficial for cloud storage where each
//! request has significant latency overhead.

use tracing::{debug, trace};

use super::traits::{ByteRange, RangeMapping, RangeOptimizer, RangeOptimizerWithMapping};

/// Default coalescing strategy that merges adjacent ranges
#[derive(Debug, Clone)]
pub struct DefaultRangeCoalescer {
    /// Default threshold for coalescing (bytes)
    default_threshold: u64,
    /// Minimum range size before considering splitting
    min_range_for_split: u64,
}

impl DefaultRangeCoalescer {
    /// Create a new coalescer with default settings
    pub fn new() -> Self {
        Self {
            default_threshold: 64 * 1024,     // 64KB default gap threshold
            min_range_for_split: 1024 * 1024, // 1MB minimum for splitting
        }
    }

    /// Create with custom threshold
    pub fn with_threshold(threshold: u64) -> Self {
        Self {
            default_threshold: threshold,
            min_range_for_split: 1024 * 1024,
        }
    }

    /// Create with custom configuration
    pub fn with_config(threshold: u64, min_range_for_split: u64) -> Self {
        Self {
            default_threshold: threshold,
            min_range_for_split,
        }
    }

    /// Get the default threshold
    pub fn default_threshold(&self) -> u64 {
        self.default_threshold
    }
}

impl Default for DefaultRangeCoalescer {
    fn default() -> Self {
        Self::new()
    }
}

impl RangeOptimizer for DefaultRangeCoalescer {
    fn coalesce(&self, ranges: Vec<ByteRange>, threshold: u64) -> Vec<ByteRange> {
        if ranges.is_empty() {
            return vec![];
        }

        if ranges.len() == 1 {
            return ranges;
        }

        // Sort ranges by start offset
        let mut sorted_ranges = ranges;
        sorted_ranges.sort_by_key(|r| r.start);

        let mut coalesced: Vec<ByteRange> = Vec::with_capacity(sorted_ranges.len());
        let mut current = sorted_ranges[0].clone();

        for range in sorted_ranges.into_iter().skip(1) {
            // Check if this range can be merged with current
            if current.is_adjacent(&range, threshold) || current.overlaps(&range) {
                // Merge the ranges
                current = current.merge(&range);
                trace!("Merged range: [{}, {})", current.start, current.end);
            } else {
                // Gap too large, start a new range
                coalesced.push(current);
                current = range;
            }
        }

        // Don't forget the last range
        coalesced.push(current);

        debug!(
            "Coalesced {} ranges to {} ranges (threshold: {} bytes)",
            coalesced.len() + (coalesced.len() - 1),
            coalesced.len(),
            threshold
        );

        coalesced
    }

    fn split_for_parallelism(&self, range: ByteRange, target_size: u64) -> Vec<ByteRange> {
        if range.len() <= target_size || range.len() < self.min_range_for_split {
            return vec![range];
        }

        let mut splits = Vec::new();
        let mut current_start = range.start;

        while current_start < range.end {
            let current_end = std::cmp::min(current_start + target_size, range.end);
            splits.push(ByteRange::new(current_start, current_end));
            current_start = current_end;
        }

        debug!(
            "Split range [{}, {}) into {} chunks of {} bytes",
            range.start,
            range.end,
            splits.len(),
            target_size
        );

        splits
    }
}

impl RangeOptimizerWithMapping for DefaultRangeCoalescer {
    fn coalesce_with_mapping(
        &self,
        ranges: Vec<ByteRange>,
        threshold: u64,
    ) -> (Vec<ByteRange>, Vec<RangeMapping>) {
        if ranges.is_empty() {
            return (vec![], vec![]);
        }

        // Create indexed ranges to track original positions
        let mut indexed_ranges: Vec<(usize, ByteRange)> = ranges
            .iter()
            .enumerate()
            .map(|(i, r)| (i, r.clone()))
            .collect();

        // Sort by start offset but remember original indices
        indexed_ranges.sort_by_key(|(_, r)| r.start);

        let mut coalesced: Vec<ByteRange> = Vec::with_capacity(indexed_ranges.len());
        let mut mappings: Vec<RangeMapping> = vec![
            RangeMapping {
                coalesced_index: 0,
                offset_in_coalesced: 0,
                length: 0,
            };
            ranges.len()
        ];

        // Track which original ranges map to each coalesced range
        let mut current_coalesced_start = indexed_ranges[0].1.start;
        let mut current_coalesced_end = indexed_ranges[0].1.end;
        let mut current_coalesced_index = 0;

        // Process first range
        let (orig_idx, ref first_range) = indexed_ranges[0];
        mappings[orig_idx] = RangeMapping {
            coalesced_index: 0,
            offset_in_coalesced: first_range.start - current_coalesced_start,
            length: first_range.len(),
        };

        for (orig_idx, range) in indexed_ranges.into_iter().skip(1) {
            let current = ByteRange::new(current_coalesced_start, current_coalesced_end);

            if current.is_adjacent(&range, threshold) || current.overlaps(&range) {
                // Merge - update the current coalesced range
                current_coalesced_end = std::cmp::max(current_coalesced_end, range.end);

                // Map this original range to the current coalesced range
                mappings[orig_idx] = RangeMapping {
                    coalesced_index: current_coalesced_index,
                    offset_in_coalesced: range.start - current_coalesced_start,
                    length: range.len(),
                };
            } else {
                // Gap too large - finalize current and start new
                coalesced.push(ByteRange::new(
                    current_coalesced_start,
                    current_coalesced_end,
                ));

                // Start new coalesced range
                current_coalesced_index += 1;
                current_coalesced_start = range.start;
                current_coalesced_end = range.end;

                mappings[orig_idx] = RangeMapping {
                    coalesced_index: current_coalesced_index,
                    offset_in_coalesced: 0,
                    length: range.len(),
                };
            }
        }

        // Don't forget the last coalesced range
        coalesced.push(ByteRange::new(
            current_coalesced_start,
            current_coalesced_end,
        ));

        trace!(
            "Coalesced with mapping: {} original -> {} coalesced",
            mappings.len(),
            coalesced.len()
        );

        (coalesced, mappings)
    }
}

/// Adaptive coalescer that adjusts threshold based on file characteristics
#[derive(Debug, Clone)]
pub struct AdaptiveRangeCoalescer {
    /// Base coalescer
    base: DefaultRangeCoalescer,
    /// Storage tier latency (affects threshold calculation)
    #[allow(dead_code)]
    storage_latency_us: u64,
    /// Target I/O size for this storage tier
    target_io_size: u64,
}

impl AdaptiveRangeCoalescer {
    /// Create for local storage (low latency, smaller threshold)
    pub fn for_local() -> Self {
        Self {
            base: DefaultRangeCoalescer::with_threshold(32 * 1024), // 32KB
            storage_latency_us: 100,
            target_io_size: 128 * 1024,
        }
    }

    /// Create for cloud storage (high latency, larger threshold)
    pub fn for_cloud() -> Self {
        Self {
            base: DefaultRangeCoalescer::with_threshold(256 * 1024), // 256KB
            storage_latency_us: 50_000,                              // 50ms
            target_io_size: 1024 * 1024,                             // 1MB
        }
    }

    /// Create with custom parameters
    pub fn with_storage_profile(latency_us: u64, target_io_size: u64) -> Self {
        // Calculate threshold: for high latency, we want larger reads
        // to amortize the cost of each I/O operation
        let threshold = if latency_us > 10_000 {
            // Cloud: aggressive coalescing (up to 1MB gaps)
            std::cmp::min(latency_us * 10, 1024 * 1024)
        } else if latency_us > 1_000 {
            // Network storage: moderate coalescing
            std::cmp::min(latency_us * 5, 256 * 1024)
        } else {
            // Local: conservative coalescing
            std::cmp::min(latency_us * 2, 64 * 1024)
        };

        Self {
            base: DefaultRangeCoalescer::with_threshold(threshold),
            storage_latency_us: latency_us,
            target_io_size,
        }
    }

    /// Calculate optimal threshold for given ranges
    pub fn calculate_optimal_threshold(&self, ranges: &[ByteRange]) -> u64 {
        if ranges.is_empty() {
            return self.base.default_threshold();
        }

        // Calculate average range size
        let total_bytes: u64 = ranges.iter().map(|r| r.len()).sum();
        let avg_size = total_bytes / ranges.len() as u64;

        // For small ranges, we want to coalesce more aggressively
        // For large ranges, we want to be more conservative
        if avg_size < 4096 {
            // Small ranges: aggressive coalescing
            self.base.default_threshold() * 2
        } else if avg_size < 64 * 1024 {
            // Medium ranges: standard coalescing
            self.base.default_threshold()
        } else {
            // Large ranges: conservative coalescing
            self.base.default_threshold() / 2
        }
    }
}

impl RangeOptimizer for AdaptiveRangeCoalescer {
    fn coalesce(&self, ranges: Vec<ByteRange>, threshold: u64) -> Vec<ByteRange> {
        // Use provided threshold or calculate optimal
        let effective_threshold = if threshold > 0 {
            threshold
        } else {
            self.calculate_optimal_threshold(&ranges)
        };

        self.base.coalesce(ranges, effective_threshold)
    }

    fn split_for_parallelism(&self, range: ByteRange, target_size: u64) -> Vec<ByteRange> {
        // Use storage-appropriate target size
        let effective_target = if target_size > 0 {
            target_size
        } else {
            self.target_io_size
        };

        self.base.split_for_parallelism(range, effective_target)
    }
}

impl RangeOptimizerWithMapping for AdaptiveRangeCoalescer {
    fn coalesce_with_mapping(
        &self,
        ranges: Vec<ByteRange>,
        threshold: u64,
    ) -> (Vec<ByteRange>, Vec<RangeMapping>) {
        let effective_threshold = if threshold > 0 {
            threshold
        } else {
            self.calculate_optimal_threshold(&ranges)
        };

        self.base.coalesce_with_mapping(ranges, effective_threshold)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coalesce_empty() {
        let coalescer = DefaultRangeCoalescer::new();
        let result = coalescer.coalesce(vec![], 1000);
        assert!(result.is_empty());
    }

    #[test]
    fn test_coalesce_single() {
        let coalescer = DefaultRangeCoalescer::new();
        let ranges = vec![ByteRange::new(0, 100)];
        let result = coalescer.coalesce(ranges.clone(), 1000);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], ranges[0]);
    }

    #[test]
    fn test_coalesce_adjacent_ranges() {
        let coalescer = DefaultRangeCoalescer::new();
        let ranges = vec![
            ByteRange::new(0, 100),
            ByteRange::new(100, 200),
            ByteRange::new(200, 300),
        ];

        // With threshold 0, adjacent ranges should merge
        let result = coalescer.coalesce(ranges, 0);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].start, 0);
        assert_eq!(result[0].end, 300);
    }

    #[test]
    fn test_coalesce_with_gap_below_threshold() {
        let coalescer = DefaultRangeCoalescer::new();
        let ranges = vec![
            ByteRange::new(0, 100),
            ByteRange::new(110, 200), // 10-byte gap
        ];

        // With threshold 50, should merge
        let result = coalescer.coalesce(ranges.clone(), 50);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].start, 0);
        assert_eq!(result[0].end, 200);

        // With threshold 5, should not merge
        let result = coalescer.coalesce(ranges, 5);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_coalesce_with_gap_above_threshold() {
        let coalescer = DefaultRangeCoalescer::new();
        let ranges = vec![
            ByteRange::new(0, 100),
            ByteRange::new(1000, 1100), // 900-byte gap
        ];

        // With threshold 100, should not merge
        let result = coalescer.coalesce(ranges, 100);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_coalesce_overlapping_ranges() {
        let coalescer = DefaultRangeCoalescer::new();
        let ranges = vec![ByteRange::new(0, 150), ByteRange::new(100, 200)];

        let result = coalescer.coalesce(ranges, 0);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].start, 0);
        assert_eq!(result[0].end, 200);
    }

    #[test]
    fn test_coalesce_unsorted_ranges() {
        let coalescer = DefaultRangeCoalescer::new();
        let ranges = vec![
            ByteRange::new(200, 300),
            ByteRange::new(0, 100),
            ByteRange::new(100, 200),
        ];

        let result = coalescer.coalesce(ranges, 0);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].start, 0);
        assert_eq!(result[0].end, 300);
    }

    #[test]
    fn test_coalesce_with_mapping() {
        let coalescer = DefaultRangeCoalescer::new();
        let ranges = vec![
            ByteRange::new(0, 100),
            ByteRange::new(100, 200),
            ByteRange::new(500, 600), // Gap of 300 bytes
        ];

        let (coalesced, mappings) = coalescer.coalesce_with_mapping(ranges, 50);

        // Should produce 2 coalesced ranges
        assert_eq!(coalesced.len(), 2);
        assert_eq!(coalesced[0].start, 0);
        assert_eq!(coalesced[0].end, 200);
        assert_eq!(coalesced[1].start, 500);
        assert_eq!(coalesced[1].end, 600);

        // Check mappings
        assert_eq!(mappings.len(), 3);

        // First original range maps to first coalesced
        assert_eq!(mappings[0].coalesced_index, 0);
        assert_eq!(mappings[0].offset_in_coalesced, 0);
        assert_eq!(mappings[0].length, 100);

        // Second original range maps to first coalesced
        assert_eq!(mappings[1].coalesced_index, 0);
        assert_eq!(mappings[1].offset_in_coalesced, 100);
        assert_eq!(mappings[1].length, 100);

        // Third original range maps to second coalesced
        assert_eq!(mappings[2].coalesced_index, 1);
        assert_eq!(mappings[2].offset_in_coalesced, 0);
        assert_eq!(mappings[2].length, 100);
    }

    #[test]
    fn test_split_for_parallelism() {
        let coalescer = DefaultRangeCoalescer::with_config(64 * 1024, 100); // 100 byte min for splitting
        let range = ByteRange::new(0, 1000);

        let splits = coalescer.split_for_parallelism(range, 300);

        assert_eq!(splits.len(), 4);
        assert_eq!(splits[0], ByteRange::new(0, 300));
        assert_eq!(splits[1], ByteRange::new(300, 600));
        assert_eq!(splits[2], ByteRange::new(600, 900));
        assert_eq!(splits[3], ByteRange::new(900, 1000));
    }

    #[test]
    fn test_split_small_range() {
        let coalescer = DefaultRangeCoalescer::new();
        let range = ByteRange::new(0, 100);

        // Small range should not be split
        let splits = coalescer.split_for_parallelism(range.clone(), 300);
        assert_eq!(splits.len(), 1);
        assert_eq!(splits[0], range);
    }

    #[test]
    fn test_coalesce_efficiency() {
        let coalescer = DefaultRangeCoalescer::new();

        let original = vec![
            ByteRange::new(0, 100),
            ByteRange::new(100, 200),
            ByteRange::new(200, 300),
            ByteRange::new(300, 400),
        ];

        let coalesced = coalescer.coalesce(original.clone(), 0);
        let efficiency = coalescer.coalesce_efficiency(&original, &coalesced);

        // 4 ranges -> 1 range = 75% reduction
        assert!((efficiency - 0.75).abs() < 0.01);
    }

    #[test]
    fn test_adaptive_coalescer_local() {
        let coalescer = AdaptiveRangeCoalescer::for_local();
        let ranges = vec![
            ByteRange::new(0, 1000),
            ByteRange::new(1100, 2000), // 100 byte gap
        ];

        // Local storage uses smaller threshold (32KB)
        let result = coalescer.coalesce(ranges, 0);
        assert_eq!(result.len(), 1); // Should still merge with 100 byte gap
    }

    #[test]
    fn test_adaptive_coalescer_cloud() {
        let coalescer = AdaptiveRangeCoalescer::for_cloud();
        let ranges = vec![
            ByteRange::new(0, 1000),
            ByteRange::new(100_000, 101_000), // 99KB gap
        ];

        // Cloud storage uses larger threshold (256KB)
        let _result = coalescer.coalesce(ranges.clone(), 0);
        // With 0 threshold, should not merge (gaps are not considered)
        // Need to use the calculated threshold
        let optimal_threshold = coalescer.calculate_optimal_threshold(&ranges);
        let result = coalescer.coalesce(ranges, optimal_threshold);
        assert_eq!(result.len(), 1); // Should merge with cloud threshold
    }

    #[test]
    fn test_optimize_access_order() {
        let coalescer = DefaultRangeCoalescer::new();
        let ranges = vec![
            ByteRange::new(500, 600),
            ByteRange::new(0, 100),
            ByteRange::new(200, 300),
        ];

        let ordered = coalescer.optimize_access_order(ranges);

        assert_eq!(ordered[0].start, 0);
        assert_eq!(ordered[1].start, 200);
        assert_eq!(ordered[2].start, 500);
    }
}
