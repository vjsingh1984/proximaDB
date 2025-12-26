//! NOVA Engine Optimization Paths
//!
//! NOVA uses progressive columnar storage with zone maps.
//! Paths focus on adaptive streaming and progressive refinement.

use super::{OptimizationPath, PathExpectation};
use crate::query::rl_planner::action::{
    ExecutionAction, IndexStrategy, QuantizationStage, SearchModeAction,
};

/// Get all NOVA optimization paths
pub fn paths() -> Vec<OptimizationPath> {
    vec![
        // Path 1: Columnar + FP32 (baseline)
        OptimizationPath::new(
            "nova_baseline",
            "Columnar Scan (Baseline)",
            "Full columnar scan. 100% recall.",
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

        // Path 2: ZoneMap + FP32
        OptimizationPath::new(
            "nova_zonemap",
            "Zone Map Filtering",
            "Use zone maps to skip irrelevant column segments.",
            ExecutionAction::default().with_zone_map(),
        )
        .with_expectation(PathExpectation {
            recall_range: (0.98, 1.0),
            latency_multiplier: 0.6,
            throughput_multiplier: 1.7,
            memory_factor: 0.9,
        })
        .with_use_cases(vec![
            "Well-organized columnar data",
            "Range-based queries",
        ]),

        // Path 3: Progressive + ZoneMap
        OptimizationPath::new(
            "nova_progressive_zonemap",
            "Progressive + Zone Maps",
            "Combine zone map pruning with progressive quantization.",
            ExecutionAction::with_progressive_quantization().with_zone_map(),
        )
        .with_expectation(PathExpectation {
            recall_range: (0.92, 0.97),
            latency_multiplier: 0.4,
            throughput_multiplier: 2.5,
            memory_factor: 0.7,
        })
        .with_use_cases(vec![
            "Large columnar datasets",
            "Balanced performance",
        ]),

        // Path 4: IVF + Columnar
        OptimizationPath::new(
            "nova_ivf_columnar",
            "IVF + Columnar",
            "Cluster-based search over columnar storage.",
            ExecutionAction::with_ivf(16),
        )
        .with_expectation(PathExpectation {
            recall_range: (0.90, 0.96),
            latency_multiplier: 0.35,
            throughput_multiplier: 3.0,
            memory_factor: 1.1,
        })
        .with_use_cases(vec![
            "Large collections",
            "When IVF index is available",
        ]),

        // Path 5: Streaming + Progressive
        OptimizationPath::new(
            "nova_streaming_progressive",
            "Adaptive Streaming",
            "Stream results progressively, refining as more data arrives.",
            ExecutionAction {
                search_mode: SearchModeAction::Adaptive { threshold: 5000 },
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
            latency_multiplier: 0.3,
            throughput_multiplier: 3.5,
            memory_factor: 0.6,
        })
        .with_use_cases(vec![
            "Real-time streaming",
            "Progressive result delivery",
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nova_paths_count() {
        let paths = paths();
        assert_eq!(paths.len(), 5, "NOVA should have 5 optimization paths");
    }
}
