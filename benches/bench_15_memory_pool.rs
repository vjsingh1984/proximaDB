// Memory Pool Benchmarks - Comprehensive Analysis
// Tests various pool sizes, stats overhead, and workload patterns
//
// Run with specific configurations:
//   cargo bench --bench bench_15_memory_pool
//
// Run specific benchmark group:
//   cargo bench --bench bench_15_memory_pool -- pool_config_matrix

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use proximadb::core::memory::pool::{Pool, PoolConfig, VectorMemoryPool};
use proximadb::compute::distance_computation::{DistanceMetric, engine::UnifiedDistanceCompute};
use std::sync::Arc;
use std::time::Duration;

/// Initialize hardware capabilities
fn init_hardware() {
    // Hardware detection is automatically done by the system
}

/// Pool configuration for benchmarking
#[derive(Debug, Clone, Copy)]
struct PoolTestConfig {
    pool_size: usize,
    enable_stats: bool,
}

impl PoolTestConfig {
    fn to_pool_config(&self) -> PoolConfig {
        PoolConfig {
            initial_size: self.pool_size,
            max_size: self.pool_size * 4,
            min_size: self.pool_size / 4,
            max_idle_duration: Duration::from_secs(300),
            growth_factor: 1.5,
            enable_stats: self.enable_stats,
        }
    }

    fn label(&self) -> String {
        format!("size{}_stats{}", self.pool_size, if self.enable_stats { "on" } else { "off" })
    }
}

// ============================================================================
// BASIC POOL SIZES - Simple baseline benchmark
// ============================================================================

fn benchmark_pool_sizes(c: &mut Criterion) {
    init_hardware();
    let mut group = c.benchmark_group("pool_sizes_basic");
    group.sample_size(50);

    let pool_sizes = vec![16, 64, 256, 1024];
    let dimension = 768;
    let batch_size = 1000;

    // Create test data
    let query: Vec<f32> = (0..dimension).map(|i| (i as f32).sin()).collect();
    let vectors: Vec<Vec<f32>> = (0..batch_size)
        .map(|j| (0..dimension).map(|i| ((i + j) as f32).cos()).collect())
        .collect();
    let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();

    group.throughput(Throughput::Elements(1000));

    for &pool_size in &pool_sizes {
        let config = PoolConfig {
            initial_size: pool_size,
            max_size: pool_size * 4,
            min_size: pool_size / 4,
            max_idle_duration: Duration::from_secs(300),
            growth_factor: 1.5,
            enable_stats: false,
        };

        group.bench_with_input(
            BenchmarkId::from_parameter(pool_size),
            &pool_size,
            |b, _| {
                let memory_pool = Arc::new(VectorMemoryPool::with_config(config.clone()));
                let compute = UnifiedDistanceCompute::default();

                b.iter(|| {
                    let _buf1 = memory_pool.vector_buffers.acquire();
                    let _buf2 = memory_pool.serialization_buffers.acquire();

                    let results = compute.batch_distance_pooled_simd(
                        &query,
                        &vector_refs,
                        &DistanceMetric::Cosine,
                    );
                    black_box(results)
                });
            },
        );
    }

    group.finish();
}

// ============================================================================
// PARAMETERIZED POOL CONFIGURATION MATRIX
// Tests all combinations of pool_size × stats_enabled
// ============================================================================

