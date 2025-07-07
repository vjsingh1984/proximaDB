use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use futures;
use proximadb::services::vector_service::VectorService;
use proximadb::core::config::VectorServiceConfig;
use proximadb::core::config::AxisIndexConfig;
use proximadb::storage::config::StorageConfig;
use proximadb::storage::persistence::wal::config::WalConfig;
use proximadb::storage::fs::local::LocalFilesystem;
use std::sync::Arc;
use std::collections::HashMap;
use uuid::Uuid;
use tokio::runtime::Runtime;

// Helper function to create test vector service
async fn create_test_vector_service() -> Arc<VectorService> {
    let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
    let base_path = temp_dir.path().to_str().unwrap();
    
    let config = VectorServiceConfig {
        base_data_path: base_path.to_string(),
        axis_config: AxisIndexConfig::default(),
        storage_config: StorageConfig::default(),
        wal_config: WalConfig {
            base_wal_path: format!("{}/wal", base_path),
            flush_interval_ms: 5000,
            max_wal_size_mb: 100,
            wal_strategy: proximadb::storage::persistence::wal::config::WalStrategy::Avro,
            sync_mode: proximadb::storage::persistence::wal::config::SyncMode::PerBatch,
        },
    };
    
    Arc::new(VectorService::new(config).await.expect("Failed to create VectorService"))
}

// Helper function to generate test vectors
fn generate_test_vectors(count: usize, dimensions: usize) -> Vec<Vec<f32>> {
    (0..count)
        .map(|i| {
            (0..dimensions)
                .map(|j| (i * dimensions + j) as f32 * 0.001)
                .collect()
        })
        .collect()
}

// Helper function to generate test metadata
fn generate_test_metadata(i: usize) -> HashMap<String, serde_json::Value> {
    let mut metadata = HashMap::new();
    metadata.insert("category".to_string(), serde_json::Value::String(format!("category_{}", i % 10)));
    metadata.insert("priority".to_string(), serde_json::Value::Number(serde_json::Number::from(i % 5)));
    metadata.insert("timestamp".to_string(), serde_json::Value::Number(serde_json::Number::from(1000000 + i)));
    metadata
}

fn bench_vector_insertion(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("vector_insertion");
    
    for batch_size in [1, 10, 100, 1000].iter() {
        group.throughput(Throughput::Elements(*batch_size as u64));
        
        group.bench_with_input(
            BenchmarkId::new("batch_insert", batch_size),
            batch_size,
            |b, &batch_size| {
                let vector_service = rt.block_on(create_test_vector_service());
                let collection_id = format!("bench_collection_{}", Uuid::new_v4());
                let vectors = generate_test_vectors(batch_size, 384); // BERT-like dimensions
                
                b.to_async(&rt).iter(|| async {
                    let requests: Vec<_> = vectors.iter().enumerate().map(|(i, vector)| {
                        proximadb::proto::proximadb::VectorInsertRequest {
                            collection_id: collection_id.clone(),
                            vector_id: format!("vector_{}", i),
                            vector: vector.clone(),
                            metadata: Some(serde_json::to_string(&generate_test_metadata(i)).unwrap()),
                            upsert: true,
                        }
                    }).collect();
                    
                    for request in requests {
                        let _ = vector_service.handle_vector_insert(request).await;
                    }
                });
            },
        );
    }
    
    group.finish();
}

fn bench_vector_search(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("vector_search");
    
    // Pre-populate data for search benchmarks
    let vector_service = rt.block_on(create_test_vector_service());
    let collection_id = format!("search_bench_collection_{}", Uuid::new_v4());
    let vectors = generate_test_vectors(1000, 384);
    
    // Insert test vectors
    rt.block_on(async {
        for (i, vector) in vectors.iter().enumerate() {
            let request = proximadb::proto::proximadb::VectorInsertRequest {
                collection_id: collection_id.clone(),
                vector_id: format!("vector_{}", i),
                vector: vector.clone(),
                metadata: Some(serde_json::to_string(&generate_test_metadata(i)).unwrap()),
                upsert: true,
            };
            let _ = vector_service.handle_vector_insert(request).await;
        }
    });
    
    for k in [1, 10, 100].iter() {
        group.throughput(Throughput::Elements(1));
        
        group.bench_with_input(
            BenchmarkId::new("similarity_search", k),
            k,
            |b, &k| {
                let query_vector = generate_test_vectors(1, 384)[0].clone();
                
                b.to_async(&rt).iter(|| async {
                    let request = proximadb::proto::proximadb::VectorSearchRequest {
                        collection_id: collection_id.clone(),
                        vector: query_vector.clone(),
                        k: k as u32,
                        metadata_filters: None,
                        include_vectors: false,
                        include_metadata: true,
                    };
                    
                    let _ = black_box(vector_service.search_vectors_polymorphic(request).await);
                });
            },
        );
    }
    
    group.finish();
}

fn bench_memory_operations(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("memory_operations");
    
    // Benchmark coordinator cloning
    group.bench_function("coordinator_creation", |b| {
        let vector_service = rt.block_on(create_test_vector_service());
        let collection_id = format!("coordinator_bench_{}", Uuid::new_v4());
        
        b.to_async(&rt).iter(|| async {
            let _ = black_box(
                vector_service
                    .get_or_create_coordinator(&collection_id, "VIPER")
                    .await
            );
        });
    });
    
    // Benchmark metrics retrieval
    group.bench_function("metrics_retrieval", |b| {
        let vector_service = rt.block_on(create_test_vector_service());
        
        b.to_async(&rt).iter(|| async {
            let _ = black_box(vector_service.get_metrics().await);
        });
    });
    
    group.finish();
}

