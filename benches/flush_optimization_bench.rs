/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Benchmarks for flush result optimization

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use proximadb::core::VectorRecord;
use proximadb::storage::persistence::write_buffer::flush_result_optimization::{
    OptimizedFlushCoordinator, BatchFlushProcessor, VectorMemoryPool,
};
use proximadb::storage::persistence::write_buffer::enhanced_flush_result::EnhancedFlushResult;
use proximadb::storage::traits::FlushResult;
use std::sync::Arc;
use tokio::runtime::Runtime;

fn create_test_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
    let now = chrono::Utc::now().timestamp_micros();
    (0..count)
        .map(|i| VectorRecord {
            id: Some(format!("vec_{}", i)),
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
        }", i)),
            vector: vec![i as f32; dimension],
            metadata: vec![],
            timestamp: now as u32,
            created_at: now,
            updated_at: Some(now as u32),
            expires_at: None,
            version: Some(0),
            rank: None,
            score: None,
            distance: None,
        })
        .collect()
}

fn benchmark_standard_flush(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("standard_flush");
    
    for count in [100, 1000, 10000].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(count),
            count,
            |b, &count| {
                let vectors = create_test_vectors(count, 128);
                
                b.iter(|| {
                    let vectors_clone = vectors.clone();
                    rt.block_on(async {
                        // Simulate standard flush
                        let base_result = FlushResult {
                            success: true,
                            collections_affected: vec!["test".to_string()],
                            entries_flushed: vectors_clone.len() as u64,
                            bytes_written: vectors_clone.len() as u64 * 512,
                            files_created: 1,
                            duration_ms: 0,
                            completed_at: chrono::Utc::now(),
                            engine_metrics: Default::default(),
                            compaction_triggered: false,
                            flushed_batch_ids: vec![],
                        };
                        
                        EnhancedFlushResult::new(base_result, vectors_clone)
                    })
                });
            },
        );
    }
    
    group.finish();
}

fn benchmark_optimized_flush(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("optimized_flush");
    
    for count in [100, 1000, 10000].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(count),
            count,
            |b, &count| {
                let vectors = create_test_vectors(count, 128);
                let coordinator = Arc::new(OptimizedFlushCoordinator::new(
                    100,  // batch_size
                    4,    // worker_count
                    128,  // dimension
                ));
                
                b.iter(|| {
                    let vectors_clone = vectors.clone();
                    let coord = coordinator.clone();
                    rt.block_on(async move {
                        coord.execute_optimized_flush("test", vectors_clone).await.unwrap()
                    })
                });
            },
        );
    }
    
    group.finish();
}

fn benchmark_memory_pool(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("memory_pool");
    
    group.bench_function("with_pool", |b| {
        let pool = Arc::new(VectorMemoryPool::new(100, 128));
        
        b.iter(|| {
            let pool_clone = pool.clone();
            rt.block_on(async move {
                let mut buffers = Vec::new();
                for _ in 0..10 {
                    buffers.push(pool_clone.acquire().await);
                }
                for buffer in buffers {
                    pool_clone.release(buffer).await;
                }
            })
        });
    });
    
    group.bench_function("without_pool", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut buffers = Vec::new();
                for _ in 0..10 {
                    buffers.push(vec![0.0f32; 128]);
                }
                black_box(buffers);
            })
        });
    });
    
    group.finish();
}

fn benchmark_batch_processing(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    
    let mut group = c.benchmark_group("batch_processing");
    
    for worker_count in [1, 2, 4, 8].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(worker_count),
            worker_count,
            |b, &worker_count| {
                let vectors = create_test_vectors(1000, 128);
                let processor = BatchFlushProcessor::new(100, worker_count, 128);
                
                b.iter(|| {
                    let vectors_clone = vectors.clone();
                    let proc = &processor;
                    rt.block_on(async move {
                        proc.process_batch(vectors_clone).await.unwrap()
                    })
                });
            },
        );
    }
    
    group.finish();
}

criterion_group!(
    benches,
    benchmark_standard_flush,
    benchmark_optimized_flush,
    benchmark_memory_pool,
    benchmark_batch_processing
);
criterion_main!(benches);