//! Query Cache Performance Benchmarks
//!
//! Measures query result cache performance including:
//! - Cache hit overhead (target: <1ms)
//! - Cache write overhead
//! - Invalidation cost at various cache sizes
//! - Concurrent read/write performance
//! - Cached vs uncached query execution comparison

mod common;

use arrow::array::{ArrayRef, Float32Array, Int64Array, RecordBatch, StringArray};
use arrow::datatypes::{DataType, Field, Schema};
use common::benchmark_utils::print_system_info;
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use proximadb::query::cache::{
    CacheInvalidator, ChangeOperation, InvalidationConfig, InvalidationEvent, QueryKey,
    QueryResultCache, QueryResultCacheConfig,
};
use proximadb::query::federated::ExecutionResult;
use std::sync::Arc;
use std::time::Duration;

/// Cache sizes to benchmark
const CACHE_SIZES: &[usize] = &[100, 1000, 10000];

/// Number of concurrent threads for parallel benchmarks
const THREAD_COUNTS: &[usize] = &[2, 4, 8];

/// Create a test ExecutionResult with configurable size
fn create_test_result(row_count: usize) -> ExecutionResult {
    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("name", DataType::Utf8, true),
        Field::new("score", DataType::Float32, false),
        Field::new("timestamp", DataType::Int64, true),
    ]));

    let ids: Vec<String> = (0..row_count).map(|i| format!("id_{}", i)).collect();
    let names: Vec<String> = (0..row_count).map(|i| format!("name_{}", i)).collect();
    let scores: Vec<f32> = (0..row_count).map(|i| (i as f32) * 0.1).collect();
    let timestamps: Vec<i64> = (0..row_count).map(|i| i as i64 * 1000).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(ids)) as ArrayRef,
            Arc::new(StringArray::from(names)) as ArrayRef,
            Arc::new(Float32Array::from(scores)) as ArrayRef,
            Arc::new(Int64Array::from(timestamps)) as ArrayRef,
        ],
    )
    .expect("Failed to create test batch");

    ExecutionResult::from_batch(batch)
}

/// Pre-populate a cache with entries (creates a fresh result for each entry)
fn populate_cache(cache: &QueryResultCache, count: usize, collection: &str) {
    for i in 0..count {
        let key = QueryKey::from_sql(&format!("SELECT * FROM {} WHERE id = {}", collection, i));
        let result = create_test_result(10);
        let _ = cache.insert(key, result, vec![collection.to_string()]);
    }
}

/// Benchmark cache hit latency
fn bench_cache_hit_latency(c: &mut Criterion) {
    print_system_info("Query Cache Hit Latency");

    let mut group = c.benchmark_group("cache_hit_latency");
    group.measurement_time(Duration::from_secs(10));

    // Test different result sizes
    let result_sizes = [10, 100, 1000];

    for size in result_sizes {
        let cache = QueryResultCache::with_defaults();
        let result = create_test_result(size);
        let key = QueryKey::from_sql("SELECT * FROM products WHERE id = 1");

        // Insert the result first
        cache
            .insert(key.clone(), result, vec!["products".to_string()])
            .expect("Failed to insert");

        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::new("result_rows", size), &size, |b, _| {
            b.iter(|| {
                let cached = cache.get(&key);
                black_box(cached)
            })
        });
    }

    group.finish();
}

/// Benchmark cache miss + store latency
fn bench_cache_write_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_write_overhead");
    group.measurement_time(Duration::from_secs(10));

    let result_sizes = [10, 100, 1000];

    for size in result_sizes {
        group.throughput(Throughput::Elements(1));
        group.bench_with_input(BenchmarkId::new("result_rows", size), &size, |b, &sz| {
            let cache = QueryResultCache::with_defaults();
            let mut counter = 0u64;
            b.iter(|| {
                counter += 1;
                let key =
                    QueryKey::from_sql(&format!("SELECT * FROM products WHERE id = {}", counter));
                let result = create_test_result(sz);
                let r = cache.insert(key, result, vec!["products".to_string()]);
                black_box(r)
            })
        });
    }

    group.finish();
}