fn bench_metadata_processing(c: &mut Criterion) {
    let mut group = c.benchmark_group("metadata_processing");
    
    for metadata_size in [1, 10, 100].iter() {
        group.throughput(Throughput::Elements(*metadata_size as u64));
        
        group.bench_with_input(
            BenchmarkId::new("metadata_clone", metadata_size),
            metadata_size,
            |b, &metadata_size| {
                let metadata = (0..metadata_size)
                    .map(|i| (format!("key_{}", i), serde_json::Value::String(format!("value_{}", i))))
                    .collect::<HashMap<String, serde_json::Value>>();
                
                b.iter(|| {
                    let _cloned: HashMap<String, serde_json::Value> = black_box(
                        metadata.iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect()
                    );
                });
            },
        );
    }
    
    group.finish();
}

fn bench_search_result_aggregation(c: &mut Criterion) {
    let mut group = c.benchmark_group("search_result_aggregation");
    
    for result_count in [10, 100, 1000].iter() {
        group.throughput(Throughput::Elements(*result_count as u64));
        
        group.bench_with_input(
            BenchmarkId::new("result_cloning", result_count),
            result_count,
            |b, &result_count| {
                let results: Vec<proximadb::core::SearchResult> = (0..result_count)
                    .map(|i| proximadb::core::SearchResult {
                        id: format!("result_{}", i),
                        vector_id: Some(format!("vector_{}", i)),
                        score: i as f32 * 0.001,
                        vector: Some(generate_test_vectors(1, 384)[0].clone()),
                        metadata: Some(generate_test_metadata(i)),
                    })
                    .collect();
                
                b.iter(|| {
                    let _cloned_results = black_box(results.clone());
                });
            },
        );
    }
    
    group.finish();
}

fn bench_concurrent_operations(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("concurrent_operations");
    
    for concurrency in [1, 4, 8, 16].iter() {
        group.bench_with_input(
            BenchmarkId::new("concurrent_inserts", concurrency),
            concurrency,
            |b, &concurrency| {
                let vector_service = rt.block_on(create_test_vector_service());
                
                b.to_async(&rt).iter(|| async {
                    let tasks: Vec<_> = (0..concurrency).map(|i| {
                        let vector_service = vector_service.clone();
                        let collection_id = format!("concurrent_collection_{}", i);
                        let vector = generate_test_vectors(1, 384)[0].clone();
                        
                        tokio::spawn(async move {
                            let request = proximadb::proto::proximadb::VectorInsertRequest {
                                collection_id,
                                vector_id: format!("vector_{}", i),
                                vector,
                                metadata: Some(serde_json::to_string(&generate_test_metadata(i)).unwrap()),
                                upsert: true,
                            };
                            
                            vector_service.handle_vector_insert(request).await
                        })
                    }).collect();
                    
                    let _results = futures::future::join_all(tasks).await;
                });
            },
        );
    }
    
    group.finish();
}

fn bench_memory_allocation_patterns(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_allocation_patterns");
    
    // Benchmark different allocation strategies
    group.bench_function("vec_with_capacity", |b| {
        b.iter(|| {
            let mut vec = Vec::with_capacity(1000);
            for i in 0..1000 {
                vec.push(black_box(i));
            }
            vec
        });
    });
    
    group.bench_function("vec_without_capacity", |b| {
        b.iter(|| {
            let mut vec = Vec::new();
            for i in 0..1000 {
                vec.push(black_box(i));
            }
            vec
        });
    });
    
    group.bench_function("hashmap_with_capacity", |b| {
        b.iter(|| {
            let mut map = HashMap::with_capacity(1000);
            for i in 0..1000 {
                map.insert(black_box(i), black_box(format!("value_{}", i)));
            }
            map
        });
    });
    
    group.bench_function("hashmap_without_capacity", |b| {
        b.iter(|| {
            let mut map = HashMap::new();
            for i in 0..1000 {
                map.insert(black_box(i), black_box(format!("value_{}", i)));
            }
            map
        });
    });
    
    group.finish();
}

fn bench_async_vs_sync_operations(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let mut group = c.benchmark_group("async_vs_sync");
    
    // Benchmark async overhead
    group.bench_function("sync_computation", |b| {
        b.iter(|| {
            let mut sum = 0u64;
            for i in 0..1000 {
                sum += black_box(i);
            }
            sum
        });
    });
    
    group.bench_function("async_computation", |b| {
        b.to_async(&rt).iter(|| async {
            let mut sum = 0u64;
            for i in 0..1000 {
                sum += black_box(i);
                // Yield occasionally to simulate async operations
                if i % 100 == 0 {
                    tokio::task::yield_now().await;
                }
            }
            sum
        });
    });
    
    group.finish();
}

criterion_group!(
    benches,
    bench_vector_insertion,
    bench_vector_search,
    bench_memory_operations,
    bench_metadata_processing,
    bench_search_result_aggregation,
    bench_concurrent_operations,
    bench_memory_allocation_patterns,
    bench_async_vs_sync_operations
);
criterion_main!(benches);