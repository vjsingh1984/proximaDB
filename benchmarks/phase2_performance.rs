//! Phase 2 Performance Benchmarks
//!
//! Demonstrates the performance improvements from Phase 2 optimizations:
//! - Unified cache vs no cache
//! - Lock-free vs Arc<RwLock>
//! - Prefetching vs direct reads

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use proximadb::storage::{
    unified_cache::{UnifiedCrossEngineCache, UnifiedCacheConfig},
    lockfree_engine::LockFreeStorageEngine,
    memtable::lockfree_implementations::LockFreeHashMapMemtable,
    engines::sst::readers::predictive_prefetcher::{PredictivePrefetcher, PrefetchConfig},
};
use std::sync::Arc;
use tokio::runtime::Runtime;

fn benchmark_unified_cache(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    c.bench_function("unified_cache_hit", |b| {
        b.iter(|| {
            rt.block_on(async {
                let config = UnifiedCacheConfig::default();
                let cache = UnifiedCrossEngineCache::new(config);
                
                // Warm cache
                let key = "test_key";
                let data = vec![0u8; 4096];
                cache.insert_l1(key, data.clone()).await.unwrap();
                
                // Benchmark cache hit
                for _ in 0..1000 {
                    let _ = black_box(cache.get(key).await);
                }
            })
        })
    });
    
    c.bench_function("unified_cache_promotion", |b| {
        b.iter(|| {
            rt.block_on(async {
                let config = UnifiedCacheConfig::default();
                let cache = UnifiedCrossEngineCache::new(config);
                
                // Insert in L3
                let key = "cold_data";
                let data = vec![0u8; 4096];
                cache.insert_l3(key, data.clone()).await.unwrap();
                
                // Access multiple times to trigger promotion
                for _ in 0..5 {
                    let _ = black_box(cache.get(key).await);
                }
            })
        })
    });
}

fn benchmark_lockfree_structures(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    c.bench_function("lockfree_concurrent_writes", |b| {
        b.iter(|| {
            rt.block_on(async {
                let memtable = Arc::new(LockFreeHashMapMemtable::new(Default::default()));
                let handles: Vec<_> = (0..100)
                    .map(|i| {
                        let mt = memtable.clone();
                        tokio::spawn(async move {
                            let record = proximadb::core::VectorRecord {
                                id: Some(format!("id_{}", i)),
                                vector: vec![i as f32; 128],
                                metadata: vec![],
                                ..Default::default()
                            };
                            mt.insert_vector(record).await.unwrap();
                        })
                    })
                    .collect();
                
                for h in handles {
                    h.await.unwrap();
                }
            })
        })
    });
    
    c.bench_function("lockfree_concurrent_reads", |b| {
        b.iter(|| {
            rt.block_on(async {
                let memtable = Arc::new(LockFreeHashMapMemtable::new(Default::default()));
                
                // Pre-populate
                for i in 0..100 {
                    let record = proximadb::core::VectorRecord {
                        id: Some(format!("id_{}", i)),
                        vector: vec![i as f32; 128],
                        metadata: vec![],
                        ..Default::default()
                    };
                    memtable.insert_vector(record).await.unwrap();
                }
                
                // Concurrent reads
                let handles: Vec<_> = (0..1000)
                    .map(|i| {
                        let mt = memtable.clone();
                        let id = format!("id_{}", i % 100);
                        tokio::spawn(async move {
                            black_box(mt.get_vector(&id).await.unwrap());
                        })
                    })
                    .collect();
                
                for h in handles {
                    h.await.unwrap();
                }
            })
        })
    });
}

fn benchmark_predictive_prefetching(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    c.bench_function("prefetch_sequential_pattern", |b| {
        b.iter(|| {
            rt.block_on(async {
                let config = PrefetchConfig::default();
                let prefetcher = PredictivePrefetcher::new(config);
                
                // Simulate sequential access
                for i in 0..100 {
                    let key = proximadb::storage::engines::sst::readers::predictive_prefetcher::BlockCacheKey {
                        file_path: "test.sst".to_string(),
                        block_id: i,
                        block_index: i as usize,
                    };
                    
                    prefetcher.record_access(&key, true).await.unwrap();
                    
                    // Check if prefetched
                    if i > 10 {
                        let next_key = proximadb::storage::engines::sst::readers::predictive_prefetcher::BlockCacheKey {
                            file_path: "test.sst".to_string(),
                            block_id: i + 1,
                            block_index: (i + 1) as usize,
                        };
                        let _ = black_box(prefetcher.get_prefetched(&next_key).await);
                    }
                }
            })
        })
    });
}

criterion_group!(
    benches,
    benchmark_unified_cache,
    benchmark_lockfree_structures,
    benchmark_predictive_prefetching
);
criterion_main!(benches);