// Comprehensive Optimization Benchmark
//
// Tests all optimization permutations systematically using the embedded API:
// - 6 storage engines (SST, HELIX, VIPER, SWIFT, NOVA, RAPTOR)
// - 3 search modes (Exact, Approximate, Adaptive)
// - 2 distance metrics (Cosine, Euclidean)
//
// This benchmark uses the embedded database API for simplicity and stability.

mod common;
use common::benchmark_utils::print_system_info;

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use proximadb::embedded::{EmbeddedConfig, EmbeddedProximaDB, StorageLocationConfig};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

// ============================================================================
// CONFIGURATION DIMENSIONS
// ============================================================================

/// Storage engines to benchmark
const ENGINES: &[&str] = &["sst", "helix", "viper", "swift", "nova", "raptor"];

/// Search modes: "exact", "approximate:N", "adaptive:N"
const SEARCH_MODES: &[&str] = &["exact", "approximate:10", "adaptive:5000"];

// ============================================================================
// BENCHMARK CONSTANTS
// ============================================================================

const DIMENSION: usize = 768;
const TOP_K: usize = 10;
const NUM_QUERIES: usize = 50;

// ============================================================================
// BENCHMARK CONFIGURATION
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchmarkConfig {
    engine: String,
    search_mode: String,
    vector_count: usize,
    dimension: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchmarkResult {
    config: BenchmarkConfig,
    insert_qps: f64,
    search_qps: f64,
    latency_p50_us: f64,
    latency_p99_us: f64,
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/// Generate test vectors with controlled clustering
fn generate_test_vectors(count: usize, dimension: usize) -> (Vec<String>, Vec<Vec<f32>>) {
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    let mut rng = StdRng::seed_from_u64(42);
    let num_clusters = 10;

    // Create cluster centers
    let cluster_centers: Vec<Vec<f32>> = (0..num_clusters)
        .map(|_| {
            let mut center: Vec<f32> = (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect();
            let norm: f32 = center.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for val in &mut center {
                    *val /= norm;
                }
            }
            center
        })
        .collect();

    // Generate vectors around cluster centers
    let mut ids = Vec::with_capacity(count);
    let mut vectors = Vec::with_capacity(count);

    for i in 0..count {
        let cluster_idx = i % num_clusters;
        let center = &cluster_centers[cluster_idx];

        let mut vector: Vec<f32> = center
            .iter()
            .map(|&c| c + rng.gen_range(-0.3..0.3))
            .collect();

        // Normalize
        let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for val in &mut vector {
                *val /= norm;
            }
        }

        ids.push(format!("vec_{}", i));
        vectors.push(vector);
    }

    (ids, vectors)
}

/// Generate query vectors
fn generate_query_vectors(count: usize, dimension: usize) -> Vec<Vec<f32>> {
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    let mut rng = StdRng::seed_from_u64(123);

    (0..count)
        .map(|_| {
            let mut vec: Vec<f32> = (0..dimension).map(|_| rng.gen_range(-1.0..1.0)).collect();
            let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for val in &mut vec {
                    *val /= norm;
                }
            }
            vec
        })
        .collect()
}

/// Run a single benchmark configuration
fn run_benchmark(engine: &str, search_mode: &str, vector_count: usize) -> BenchmarkResult {
    use uuid::Uuid;

    let collection_name = format!("bench_{}", Uuid::new_v4().to_string()[..8].to_string());
    let data_path = format!("/tmp/proximadb-comprehensive-bench/{}", collection_name);

    // Ensure parent directory exists
    let _ = std::fs::create_dir_all("/tmp/proximadb-comprehensive-bench");

    // Create embedded database
    let config = EmbeddedConfig {
        storage_locations: vec![StorageLocationConfig {
            path: data_path.clone(),
            weight: 1,
            tags: vec!["benchmark".to_string()],
        }],
        metadata_path: format!("{}/metadata", data_path),
        cache_size_mb: 256,
        default_engine: engine.to_string(),
        enable_wal: false, // Disable WAL for benchmark speed
        wal_sync_mode: "batch".to_string(),
        block_prune_mode: "sqrt".to_string(),
        block_prune_ratio: 0.2,
        block_prune_min_keep: 1,
        block_prune_max_keep: 0,
        enable_rl_planner: true, // Enable RL planner for optimization benchmarks
        rl_policy_path: None,    // Use default path
    };

    let db = EmbeddedProximaDB::new(config).unwrap();

    // Create collection
    db.create_collection(&collection_name, DIMENSION as u32, Some(engine))
        .unwrap();

    // Generate test data
    let (ids, vectors) = generate_test_vectors(vector_count, DIMENSION);
    let queries = generate_query_vectors(NUM_QUERIES, DIMENSION);

    // Measure insert performance
    let insert_start = Instant::now();
    db.insert(&collection_name, ids, vectors, None).unwrap();
    db.flush().unwrap();
    let insert_duration = insert_start.elapsed();
    let insert_qps = vector_count as f64 / insert_duration.as_secs_f64();

    // Measure search performance
    let mut search_latencies = Vec::with_capacity(queries.len());

    for query in &queries {
        let search_start = Instant::now();
        let _results = db
            .search_with_mode(
                &collection_name,
                query.clone(),
                TOP_K,
                None,
                Some(search_mode),
            )
            .unwrap();
        search_latencies.push(search_start.elapsed());
    }

    // Calculate metrics
    let total_search_time: Duration = search_latencies.iter().sum();
    let search_qps = queries.len() as f64 / total_search_time.as_secs_f64();

    search_latencies.sort();
    let latency_p50_us = if !search_latencies.is_empty() {
        search_latencies[search_latencies.len() / 2].as_micros() as f64
    } else {
        0.0
    };
    let latency_p99_us = if !search_latencies.is_empty() {
        search_latencies[(search_latencies.len() * 99) / 100].as_micros() as f64
    } else {
        0.0
    };

    // Cleanup - drop db first, then remove directory
    drop(db);
    let _ = std::fs::remove_dir_all(&data_path);

    BenchmarkResult {
        config: BenchmarkConfig {
            engine: engine.to_string(),
            search_mode: search_mode.to_string(),
            vector_count,
            dimension: DIMENSION,
        },
        insert_qps,
        search_qps,
        latency_p50_us,
        latency_p99_us,
    }
}

