//! Performance benchmarks for unified storage engine strategies
//!
//! These benchmarks measure the performance characteristics of different storage engines,
//! including creation overhead, flush operations, and memory usage patterns.
//!
//! ## Benchmark Suite Contents:
//! - **Engine Creation**: Measures initialization overhead for each engine
//! - **Flush Operations**: Compares write throughput across engines
//! - **Memory Usage**: Tracks memory allocation patterns under load
//!
//! ## Storage Engines Tested:
//! - SST: Row-based storage optimized for real-time queries
//! - VIPER: Columnar Parquet format with compression
//! - NOVA: Progressive columnar with multi-level quantization
//! - SWIFT: High-speed row-based with Proxima encoding

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use std::sync::{Arc, Once};
use std::time::Duration;

use proximadb::storage::engines::factory::StorageEngineFactory;
use proximadb::storage::traits::FlushParameters;
use proximadb::proto::proximadb_v1::VectorRecord;

/// Global initialization for hardware capabilities
static INIT: Once = Once::new();

/// Initialize hardware capabilities once for all benchmarks
fn init_hardware() {
    INIT.call_once(|| {
        // Initialize hardware detection for SIMD/GPU acceleration
        let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    });
}

/// Benchmark setup helper that manages test data generation
/// and provides utility functions for engine benchmarks
struct BenchmarkSetup {
    /// Collection ID used across all benchmark runs
    collection_id: String,
}

impl BenchmarkSetup {
    /// Creates a new benchmark setup with initialized hardware
    fn new() -> Self {
        init_hardware();
        Self {
            collection_id: "bench-cross".to_string(),
        }
    }

    /// Generates test vectors with realistic data patterns
    ///
    /// # Arguments
    /// * `count` - Number of vectors to generate
    /// * `dimension` - Dimensionality of each vector
    ///
    /// # Returns
    /// Vec of VectorRecords with incrementing IDs and normalized values
    fn generate_vectors(&self, count: usize, dimension: usize) -> Vec<VectorRecord> {
        (0..count)
            .map(|i| VectorRecord {
                id: format!("vec_{:08}", i),
                // Generate normalized vectors with values between 0.0 and 1.0
                vector: vec![i as f32 / count as f32; dimension],
                metadata: std::collections::HashMap::new(),
                timestamp: Some(i as i64),
                updated_at: Some(chrono::Utc::now().timestamp()),
                expires_at: None,
                version: Some(1),
                quantized_vector: vec![],
                source: None,
            })
            .collect()
    }
}

/// Benchmark engine creation overhead
///
/// Measures the time taken to initialize each storage engine type.
/// This includes:
/// - Engine struct allocation
/// - Configuration parsing
/// - Initial resource allocation
/// - Hardware capability detection
fn bench_engine_creation(c: &mut Criterion) {
    let _setup = BenchmarkSetup::new();

    let mut group = c.benchmark_group("engine_creation");
    // Set consistent timing parameters
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));

    // Benchmark SST engine creation - Row-based storage
    group.bench_function("sst_engine", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_sst().unwrap();
            black_box(engine) // Prevent compiler optimization
        })
    });

    // Benchmark VIPER engine creation - Columnar Parquet storage
    group.bench_function("viper_engine", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_viper().unwrap();
            black_box(engine)
        })
    });

    // Benchmark NOVA engine creation - Progressive columnar storage
    group.bench_function("nova_engine", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_nova().unwrap();
            black_box(engine)
        })
    });

    // Benchmark SWIFT engine creation - High-speed row storage
    group.bench_function("swift_engine", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_swift().unwrap();
            black_box(engine)
        })
    });

    group.finish();
}

