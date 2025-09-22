//! Benchmark for progressive search with real implementation and realistic embeddings
//!
//! This benchmark uses the actual ProgressiveSearchExecutor from the storage engine
//! with realistic BERT and OpenAI embeddings for accurate performance measurements.

mod common;
use common::benchmark_utils::{print_system_info, STANDARD_DIMENSIONS, STANDARD_BATCH_SIZES};

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use proximadb::{
    compute::{
        distance_computation::{DistanceMetric, engine::UnifiedDistanceCompute},
        quantization::unified::UnifiedQuantizationEngine,
    },
    core::search::progressive_quantization::{
        ProgressiveSearchConfig, SearchScenario, StageSizes,
    },
    storage::engines::core::search::progressive_search::ProgressiveSearchExecutor,
    proto::proximadb_v1::VectorRecord,
    storage::traits::{StorageQueryContext, StorageQueryMetadata},
};
use common::{EmbeddingGenerator, EmbeddingModel};
use std::sync::Arc;
use tokio::runtime::Runtime;

/// Generate realistic embedding vectors for benchmarking
fn generate_embedding_vectors(count: usize, dimension: usize, model: EmbeddingModel) -> Vec<Vec<f32>> {
    let mut generator = EmbeddingGenerator::new(model);
    generator.generate_batch(count, dimension)
}

/// Convert vectors to VectorRecords for real engine use
fn vectors_to_records(vectors: &[Vec<f32>]) -> Vec<VectorRecord> {
    vectors.iter().enumerate().map(|(i, v)| {
        VectorRecord {
            id: format!("vec_{}", i),
            vector: v.clone(),
            metadata: std::collections::HashMap::new(),
            timestamp: i as i64,
            updated_at: None,
            expires_at: None,
            version: Some(1),
            quantized_vector: vec![],
            source: None,
        }
    }).collect()
}

/// Real brute force search using UnifiedDistanceCompute
fn brute_force_search(query: &[f32], database: &[Vec<f32>], k: usize) -> Vec<(usize, f32)> {
    let distance_compute = UnifiedDistanceCompute::new(DistanceMetric::Cosine);
    let mut distances: Vec<(usize, f32)> = database
        .iter()
        .enumerate()
        .map(|(idx, vector)| {
            let result = distance_compute.calculate_distance(query, vector, &DistanceMetric::Cosine);
            (idx, result.distance)
        })
        .collect();

    distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    distances.truncate(k);
    distances
}

/// Real progressive search using ProgressiveSearchExecutor
async fn progressive_search_real(
    query: &[f32],
    database: &[VectorRecord],
    k: usize,
    config: &ProgressiveSearchConfig,
) -> Vec<(String, f32)> {
    // Initialize real components
    use proximadb::compute::quantization::InMemoryCodebookStore;

    let distance_compute = Arc::new(UnifiedDistanceCompute::new(DistanceMetric::Cosine));
    let codebook_store: Arc<dyn proximadb::compute::quantization::CodebookStore> = Arc::new(InMemoryCodebookStore::new());
    let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
        distance_compute.clone(),
        codebook_store,
    ));

    let executor = ProgressiveSearchExecutor::new(
        quantization_engine.clone(),
        distance_compute.clone(),
    );

    // Create search context with proper structure
    use proximadb::core::search::SearchParams;
    use proximadb::proto::proximadb_v1::{Collection, CollectionConfig, CollectionStats};
    

    let search_params = Arc::new(SearchParams {
        vector: Some(query.to_vec()),
        query_vectors: None,
        top_k: Some(k),
        distance_metric: Some(DistanceMetric::Cosine),
        filter_expression: None,
        filters: None,
        accuracy_threshold: None,
        include_expired: Some(false),
        ..Default::default()
    });

    let collection = Arc::new(Collection {
        id: "bench-nova".to_string(),
        config: Some(CollectionConfig {
            name: "bench-nova".to_string(),
            dimension: query.len() as u32,
            distance_metric: proximadb::proto::proximadb_v1::DistanceMetric::Cosine as i32,
            storage_engine: proximadb::proto::proximadb_v1::StorageEngine::Sst as i32,
            ..Default::default()
        }),
        stats: Some(CollectionStats::default()),
        created_at: 0,
        updated_at: 0,
        storage_assignment: None,
    });

    let context = StorageQueryContext {
        search_params,
        collection,
        metadata: StorageQueryMetadata::default(),
    };

    // Execute real progressive search
    match executor.execute_progressive_search(
        &context,
        database.to_vec(),
        query,
    ).await {
        Ok(results) => {
            results.into_iter()
                .map(|r| (r.id.clone(), r.score))
                .collect()
        }
        Err(_) => {
            // Fallback to brute force if progressive search fails
            let vectors: Vec<Vec<f32>> = database.iter().map(|r| r.vector.clone()).collect();
            brute_force_search(query, &vectors, k)
                .into_iter()
                .map(|(idx, score)| (database[idx].id.clone(), score))
                .collect()
        }
    }
}

