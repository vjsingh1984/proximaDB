//! Performance benchmarks for unified strategy-driven read architecture
//!
//! These benchmarks measure the performance impact of different strategies
//! and validate that strategy selection provides expected optimizations.

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;

use proximadb::storage::engines::core::read_strategy::ReadAccessStrategy;
use proximadb::storage::persistence::filesystem::FilesystemFactory;

// Import unified readers for benchmarking
use proximadb::storage::engines::impls::swift::{UnifiedSWIFTReader, SwiftReaderConfig};
use proximadb::storage::engines::impls::nova::UnifiedNOVAReader;
use proximadb::storage::engines::impls::helix::UnifiedHELIXReader;

/// Benchmark setup helper
struct BenchmarkSetup {
    filesystem_factory: Arc<FilesystemFactory>,
    collection_id: String,
    runtime: Runtime,
}

impl BenchmarkSetup {
    fn new() -> Self {
        Self {
            filesystem_factory: Arc::new(FilesystemFactory::default()),
            collection_id: "benchmark_collection".to_string(),
            runtime: Runtime::new().unwrap(),
        }
    }
}

/// Benchmark strategy creation overhead
fn bench_strategy_creation(c: &mut Criterion) {
    let setup = BenchmarkSetup::new();

    let mut group = c.benchmark_group("strategy_creation");

    // Benchmark DirectStream strategy creation
    group.bench_function("direct_stream", |b| {
        b.iter(|| {
            black_box(ReadAccessStrategy::DirectStream)
        })
    });

    // Benchmark CachedSearch strategy creation
    group.bench_function("cached_search", |b| {
        b.iter(|| {
            black_box(ReadAccessStrategy::CachedSearch {
                prefetch_metadata: true
            })
        })
    });

    // Benchmark CachedSelective strategy creation
    group.bench_function("cached_selective", |b| {
        b.iter(|| {
            black_box(ReadAccessStrategy::CachedSelective {
                filter: None
            })
        })
    });

    // Benchmark Adaptive strategy creation
    group.bench_function("adaptive", |b| {
        b.iter(|| {
            black_box(ReadAccessStrategy::Adaptive {
                initial_strategy: Box::new(ReadAccessStrategy::DirectStream),
                fallback_threshold: 5,
            })
        })
    });

    group.finish();
}

/// Benchmark reader creation with different strategies
fn bench_reader_creation(c: &mut Criterion) {
    let setup = BenchmarkSetup::new();

    let mut group = c.benchmark_group("reader_creation");

    let strategies = vec![
        ("direct_stream", ReadAccessStrategy::DirectStream),
        ("cached_search", ReadAccessStrategy::CachedSearch { prefetch_metadata: true }),
        ("cached_selective", ReadAccessStrategy::CachedSelective { filter: None }),
    ];

    // Benchmark SWIFT reader creation
    for (name, strategy) in &strategies {
        group.bench_with_input(
            BenchmarkId::new("swift", name),
            strategy,
            |b, strategy| {
                let config = SwiftReaderConfig::default();
                b.iter(|| {
                    setup.runtime.block_on(async {
                        let reader = UnifiedSWIFTReader::new(
                            setup.filesystem_factory.clone(),
                            setup.collection_id.clone(),
                            strategy.clone(),
                            config.clone(),
                        ).unwrap();
                        black_box(reader)
                    })
                })
            },
        );
    }

    // Benchmark NOVA reader creation
    for (name, strategy) in &strategies {
        group.bench_with_input(
            BenchmarkId::new("nova", name),
            strategy,
            |b, strategy| {
                b.iter(|| {
                    setup.runtime.block_on(async {
                        let reader = UnifiedNOVAReader::new(
                            setup.filesystem_factory.clone(),
                            setup.collection_id.clone(),
                            strategy.clone(),
                        ).unwrap();
                        black_box(reader)
                    })
                })
            },
        );
    }

    // Benchmark HELIX reader creation
    for (name, strategy) in &strategies {
        group.bench_with_input(
            BenchmarkId::new("helix", name),
            strategy,
            |b, strategy| {
                b.iter(|| {
                    setup.runtime.block_on(async {
                        let reader = UnifiedHELIXReader::new(
                            setup.filesystem_factory.clone(),
                            setup.collection_id.clone(),
                            strategy.clone(),
                        ).unwrap();
                        black_box(reader)
                    })
                })
            },
        );
    }

    group.finish();
}