/// Benchmark cache invalidation with varying cache sizes
fn bench_invalidation_cost(c: &mut Criterion) {
    let mut group = c.benchmark_group("invalidation_cost");
    group.measurement_time(Duration::from_secs(10));

    for &cache_size in CACHE_SIZES {
        // Benchmark invalidating all entries for a collection
        group.bench_with_input(
            BenchmarkId::new("invalidate_all", cache_size),
            &cache_size,
            |b, &size| {
                b.iter_with_setup(
                    || {
                        // Setup: create and populate cache
                        let config = QueryResultCacheConfig {
                            max_entries: size * 2,
                            ..Default::default()
                        };
                        let cache = QueryResultCache::new(config);
                        populate_cache(&cache, size, "products");
                        cache
                    },
                    |cache| {
                        // Benchmark: invalidate all entries
                        let count = cache.invalidate_collection("products");
                        black_box(count)
                    },
                )
            },
        );

        // Benchmark invalidating a fraction of entries (10%)
        group.bench_with_input(
            BenchmarkId::new("invalidate_partial", cache_size),
            &cache_size,
            |b, &size| {
                b.iter_with_setup(
                    || {
                        let config = QueryResultCacheConfig {
                            max_entries: size * 2,
                            ..Default::default()
                        };
                        let cache = QueryResultCache::new(config);
                        // Populate with entries from different collections
                        for i in 0..size {
                            let collection = if i % 10 == 0 { "target" } else { "other" };
                            let key = QueryKey::from_sql(&format!(
                                "SELECT * FROM {} WHERE id = {}",
                                collection, i
                            ));
                            let result = create_test_result(10);
                            let _ = cache.insert(key, result, vec![collection.to_string()]);
                        }
                        cache
                    },
                    |cache| {
                        let count = cache.invalidate_collection("target");
                        black_box(count)
                    },
                )
            },
        );
    }

    group.finish();
}

/// Benchmark invalidator with event processing
fn bench_invalidator_events(c: &mut Criterion) {
    let mut group = c.benchmark_group("invalidator_events");
    group.measurement_time(Duration::from_secs(10));

    // Direct invalidation (no batching)
    group.bench_function("direct_invalidation", |b| {
        b.iter_with_setup(
            || {
                let cache = Arc::new(QueryResultCache::with_defaults());
                populate_cache(&cache, 1000, "products");
                let config = InvalidationConfig {
                    batch_invalidations: false,
                    ..Default::default()
                };
                CacheInvalidator::with_config(cache, config)
            },
            |invalidator| {
                let event = InvalidationEvent::new("products", ChangeOperation::Update);
                let count = invalidator.on_change_event(event);
                black_box(count)
            },
        )
    });

    // Batched invalidation
    group.bench_function("batched_invalidation", |b| {
        b.iter_with_setup(
            || {
                let cache = Arc::new(QueryResultCache::with_defaults());
                populate_cache(&cache, 1000, "products");
                let config = InvalidationConfig {
                    batch_invalidations: true,
                    max_batch_size: 100,
                    ..Default::default()
                };
                CacheInvalidator::with_config(cache, config)
            },
            |invalidator| {
                // Queue up events
                for i in 0..10 {
                    let event = InvalidationEvent::new(
                        format!("collection_{}", i % 5),
                        ChangeOperation::Insert,
                    );
                    invalidator.on_change_event(event);
                }
                // Flush batch
                let count = invalidator.flush_batch();
                black_box(count)
            },
        )
    });

    group.finish();
}

/// Benchmark concurrent cache reads
fn bench_concurrent_reads(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_reads");
    group.measurement_time(Duration::from_secs(15));

    for &thread_count in THREAD_COUNTS {
        let cache = Arc::new(QueryResultCache::with_defaults());

        // Pre-populate with entries
        for i in 0..1000 {
            let key = QueryKey::from_sql(&format!("SELECT * FROM products WHERE id = {}", i));
            let result = create_test_result(100);
            let _ = cache.insert(key, result, vec!["products".to_string()]);
        }

        group.throughput(Throughput::Elements(thread_count as u64 * 100));
        group.bench_with_input(
            BenchmarkId::new("threads", thread_count),
            &thread_count,
            |b, &threads| {
                b.iter(|| {
                    let handles: Vec<_> = (0..threads)
                        .map(|t| {
                            let cache_ref = Arc::clone(&cache);
                            std::thread::spawn(move || {
                                for i in 0..100 {
                                    let idx = (t * 100 + i) % 1000;
                                    let key = QueryKey::from_sql(&format!(
                                        "SELECT * FROM products WHERE id = {}",
                                        idx
                                    ));
                                    let _ = cache_ref.get(&key);
                                }
                            })
                        })
                        .collect();

                    for handle in handles {
                        handle.join().expect("Thread panicked");
                    }
                })
            },
        );
    }

    group.finish();
}

