//! SWIFT Engine Optimization Paths
//!
//! SWIFT is optimized for ultra-low latency with in-memory operations.
//! Paths focus on minimal overhead and parallel execution.

use super::{OptimizationPath, PathExpectation};
use crate::query::rl_planner::action::{
    ExecutionAction, QuantizationStage, ParallelismConfig,
};

/// Get all SWIFT optimization paths
pub fn paths() -> Vec<OptimizationPath> {
    vec![
        // Path 1: InMemory + FP32 (baseline)
        OptimizationPath::new(
            "swift_baseline",
            "In-Memory Scan (Baseline)",
            "Full in-memory scan. Ultra-low latency, 100% recall.",
            ExecutionAction::default(),
        )
        .with_expectation(PathExpectation {
            recall_range: (1.0, 1.0),
            latency_multiplier: 1.0,
            throughput_multiplier: 1.0,
            memory_factor: 1.0,
        })
        .with_use_cases(vec![
            "Small collections (<5K vectors)",
            "When latency is critical",
            "Real-time applications",
        ]),

        // Path 2: InMemory + INT8
        OptimizationPath::new(
            "swift_int8",
            "In-Memory INT8",
            "INT8 quantization for reduced memory and faster SIMD.",
            ExecutionAction {
                quantization_stages: vec![QuantizationStage::INT8],
                ..Default::default()
            },
        )
        .with_expectation(PathExpectation {
            recall_range: (0.95, 0.99),
            latency_multiplier: 0.7,
            throughput_multiplier: 1.5,
            memory_factor: 0.25,
        })
        .with_use_cases(vec![
            "Memory-constrained edge devices",
            "High-throughput scenarios",
        ]),

        // Path 3: Progressive(Binary→INT8→FP32)
        OptimizationPath::new(
            "swift_progressive",
            "Progressive Quantization",
            "Multi-stage in-memory filtering.",
            ExecutionAction::with_progressive_quantization(),
        )
        .with_expectation(PathExpectation {
            recall_range: (0.93, 0.98),
            latency_multiplier: 0.5,
            throughput_multiplier: 2.0,
            memory_factor: 0.4,
        })
        .with_use_cases(vec![
            "Larger in-memory collections",
            "When some recall tradeoff is acceptable",
        ]),

        // Path 4: HNSW + InMemory
        OptimizationPath::new(
            "swift_hnsw",
            "HNSW In-Memory",
            "HNSW index fully in memory for minimum latency.",
            ExecutionAction::with_hnsw(50),
        )
        .with_expectation(PathExpectation {
            recall_range: (0.92, 0.97),
            latency_multiplier: 0.3,
            throughput_multiplier: 3.5,
            memory_factor: 1.5,
        })
        .with_use_cases(vec![
            "Collections > 5K vectors",
            "Sub-millisecond requirements",
        ]),

        // Path 5: Parallel + Progressive
        OptimizationPath::new(
            "swift_parallel_progressive",
            "Parallel Progressive",
            "Multi-threaded progressive search with SIMD.",
            ExecutionAction {
                quantization_stages: vec![
                    QuantizationStage::Binary,
                    QuantizationStage::INT8,
                    QuantizationStage::FP32,
                ],
                parallelism: ParallelismConfig {
                    num_threads: 8,
                    enable_simd: true,
                    batch_size: 512,
                },
                ..Default::default()
            },
        )
        .with_expectation(PathExpectation {
            recall_range: (0.93, 0.98),
            latency_multiplier: 0.25,
            throughput_multiplier: 4.0,
            memory_factor: 0.5,
        })
        .with_use_cases(vec![
            "Multi-core systems",
            "Maximum throughput with low latency",
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swift_paths_count() {
        let paths = paths();
        assert_eq!(paths.len(), 5, "SWIFT should have 5 optimization paths");
    }

    #[test]
    fn test_swift_has_low_latency() {
        let paths = paths();
        // All SWIFT paths should have latency_multiplier <= 1.0
        for path in &paths {
            assert!(
                path.expected.latency_multiplier <= 1.0,
                "SWIFT path {} should have low latency",
                path.name
            );
        }
    }
}
