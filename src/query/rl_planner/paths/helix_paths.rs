//! HELIX Engine Optimization Paths
//!
//! HELIX uses Hilbert curve ordering with PCA for high-dimensional vectors.
//! Paths leverage spatial clustering, zone maps, and progressive quantization.

use super::{OptimizationPath, PathExpectation};
use crate::query::rl_planner::action::{
    BlockPruneConfig, ExecutionAction, IndexStrategy, QuantizationStage, SearchModeAction,
};

/// Get all HELIX optimization paths
pub fn paths() -> Vec<OptimizationPath> {
    vec![
        // Path 1: DirectScan + FP32 (baseline)
        OptimizationPath::new(
            "helix_baseline",
            "Direct Scan (Baseline)",
            "Full scan of Hilbert-ordered blocks. 100% recall.",
            ExecutionAction::default(),
        )
        .with_expectation(PathExpectation {
            recall_range: (1.0, 1.0),
            latency_multiplier: 1.0,
            throughput_multiplier: 1.0,
            memory_factor: 1.0,
        })
        .with_use_cases(vec![
            "Small collections",
            "Exact search required",
            "Baseline comparison",
        ]),

        // Path 2: HNSW + ZoneMap + FP32
        OptimizationPath::new(
            "helix_hnsw_zonemap",
            "HNSW + Zone Maps",
            "HNSW index with zone map pruning for Hilbert blocks.",
            ExecutionAction::with_hnsw(100).with_zone_map(),
        )
        .with_expectation(PathExpectation {
            recall_range: (0.92, 0.98),
            latency_multiplier: 0.3,
            throughput_multiplier: 3.5,
            memory_factor: 1.3,
        })
        .with_use_cases(vec![
            "High-dimensional vectors",
            "Spatially clustered data",
        ]),

        // Path 3: Progressive(Binary→INT8→PQ→FP32)
        OptimizationPath::new(
            "helix_full_progressive",
            "Full Progressive Pipeline",
            "4-stage progressive search using Hilbert locality.",
            ExecutionAction {
                quantization_stages: vec![
                    QuantizationStage::Binary,
                    QuantizationStage::INT8,
                    QuantizationStage::PQ8,
                    QuantizationStage::FP32,
                ],
                search_mode: SearchModeAction::Approximate {
                    expansion_factor: 2.0,
                },
                ..Default::default()
            },
        )
        .with_expectation(PathExpectation {
            recall_range: (0.90, 0.96),
            latency_multiplier: 0.25,
            throughput_multiplier: 4.0,
            memory_factor: 0.8,
        })
        .with_use_cases(vec![
            "Very high-dimensional vectors (768+)",
            "Memory-constrained systems",
            "Batch search workloads",
        ]),

        // Path 4: HilbertPrune + Progressive
        OptimizationPath::new(
            "helix_hilbert_progressive",
            "Hilbert Pruning + Progressive",
            "Leverage Hilbert ordering for centroid-based pruning with progressive search.",
            ExecutionAction {
                block_pruning: BlockPruneConfig::CentroidDistance { threshold: 1.5 },
                quantization_stages: vec![
                    QuantizationStage::Binary,
                    QuantizationStage::INT8,
                    QuantizationStage::FP32,
                ],
                ..Default::default()
            },
        )
        .with_expectation(PathExpectation {
            recall_range: (0.88, 0.95),
            latency_multiplier: 0.2,
            throughput_multiplier: 5.0,
            memory_factor: 0.7,
        })
        .with_use_cases(vec![
            "Well-clustered data",
            "When Hilbert ordering is effective",
        ]),

        // Path 5: PCA + HilbertPrune + FP32
        OptimizationPath::new(
            "helix_pca_hilbert",
            "PCA + Hilbert Pruning",
            "Use PCA-reduced representation for coarse filtering.",
            ExecutionAction {
                block_pruning: BlockPruneConfig::CentroidDistance { threshold: 2.0 },
                ..Default::default()
            },
        )
        .with_expectation(PathExpectation {
            recall_range: (0.93, 0.98),
            latency_multiplier: 0.5,
            throughput_multiplier: 2.0,
            memory_factor: 1.0,
        })
        .with_use_cases(vec![
            "High-dimensional data",
            "When PCA is well-trained",
        ]),

        // Path 6: IVF + ZoneMap + INT8
        OptimizationPath::new(
            "helix_ivf_zonemap_int8",
            "IVF + Zone Maps + INT8",
            "Cluster-based search with zone maps and INT8 quantization.",
            ExecutionAction {
                index_strategy: Some(IndexStrategy::IVF { n_probe: 16 }),
                quantization_stages: vec![QuantizationStage::INT8, QuantizationStage::FP32],
                zone_map_enabled: true,
                block_pruning: BlockPruneConfig::ZoneMap,
                ..Default::default()
            },
        )
        .with_expectation(PathExpectation {
            recall_range: (0.89, 0.95),
            latency_multiplier: 0.35,
            throughput_multiplier: 3.0,
            memory_factor: 0.9,
        })
        .with_use_cases(vec![
            "Large collections",
            "Production workloads",
        ]),

        // Path 7: LSH + Progressive
        OptimizationPath::new(
            "helix_lsh_progressive",
            "LSH + Progressive",
            "Locality-sensitive hashing with progressive refinement.",
            ExecutionAction {
                index_strategy: Some(IndexStrategy::LSH {
                    n_tables: 10,
                    n_hashes: 8,
                }),
                quantization_stages: vec![
                    QuantizationStage::Binary,
                    QuantizationStage::INT8,
                    QuantizationStage::FP32,
                ],
                ..Default::default()
            },
        )
        .with_expectation(PathExpectation::fast_approximate())
        .with_use_cases(vec![
            "Very fast approximate search",
            "Binary-friendly data",
        ]),

        // Path 8: Full Progressive (5-stage)
        OptimizationPath::new(
            "helix_5stage_progressive",
            "5-Stage Progressive Pipeline",
            "Maximum pruning with 5 quantization stages.",
            ExecutionAction {
                quantization_stages: vec![
                    QuantizationStage::Binary,
                    QuantizationStage::INT8,
                    QuantizationStage::PQ4,
                    QuantizationStage::PQ8,
                    QuantizationStage::FP32,
                ],
                search_mode: SearchModeAction::Approximate {
                    expansion_factor: 3.0,
                },
                block_pruning: BlockPruneConfig::CentroidDistance { threshold: 2.0 },
                ..Default::default()
            },
        )
        .with_expectation(PathExpectation {
            recall_range: (0.85, 0.93),
            latency_multiplier: 0.15,
            throughput_multiplier: 6.0,
            memory_factor: 0.6,
        })
        .with_use_cases(vec![
            "Maximum throughput",
            "When recall can be sacrificed",
            "Streaming workloads",
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_helix_paths_count() {
        let paths = paths();
        assert_eq!(paths.len(), 8, "HELIX should have 8 optimization paths");
    }

    #[test]
    fn test_progressive_paths_have_multiple_stages() {
        let paths = paths();
        for path in paths.iter().filter(|p| p.id.contains("progressive")) {
            assert!(
                path.action.quantization_stages.len() >= 2,
                "Progressive path {} should have multiple stages",
                path.name
            );
        }
    }
}