fn benchmark_pool_config_matrix(c: &mut Criterion) {
    init_hardware();
    let mut group = c.benchmark_group("pool_config_matrix");
    group.sample_size(50);

    // Workload parameters
    let batch_size = 1000;
    let dimension = 768;
    let base_requirement = batch_size * dimension; // Number of f32 elements needed

    // Pool sizes based on workload multipliers: 0.25x, 0.5x, 1x, 2x, 4x
    // Using geometric progression with ratio of 2
    let multipliers = vec![0.25, 0.5, 1.0, 2.0, 4.0];

    // Calculate pool sizes: number of buffers needed
    // Each buffer in vector_buffers pool holds ~1024 f32 elements
    // So we need: (batch_size × dimension × multiplier) / elements_per_buffer
    let elements_per_buffer = 1024_usize; // From VectorMemoryPool::with_config

    let mut pool_sizes = Vec::new();
    for mult in &multipliers {
        let required_elements = (base_requirement as f64 * mult) as usize;
        let num_buffers = (required_elements + elements_per_buffer - 1) / elements_per_buffer;
        pool_sizes.push(num_buffers.max(4)); // Minimum 4 buffers
    }

    println!("\n📊 Pool Size Calculation:");
    println!("  Workload: {} vectors × {} dimensions = {} f32 elements",
        batch_size, dimension, base_requirement);
    println!("  Buffer capacity: {} f32 elements", elements_per_buffer);
    println!("\n  Multiplier → Required elements → Pool size (buffers):");
    for (mult, &pool_size) in multipliers.iter().zip(pool_sizes.iter()) {
        let required = (base_requirement as f64 * mult) as usize;
        println!("    {:.2}x → {} elements → {} buffers", mult, required, pool_size);
    }
    println!();

    let stats_options = vec![false, true]; // Test with stats off and on

    // Create all combinations
    let mut configs = Vec::new();
    for &pool_size in &pool_sizes {
        for &enable_stats in &stats_options {
            configs.push(PoolTestConfig { pool_size, enable_stats });
        }
    }

    // Create 1000 vectors × 768 dimensions for testing
    let query: Vec<f32> = (0..dimension).map(|i| (i as f32).sin()).collect();
    let vectors: Vec<Vec<f32>> = (0..batch_size)
        .map(|j| (0..dimension).map(|i| ((i + j) as f32).cos()).collect())
        .collect();
    let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();

    group.throughput(Throughput::Elements(1000)); // 1000 vectors

    for config in configs {
        group.bench_with_input(
            BenchmarkId::from_parameter(config.label()),
            &config,
            |b, cfg| {
                let memory_pool = Arc::new(VectorMemoryPool::with_config(cfg.to_pool_config()));
                let compute = UnifiedDistanceCompute::default();

                b.iter(|| {
                    // Simulate pool acquisition and usage
                    let _buf1 = memory_pool.vector_buffers.acquire();
                    let _buf2 = memory_pool.serialization_buffers.acquire();

                    // Do actual work
                    let results = compute.batch_distance_pooled_simd(
                        &query,
                        &vector_refs,
                        &DistanceMetric::Cosine,
                    );
                    black_box(results)
                });
            },
        );
    }

    group.finish();
}

// ============================================================================
// STATS OVERHEAD BENCHMARKS - Measure impact of enable_stats
// ============================================================================

fn benchmark_stats_overhead(c: &mut Criterion) {
    init_hardware();
    let mut group = c.benchmark_group("stats_overhead");
    group.sample_size(100);

    // Test with and without stats at different pool sizes
    let pool_sizes = vec![16, 64, 256];

    for &pool_size in &pool_sizes {
        // Without stats
        let config_no_stats = PoolConfig {
            initial_size: pool_size,
            max_size: pool_size * 4,
            min_size: pool_size / 4,
            max_idle_duration: Duration::from_secs(300),
            growth_factor: 1.5,
            enable_stats: false,
        };

        // With stats
        let config_with_stats = PoolConfig {
            enable_stats: true,
            ..config_no_stats.clone()
        };

        group.throughput(Throughput::Elements(1000));

        // Benchmark WITHOUT stats
        group.bench_with_input(
            BenchmarkId::new("no_stats", pool_size),
            &pool_size,
            |b, _| {
                let pool: Pool<Vec<u8>> = Pool::new(config_no_stats.clone(), || {
                    Vec::with_capacity(64 * 1024)
                });

                b.iter(|| {
                    // Simulate 1000 acquisitions
                    for _ in 0..1000 {
                        let _item = pool.acquire();
                        black_box(_item);
                    }
                });
            },
        );

        // Benchmark WITH stats
        group.bench_with_input(
            BenchmarkId::new("with_stats", pool_size),
            &pool_size,
            |b, _| {
                let pool: Pool<Vec<u8>> = Pool::new(config_with_stats.clone(), || {
                    Vec::with_capacity(64 * 1024)
                });

                b.iter(|| {
                    // Simulate 1000 acquisitions
                    for _ in 0..1000 {
                        let _item = pool.acquire();
                        black_box(_item);
                    }
                });
            },
        );
    }

    group.finish();
}

// ============================================================================
// CONCURRENT ACCESS BENCHMARKS - Test multi-threaded performance
// ============================================================================