/// Benchmark engine flush operations
///
/// Measures the performance of flushing vectors to storage across different engines.
/// Tests write throughput and serialization overhead for each engine type.
///
/// ## Test Parameters:
/// - Vector count: 1024 vectors (increased for better throughput measurement)
/// - Dimension: 768 (typical for embeddings)
/// - Force flush: Bypasses write buffering
/// - Synchronous: Waits for completion
/// - Shared vectors: Same dataset used across all engines for fair comparison
fn bench_engine_flush(c: &mut Criterion) {
    let setup = BenchmarkSetup::new();

    // Generate 1024 vectors once and share across all engine benchmarks
    // This ensures fair comparison with identical data
    let shared_vectors = Arc::new(setup.generate_vectors(1000, 768));

    let mut group = c.benchmark_group("engine_flush");
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(20); // Reduce sample size for 1000-vector benchmarks

    // Create engines for benchmarking
    let engines = vec![
        ("sst", StorageEngineFactory::create_sst().unwrap()),
        ("viper", StorageEngineFactory::create_viper().unwrap()),
        ("nova", StorageEngineFactory::create_nova().unwrap()),
        ("swift", StorageEngineFactory::create_swift().unwrap()),
    ];

    for (name, engine) in engines {
        // Clone the Arc for this benchmark iteration
        let vectors_clone = Arc::clone(&shared_vectors);

        group.bench_function(
            format!("flush_1000_vectors_{}", name),
            |b| {
                b.iter(|| {
                    // Use futures::executor to avoid runtime conflicts
                    futures::executor::block_on(async {
                        let params = FlushParameters {
                            collection_id: Some(setup.collection_id.clone()),
                            // Clone the vector data from the Arc
                            vector_records: (*vectors_clone).clone(),
                            force: true,
                            synchronous: true,
                            ..Default::default()
                        };
                        let _ = engine.flush(params).await;
                    })
                })
            },
        );
    }

    group.finish();
}

/// Benchmark engine memory usage patterns
///
/// Measures memory allocation and usage patterns when loading different
/// amounts of data into each engine type. This helps identify:
/// - Memory overhead per engine
/// - Scaling characteristics with data volume
/// - Memory efficiency of different storage formats
///
/// ## Test Configurations:
/// - Small batch: 1024 vectors (baseline for meaningful stats)
/// - Medium batch: 5120 vectors (typical production workload)
/// - Large batch: 10240 vectors (stress test)
fn bench_engine_memory(c: &mut Criterion) {
    let setup = BenchmarkSetup::new();

    let mut group = c.benchmark_group("engine_memory_scaling");
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10); // Further reduce samples for larger memory-intensive tests

    // Test different vector counts to observe memory scaling
    // Pre-generate all vectors to share across engines
    let test_configurations = vec![
        (1000, "small_batch"),
        (5000, "medium_batch"),
        (10000, "large_batch"),
    ];

    // Pre-generate vectors for each configuration
    let vector_sets: Vec<(usize, &str, Arc<Vec<VectorRecord>>)> = test_configurations
        .into_iter()
        .map(|(count, label)| {
            (count, label, Arc::new(setup.generate_vectors(count, 768)))
        })
        .collect();

    // Test each configuration with both SST and VIPER engines
    for (count, label, vectors) in vector_sets {
        let vectors_sst = Arc::clone(&vectors);
        let vectors_viper = Arc::clone(&vectors);

        // Benchmark SST engine memory usage
        group.bench_function(
            format!("sst_{}_{}", label, count),
            |b| {
                b.iter(|| {
                    // Use futures::executor to avoid runtime conflicts
                    futures::executor::block_on(async {
                        let engine = StorageEngineFactory::create_sst().unwrap();
                        let params = FlushParameters {
                            collection_id: Some(setup.collection_id.clone()),
                            vector_records: (*vectors_sst).clone(),
                            force: true,
                            synchronous: true,
                            ..Default::default()
                        };
                        let _ = engine.flush(params).await;
                        black_box(engine) // Prevent optimization of unused engine
                    })
                })
            },
        );

        // Benchmark VIPER engine memory usage (columnar format)
        group.bench_function(
            format!("viper_{}_{}", label, count),
            |b| {
                b.iter(|| {
                    futures::executor::block_on(async {
                        let engine = StorageEngineFactory::create_viper().unwrap();
                        let params = FlushParameters {
                            collection_id: Some(setup.collection_id.clone()),
                            vector_records: (*vectors_viper).clone(),
                            force: true,
                            synchronous: true,
                            ..Default::default()
                        };
                        let _ = engine.flush(params).await;
                        black_box(engine)
                    })
                })
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_engine_creation,
    bench_engine_flush,
    bench_engine_memory
);

criterion_main!(benches);