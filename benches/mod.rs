// ProximaDB Benchmarks
//
// Comprehensive benchmark suite for ProximaDB including:
// - ANN-benchmarks standardized tests
// - Competitor comparisons (Qdrant, Weaviate, Milvus, Pinecone)
// - Storage engine performance tests
// - Index type comparisons

pub mod ann_benchmarks;
pub mod vectordb_comparison;

// Re-export commonly used types
pub use ann_benchmarks::{
    ANNBenchmarkConfig, ANNBenchmarksConfig, ANNBenchmarksRunner,
    BenchmarkResults, BuildParams, DatasetMetadata, QueryStats, SearchParams,
};
pub use vectordb_comparison::{ComparisonResult, ComparisonRunner, CompetitorConfig};