// ============================================================================
// BENCHMARK GROUPS
// ============================================================================

/// Benchmark 1: Engine comparison (1K vectors, exact search)
fn benchmark_engines_1k(c: &mut Criterion) {
    print_system_info("Comprehensive Optimization - Engines (1K vectors)");

    let mut group = c.benchmark_group("engines_1k");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(30));

    for &engine in ENGINES {
        group.bench_with_input(
            BenchmarkId::from_parameter(engine),
            &engine,
            |b, &engine| {
                b.iter(|| {
                    let result = run_benchmark(engine, "exact", 1_000);
                    black_box(result)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark 2: Engine comparison (10K vectors, exact search)
fn benchmark_engines_10k(c: &mut Criterion) {
    print_system_info("Comprehensive Optimization - Engines (10K vectors)");

    let mut group = c.benchmark_group("engines_10k");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(60));

    for &engine in ENGINES {
        group.bench_with_input(
            BenchmarkId::from_parameter(engine),
            &engine,
            |b, &engine| {
                b.iter(|| {
                    let result = run_benchmark(engine, "exact", 10_000);
                    black_box(result)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark 3: Search mode comparison (SST engine)
fn benchmark_search_modes_sst(c: &mut Criterion) {
    print_system_info("Comprehensive Optimization - Search Modes (SST)");

    let mut group = c.benchmark_group("search_modes_sst");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(30));

    for &mode in SEARCH_MODES {
        let mode_name = mode.replace(":", "_");
        group.bench_with_input(
            BenchmarkId::from_parameter(&mode_name),
            &mode,
            |b, &mode| {
                b.iter(|| {
                    let result = run_benchmark("sst", mode, 10_000);
                    black_box(result)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark 4: Search mode comparison (HELIX engine)
fn benchmark_search_modes_helix(c: &mut Criterion) {
    print_system_info("Comprehensive Optimization - Search Modes (HELIX)");

    let mut group = c.benchmark_group("search_modes_helix");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(30));

    for &mode in SEARCH_MODES {
        let mode_name = mode.replace(":", "_");
        group.bench_with_input(
            BenchmarkId::from_parameter(&mode_name),
            &mode,
            |b, &mode| {
                b.iter(|| {
                    let result = run_benchmark("helix", mode, 10_000);
                    black_box(result)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark 5: Full matrix - all engines × search modes (10K vectors)
fn benchmark_full_matrix(c: &mut Criterion) {
    print_system_info("Comprehensive Optimization - Full Matrix");

    let mut group = c.benchmark_group("full_matrix");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(60));

    // Focus on key engines for the matrix
    let key_engines = &["sst", "helix", "nova"];
    let key_modes = &["exact", "approximate:10"];

    for &engine in key_engines {
        for &mode in key_modes {
            let mode_name = mode.replace(":", "_");
            let bench_id = format!("{}/{}", engine, mode_name);

            group.bench_with_input(
                BenchmarkId::from_parameter(&bench_id),
                &(engine, mode),
                |b, &(engine, mode)| {
                    b.iter(|| {
                        let result = run_benchmark(engine, mode, 10_000);
                        black_box(result)
                    });
                },
            );
        }
    }

    group.finish();
}

/// Quick sanity check benchmark
fn benchmark_quick_sanity(c: &mut Criterion) {
    print_system_info("Comprehensive Optimization - Quick Sanity Check");

    let mut group = c.benchmark_group("quick_sanity");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(15));

    // Just test SST with 1K vectors to verify the benchmark works
    group.bench_function("sst_1k_exact", |b| {
        b.iter(|| {
            let result = run_benchmark("sst", "exact", 1_000);
            eprintln!(
                "SST 1K: insert={:.0} QPS, search={:.0} QPS, p50={:.0}µs",
                result.insert_qps, result.search_qps, result.latency_p50_us
            );
            black_box(result)
        });
    });

    group.finish();
}

// ============================================================================
// CRITERION CONFIGURATION
// ============================================================================

criterion_group!(
    name = quick;
    config = Criterion::default().sample_size(10);
    targets = benchmark_quick_sanity
);

criterion_group!(
    name = engines;
    config = Criterion::default().sample_size(10);
    targets = benchmark_engines_1k, benchmark_engines_10k
);

criterion_group!(
    name = search_modes;
    config = Criterion::default().sample_size(10);
    targets = benchmark_search_modes_sst, benchmark_search_modes_helix
);

criterion_group!(
    name = full_matrix;
    config = Criterion::default().sample_size(10);
    targets = benchmark_full_matrix
);

criterion_main!(quick, engines, search_modes, full_matrix);
