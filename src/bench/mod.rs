// ProximaDB Benchmarks
//
// Comprehensive benchmark suite for ProximaDB including:
// - ANN-benchmarks standardized tests
// - Competitor comparisons (Qdrant, Weaviate, Milvus, Pinecone)
// - Storage engine performance tests
// - Index type comparisons

/// ANN benchmark suite for evaluating recall, throughput, and latency.
pub mod ann_benchmarks;

// Note: vectordb_comparison is a standalone binary at src/bin/vectordb_comparison.rs
// It contains all its code inline and doesn't need to be a library module

// Re-export commonly used types
pub use ann_benchmarks::{
    ANNBenchmarkConfig as ANNBenchConfig, ANNBenchmarksRunner, BenchmarkResults, BuildParams,
    DatasetMetadata, QueryStats, SearchParams,
};