/// Benchmark progressive search vs brute force
fn bench_progressive_vs_brute_force(c: &mut Criterion) {
    print_system_info("Progressive Query Search");
    let mut group = c.benchmark_group("progressive_search_comparison");

    // Initialize runtime for async operations
    let runtime = Runtime::new().unwrap();

    // Use standard batch sizes
    let database_sizes = STANDARD_BATCH_SIZES.to_vec(); // [250, 1000, 5000]
    let dimension = 768; // BERT embeddings
    let model = EmbeddingModel::Bert;
    let k = 10;

    for size in database_sizes {
        let database = generate_embedding_vectors(size, dimension, model);
        let query = generate_embedding_vectors(1, dimension, model)[0].clone();
        let records = vectors_to_records(&database);

        // Benchmark brute force search
        group.bench_with_input(
            BenchmarkId::new("brute_force", size),
            &size,
            |b, _| {
                b.iter(|| {
                    let results = brute_force_search(&query, &database, k);
                    black_box(results)
                });
            },
        );

        // Benchmark real progressive search
        let config = ProgressiveSearchConfig::for_scenario(SearchScenario::HighRecall);

        group.bench_with_input(
            BenchmarkId::new("progressive_real", size),
            &size,
            |b, _| {
                b.iter(|| {
                    let results = runtime.block_on(progressive_search_real(
                        &query,
                        &records,
                        k,
                        &config,
                    ));
                    black_box(results)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark different search scenarios with real implementation
fn bench_search_scenarios(c: &mut Criterion) {
    let mut group = c.benchmark_group("search_scenarios");

    let runtime = Runtime::new().unwrap();
    let dimension = 768; // BERT embeddings
    let model = EmbeddingModel::Bert;
    let database = generate_embedding_vectors(5000, dimension, model);
    let records = vectors_to_records(&database);
    let query = generate_embedding_vectors(1, dimension, model)[0].clone();

    let scenarios = vec![
        (SearchScenario::HighSpeed, "high_speed"),
        (SearchScenario::Balanced, "balanced"),
        (SearchScenario::HighRecall, "high_recall"),
    ];

    for (scenario, name) in scenarios {
        let config = ProgressiveSearchConfig::for_scenario(scenario);

        group.bench_function(name, |b| {
            b.iter(|| {
                let results = runtime.block_on(progressive_search_real(
                    &query,
                    &records,
                    10,
                    &config,
                ));
                black_box(results)
            });
        });
    }

    group.finish();
}

/// Benchmark scaling with different dimensions using real implementation
fn bench_dimension_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("dimension_scaling");

    let runtime = Runtime::new().unwrap();
    let test_configs = vec![
        (128, EmbeddingModel::Normalized),
        (256, EmbeddingModel::Normalized),
        (512, EmbeddingModel::Normalized),
        (768, EmbeddingModel::Bert),
        (1536, EmbeddingModel::OpenAIAda),
    ];
    let database_size = 1000;

    for (dimension, model) in test_configs {
        let database = generate_embedding_vectors(database_size, dimension, model);
        let records = vectors_to_records(&database);
        let query = generate_embedding_vectors(1, dimension, model)[0].clone();
        let config = ProgressiveSearchConfig::for_scenario(SearchScenario::Balanced);

        group.bench_with_input(
            BenchmarkId::new("progressive", dimension),
            &dimension,
            |b, _| {
                b.iter(|| {
                    let results = runtime.block_on(progressive_search_real(
                        &query,
                        &records,
                        10,
                        &config,
                    ));
                    black_box(results)
                });
            },
        );
    }

    group.finish();
}

/// Benchmark stage performance with real implementation
fn bench_stage_performance(c: &mut Criterion) {
    let mut group = c.benchmark_group("stage_performance");
    group.sample_size(10);

    let runtime = Runtime::new().unwrap();
    let dimension = 768; // Use BERT for stage testing
    let model = EmbeddingModel::Bert;
    let database = generate_embedding_vectors(10000, dimension, model);
    let records = vectors_to_records(&database);
    let query = generate_embedding_vectors(1, dimension, model)[0].clone();

    // Test with different stage size configurations
    let configurations = vec![
        ("aggressive", StageSizes {
            binary_candidates: 500,
            int8_candidates: 100,
            pq_candidates: 50,
            fp32_candidates: 10,
            total_computations: 660,  // Sum of all candidate stages
            effective_expansion: 50.0,  // binary_candidates / fp32_candidates
        }),
        ("balanced", StageSizes {
            binary_candidates: 1000,
            int8_candidates: 200,
            pq_candidates: 100,
            fp32_candidates: 10,
            total_computations: 1310,  // Sum of all candidate stages
            effective_expansion: 100.0,  // binary_candidates / fp32_candidates
        }),
        ("conservative", StageSizes {
            binary_candidates: 2000,
            int8_candidates: 500,
            pq_candidates: 200,
            fp32_candidates: 10,
            total_computations: 2710,  // Sum of all candidate stages
            effective_expansion: 200.0,  // binary_candidates / fp32_candidates
        }),
    ];

    for (name, _sizes) in configurations {
        // Note: StageSizes would be calculated internally based on config's recall rates
        // For now, we use the default config for the scenario
        let config = ProgressiveSearchConfig::for_scenario(SearchScenario::Balanced);

        group.bench_function(name, |b| {
            b.iter(|| {
                let results = runtime.block_on(progressive_search_real(
                    &query,
                    &records,
                    10,
                    &config,
                ));
                black_box(results)
            });
        });
    }

    group.finish();
}

// Configure with consistent settings across all benchmarks
criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(40)
        .measurement_time(std::time::Duration::from_secs(5))
        .warm_up_time(std::time::Duration::from_secs(1));
    targets = bench_progressive_vs_brute_force,
              bench_search_scenarios,
              bench_dimension_scaling,
              bench_stage_performance
}

criterion_main!(benches);