fn benchmark_concurrent_access(c: &mut Criterion) {
    init_hardware();
    let mut group = c.benchmark_group("concurrent_access");
    group.sample_size(30); // Fewer samples for multi-threaded tests

    let pool_sizes = vec![16, 64, 256];
    let thread_counts = vec![1, 4, 8, 16];

    for &pool_size in &pool_sizes {
        for &num_threads in &thread_counts {
            let config = PoolConfig {
                initial_size: pool_size,
                max_size: pool_size * 4,
                min_size: pool_size / 4,
                max_idle_duration: Duration::from_secs(300),
                growth_factor: 1.5,
                enable_stats: false,
            };

            let bench_id = format!("pool{}_threads{}", pool_size, num_threads);
            group.throughput(Throughput::Elements((num_threads * 100) as u64));

            group.bench_with_input(
                BenchmarkId::new("concurrent", bench_id),
                &(pool_size, num_threads),
                |b, _| {
                    let pool = Arc::new(Pool::new(config.clone(), || {
                        Vec::<u8>::with_capacity(64 * 1024)
                    }));

                    b.iter(|| {
                        let handles: Vec<_> = (0..num_threads)
                            .map(|_| {
                                let pool_clone = Arc::clone(&pool);
                                std::thread::spawn(move || {
                                    // Each thread does 100 acquisitions
                                    for _ in 0..100 {
                                        let _item = pool_clone.acquire();
                                        black_box(_item);
                                        // Simulate some work
                                        std::thread::sleep(Duration::from_nanos(100));
                                    }
                                })
                            })
                            .collect();

                        for handle in handles {
                            handle.join().unwrap();
                        }
                    });
                },
            );
        }
    }

    group.finish();
}

// ============================================================================
// HIT RATE ANALYSIS - Measure cache hit rates at different sizes
// ============================================================================

fn benchmark_hit_rates(c: &mut Criterion) {
    init_hardware();
    let mut group = c.benchmark_group("hit_rate_analysis");
    group.sample_size(50);

    let pool_sizes = vec![4, 8, 16, 32, 64, 128, 256];

    for &pool_size in &pool_sizes {
        let config = PoolConfig {
            initial_size: pool_size,
            max_size: pool_size, // Don't allow growth to measure true hit rate
            min_size: pool_size,
            max_idle_duration: Duration::from_secs(300),
            growth_factor: 1.0,
            enable_stats: true, // Enable stats to measure hit rate
        };

        group.bench_with_input(
            BenchmarkId::new("hit_rate", pool_size),
            &pool_size,
            |b, _| {
                let pool = Pool::new(config.clone(), || Vec::<u8>::with_capacity(1024));

                b.iter(|| {
                    // Simulate realistic access pattern: burst of acquisitions
                    let mut items = Vec::new();

                    // Burst: acquire more than pool size
                    for _ in 0..pool_size * 2 {
                        items.push(pool.acquire());
                    }

                    // Release half
                    items.truncate(pool_size);

                    // Acquire again (should hit cache)
                    for _ in 0..pool_size {
                        items.push(pool.acquire());
                    }

                    black_box(items);
                });

                // Print hit rate after benchmark
                let stats = pool.stats();
                if pool_size == 16 || pool_size == 64 || pool_size == 256 {
                    println!("\n  Pool size {}: Hit rate {:.1}%, Current size: {}",
                        pool_size, stats.hit_rate() * 100.0, stats.current_size);
                }
            },
        );
    }

    group.finish();
}

// ============================================================================
// BUFFER TYPE COMPARISON - Compare different buffer pools
// ============================================================================

fn benchmark_buffer_types(c: &mut Criterion) {
    init_hardware();
    let mut group = c.benchmark_group("buffer_type_comparison");
    group.sample_size(100);

    let config = PoolConfig {
        initial_size: 64,
        max_size: 256,
        min_size: 16,
        max_idle_duration: Duration::from_secs(300),
        growth_factor: 1.5,
        enable_stats: false,
    };

    let memory_pool = VectorMemoryPool::with_config(config);

    // Benchmark vector buffer (1K f32 = 4KB)
    group.throughput(Throughput::Elements(1000));
    group.bench_function("vector_buffer", |b| {
        b.iter(|| {
            for _ in 0..1000 {
                let mut buf = memory_pool.vector_buffers.acquire();
                buf.extend_from_slice(&[1.0f32; 768]);
                buf.clear();
                black_box(buf);
            }
        });
    });

    // Benchmark serialization buffer (64KB)
    group.throughput(Throughput::Elements(1000));
    group.bench_function("serialization_buffer", |b| {
        b.iter(|| {
            for _ in 0..1000 {
                let mut buf = memory_pool.serialization_buffers.acquire();
                buf.extend_from_slice(&[42u8; 3072]); // 768 × 4 bytes
                buf.clear();
                black_box(buf);
            }
        });
    });

    // Benchmark compression buffer (32KB)
    group.throughput(Throughput::Elements(1000));
    group.bench_function("compression_buffer", |b| {
        b.iter(|| {
            for _ in 0..1000 {
                let mut buf = memory_pool.compression_buffers.acquire();
                buf.extend_from_slice(&[99u8; 1024]);
                buf.clear();
                black_box(buf);
            }
        });
    });

    // Benchmark metadata buffer (4KB)
    group.throughput(Throughput::Elements(1000));
    group.bench_function("metadata_buffer", |b| {
        b.iter(|| {
            for _ in 0..1000 {
                let mut buf = memory_pool.metadata_buffers.acquire();
                buf.extend_from_slice(&[1u8; 256]);
                buf.clear();
                black_box(buf);
            }
        });
    });

    group.finish();
}

