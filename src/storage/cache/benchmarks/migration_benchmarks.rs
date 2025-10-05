//! Performance benchmarks for cache migration from unified to specialized

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use std::sync::Arc;
use std::collections::HashMap;
use tokio::runtime::Runtime;

use proximadb::storage::cache::{
    VectorStore, MetadataStore, UnifiedCacheAdapter,
    BaseCache,
};
use proximadb::storage::legacy_cache::{
    LegacyCrossEngineCache, LegacyCacheConfig, CacheKey as OldCacheKey,
};
use proximadb::proto::proximadb::VectorRecord;

fn setup_caches() -> (Arc<LegacyCrossEngineCache>, Arc<VectorStore>, Arc<MetadataStore>) {
    let config = LegacyCacheConfig {
        l1_memory_mb: 256,
        l2_nvme_gb: 1,
        l3_network_enabled: false,
        cross_engine_sharing: true,
        promotion_threshold: 3,
        eviction_policy: proximadb::storage::legacy_cache::EvictionPolicy::LRU,
    };
    
    let legacy_cache = Arc::new(LegacyCrossEngineCache::new(config));
    let vector_cache = Arc::new(VectorStore::new(128 * 1024 * 1024)); // 128MB
    let metadata_cache = Arc::new(MetadataStore::new(64 * 1024 * 1024)); // 64MB
    
    (legacy_cache, vector_cache, metadata_cache)
}

fn benchmark_legacy_cache_operations(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (legacy_cache, _, _) = setup_caches();
    
    let mut group = c.benchmark_group("legacy_cache_info");
    
    // Benchmark put operations
    group.bench_function("put_vector", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = OldCacheKey {
                    engine: "sst".to_string(),
                    collection_id: "bench_collection".to_string(),
                    // data_type removed -  proximadb::storage::legacy_cache::CacheData::Vector,
                    item_id: "vec1".to_string(),
                };
                
                let vector = VectorRecord {
                    id: Some("vec1".to_string()),
                    vector: vec![1.0; 128],
                    metadata: HashMap::new(),
                    collection_id: "bench_collection".to_string(),
                    version: 1,
                    timestamp: Some(0),
                };
                
                legacy_cache.put_vector(&key, vector).await.unwrap();
            });
        });
    });
    
    // Benchmark get operations
    group.bench_function("get_vector", |b| {
        let key = OldCacheKey {
            engine: "sst".to_string(),
            collection_id: "bench_collection".to_string(),
            // data_type removed -  proximadb::storage::legacy_cache::CacheData::Vector,
            item_id: "vec1".to_string(),
        };
        
        b.iter(|| {
            rt.block_on(async {
                let _ = legacy_cache.vector(&black_box(item.clone())).await;
            });
        });
    });
    
    group.finish();
}

fn benchmark_specialized_cache_operations(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (_, vector_cache, _) = setup_caches();
    
    let mut group = c.benchmark_group("specialized_cache_info");
    
    // Benchmark put operations
    group.bench_function("put_vector", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = "bench_collection_sst_vec1";
                let vector = VectorRecord {
                    id: Some("vec1".to_string()),
                    vector: vec![1.0; 128],
                    metadata: HashMap::new(),
                    collection_id: "bench_collection".to_string(),
                    version: 1,
                    timestamp: Some(0),
                };
                
                vector_cache.put(&key.to_string(), vector).await.unwrap();
            });
        });
    });
    
    // Benchmark get operations
    group.bench_function("get_vector", |b| {
        let key = "bench_collection_sst_vec1";
        
        b.iter(|| {
            rt.block_on(async {
                let _ = vector_cache.get(&key).await;
            });
        });
    });
    
    // Benchmark batch operations
    group.bench_function("batch_get", |b| {
        let keys: Vec<String> = (0..10)
            .map(|i| format!("bench_collection_sst_vec{}", i))
            .collect();
        
        b.iter(|| {
            rt.block_on(async {
                let _ = vector_cache.batch_get(&black_box(keys.clone())).await;
            });
        });
    });
    
    group.finish();
}

fn benchmark_adapter_migration(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("migration");
    
    for size in [100, 1000, 10000].iter() {
        group.bench_with_input(BenchmarkId::from_parameter(size), size, |b, &size| {
            b.iter(|| {
                rt.block_on(async {
                    let (legacy_cache, vector_cache, metadata_cache) = setup_caches();
                    
                    // Pre-populate legacy cache
                    for i in 0..size {
                        let key = OldCacheKey {
                            engine: "sst".to_string(),
                            collection_id: "bench_collection".to_string(),
                            // data_type removed -  proximadb::storage::legacy_cache::CacheData::Vector,
                            item_id: format!("vec{}", i),
                        };
                        
                        let vector = VectorRecord {
                            id: Some(format!("vec{}", i)),
                            vector: vec![i as f32; 128],
                            metadata: HashMap::new(),
                            collection_id: "bench_collection".to_string(),
                            version: 1,
                            timestamp: Some(0),
                        };
                        
                        legacy_cache.put_vector(&key, vector).await.unwrap();
                    }
                    
                    // Create adapter and migrate
                    let adapter = UnifiedCacheAdapter::new(
                        legacy_cache,
                        vector_cache,
                        metadata_cache,
                    );
                    
                    adapter.migrate_data().await.unwrap();
                });
            });
        });
    }
    
    group.finish();
}

fn benchmark_adapter_operations(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (legacy_cache, vector_cache, metadata_cache) = setup_caches();
    
    let adapter = UnifiedCacheAdapter::new(
        legacy_cache,
        vector_cache,
        metadata_cache,
    );
    
    let mut group = c.benchmark_group("adapter");
    
    // Benchmark adapter put operations
    group.bench_function("put_through_adapter", |b| {
        b.iter(|| {
            rt.block_on(async {
                let key = "bench_collection_sst_vec1";
                let vector = VectorRecord {
                    id: Some("vec1".to_string()),
                    vector: vec![1.0; 128],
                    metadata: HashMap::new(),
                    collection_id: "bench_collection".to_string(),
                    version: 1,
                    timestamp: Some(0),
                };
                
                adapter.put(&key.to_string(), vector).await.unwrap();
            });
        });
    });
    
    // Benchmark adapter get operations
    group.bench_function("get_through_adapter", |b| {
        let key = "bench_collection_sst_vec1";
        
        b.iter(|| {
            rt.block_on(async {
                let _ = adapter.get(key))).await;
            });
        });
    });
    
    group.finish();
}

criterion_group!(
    benches,
    benchmark_legacy_cache_operations,
    benchmark_specialized_cache_operations,
    benchmark_adapter_migration,
    benchmark_adapter_operations
);

criterion_main!(benches);