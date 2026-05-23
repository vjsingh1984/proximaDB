/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! HMGI predicate-aware routing benchmarks.
//!
//! Measures the HMGI search-space reduction promised by the multi-model spec's
//! predicate-aware HNSW strategy using the shared SIMD distance engine.

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use proximadb::compute::distance_computation::{DistanceMetric, engine::UnifiedDistanceCompute};
use proximadb::index::axis::hmgi::{
    HmgiPartitionKey, HmgiRegistry, HmgiRouteStats, HmgiRouter, ModalityExtractor, PartitionSet,
};
use proximadb::index::axis::management::{
    FilterOperator, HybridQuery, MetadataFilter, VectorQuery,
};
use std::cmp::Ordering;
use std::sync::Arc;

const DIMENSION: usize = 128;
const TOP_K: usize = 10;
const MODALITIES: &[&str] = &["text", "image", "audio", "video"];
const DATASET_SIZES: &[usize] = &[1_000, 5_000];

#[derive(Clone)]
struct Candidate {
    modality: &'static str,
    vector: Vec<f32>,
}

fn generate_candidates(count: usize, dimension: usize) -> Vec<Candidate> {
    (0..count)
        .map(|i| {
            let modality = MODALITIES[i % MODALITIES.len()];
            let vector = (0..dimension)
                .map(|d| {
                    let base = (i as f32 * 0.017) + (d as f32 * 0.031);
                    (base.sin() + base.cos() * 0.5) / 1.5
                })
                .collect();

            Candidate { modality, vector }
        })
        .collect()
}

fn query_vector(dimension: usize) -> Vec<f32> {
    (0..dimension)
        .map(|d| {
            let base = d as f32 * 0.019;
            (base.sin() + base.cos() * 0.25) / 1.25
        })
        .collect()
}

fn modality_query(modality: &str) -> HybridQuery {
    HybridQuery {
        collection_id: "hmgi_benchmark".to_string(),
        vector_query: Some(VectorQuery::Dense {
            vector: query_vector(DIMENSION),
            similarity_threshold: 0.0,
        }),
        metadata_filters: vec![MetadataFilter {
            field: "_modality".to_string(),
            operator: FilterOperator::Equals,
            value: serde_json::json!(modality),
        }],
        id_filters: Vec::new(),
        top_k: TOP_K,
        include_expired: false,
        ann_filtering_mode: Default::default(),
    }
}

fn partition_set() -> PartitionSet {
    MODALITIES
        .iter()
        .map(|modality| HmgiPartitionKey::new(123, 1, (*modality).to_string(), None))
        .collect()
}

fn exact_top_k(
    compute: &UnifiedDistanceCompute,
    query: &[f32],
    candidates: &[Candidate],
    top_k: usize,
) -> Vec<(usize, f32)> {
    let vector_refs: Vec<&[f32]> = candidates
        .iter()
        .map(|candidate| candidate.vector.as_slice())
        .collect();
    let similarities = compute.similarity_batch(query, &vector_refs, Some(DistanceMetric::Cosine));

    let mut ranked: Vec<(usize, f32)> = similarities
        .into_iter()
        .enumerate()
        .map(|(idx, similarity)| (idx, similarity.normalized_score))
        .collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(Ordering::Equal));
    ranked.truncate(top_k);
    ranked
}

fn benchmark_hmgi_search_space_reduction(c: &mut Criterion) {
    let compute = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
    let query = query_vector(DIMENSION);

    let mut group = c.benchmark_group("hmgi_search_space_reduction");
    group.sample_size(10);

    for &dataset_size in DATASET_SIZES {
        let candidates = generate_candidates(dataset_size, DIMENSION);
        let routed_candidates: Vec<Candidate> = candidates
            .iter()
            .filter(|candidate| candidate.modality == "text")
            .cloned()
            .collect();
        let reduction = 1.0 - (routed_candidates.len() as f32 / candidates.len().max(1) as f32);
        assert!(reduction >= 0.70);

        group.throughput(Throughput::Elements(dataset_size as u64));
        group.bench_with_input(
            BenchmarkId::new("monolithic_exact_scan", dataset_size),
            &dataset_size,
            |b, _| {
                b.iter(|| {
                    black_box(exact_top_k(
                        &compute,
                        black_box(&query),
                        black_box(&candidates),
                        TOP_K,
                    ))
                });
            },
        );

        group.throughput(Throughput::Elements(routed_candidates.len() as u64));
        group.bench_with_input(
            BenchmarkId::new("hmgi_modality_routed_scan", dataset_size),
            &dataset_size,
            |b, _| {
                b.iter(|| {
                    black_box(exact_top_k(
                        &compute,
                        black_box(&query),
                        black_box(&routed_candidates),
                        TOP_K,
                    ))
                });
            },
        );
    }

    group.finish();
}

fn benchmark_hmgi_route_stats(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
    let router = HmgiRouter::new(
        Arc::new(HmgiRegistry::new()),
        Arc::new(ModalityExtractor::new()),
    );
    let query = modality_query("text");
    let partitions = partition_set();

    let stats = runtime
        .block_on(router.route_stats("hmgi_benchmark", &query, partitions.clone()))
        .expect("route stats");
    assert_eq!(
        stats,
        HmgiRouteStats {
            total_partitions: 4,
            searched_partitions: 1,
            pruned_partitions: 3,
            search_space_reduction: 0.75,
            fanout_ratio: 0.25,
        }
    );

    let mut group = c.benchmark_group("hmgi_route_stats");
    group.sample_size(20);
    group.bench_function("modality_filter_stats", |b| {
        b.iter(|| {
            let stats = runtime
                .block_on(router.route_stats(
                    "hmgi_benchmark",
                    black_box(&query),
                    black_box(partitions.clone()),
                ))
                .expect("route stats");
            black_box(stats)
        });
    });
    group.finish();
}

criterion_group!(
    hmgi_benches,
    benchmark_hmgi_search_space_reduction,
    benchmark_hmgi_route_stats
);
criterion_main!(hmgi_benches);
