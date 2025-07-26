/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Benchmarks for SIMD-accelerated distance computation

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId, Throughput};
use proximadb::proto::proximadb::DistanceMetric;
use proximadb::compute::unified_distance::UnifiedDistanceCompute;
use rand::prelude::*;

/// Generate random vectors for benchmarking
fn generate_random_vectors(count: usize, dimension: usize) -> Vec<Vec<f32>> {
    let mut rng = rand::thread_rng();
    (0..count)
        .map(|_| {
            (0..dimension)
                .map(|_| rng.gen_range(-1.0..1.0))
                .collect()
        })
        .collect()
}

/// Benchmark different vector dimensions
fn benchmark_dimensions(c: &mut Criterion) {
    let mut group = c.benchmark_group("simd_distance_dimensions");
    
    // Test different dimensions (powers of 2 for optimal SIMD)
    for dimension in [64, 128, 256, 512, 1024, 2048].iter() {
        let vec_a = generate_random_vectors(1, *dimension)[0].clone();
        let vec_b = generate_random_vectors(1, *dimension)[0].clone();
        
        group.throughput(Throughput::Elements(*dimension as u64));
        
        // Cosine distance
        group.bench_with_input(
            BenchmarkId::new("cosine", dimension),
            dimension,
            |b, _| {
                let calculator = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
                b.iter(|| {
                    let result = calculator.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);
                    black_box(result.raw_value)
                });
            }
        );
        
        // Euclidean distance
        group.bench_with_input(
            BenchmarkId::new("euclidean", dimension),
            dimension,
            |b, _| {
                let calculator = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
                b.iter(|| {
                    let result = calculator.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Euclidean);
                    black_box(result.raw_value)
                });
            }
        );
        
        // Dot product
        group.bench_with_input(
            BenchmarkId::new("dot_product", dimension),
            dimension,
            |b, _| {
                let calculator = UnifiedDistanceCompute::new(DistanceMetric::DotProduct);
                b.iter(|| {
                    let result = calculator.calculate_distance(&vec_a, &vec_b, &DistanceMetric::DotProduct);
                    black_box(result.raw_value)
                });
            }
        );
    }
    
    group.finish();
}

/// Benchmark batch processing
fn benchmark_batch_processing(c: &mut Criterion) {
    let mut group = c.benchmark_group("simd_distance_batch");
    
    let dimension = 128;
    let query = generate_random_vectors(1, dimension)[0].clone();
    
    // Test different batch sizes
    for batch_size in [100, 500, 1000, 5000, 10000].iter() {
        let vectors = generate_random_vectors(*batch_size, dimension);
        let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();
        
        group.throughput(Throughput::Elements(*batch_size as u64));
        
        // Standard batch processing
        group.bench_with_input(
            BenchmarkId::new("standard", batch_size),
            batch_size,
            |b, _| {
                let calculator = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
                b.iter(|| {
                    let results = calculator.calculate_distance_batch(&query, &vector_refs, &DistanceMetric::Cosine);
                    black_box(results)
                });
            }
        );
        
        // GPU-enabled batch processing
        group.bench_with_input(
            BenchmarkId::new("gpu_enabled", batch_size),
            batch_size,
            |b, _| {
                let mut calculator = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
                calculator.set_gpu_enabled(true);
                b.iter(|| {
                    let results = calculator.calculate_distance_batch(&query, &vector_refs, &DistanceMetric::Cosine);
                    black_box(results)
                });
            }
        );
    }
    
    group.finish();
}

/// Benchmark hardware backends
fn benchmark_hardware_backends(c: &mut Criterion) {
    // Get hardware info
    let calculator = UnifiedDistanceCompute::default();
    println!("\nHardware Backend:");
    println!("  Using: {}", calculator.preferred_backend());
    
    let mut group = c.benchmark_group("hardware_backends");
    
    let dimension = 1024; // Large dimension to show SIMD benefits
    let vec_a = generate_random_vectors(1, dimension)[0].clone();
    let vec_b = generate_random_vectors(1, dimension)[0].clone();
    
    group.throughput(Throughput::Elements(dimension as u64));
    
    // CPU backend
    group.bench_function("cpu", |b| {
        let mut calc = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
        calc.set_gpu_enabled(false);
        b.iter(|| {
            let result = calc.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);
            black_box(result.raw_value)
        });
    });
    
    // GPU backend (if available)
    group.bench_function("gpu", |b| {
        let mut calc = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
        calc.set_gpu_enabled(true);
        b.iter(|| {
            let result = calc.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);
            black_box(result.raw_value)
        });
    });
    
    group.finish();
}

criterion_group!(
    benches,
    benchmark_dimensions,
    benchmark_batch_processing,
    benchmark_hardware_backends
);
criterion_main!(benches);