/// Benchmark concurrent reads and writes
fn bench_concurrent_read_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_read_write");
    group.measurement_time(Duration::from_secs(15));

    for &thread_count in THREAD_COUNTS {
        let cache = Arc::new(QueryResultCache::new(QueryResultCacheConfig {
            max_entries: 50000,
            ..Default::default()
        }));

        // Pre-populate
        for i in 0..1000 {
            let key = QueryKey::from_sql(&format!("SELECT * FROM init WHERE id = {}", i));
            let result = create_test_result(50);
            let _ = cache.insert(key, result, vec!["init".to_string()]);
        }

        group.throughput(Throughput::Elements(thread_count as u64 * 100));
        group.bench_with_input(
            BenchmarkId::new("threads", thread_count),
            &thread_count,
            |b, &threads| {
                b.iter(|| {
                    let handles: Vec<_> = (0..threads)
                        .map(|t| {
                            let cache_ref = Arc::clone(&cache);
                            std::thread::spawn(move || {
                                for i in 0..100 {
                                    let idx = t * 100 + i;
                                    if i % 5 == 0 {
                                        // 20% writes
                                        let key = QueryKey::from_sql(&format!(
                                            "SELECT * FROM bench WHERE id = {}",
                                            idx
                                        ));
                                        let result = create_test_result(50);
                                        let _ = cache_ref.insert(
                                            key,
                                            result,
                                            vec!["bench".to_string()],
                                        );
                                    } else {
                                        // 80% reads
                                        let key = QueryKey::from_sql(&format!(
                                            "SELECT * FROM init WHERE id = {}",
                                            idx % 1000
                                        ));
                                        let _ = cache_ref.get(&key);
                                    }
                                }
                            })
                        })
                        .collect();

                    for handle in handles {
                        handle.join().expect("Thread panicked");
                    }
                })
            },
        );
    }

    group.finish();
}

/// Benchmark QueryKey fingerprint computation
fn bench_query_key_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_key_creation");
    group.measurement_time(Duration::from_secs(5));

    // Simple query
    group.bench_function("simple_query", |b| {
        b.iter(|| {
            let key = QueryKey::from_sql("SELECT * FROM products WHERE id = 1");
            black_box(key)
        })
    });

    // Complex query with multiple tables
    group.bench_function("complex_query", |b| {
        let sql = "SELECT p.name, c.category_name, v.embedding \
                   FROM products p \
                   JOIN categories c ON p.category_id = c.id \
                   JOIN LATERAL VECTOR_SEARCH('embeddings', p.embedding, 10) v ON true \
                   WHERE p.price > 100 AND c.active = true \
                   ORDER BY v.score DESC LIMIT 50";
        b.iter(|| {
            let key = QueryKey::from_sql(sql);
            black_box(key)
        })
    });

    // Query with parameters
    group.bench_function("parameterized_query", |b| {
        let sql = "SELECT * FROM products WHERE category = $1 AND price < $2";
        let params = ["electronics", "500"];
        b.iter(|| {
            let key = QueryKey::from_sql_with_params(sql, &params);
            black_box(key)
        })
    });

    group.finish();
}

