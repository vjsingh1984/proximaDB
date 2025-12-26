//! RAPTOR Engine Optimization Paths
//!
//! RAPTOR uses adaptive row-group organization with multi-tier storage.
//! Paths focus on matrix operations and adaptive routing.

use super::{OptimizationPath, PathExpectation};
use crate::query::rl_planner::action::{
    BlockPruneConfig, ExecutionAction, IndexStrategy, QuantizationStage, SearchModeAction,
};

/// Get all RAPTOR optimization paths
pub fn paths() -> Vec<OptimizationPath> {
    vec![
        // Path 1: MatrixScan + FP32 (baseline)
        OptimizationPath::new(
            "raptor_baseline",
            "Matrix Scan (Baseline)",
            "Full matrix scan. 100% recall.",
            ExecutionAction::default(),
        )
        .with_expectation(PathExpectation {
            recall_range: (1.0, 1.0),
            latency_multiplier: 1.0,
            throughput_multiplier: 1.0,
            memory_factor: 1.0,
        })
        .with_use_cases(vec![
            "Small datasets",
            "Exact search required",
        ]),

        // Path 2: P²Matrix + FP32
        OptimizationPath::new(
            "raptor_p2matrix",
            "P² Matrix Pruning",
            "Use P² matrix organization for sqrt(N) block pruning.",
            ExecutionAction::default()
                .with_block_pruning(BlockPruneConfig::Sqrt),
        )
        .with_expectation(PathExpectation {
            recall_range: (0.95, 0.99),
            latency_multiplier: 0.5,
            throughput_multiplier: 2.0,
            memory_factor: 0.9,
        })
        .with_use_cases(vec![
            "Well-organized matrices",
            "When sqrt(N) blocks can be pruned",
        ]),

        // Path 3: K²Matrix + Pruning
        OptimizationPath::new(
            "raptor_k2matrix",
            "K² Matrix Pruning",
            "Use K² tree-based matrix organization with centroid pruning.",
            ExecutionAction::default()
                .with_block_pruning(BlockPruneConfig::CentroidDistance { threshold: 1.5 }),
        )
        .with_expectation(PathExpectation {
            recall_range: (0.92, 0.98),
            latency_multiplier: 0.4,
            throughput_multiplier: 2.5,
            memory_factor: 0.85,
        })
        .with_use_cases(vec![
            "Hierarchically organized data",
            "Clustered workloads",
        ]),

        // Path 4: Adaptive + Progressive
        OptimizationPath::new(
            "raptor_adaptive_progressive",
            "Adaptive Progressive",
            "Adaptively switch search modes with progressive quantization.",
            ExecutionAction {
                search_mode: SearchModeAction::Adaptive { threshold: 10000 },
                quantization_stages: vec![
                    QuantizationStage::Binary,
                    QuantizationStage::INT8,
                    QuantizationStage::FP32,
                ],
                ..Default::default()
            },
        )
        .with_expectation(PathExpectation {
            recall_range: (0.90, 0.96),
            latency_multiplier: 0.35,
            throughput_multiplier: 3.0,
            memory_factor: 0.7,
        })
        .with_use_cases(vec![
            "Variable workloads",
            "When optimal mode isn't known",
        ]),

        // Path 5: MultiTier + Quantized
        OptimizationPath::new(
            "raptor_multitier_quantized",
            "Multi-Tier + Quantized",
            "Use IVF with multi-tier storage and INT8 quantization.",
            ExecutionAction {
                index_strategy: Some(IndexStrategy::IVF { n_probe: 16 }),
                quantization_stages: vec![
                    QuantizationStage::INT8,
                    QuantizationStage::FP32,
                ],
                block_pruning: BlockPruneConfig::Ratio(0.3),
                ..Default::default()
            },
        )
        .with_expectation(PathExpectation {
            recall_range: (0.88, 0.95),
            latency_multiplier: 0.3,
            throughput_multiplier: 3.5,
            memory_factor: 0.8,
        })
        .with_use_cases(vec![
            "Large multi-tier datasets",
            "Cost-optimized storage",
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_raptor_paths_count() {
        let paths = paths();
        assert_eq!(paths.len(), 5, "RAPTOR should have 5 optimization paths");
    }
}
