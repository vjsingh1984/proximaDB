//! Common benchmark utilities and initialization

pub mod embedding_generator;
pub mod benchmark_utils;
pub mod validation;

#[allow(unused_imports)]
pub use embedding_generator::{EmbeddingGenerator, EmbeddingModel};
// Re-export validation functions if needed by other benchmarks
// Currently unused in bench_04_storage_unified
#[allow(unused_imports)]
pub use validation::{
    validate_flush_result, validate_search_results, validate_metadata_filter,
    validate_filesystem_write, calculate_compression_metrics,
    BenchmarkValidation, BenchmarkStatus, CompressionMetrics,
};

use std::sync::Once;

static INIT: Once = Once::new();

/// Initialize hardware capabilities and runtime environment for benchmarks
pub fn init_benchmark_environment() {
    INIT.call_once(|| {
        // Initialize hardware capabilities once
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();

        // Initialize tokio runtime for async benchmarks
        let _ = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build();
    });
}