// ============================================================================
// GROWTH STRATEGY COMPARISON - Test different growth factors
// ============================================================================

fn benchmark_growth_strategies(c: &mut Criterion) {
    init_hardware();
    let mut group = c.benchmark_group("growth_strategy");
    group.sample_size(50);

    let growth_factors = vec![1.25, 1.5, 2.0, 3.0];

    for &growth_factor in &growth_factors {
        let config = PoolConfig {
            initial_size: 8, // Start small to force growth
            max_size: 256,
            min_size: 4,
            max_idle_duration: Duration::from_secs(300),
            growth_factor,
            enable_stats: true, // Track growth events
        };

        group.bench_with_input(
            BenchmarkId::new("growth_factor", (growth_factor * 100.0) as u32),
            &growth_factor,
            |b, _| {
                let pool = Pool::new(config.clone(), || Vec::<u8>::with_capacity(1024));

                b.iter(|| {
                    let mut items = Vec::new();

                    // Force pool to grow by acquiring many items
                    for _ in 0..200 {
                        items.push(pool.acquire());
                    }

                    black_box(items);
                });

                // Print growth stats
                let stats = pool.stats();
                println!("\n  Growth factor {:.2}: {} grows, peak size {}",
                    growth_factor, stats.pool_grows, stats.peak_size);
            },
        );
    }

    group.finish();
}

// ============================================================================
// MEMORY FOOTPRINT - Measure actual memory usage
// ============================================================================

fn benchmark_memory_footprint(c: &mut Criterion) {
    init_hardware();
    let mut group = c.benchmark_group("memory_footprint");
    group.sample_size(20);

    let pool_sizes = vec![16, 64, 256, 1024];

    println!("\n📊 Memory Footprint Analysis:");
    println!("  (Pool size × Buffer capacity = Total memory)");

    for &pool_size in &pool_sizes {
        let config = PoolConfig {
            initial_size: pool_size,
            max_size: pool_size,
            min_size: pool_size,
            max_idle_duration: Duration::from_secs(300),
            growth_factor: 1.0,
            enable_stats: false,
        };

        // Calculate theoretical memory
        let buffer_size = 64 * 1024; // 64KB per buffer
        let total_memory = pool_size * buffer_size;

        println!("  Pool size {:4}: {} buffers × 64KB = {:.2} MB",
            pool_size, pool_size, total_memory as f64 / (1024.0 * 1024.0));

        group.bench_with_input(
            BenchmarkId::new("footprint", pool_size),
            &pool_size,
            |b, _| {
                let pool = Pool::new(config.clone(), || {
                    Vec::<u8>::with_capacity(64 * 1024)
                });

                b.iter(|| {
                    // Keep all buffers in memory to measure true footprint
                    let items: Vec<_> = (0..pool_size).map(|_| pool.acquire()).collect();
                    black_box(items);
                });
            },
        );
    }

    group.finish();
}

// Configure with consistent settings for all benchmarks
criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(50)
        .measurement_time(Duration::from_secs(5))
        .warm_up_time(Duration::from_secs(1));
    targets = benchmark_pool_sizes,
              benchmark_pool_config_matrix,
              benchmark_stats_overhead,
              benchmark_concurrent_access,
              benchmark_hit_rates,
              benchmark_buffer_types,
              benchmark_growth_strategies,
              benchmark_memory_footprint
}

criterion_main!(benches);