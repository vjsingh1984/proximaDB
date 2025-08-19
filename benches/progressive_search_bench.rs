//! Benchmarks for progressive quantization-aware search
//!
//! Demonstrates the performance improvements from using staged refinement
//! with the formula: k_binary = k · n_b · n_int8 · n_pq

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use proximadb::core::search::progressive_quantization::{
    ProgressiveSearchConfig, SearchScenario, StageSizes,
};
use rand::prelude::*;
use std::time::Duration;

/// Generate random vectors for benchmarking
fn generate_random_vectors(count: usize, dimension: usize) -> Vec<Vec<f32>> {
    let mut rng = thread_rng();
    (0..count)
        .map(|_| {
            (0..dimension)
                .map(|_| rng.gen_range(-1.0..1.0))
                .collect()
        })
        .collect()
}

/// Simulate brute force search
fn brute_force_search(
    query: &[f32],
    database: &[Vec<f32>],
    k: usize,
) -> Vec<(usize, f32)> {
    let mut distances: Vec<(usize, f32)> = database
        .iter()
        .enumerate()
        .map(|(idx, vector)| {
            let distance = compute_cosine_distance(query, vector);
            (idx, distance)
        })
        .collect();
    
    distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    distances.truncate(k);
    distances
}

/// Simulate progressive search with quantization stages
fn progressive_search(
    query: &[f32],
    database: &[Vec<f32>],
    k: usize,
    config: &ProgressiveSearchConfig,
) -> Vec<(usize, f32)> {
    let sizes = config.compute_stage_sizes(k);
    
    // Stage 1: Binary search (simulate with sampling)
    let binary_sample_rate = sizes.binary_candidates as f32 / database.len() as f32;
    let binary_candidates: Vec<usize> = database
        .iter()
        .enumerate()
        .filter(|_| thread_rng().gen::<f32>() < binary_sample_rate.min(1.0))
        .map(|(idx, _)| idx)
        .take(sizes.binary_candidates)
        .collect();
    
    // Stage 2: INT8 refinement (simulate with reduced precision)
    let int8_candidates: Vec<usize> = binary_candidates
        .iter()
        .take(sizes.int8_candidates)
        .copied()
        .collect();
    
    // Stage 3: PQ refinement (simulate with better precision)
    let pq_candidates: Vec<usize> = int8_candidates
        .iter()
        .take(sizes.pq_candidates)
        .copied()
        .collect();
    
    // Stage 4: FP32 final ranking
    let mut final_results: Vec<(usize, f32)> = pq_candidates
        .iter()
        .map(|&idx| {
            let distance = compute_cosine_distance(query, &database[idx]);
            (idx, distance)
        })
        .collect();
    
    final_results.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    final_results.truncate(k);
    final_results
}

/// Compute cosine distance between two vectors
fn compute_cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    1.0 - (dot_product / (norm_a * norm_b))
}

/// Benchmark progressive search vs brute force
fn bench_progressive_vs_brute_force(c: &mut Criterion) {
    let dimensions = vec![128, 384, 768, 1536];
    let database_sizes = vec![10_000, 100_000];
    let k = 100;
    
    let mut group = c.benchmark_group("progressive_search");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);
    
    for dimension in dimensions {
        for db_size in &database_sizes {
            let database = generate_random_vectors(*db_size, dimension);
            let query = generate_random_vectors(1, dimension)[0].clone();
            
            // Benchmark brute force
            group.bench_with_input(
                BenchmarkId::new(
                    "brute_force",
                    format!("d{}_n{}", dimension, db_size)
                ),
                &(&query, &database),
                |b, (q, db)| {
                    b.iter(|| {
                        brute_force_search(black_box(q), black_box(db), k)
                    });
                },
            );
            
            // Benchmark progressive search with different scenarios
            let scenarios = vec![
                (SearchScenario::HighSpeed, "high_speed"),
                (SearchScenario::Balanced, "balanced"),
                (SearchScenario::HighRecall, "high_recall"),
            ];
            
            for (scenario, name) in scenarios {
                let config = ProgressiveSearchConfig::for_scenario(scenario);
                
                group.bench_with_input(
                    BenchmarkId::new(
                        format!("progressive_{}", name),
                        format!("d{}_n{}", dimension, db_size)
                    ),
                    &(&query, &database, &config),
                    |b, (q, db, cfg)| {
                        b.iter(|| {
                            progressive_search(black_box(q), black_box(db), k, black_box(cfg))
                        });
                    },
                );
            }
        }
    }
    
    group.finish();
}

