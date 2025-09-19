//! Benchmarks for VIPER engine Parquet operations
//!
//! Measures performance of:
//! - Vector writing and reading
//! - Flush operations
//! - Search performance
//! - Compaction operations

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use proximadb::{
    core::VectorRecord,
    storage::{
        engines::{
            factory::StorageEngineFactory,
            impls::viper::ViperEngine,
        },
        traits::{UnifiedStorageEngine, FlushParameters, CompactionParameters, StorageQueryContext, StorageQueryMetadata},
    },
    core::search::SearchParams,
    compute::distance_computation::DistanceMetric,
    proto::proximadb_v1::{Collection, CollectionConfig, CollectionStats},
};
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::tempdir;
use tokio::runtime::Runtime;

/// Generate test vectors
fn generate_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| {
            let vector = (0..dimension)
                .map(|j| ((i + j) as f32 * 0.001) % 1.0)
                .collect();

            let mut metadata = HashMap::new();
            metadata.insert("category".to_string(), format!("cat_{}", i % 10));
            metadata.insert("is_active".to_string(), (i % 2 == 0).to_string());
            metadata.insert("count".to_string(), i.to_string());
            metadata.insert("score".to_string(), format!("{}", (i as f32 * 0.1) % 100.0));

            VectorRecord {
                id: format!("vec_{:08}", i),
                vector,
                metadata,
                timestamp: i as i64,
                updated_at: Some(chrono::Utc::now().timestamp()),
                expires_at: None,
                version: Some(1),
            }
        })
        .collect()
}

/// Benchmark VIPER engine flush operations
fn bench_viper_flush(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("viper_flush");

    for size in [100, 1000, 5000].iter() {
        let vectors = generate_vectors(*size, 768);

        group.bench_with_input(BenchmarkId::new("flush", size), size, |b, _| {
            b.iter(|| {
                rt.block_on(async {
                    let engine = StorageEngineFactory::create_viper().unwrap();

                    let params = FlushParameters {
                        collection_id: Some("bench_collection".to_string()),
                        vector_records: vectors.clone(),
                        force: true,
                        synchronous: true,
                        ..Default::default()
                    };

                    let _ = engine.flush(params).await;
                })
            });
        });
    }

    group.finish();
}

/// Benchmark VIPER engine search operations
fn bench_viper_search(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("viper_search");

    // Setup test data
    let vectors = generate_vectors(1000, 768);
    let query_vector = vec![0.5; 768];

    // Create and populate engine
    let engine = rt.block_on(async {
        let engine = StorageEngineFactory::create_viper().unwrap();

        let params = FlushParameters {
            collection_id: Some("bench_collection".to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            ..Default::default()
        };

        engine.flush(params).await.unwrap();
        engine
    });

    for top_k in [10, 50, 100].iter() {
        group.bench_with_input(BenchmarkId::new("search", top_k), top_k, |b, &k| {
            b.iter(|| {
                rt.block_on(async {
                    let search_params = Arc::new(SearchParams {
                        vector: Some(query_vector.clone()),
                        query_vectors: None,
                        top_k: Some(k),
                        distance_metric: Some(DistanceMetric::Euclidean),
                        filter_expression: None,
                        filters: None,
                        accuracy_threshold: None,
                        include_metadata: Some(false),
                        include_expired: Some(false),
                        ..Default::default()
                    });

                    let collection = Arc::new(Collection {
                        id: "bench_collection".to_string(),
                        config: Some(CollectionConfig {
                            name: "bench_collection".to_string(),
                            dimension: 768,
                            distance_metric: proximadb::proto::proximadb_v1::DistanceMetric::Euclidean as i32,
                            storage_engine: proximadb::proto::proximadb_v1::StorageEngine::Viper as i32,
                            ..Default::default()
                        }),
                        stats: Some(CollectionStats::default()),
                        created_at: 0,
                        updated_at: 0,
                        storage_assignment: None,
                    });

                    let ctx = StorageQueryContext {
                        search_params,
                        collection,
                        metadata: StorageQueryMetadata::default(),
                    };

                    let results = engine.search_vectors_unified(&ctx).await.unwrap();
                    black_box(results);
                })
            });
        });
    }

    group.finish();
}

/// Benchmark VIPER engine compaction
fn bench_viper_compaction(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("viper_compaction");

    for size in [500, 1000, 2000].iter() {
        let vectors = generate_vectors(*size, 768);

        group.bench_with_input(BenchmarkId::new("compact", size), size, |b, _| {
            b.iter(|| {
                rt.block_on(async {
                    let engine = StorageEngineFactory::create_viper().unwrap();

                    // First flush data
                    let flush_params = FlushParameters {
                        collection_id: Some("bench_collection".to_string()),
                        vector_records: vectors.clone(),
                        force: true,
                        synchronous: true,
                        ..Default::default()
                    };

                    engine.flush(flush_params).await.unwrap();

                    // Then compact
                    let compact_params = CompactionParameters {
                        collection_id: Some("bench_collection".to_string()),
                        force: true,
                        synchronous: true,
                        ..Default::default()
                    };

                    let _ = engine.compact(compact_params).await;
                })
            });
        });
    }

    group.finish();
}

/// Benchmark VIPER vs SST engine comparison
fn bench_engine_comparison(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("engine_comparison");

    let vectors = generate_vectors(1000, 768);

    // Benchmark VIPER engine
    group.bench_function("viper", |b| {
        b.iter(|| {
            rt.block_on(async {
                let engine = StorageEngineFactory::create_viper().unwrap();

                let params = FlushParameters {
                    collection_id: Some("bench_collection".to_string()),
                    vector_records: vectors.clone(),
                    force: true,
                    synchronous: true,
                    ..Default::default()
                };

                engine.flush(params).await.unwrap();
            })
        });
    });

    // Benchmark SST engine
    group.bench_function("sst", |b| {
        b.iter(|| {
            rt.block_on(async {
                let engine = StorageEngineFactory::create_sst().unwrap();

                let params = FlushParameters {
                    collection_id: Some("bench_collection".to_string()),
                    vector_records: vectors.clone(),
                    force: true,
                    synchronous: true,
                    ..Default::default()
                };

                engine.flush(params).await.unwrap();
            })
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_viper_flush,
    bench_viper_search,
    bench_viper_compaction,
    bench_engine_comparison
);
criterion_main!(benches);