/// Benchmark cache lookup vs contains check
fn bench_cache_lookup_patterns(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_lookup_patterns");
    group.measurement_time(Duration::from_secs(10));

    let cache = QueryResultCache::with_defaults();

    // Populate cache
    for i in 0..1000 {
        let key = QueryKey::from_sql(&format!("SELECT * FROM products WHERE id = {}", i));
        let result = create_test_result(100);
        let _ = cache.insert(key, result, vec!["products".to_string()]);
    }

    let hit_key = QueryKey::from_sql("SELECT * FROM products WHERE id = 500");
    let miss_key = QueryKey::from_sql("SELECT * FROM products WHERE id = 99999");

    // Contains check - hit
    group.bench_function("contains_hit", |b| {
        b.iter(|| black_box(cache.contains(&hit_key)))
    });

    // Contains check - miss
    group.bench_function("contains_miss", |b| {
        b.iter(|| black_box(cache.contains(&miss_key)))
    });

    // Full get - hit
    group.bench_function("get_hit", |b| b.iter(|| black_box(cache.get(&hit_key))));

    // Full get - miss
    group.bench_function("get_miss", |b| b.iter(|| black_box(cache.get(&miss_key))));

    group.finish();
}

/// Benchmark cleanup operations
fn bench_cache_cleanup(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_cleanup");
    group.measurement_time(Duration::from_secs(10));

    for &cache_size in CACHE_SIZES {
        // Cleanup expired entries
        group.bench_with_input(
            BenchmarkId::new("cleanup_expired", cache_size),
            &cache_size,
            |b, &size| {
                b.iter_with_setup(
                    || {
                        let config = QueryResultCacheConfig {
                            max_entries: size * 2,
                            default_ttl: Duration::from_millis(1), // Very short TTL
                            ..Default::default()
                        };
                        let cache = QueryResultCache::new(config);
                        populate_cache(&cache, size, "products");
                        // Wait for entries to expire
                        std::thread::sleep(Duration::from_millis(5));
                        cache
                    },
                    |cache| {
                        let count = cache.cleanup_expired();
                        black_box(count)
                    },
                )
            },
        );

        // Clear all entries
        group.bench_with_input(
            BenchmarkId::new("clear_all", cache_size),
            &cache_size,
            |b, &size| {
                b.iter_with_setup(
                    || {
                        let config = QueryResultCacheConfig {
                            max_entries: size * 2,
                            ..Default::default()
                        };
                        let cache = QueryResultCache::new(config);
                        populate_cache(&cache, size, "products");
                        cache
                    },
                    |cache| {
                        cache.clear();
                        black_box(())
                    },
                )
            },
        );
    }

    group.finish();
}

/// Benchmark cache statistics collection
fn bench_cache_stats(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_stats");
    group.measurement_time(Duration::from_secs(5));

    let cache = QueryResultCache::with_defaults();
    populate_cache(&cache, 5000, "products");

    // Simulate some operations for realistic stats
    for i in 0..100 {
        let key = QueryKey::from_sql(&format!("SELECT * FROM products WHERE id = {}", i));
        let _ = cache.get(&key);
    }
    for i in 0..50 {
        let key = QueryKey::from_sql(&format!("SELECT * FROM missing WHERE id = {}", i));
        let _ = cache.get(&key);
    }

    group.bench_function("stats_collection", |b| {
        b.iter(|| {
            let stats = cache.stats();
            black_box(stats)
        })
    });

    group.finish();
}

// Configure benchmark groups
criterion_group! {
    name = cache_latency_benches;
    config = Criterion::default()
        .sample_size(100)
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(2));
    targets = bench_cache_hit_latency,
              bench_cache_write_overhead,
              bench_query_key_creation,
              bench_cache_lookup_patterns
}

criterion_group! {
    name = cache_invalidation_benches;
    config = Criterion::default()
        .sample_size(50)
        .measurement_time(Duration::from_secs(10))
        .warm_up_time(Duration::from_secs(1));
    targets = bench_invalidation_cost,
              bench_invalidator_events,
              bench_cache_cleanup
}

criterion_group! {
    name = cache_concurrent_benches;
    config = Criterion::default()
        .sample_size(30)
        .measurement_time(Duration::from_secs(15))
        .warm_up_time(Duration::from_secs(2));
    targets = bench_concurrent_reads,
              bench_concurrent_read_write
}

criterion_group! {
    name = cache_stats_benches;
    config = Criterion::default()
        .sample_size(100)
        .measurement_time(Duration::from_secs(5))
        .warm_up_time(Duration::from_secs(1));
    targets = bench_cache_stats
}

criterion_main!(
    cache_latency_benches,
    cache_invalidation_benches,
    cache_concurrent_benches,
    cache_stats_benches
);
