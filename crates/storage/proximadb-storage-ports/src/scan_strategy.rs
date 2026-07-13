//! Unified scan strategy types — hoisted from the root crate's
//! `src/storage/scan_strategy.rs`.
//!
//! This is the pure-data slice of the scan-strategy subsystem: the
//! `ScanStrategy` enum, the `ScanIterator` trait, `ScanStatistics`, and
//! `ScanCostEstimate`. They depend only on `anyhow`, `async_trait`,
//! `proximadb_filter_expression::FilterExpression`, and
//! `proximadb_records::ProximaRecord` — no root-internal types, no concrete
//! engine references. Hoisting them clears two more `crate::` references from
//! `src/storage/traits` (the gap-2 blockers of the root-crate decomposition).
//!
//! The old import paths are preserved via `pub use` re-export shims in the
//! root crate's `src/storage/scan_strategy.rs` so every existing caller
//! resolves unchanged. The `UnifiedScanEngine` trait (which names the proto
//! `Collection`) stays in the root — it is not pure-data and is out of scope.

use anyhow::Result;
use async_trait::async_trait;
use proximadb_filter_expression::FilterExpression;
use proximadb_records::ProximaRecord;

/// Unified scan strategy based on RAPTOR's successful pattern
#[derive(Debug, Clone)]
pub enum ScanStrategy {
    /// Full scan for compaction, maintenance, export
    /// Based on: SST's full_scan_strategy, VIPER's full file read, RAPTOR's FullScan
    FullScan {
        /// Include deleted/tombstoned records (for compaction)
        include_deleted: bool,
        /// Batch size for memory control
        batch_size: usize,
        /// Enable parallel file processing (from SST)
        parallel: bool,
        /// Use cache if available (from SST)
        use_cache: bool,
    },

    /// Filtered scan with optimizations
    /// Based on: VIPER's predicate_pushdown, NOVA's progressive search, RAPTOR's Filtering
    FilteredScan {
        /// Target IDs for direct lookup (from RAPTOR)
        target_ids: Option<Vec<String>>,
        /// Metadata predicates (from VIPER/NOVA)
        predicates: Option<FilterExpression>,
        /// Maximum row groups/blocks to scan (from RAPTOR)
        max_blocks: Option<usize>,
        /// Enable predicate pushdown (from VIPER/NOVA)
        enable_pushdown: bool,
        /// Enable column projection (from NOVA)
        enable_projection: bool,
        /// Early termination on limit
        early_termination: bool,
        /// Result limit
        limit: Option<usize>,
    },

    /// Progressive scan with quantization stages (from NOVA)
    ProgressiveScan {
        /// Binary filtering stage
        binary_candidates: usize,
        /// INT8 refinement stage
        int8_candidates: usize,
        /// Final FP32 stage
        fp32_candidates: usize,
        /// Memory budget
        memory_budget_bytes: Option<usize>,
        /// Latency budget
        latency_budget_ms: Option<u64>,
    },

    /// Range scan for ordered data (from SST)
    RangeScan {
        /// Start key (inclusive)
        start_key: Option<String>,
        /// End key (exclusive)
        end_key: Option<String>,
        /// Scan direction
        reverse: bool,
        /// Use index if available
        use_index: bool,
    },
}

/// Scan iterator following existing patterns
#[async_trait]
pub trait ScanIterator: Send {
    /// Get next batch (all engines use batched reading)
    async fn next_batch(&mut self) -> Result<Option<Vec<ProximaRecord>>>;

    /// Skip to position (for range scans)
    async fn seek(&mut self, _key: &str) -> Result<()> {
        Err(anyhow::anyhow!("Seek not supported by this iterator"))
    }

    /// Get current statistics
    fn statistics(&self) -> ScanStatistics;

    /// Cancel scan
    fn cancel(&mut self);
}

/// Scan statistics from existing implementations
#[derive(Debug, Clone, Default)]
pub struct ScanStatistics {
    // From all engines
    pub records_scanned: usize,
    pub records_matched: usize,
    pub bytes_read: usize,

    // From columnar engines (VIPER/NOVA)
    pub row_groups_scanned: usize,
    pub row_groups_pruned: usize,
    pub columns_read: usize,

    // From SST
    pub blocks_scanned: usize,
    pub blocks_filtered: usize,
    pub bloom_filter_hits: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,

    // From NOVA progressive search
    pub binary_candidates: usize,
    pub int8_candidates: usize,
    pub fp32_candidates: usize,

    // Timing
    pub io_time_ms: u64,
    pub filter_time_ms: u64,
    pub total_time_ms: u64,
}

/// Scan cost estimate for query planning
#[derive(Debug, Clone)]
pub struct ScanCostEstimate {
    pub estimated_records: usize,
    pub estimated_bytes: usize,
    pub estimated_time_ms: u64,
    pub estimated_memory_bytes: usize,
    pub confidence: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_strategy_creation() {
        // Test patterns from existing engines
        let full_scan = ScanStrategy::FullScan {
            include_deleted: false,
            batch_size: 1024,
            parallel: true,
            use_cache: true,
        };

        let filtered_scan = ScanStrategy::FilteredScan {
            target_ids: Some(vec!["id1".to_string()]),
            predicates: None,
            max_blocks: Some(100),
            enable_pushdown: true,
            enable_projection: true,
            early_termination: true,
            limit: Some(10),
        };

        let progressive_scan = ScanStrategy::ProgressiveScan {
            binary_candidates: 10000,
            int8_candidates: 1000,
            fp32_candidates: 100,
            memory_budget_bytes: Some(1024 * 1024 * 1024),
            latency_budget_ms: Some(1000),
        };

        // All strategies should be constructible
        matches!(full_scan, ScanStrategy::FullScan { .. });
        matches!(filtered_scan, ScanStrategy::FilteredScan { .. });
        matches!(progressive_scan, ScanStrategy::ProgressiveScan { .. });
    }
}
