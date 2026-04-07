// ProximaDB Benchmarks
//
// Comprehensive benchmark suite for ProximaDB including:
// - ANN-benchmarks standardized tests
// - Storage engine performance tests
// - Index type comparisons

#[path = "../src/bench/ann_benchmarks/mod.rs"]
pub mod ann_benchmarks;

// Re-export commonly used types
pub use ann_benchmarks::{
    ANNBenchmarkConfig, ANNBenchmarksRunner, BenchmarkResults, BuildParams, DatasetMetadata,
    QueryStats, SearchParams,
};
