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

//! Smart I/O Layer Traits
//!
//! Defines the core abstractions for I/O strategy and range optimization.
//! These traits follow SOLID principles, particularly Dependency Inversion (D).

use async_trait::async_trait;
use std::fmt::Debug;
use std::ops::Range;

use crate::storage::persistence::filesystem::FsResult;

/// Represents a byte range within a file
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ByteRange {
    /// Start offset (inclusive)
    pub start: u64,
    /// End offset (exclusive)
    pub end: u64,
}

impl ByteRange {
    /// Create a new byte range
    pub fn new(start: u64, end: u64) -> Self {
        debug_assert!(start <= end, "start must be <= end");
        Self { start, end }
    }

    /// Create from a standard Range
    pub fn from_range(range: Range<u64>) -> Self {
        Self::new(range.start, range.end)
    }

    /// Get the length of this range
    pub fn len(&self) -> u64 {
        self.end - self.start
    }

    /// Check if range is empty
    pub fn is_empty(&self) -> bool {
        self.start >= self.end
    }

    /// Check if this range overlaps with another
    pub fn overlaps(&self, other: &ByteRange) -> bool {
        self.start < other.end && other.start < self.end
    }

    /// Check if this range is adjacent to another (within threshold)
    pub fn is_adjacent(&self, other: &ByteRange, threshold: u64) -> bool {
        if self.end <= other.start {
            other.start - self.end <= threshold
        } else if other.end <= self.start {
            self.start - other.end <= threshold
        } else {
            // Overlapping ranges are considered adjacent
            true
        }
    }

    /// Merge with another range (assumes they overlap or are adjacent)
    pub fn merge(&self, other: &ByteRange) -> ByteRange {
        ByteRange::new(
            std::cmp::min(self.start, other.start),
            std::cmp::max(self.end, other.end),
        )
    }

    /// Convert to standard Range
    pub fn to_range(&self) -> Range<u64> {
        self.start..self.end
    }
}

impl From<Range<u64>> for ByteRange {
    fn from(range: Range<u64>) -> Self {
        Self::from_range(range)
    }
}

impl From<ByteRange> for Range<u64> {
    fn from(br: ByteRange) -> Self {
        br.to_range()
    }
}

/// Estimated cost of an I/O operation
#[derive(Debug, Clone)]
pub struct IoCostEstimate {
    /// Estimated number of I/O operations
    pub io_operations: usize,
    /// Total bytes to read
    pub bytes_to_read: u64,
    /// Estimated latency in microseconds
    pub estimated_latency_us: u64,
    /// Whether parallel execution is recommended
    pub recommend_parallel: bool,
}

impl IoCostEstimate {
    /// Create a new I/O cost estimate
    pub fn new(io_operations: usize, bytes_to_read: u64) -> Self {
        // Estimate latency: base latency + per-byte latency
        // Base: 100us per operation, Per-byte: 1us per 10KB
        let estimated_latency_us =
            (io_operations as u64 * 100) + (bytes_to_read / 10_240);

        Self {
            io_operations,
            bytes_to_read,
            estimated_latency_us,
            recommend_parallel: io_operations > 1,
        }
    }

    /// Check if this operation should be parallelized
    pub fn should_parallelize(&self, min_parallel_ops: usize) -> bool {
        self.io_operations >= min_parallel_ops
    }
}

/// I/O execution strategy (D in SOLID - depend on abstraction)
///
/// Implementations can choose different strategies:
/// - Sequential reads for small operations
/// - Parallel reads for large operations
/// - Batched reads for cloud storage
#[async_trait]
pub trait IoStrategy: Send + Sync + Debug {
    /// Execute optimized reads for the given ranges
    ///
    /// # Arguments
    /// * `file` - Path to the file to read from
    /// * `ranges` - Byte ranges to read (should be pre-optimized/coalesced)
    ///
    /// # Returns
    /// Vector of data buffers, one per input range
    async fn execute_read(&self, file: &str, ranges: &[ByteRange]) -> FsResult<Vec<Vec<u8>>>;

    /// Estimate the cost of reading the given ranges
    fn estimate_cost(&self, ranges: &[ByteRange]) -> IoCostEstimate;

    /// Get the name of this strategy (for logging/metrics)
    fn strategy_name(&self) -> &'static str;
}

/// Range optimization strategy
///
/// Implementations can optimize range access patterns:
/// - Coalescing adjacent ranges to reduce I/O operations
/// - Splitting large ranges for better parallelism
/// - Reordering ranges for sequential access
pub trait RangeOptimizer: Send + Sync + Debug {
    /// Coalesce ranges to reduce the number of I/O requests
    ///
    /// Merges adjacent or nearby ranges when the gap between them
    /// is less than the threshold. This reduces I/O operations at
    /// the cost of potentially reading some unused bytes.
    ///
    /// # Arguments
    /// * `ranges` - Input ranges to coalesce
    /// * `threshold` - Maximum gap (in bytes) between ranges to merge
    ///
    /// # Returns
    /// Coalesced ranges, sorted by start offset
    fn coalesce(&self, ranges: Vec<ByteRange>, threshold: u64) -> Vec<ByteRange>;

