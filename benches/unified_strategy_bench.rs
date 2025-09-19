//! Performance benchmarks for unified storage engine strategies
//!
//! These benchmarks measure the performance impact of different storage engines
//! and validate that engine selection provides expected optimizations.

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;

use proximadb::storage::engines::factory::StorageEngineFactory;
use proximadb::storage::traits::{UnifiedStorageEngine, FlushParameters};
use proximadb::core::VectorRecord;

/// Benchmark setup helper
struct BenchmarkSetup {
    collection_id: String,
    runtime: Runtime,
}

impl BenchmarkSetup {
    fn new() -> Self {
        Self {
            collection_id: "benchmark_collection".to_string(),
            runtime: Runtime::new().unwrap(),
        }
    }

    fn generate_vectors(&self, count: usize, dimension: usize) -> Vec<VectorRecord> {
        (0..count)
            .map(|i| VectorRecord {
                id: format!("vec_{}", i),
                vector: vec![i as f32 / count as f32; dimension],
                metadata: Default::default(),
                timestamp: i as i64,
                updated_at: Some(chrono::Utc::now().timestamp()),
                expires_at: None,
                version: Some(1),
            })
            .collect()
    }
}

/// Benchmark engine creation overhead
fn bench_engine_creation(c: &mut Criterion) {
    let _setup = BenchmarkSetup::new();

    let mut group = c.benchmark_group("engine_creation");

    // Benchmark SST engine creation
    group.bench_function("sst_engine", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_sst().unwrap();
            black_box(engine)
        })
    });

    // Benchmark VIPER engine creation
    group.bench_function("viper_engine", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_viper().unwrap();
            black_box(engine)
        })
    });

    // Benchmark NOVA engine creation
    group.bench_function("nova_engine", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_nova().unwrap();
            black_box(engine)
        })
    });

    // Benchmark SWIFT engine creation
    group.bench_function("swift_engine", |b| {
        b.iter(|| {
            let engine = StorageEngineFactory::create_swift().unwrap();
            black_box(engine)
        })
    });

    group.finish();
}

/// Benchmark engine flush operations
fn bench_engine_flush(c: &mut Criterion) {
    let setup = BenchmarkSetup::new();
    let vectors = setup.generate_vectors(100, 768);

    let mut group = c.benchmark_group("engine_flush");

    let engines = vec![
        ("sst", StorageEngineFactory::create_sst().unwrap()),
        ("viper", StorageEngineFactory::create_viper().unwrap()),
        ("nova", StorageEngineFactory::create_nova().unwrap()),
        ("swift", StorageEngineFactory::create_swift().unwrap()),
    ];

    for (name, engine) in engines {
        group.bench_with_input(
            BenchmarkId::new("flush", name),
            &vectors,
            |b, vectors| {
                b.iter(|| {
                    setup.runtime.block_on(async {
                        let params = FlushParameters {
                            collection_id: Some(setup.collection_id.clone()),
                            vector_records: vectors.clone(),
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

/// Benchmark engine memory usage
fn bench_engine_memory(c: &mut Criterion) {
    let setup = BenchmarkSetup::new();

    let mut group = c.benchmark_group("engine_memory");
    group.measurement_time(Duration::from_secs(5));

    // Test different vector counts
    let vector_counts = vec![10, 100, 1000];

    for count in vector_counts {
        let vectors = setup.generate_vectors(count, 768);

        group.bench_with_input(
            BenchmarkId::new("sst", count),
            &vectors,
            |b, vectors| {
                b.iter(|| {
                    setup.runtime.block_on(async {
                        let engine = StorageEngineFactory::create_sst().unwrap();
                        let params = FlushParameters {
                            collection_id: Some(setup.collection_id.clone()),
                            vector_records: vectors.clone(),
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