/// Benchmark factory method performance
fn bench_factory_methods(c: &mut Criterion) {
    let setup = BenchmarkSetup::new();

    let mut group = c.benchmark_group("factory_methods");

    // Benchmark for_compaction factory methods
    group.bench_function("swift_for_compaction", |b| {
        b.iter(|| {
            setup.runtime.block_on(async {
                let reader = UnifiedSWIFTReader::for_compaction(
                    setup.filesystem_factory.clone(),
                    setup.collection_id.clone(),
                ).unwrap();
                black_box(reader)
            })
        })
    });

    group.bench_function("nova_for_compaction", |b| {
        b.iter(|| {
            setup.runtime.block_on(async {
                let reader = UnifiedNOVAReader::for_compaction(
                    setup.filesystem_factory.clone(),
                    setup.collection_id.clone(),
                ).unwrap();
                black_box(reader)
            })
        })
    });

    group.bench_function("helix_for_compaction", |b| {
        b.iter(|| {
            setup.runtime.block_on(async {
                let reader = UnifiedHELIXReader::for_compaction(
                    setup.filesystem_factory.clone(),
                    setup.collection_id.clone(),
                ).unwrap();
                black_box(reader)
            })
        })
    });

    // Benchmark for_search factory methods
    group.bench_function("swift_for_search", |b| {
        b.iter(|| {
            setup.runtime.block_on(async {
                let reader = UnifiedSWIFTReader::for_search(
                    setup.filesystem_factory.clone(),
                    setup.collection_id.clone(),
                ).unwrap();
                black_box(reader)
            })
        })
    });

    group.bench_function("nova_for_search", |b| {
        b.iter(|| {
            setup.runtime.block_on(async {
                let reader = UnifiedNOVAReader::for_search(
                    setup.filesystem_factory.clone(),
                    setup.collection_id.clone(),
                ).unwrap();
                black_box(reader)
            })
        })
    });

    group.bench_function("helix_for_search", |b| {
        b.iter(|| {
            setup.runtime.block_on(async {
                let reader = UnifiedHELIXReader::for_search(
                    setup.filesystem_factory.clone(),
                    setup.collection_id.clone(),
                ).unwrap();
                black_box(reader)
            })
        })
    });

    group.finish();
}

/// Benchmark strategy switching performance
fn bench_strategy_switching(c: &mut Criterion) {
    let setup = BenchmarkSetup::new();

    let mut group = c.benchmark_group("strategy_switching");

    // Create readers for switching tests
    let swift_reader = setup.runtime.block_on(async {
        UnifiedSWIFTReader::for_search(
            setup.filesystem_factory.clone(),
            setup.collection_id.clone(),
        ).unwrap()
    });

    let nova_reader = setup.runtime.block_on(async {
        UnifiedNOVAReader::for_search(
            setup.filesystem_factory.clone(),
            setup.collection_id.clone(),
        ).unwrap()
    });

    let helix_reader = setup.runtime.block_on(async {
        UnifiedHELIXReader::for_search(
            setup.filesystem_factory.clone(),
            setup.collection_id.clone(),
        ).unwrap()
    });

    // Benchmark strategy switching overhead
    group.bench_function("swift_switch_to_direct", |b| {
        let mut reader = swift_reader.clone();
        b.iter(|| {
            reader.set_strategy(ReadAccessStrategy::DirectStream);
            black_box(&reader);
            // Switch back for next iteration
            reader.set_strategy(ReadAccessStrategy::CachedSearch { prefetch_metadata: true });
        })
    });

    group.bench_function("nova_switch_to_direct", |b| {
        let mut reader = nova_reader.clone();
        b.iter(|| {
            reader.set_strategy(ReadAccessStrategy::DirectStream);
            black_box(&reader);
            // Switch back for next iteration
            reader.set_strategy(ReadAccessStrategy::CachedSearch { prefetch_metadata: true });
        })
    });

    group.bench_function("helix_switch_to_direct", |b| {
        let mut reader = helix_reader.clone();
        b.iter(|| {
            reader.set_strategy(ReadAccessStrategy::DirectStream);
            black_box(&reader);
            // Switch back for next iteration
            reader.set_strategy(ReadAccessStrategy::CachedSearch { prefetch_metadata: true });
        })
    });

    group.finish();
}

