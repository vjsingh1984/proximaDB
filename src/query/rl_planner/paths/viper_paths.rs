//! VIPER Engine Optimization Paths
//!
//! VIPER uses columnar Parquet format for analytics-optimized storage.
//! Paths focus on row group pruning, columnar filtering, and efficient I/O.

use super::{OptimizationPath, PathExpectation};
use crate::query::rl_planner::action::{
    BlockPruneConfig, ExecutionAction, IndexStrategy, QuantizationStage,
};

/// Get all VIPER optimization paths
pub fn paths() -> Vec<OptimizationPath> {
    vec![
        // Path 1: RowGroupScan + FP32 (baseline)
        OptimizationPath::new(
            "viper_baseline",
            "Row Group Scan (Baseline)",
            "Full scan of all Parquet row groups. 100% recall.",
            ExecutionAction::default(),
        )
        .with_expectation(PathExpectation {
            recall_range: (1.0, 1.0),
            latency_multiplier: 1.0,
            throughput_multiplier: 1.0,
            memory_factor: 1.0,
        })
        .with_use_cases(vec![
            "Analytics queries",
            "When all data must be scanned",
            "Baseline comparison",
        ]),

        // Path 2: RowGroupPrune + FP32
        OptimizationPath::new(
            "viper_rowgroup_prune",
            "Row Group Pruning",
            "Skip row groups based on column statistics.",
            ExecutionAction::default()
                .with_block_pruning(BlockPruneConfig::Ratio(0.5)),
        )
        .with_expectation(PathExpectation {
            recall_range: (0.95, 1.0),
            latency_multiplier: 0.6,
            throughput_multiplier: 1.7,
            memory_factor: 0.8,
        })
        .with_use_cases(vec![
            "Large Parquet files",
            "Well-organized data with good statistics",
        ]),

        // Path 3: ColumnProjection + FP32
        OptimizationPath::new(
            "viper_column_projection",
            "Column Projection",
            "Read only vector and metadata columns needed.",
            ExecutionAction::default(),
        )
        .with_expectation(PathExpectation {
            recall_range: (1.0, 1.0),
            latency_multiplier: 0.8,
            throughput_multiplier: 1.3,
            memory_factor: 0.7,
        })
        .with_use_cases(vec![
            "Wide tables with many columns",
            "When only specific metadata needed",
        ]),

        // Path 4: Binary Pre-filter + FP32
        OptimizationPath::new(
            "viper_binary_prefilter",
            "Binary Pre-filtering",
            "Use binary quantization for initial row group filtering.",
            ExecutionAction {
                quantization_stages: vec![
                    QuantizationStage::Binary,
                    QuantizationStage::FP32,
                ],
                ..Default::default()
            },
        )
        .with_expectation(PathExpectation {
            recall_range: (0.88, 0.95),
            latency_multiplier: 0.4,
            throughput_multiplier: 2.5,
            memory_factor: 0.6,
        })
        .with_use_cases(vec![
            "Large datasets",
            "When some recall loss is acceptable",
        ]),

        // Path 5: INT8 Columnar + FP32
        OptimizationPath::new(
            "viper_int8_columnar",
            "INT8 Columnar Search",
            "Use INT8 quantized vectors stored in columnar format.",
            ExecutionAction {
                quantization_stages: vec![
                    QuantizationStage::INT8,
                    QuantizationStage::FP32,
                ],
                ..Default::default()
            },
        )
        .with_expectation(PathExpectation {
            recall_range: (0.93, 0.98),
            latency_multiplier: 0.5,
            throughput_multiplier: 2.0,
            memory_factor: 0.75,
        })
        .with_use_cases(vec![
            "Production analytics",
            "Balanced speed/recall",
        ]),

        // Path 6: HNSW + RowGroupPrune
        OptimizationPath::new(
            "viper_hnsw_rowgroup",
            "HNSW + Row Group Pruning",
            "HNSW index combined with Parquet row group pruning.",
            ExecutionAction::with_hnsw(100)
                .with_block_pruning(BlockPruneConfig::Ratio(0.3)),
        )
        .with_expectation(PathExpectation {
            recall_range: (0.90, 0.97),
            latency_multiplier: 0.3,
            throughput_multiplier: 3.5,
            memory_factor: 1.2,
        })
        .with_use_cases(vec![
            "Real-time queries on analytics data",
            "When AXIS index is available",
        ]),

        // Path 7: IVF + Columnar + INT8
        OptimizationPath::new(
            "viper_ivf_columnar_int8",
            "IVF + Columnar + INT8",
            "Cluster-based search with columnar INT8 vectors.",
            ExecutionAction {
                index_strategy: Some(IndexStrategy::IVF { n_probe: 16 }),
                quantization_stages: vec![
                    QuantizationStage::INT8,
                    QuantizationStage::FP32,
                ],
                ..Default::default()
            },
        )
        .with_expectation(PathExpectation {
            recall_range: (0.88, 0.95),
            latency_multiplier: 0.35,
            throughput_multiplier: 3.0,
            memory_factor: 0.9,
        })
        .with_use_cases(vec![
            "Large analytics datasets",
            "Batch processing",
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_viper_paths_count() {
        let paths = paths();
        assert_eq!(paths.len(), 7, "VIPER should have 7 optimization paths");
    }
}