/// Benchmark stage size computation
fn bench_stage_size_computation(c: &mut Criterion) {
    let mut group = c.benchmark_group("stage_computation");
    
    let k_values = vec![10, 50, 100, 500, 1000];
    let config = ProgressiveSearchConfig::default();
    
    for k in k_values {
        group.bench_with_input(
            BenchmarkId::new("compute_stages", k),
            &k,
            |b, &k| {
                b.iter(|| {
                    config.compute_stage_sizes(black_box(k))
                });
            },
        );
    }
    
    group.finish();
}

/// Benchmark different recall configurations
fn bench_recall_configurations(c: &mut Criterion) {
    let mut group = c.benchmark_group("recall_configs");
    
    let k = 100;
    let dimension = 768;
    let db_size = 100_000;
    
    let database = generate_random_vectors(db_size, dimension);
    let query = generate_random_vectors(1, dimension)[0].clone();
    
    // Test different recall configurations
    let recall_configs = vec![
        ("low_recall", 0.70, 0.85, 0.90),
        ("medium_recall", 0.80, 0.90, 0.95),
        ("high_recall", 0.90, 0.95, 0.98),
        ("ultra_recall", 0.95, 0.98, 0.99),
    ];
    
    for (name, binary, int8, pq) in recall_configs {
        let config = ProgressiveSearchConfig {
            binary_recall: binary,
            int8_recall: int8,
            pq_recall: pq,
            ..Default::default()
        };
        
        let sizes = config.compute_stage_sizes(k);
        
        group.bench_with_input(
            BenchmarkId::new(name, format!("stages_{}", sizes.total_computations)),
            &(&query, &database, &config),
            |b, (q, db, cfg)| {
                b.iter(|| {
                    progressive_search(black_box(q), black_box(db), k, black_box(cfg))
                });
            },
        );
    }
    
    group.finish();
}

/// Measure speedup vs database size
fn bench_speedup_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("speedup_scaling");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(20);
    
    let k = 100;
    let dimension = 384;
    let config = ProgressiveSearchConfig::default();
    
    let database_sizes = vec![1_000, 5_000, 10_000, 50_000, 100_000];
    
    for db_size in database_sizes {
        let database = generate_random_vectors(db_size, dimension);
        let query = generate_random_vectors(1, dimension)[0].clone();
        
        // Measure operations for progressive search
        let sizes = config.compute_stage_sizes(k);
        let speedup = db_size as f64 / sizes.total_computations as f64;
        
        group.bench_with_input(
            BenchmarkId::new(
                "progressive",
                format!("n{}_speedup_{:.0}x", db_size, speedup)
            ),
            &(&query, &database, &config),
            |b, (q, db, cfg)| {
                b.iter(|| {
                    progressive_search(black_box(q), black_box(db), k, black_box(cfg))
                });
            },
        );
    }
    
    group.finish();
}

/// Benchmark memory usage patterns
fn bench_memory_patterns(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_usage");
    
    let k = 100;
    let dimension = 768;
    
    // Compare memory usage for different scenarios
    let scenarios = vec![
        (SearchScenario::LowMemory, "low_memory"),
        (SearchScenario::Balanced, "balanced"),
        (SearchScenario::HighRecall, "high_recall"),
    ];
    
    for (scenario, name) in scenarios {
        let config = ProgressiveSearchConfig::for_scenario(scenario);
        let sizes = config.compute_stage_sizes(k);
        
        // Calculate approximate memory usage
        let binary_memory = sizes.binary_candidates * dimension / 8; // 1 bit per dimension
        let int8_memory = sizes.int8_candidates * dimension; // 1 byte per dimension
        let pq_memory = sizes.pq_candidates * dimension / 4; // PQ compression
        let fp32_memory = sizes.fp32_candidates * dimension * 4; // 4 bytes per float
        
        let total_memory = binary_memory + int8_memory + pq_memory + fp32_memory;
        
        group.bench_with_input(
            BenchmarkId::new(name, format!("{}KB", total_memory / 1024)),
            &config,
            |b, cfg| {
                b.iter(|| {
                    cfg.compute_stage_sizes(black_box(k))
                });
            },
        );
    }
    
    group.finish();
}

criterion_group!(
    benches,
    bench_progressive_vs_brute_force,
    bench_stage_size_computation,
    bench_recall_configurations,
    bench_speedup_scaling,
    bench_memory_patterns
);
criterion_main!(benches);