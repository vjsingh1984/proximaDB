/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Benchmarks for SIMD-accelerated distance computation with realistic embeddings

mod common;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use proximadb::compute::distance_computation::{
    DistanceMetric, DistanceMode, UnifiedDistanceCompute,
};
use proximadb::core::hardware_capabilities;
use common::{EmbeddingGenerator, EmbeddingModel};
use tracing::debug;

/// Generate realistic embedding vectors for benchmarking
fn generate_embedding_vectors(count: usize, dimension: usize, model: EmbeddingModel) -> Vec<Vec<f32>> {
    let mut generator = EmbeddingGenerator::new(model);
    generator.generate_batch(count, dimension)
}

/// Benchmark different vector dimensions
fn benchmark_dimensions(c: &mut Criterion) {
    // Initialize global hardware capabilities
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();
    let mut group = c.benchmark_group("simd_distance_dimensions");

    // Test different dimensions with appropriate embedding models
    let test_configs = vec![
        (64, EmbeddingModel::Normalized),
        (128, EmbeddingModel::Normalized),
        (256, EmbeddingModel::Normalized),
        (512, EmbeddingModel::Normalized),
        (768, EmbeddingModel::Bert),      // BERT embeddings
        (1536, EmbeddingModel::OpenAIAda), // OpenAI embeddings
    ];

    for (dimension, model) in test_configs {
        let vec_a = generate_embedding_vectors(1, dimension, model)[0].clone();
        let vec_b = generate_embedding_vectors(1, dimension, model)[0].clone();

        group.throughput(Throughput::Elements(dimension as u64));

        // Cosine distance
        group.bench_with_input(BenchmarkId::new("cosine", dimension), &dimension, |b, _| {
            let calculator = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
            b.iter(|| {
                let result = calculator.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);
                black_box(result.raw_value)
            });
        });

        // Euclidean distance
        group.bench_with_input(
            BenchmarkId::new("euclidean", dimension),
            &dimension,
            |b, _| {
                let calculator = UnifiedDistanceCompute::new(DistanceMetric::Euclidean);
                b.iter(|| {
                    let result =
                        calculator.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Euclidean);
                    black_box(result.raw_value)
                });
            },
        );

        // Dot product
        group.bench_with_input(
            BenchmarkId::new("dot_product", dimension),
            &dimension,
            |b, _| {
                let calculator = UnifiedDistanceCompute::new(DistanceMetric::DotProduct);
                b.iter(|| {
                    let result =
                        calculator.calculate_distance(&vec_a, &vec_b, &DistanceMetric::DotProduct);
                    black_box(result.raw_value)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark batch processing with realistic embeddings
fn benchmark_batch_processing(c: &mut Criterion) {
    let mut group = c.benchmark_group("simd_distance_batch");

    // Use BERT embeddings for batch processing
    let dimension = 768;
    let model = EmbeddingModel::Bert;
    let query = generate_embedding_vectors(1, dimension, model)[0].clone();

    // Test different batch sizes
    for batch_size in [100, 500, 1000, 5000, 10000].iter() {
        let vectors = generate_embedding_vectors(*batch_size, dimension, model);
        let vector_refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();

        group.throughput(Throughput::Elements(*batch_size as u64));

        // Standard batch processing
        group.bench_with_input(
            BenchmarkId::new("standard", batch_size),
            batch_size,
            |b, _| {
                let calculator = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
                b.iter(|| {
                    let results = calculator.calculate_distance_batch(
                        &query,
                        &vector_refs,
                        &DistanceMetric::Cosine,
                    );
                    black_box(results)
                });
            },
        );

        // GPU-enabled batch processing
        group.bench_with_input(
            BenchmarkId::new("gpu_enabled", batch_size),
            batch_size,
            |b, _| {
                let mut calculator = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
                // GPU acceleration enabled by default in UnifiedDistanceCompute
                b.iter(|| {
                    let results = calculator.calculate_distance_batch(
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

/// Benchmark hardware backends
fn benchmark_hardware_backends(c: &mut Criterion) {
    // Initialize global hardware capabilities
    let _ = hardware_capabilities::initialize_hardware_capabilities_default();

    // Get hardware info
    let calculator = UnifiedDistanceCompute::default();
    debug!("\nHardware Backend:");
    debug!("  Available backends: {:?}", calculator.available_backends());

    let mut group = c.benchmark_group("hardware_backends");

    let dimension = 1024; // Large dimension to show SIMD benefits
    let vec_a = generate_embedding_vectors(1, dimension, EmbeddingModel::Normalized)[0].clone();
    let vec_b = generate_embedding_vectors(1, dimension, EmbeddingModel::Normalized)[0].clone();

    group.throughput(Throughput::Elements(dimension as u64));

    // CPU backend
    group.bench_function("cpu", |b| {
        let mut calc = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
        // CPU-only mode: create with CPU backend
        b.iter(|| {
            let result = calc.calculate_distance(&vec_a, &vec_b, &DistanceMetric::Cosine);
            black_box(result.raw_value)
        });
    });

    // GPU backend (if available)
    group.bench_function("gpu", |b| {
        let mut calc = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
        // GPU acceleration enabled by default
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