    /// Split large ranges for better parallelism
    ///
    /// Divides a large range into smaller chunks that can be
    /// read in parallel for better throughput.
    ///
    /// # Arguments
    /// * `range` - The range to split
    /// * `target_size` - Target size for each chunk
    ///
    /// # Returns
    /// Vector of smaller ranges
    fn split_for_parallelism(&self, range: ByteRange, target_size: u64) -> Vec<ByteRange>;

    /// Optimize ranges for sequential access
    ///
    /// Sorts ranges by start offset for optimal disk/SSD access patterns.
    fn optimize_access_order(&self, ranges: Vec<ByteRange>) -> Vec<ByteRange> {
        let mut sorted = ranges;
        sorted.sort_by_key(|r| r.start);
        sorted
    }

    /// Calculate the efficiency gain from coalescing
    ///
    /// Returns a ratio: (original_count - coalesced_count) / original_count
    fn coalesce_efficiency(&self, original: &[ByteRange], coalesced: &[ByteRange]) -> f64 {
        if original.is_empty() {
            return 0.0;
        }
        let reduced = original.len().saturating_sub(coalesced.len());
        reduced as f64 / original.len() as f64
    }
}

/// Mapping between original and coalesced ranges
///
/// Used to extract the original requested data from coalesced reads.
#[derive(Debug, Clone)]
pub struct RangeMapping {
    /// Index of the coalesced range this original range maps to
    pub coalesced_index: usize,
    /// Offset within the coalesced range's data
    pub offset_in_coalesced: u64,
    /// Length of the original range
    pub length: u64,
}

/// Extended range optimizer with mapping support
pub trait RangeOptimizerWithMapping: RangeOptimizer {
    /// Coalesce ranges and return mapping information
    ///
    /// This allows extracting the original data from the coalesced reads.
    fn coalesce_with_mapping(
        &self,
        ranges: Vec<ByteRange>,
        threshold: u64,
    ) -> (Vec<ByteRange>, Vec<RangeMapping>);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_byte_range_basics() {
        let range = ByteRange::new(100, 200);
        assert_eq!(range.len(), 100);
        assert!(!range.is_empty());

        let empty_range = ByteRange::new(100, 100);
        assert!(empty_range.is_empty());
    }

    #[test]
    fn test_byte_range_overlap() {
        let r1 = ByteRange::new(0, 100);
        let r2 = ByteRange::new(50, 150);
        let r3 = ByteRange::new(100, 200);
        let r4 = ByteRange::new(200, 300);

        assert!(r1.overlaps(&r2)); // Overlapping
        assert!(!r1.overlaps(&r3)); // Adjacent but not overlapping
        assert!(!r1.overlaps(&r4)); // Disjoint
    }

    #[test]
    fn test_byte_range_adjacent() {
        let r1 = ByteRange::new(0, 100);
        let r2 = ByteRange::new(100, 200);
        let r3 = ByteRange::new(110, 200);
        let r4 = ByteRange::new(200, 300);

        // Adjacent with 0 threshold
        assert!(r1.is_adjacent(&r2, 0));
        assert!(!r1.is_adjacent(&r3, 0));
        assert!(!r1.is_adjacent(&r4, 0));

        // Adjacent with 10 byte threshold
        assert!(r1.is_adjacent(&r3, 10));
        assert!(!r1.is_adjacent(&r4, 10));

        // Adjacent with 100 byte threshold
        assert!(r1.is_adjacent(&r4, 100));
    }

    #[test]
    fn test_byte_range_merge() {
        let r1 = ByteRange::new(0, 100);
        let r2 = ByteRange::new(50, 150);
        let merged = r1.merge(&r2);

        assert_eq!(merged.start, 0);
        assert_eq!(merged.end, 150);
    }

    #[test]
    fn test_io_cost_estimate() {
        let estimate = IoCostEstimate::new(5, 1_000_000);

        assert_eq!(estimate.io_operations, 5);
        assert_eq!(estimate.bytes_to_read, 1_000_000);
        assert!(estimate.recommend_parallel);
        assert!(estimate.should_parallelize(3));
        assert!(!estimate.should_parallelize(10));
    }

    #[test]
    fn test_byte_range_conversions() {
        let std_range = 100u64..200u64;
        let byte_range: ByteRange = std_range.clone().into();

        assert_eq!(byte_range.start, 100);
        assert_eq!(byte_range.end, 200);

        let back_to_std: Range<u64> = byte_range.into();
        assert_eq!(back_to_std, std_range);
    }
}