/// Benchmark cache usage determination
fn bench_cache_usage_check(c: &mut Criterion) {
    let setup = BenchmarkSetup::new();

    let mut group = c.benchmark_group("cache_usage_check");

    // Create readers with different strategies
    let direct_reader = setup.runtime.block_on(async {
        UnifiedSWIFTReader::for_compaction(
            setup.filesystem_factory.clone(),
            setup.collection_id.clone(),
        ).unwrap()
    });

    let cached_reader = setup.runtime.block_on(async {
        UnifiedSWIFTReader::for_search(
            setup.filesystem_factory.clone(),
            setup.collection_id.clone(),
        ).unwrap()
    });

    // Benchmark cache usage checks
    group.bench_function("direct_cache_check", |b| {
        b.iter(|| {
            black_box(direct_reader.is_using_cache())
        })
    });

    group.bench_function("cached_cache_check", |b| {
        b.iter(|| {
            black_box(cached_reader.is_using_cache())
        })
    });

    group.finish();
}

/// Benchmark strategy comparison operations
fn bench_strategy_comparison(c: &mut Criterion) {
    let mut group = c.benchmark_group("strategy_comparison");

    let strategies = vec![
        ReadAccessStrategy::DirectStream,
        ReadAccessStrategy::CachedSearch { prefetch_metadata: true },
        ReadAccessStrategy::CachedSelective { filter: None },
        ReadAccessStrategy::CachedMetadataOnly,
    ];

    // Benchmark strategy equality checks
    group.bench_function("strategy_equality", |b| {
        b.iter(|| {
            for i in 0..strategies.len() {
                for j in 0..strategies.len() {
                    black_box(strategies[i] == strategies[j]);
                }
            }
        })
    });

    // Benchmark strategy cloning
    group.bench_function("strategy_clone", |b| {
        b.iter(|| {
            for strategy in &strategies {
                black_box(strategy.clone());
            }
        })
    });

    group.finish();
}

/// Benchmark memory usage of different strategy configurations
fn bench_memory_usage(c: &mut Criterion) {
    let setup = BenchmarkSetup::new();

    let mut group = c.benchmark_group("memory_usage");
    group.measurement_time(Duration::from_secs(10));

    // Benchmark memory allocation patterns
    group.bench_function("create_many_direct_readers", |b| {
        b.iter(|| {
            setup.runtime.block_on(async {
                let mut readers = Vec::new();
                for i in 0..100 {
                    let reader = UnifiedNOVAReader::new(
                        setup.filesystem_factory.clone(),
                        format!("collection_{}", i),
                        ReadAccessStrategy::DirectStream,
                    ).unwrap();
                    readers.push(reader);
                }
                black_box(readers)
            })
        })
    });

    group.bench_function("create_many_cached_readers", |b| {
        b.iter(|| {
            setup.runtime.block_on(async {
                let mut readers = Vec::new();
                for i in 0..100 {
                    let reader = UnifiedNOVAReader::new(
                        setup.filesystem_factory.clone(),
                        format!("collection_{}", i),
                        ReadAccessStrategy::CachedSearch { prefetch_metadata: true },
                    ).unwrap();
                    readers.push(reader);
                }
                black_box(readers)
            })
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_strategy_creation,
    bench_reader_creation,
    bench_factory_methods,
    bench_strategy_switching,
    bench_cache_usage_check,
    bench_strategy_comparison,
    bench_memory_usage
);

criterion_main!(benches);