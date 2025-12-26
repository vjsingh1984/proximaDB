//! SST Engine Optimization Paths
//!
//! SST (Sorted String Table) is the default write-optimized engine using LSM-tree structure.
//! Paths focus on index usage, bloom filters, and block pruning.

use super::{OptimizationPath, PathExpectation};
use crate::query::rl_planner::action::{
    BlockPruneConfig, ExecutionAction, IndexStrategy, QuantizationStage, SearchModeAction,
};

/// Get all SST optimization paths
pub fn paths() -> Vec<OptimizationPath> {
    vec![
        // Path 1: DirectScan + FP32 (baseline)
        OptimizationPath::new(
            "sst_baseline",
            "Direct Scan (Baseline)",
            "Full scan without any optimization. 100% recall, highest latency.",
            ExecutionAction::default(),
        )
        .with_expectation(PathExpectation {
            recall_range: (1.0, 1.0),
            latency_multiplier: 1.0,
            throughput_multiplier: 1.0,
            memory_factor: 1.0,
        })
        .with_use_cases(vec![
            "Small collections (<1000 vectors)",
            "When 100% recall is required",
            "Debugging and validation",
        ]),

        // Path 2: HNSW(ef=50) + FP32
        OptimizationPath::new(
            "sst_hnsw_50",
            "HNSW (ef=50)",
            "Fast approximate search with low expansion factor.",
            ExecutionAction::with_hnsw(50),
        )
        .with_expectation(PathExpectation {
            recall_range: (0.90, 0.96),
            latency_multiplier: 0.2,
            throughput_multiplier: 5.0,
            memory_factor: 1.2,
        })
        .with_use_cases(vec![
            "Real-time search",
            "Collections > 10K vectors",
            "When ~95% recall is acceptable",
        ]),

        // Path 3: HNSW(ef=100) + FP32
        OptimizationPath::new(
            "sst_hnsw_100",
            "HNSW (ef=100)",
            "Balanced HNSW search with moderate expansion.",
            ExecutionAction::with_hnsw(100),
        )
        .with_expectation(PathExpectation::balanced())
        .with_use_cases(vec![
            "Production workloads",
            "Balanced latency/recall tradeoff",
        ]),

        // Path 4: HNSW(ef=200) + FP32
        OptimizationPath::new(
            "sst_hnsw_200",
            "HNSW (ef=200)",
            "High-recall HNSW search with large expansion factor.",
            ExecutionAction::with_hnsw(200),
        )
        .with_expectation(PathExpectation::high_recall())
        .with_use_cases(vec![
            "When high recall (>98%) is needed",
            "Recommendation systems",
        ]),

        // Path 5: IVF(nprobe=4) + FP32
        OptimizationPath::new(
            "sst_ivf_4",
            "IVF (nprobe=4)",
            "Fast IVF search probing few clusters.",
            ExecutionAction::with_ivf(4),
        )
        .with_expectation(PathExpectation::fast_approximate())
        .with_use_cases(vec![
            "Very large collections (>1M vectors)",
            "When speed is priority over recall",
        ]),

        // Path 6: IVF(nprobe=16) + FP32
        OptimizationPath::new(
            "sst_ivf_16",
            "IVF (nprobe=16)",
            "Balanced IVF search with moderate cluster probing.",
            ExecutionAction::with_ivf(16),
        )
        .with_expectation(PathExpectation {
            recall_range: (0.90, 0.97),
            latency_multiplier: 0.4,
            throughput_multiplier: 2.5,
            memory_factor: 1.1,
        })
        .with_use_cases(vec![
            "Large collections with good recall requirements",
        ]),

        // Path 7: IVF(nprobe=32) + FP32
        OptimizationPath::new(
            "sst_ivf_32",
            "IVF (nprobe=32)",
            "High-recall IVF search probing many clusters.",
            ExecutionAction::with_ivf(32),
        )
        .with_expectation(PathExpectation {
            recall_range: (0.95, 0.99),
            latency_multiplier: 0.6,
            throughput_multiplier: 1.7,
            memory_factor: 1.1,
        })
        .with_use_cases(vec![
            "High recall with large collections",
        ]),

        // Path 8: Bloom + BlockPrune + FP32
        OptimizationPath::new(
            "sst_bloom_prune",
            "Bloom + Block Pruning",
            "Use bloom filters and block pruning without index.",
            ExecutionAction::default()
                .with_bloom_filter()
                .with_block_pruning(BlockPruneConfig::Sqrt),
        )
        .with_expectation(PathExpectation {
            recall_range: (0.95, 1.0),
            latency_multiplier: 0.7,
            throughput_multiplier: 1.5,
            memory_factor: 1.1,
        })
        .with_use_cases(vec![
            "When AXIS indexes not available",
            "Metadata-heavy queries",
        ]),

        // Path 9: HNSW + Bloom + BlockPrune
        OptimizationPath::new(
            "sst_hnsw_bloom_prune",
            "HNSW + Bloom + Pruning",
            "Full optimization stack: HNSW index with bloom filter and block pruning.",
            ExecutionAction::with_hnsw(100)
                .with_bloom_filter()
                .with_block_pruning(BlockPruneConfig::Sqrt),
        )
        .with_expectation(PathExpectation {
            recall_range: (0.92, 0.98),
            latency_multiplier: 0.25,
            throughput_multiplier: 4.0,
            memory_factor: 1.3,
        })
        .with_use_cases(vec![
            "Maximum throughput with good recall",
            "Production workloads with filters",
        ]),

        // Path 10: Progressive(Binary→INT8→FP32)
        OptimizationPath::new(
            "sst_progressive",
            "Progressive Quantization",
            "Multi-stage search: Binary filter → INT8 filter → FP32 final.",
            ExecutionAction::with_progressive_quantization(),
        )
        .with_expectation(PathExpectation {
            recall_range: (0.93, 0.98),
            latency_multiplier: 0.35,
            throughput_multiplier: 3.0,
            memory_factor: 0.9,
        })
        .with_use_cases(vec![
            "Memory-constrained environments",
            "Large vectors (>512 dimensions)",
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sst_paths_count() {
        let paths = paths();
        assert_eq!(paths.len(), 10, "SST should have 10 optimization paths");
    }

    #[test]
    fn test_baseline_has_100_recall() {
        let paths = paths();
        let baseline = &paths[0];
        assert_eq!(baseline.expected.recall_range.0, 1.0);
        assert_eq!(baseline.expected.recall_range.1, 1.0);
    }
}
