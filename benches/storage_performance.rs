// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Storage layer performance benchmarks

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId, Throughput};
use std::time::Duration;
use tokio::runtime::Runtime;

use super::{BenchmarkConfig, BenchmarkDataGenerator, BenchmarkRunner};
use proximadb::core::{VectorRecord, Config};
use proximadb::storage::StorageEngine;

/// Benchmark WAL write performance
pub fn benchmark_wal_performance(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());
    
    let mut group = c.benchmark_group("wal_performance");
    group.measurement_time(Duration::from_secs(30));
    
    for &count in &[1_000, 10_000, 50_000] {
        let dimension = 256;
        group.throughput(Throughput::Elements(count as u64));
        
        group.bench_with_input(
            BenchmarkId::new("wal_append", format!("{}x{}", count, dimension)),
            &(count, dimension),
            |b, &(count, dimension)| {
                let vectors = generator.generate_vectors(count, dimension, 0.1, "wal_bench");
                
                b.to_async(&runner.runtime).iter(|| async {
                    let storage = runner.create_test_storage().await;
                    let mut storage_guard = storage.write().await;
                    
                    for vector in &vectors {
                        let _ = storage_guard.insert_vector(black_box(vector.clone())).await;
                    }
                });
            },
        );
    }
    
    group.finish();
}

/// Benchmark memtable performance
pub fn benchmark_memtable_operations(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());
    
    let mut group = c.benchmark_group("memtable_operations");
    group.measurement_time(Duration::from_secs(20));
    
    let count = 10_000;
    let dimension = 256;
    
    // Insertion benchmark
    group.throughput(Throughput::Elements(count as u64));
    group.bench_function("memtable_insert", |b| {
        let vectors = generator.generate_vectors(count, dimension, 0.1, "memtable_bench");
        
        b.to_async(&runner.runtime).iter(|| async {
            let storage = runner.create_test_storage().await;
            let mut storage_guard = storage.write().await;
            
            for vector in &vectors {
                let _ = storage_guard.insert_vector(black_box(vector.clone())).await;
            }
        });
    });
    
    // Retrieval benchmark
    group.bench_function("memtable_get", |b| {
        let vectors = generator.generate_vectors(count, dimension, 0.1, "memtable_bench");
        
        b.to_async(&runner.runtime).iter(|| async {
            let storage = runner.create_test_storage().await;
            
            // Insert vectors first
            {
                let mut storage_guard = storage.write().await;
                for vector in &vectors {
                    let _ = storage_guard.insert_vector(vector.clone()).await;
                }
            }
            
            // Benchmark retrieval
            {
                let storage_guard = storage.read().await;
                for vector in &vectors {
                    let _ = storage_guard.get_vector(black_box(&vector.id)).await;
                }
            }
        });
    });
    
    group.finish();
}

/// Benchmark LSM tree operations
pub fn benchmark_lsm_operations(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());
    
    let mut group = c.benchmark_group("lsm_operations");
    group.measurement_time(Duration::from_secs(30));
    
    let count = 50_000;
    let dimension = 256;
    
    group.throughput(Throughput::Elements(count as u64));
    
    // Write-heavy workload
    group.bench_function("lsm_write_heavy", |b| {
        let vectors = generator.generate_vectors(count, dimension, 0.1, "lsm_bench");
        
        b.to_async(&runner.runtime).iter(|| async {
            let storage = runner.create_test_storage().await;
            let mut storage_guard = storage.write().await;
            
            // Simulate write-heavy workload
            for vector in &vectors {
                let _ = storage_guard.insert_vector(black_box(vector.clone())).await;
                
                // Occasional reads to simulate mixed workload
                if vectors.len() % 10 == 0 {
                    let _ = storage_guard.get_vector(&vector.id).await;
                }
            }
        });
    });
    
    // Read-heavy workload
    group.bench_function("lsm_read_heavy", |b| {
        let vectors = generator.generate_vectors(count / 10, dimension, 0.1, "lsm_bench");
        
        b.to_async(&runner.runtime).iter(|| async {
            let storage = runner.create_test_storage().await;
            
            // Insert base dataset
            {
                let mut storage_guard = storage.write().await;
                for vector in &vectors {
                    let _ = storage_guard.insert_vector(vector.clone()).await;
                }
            }
            
            // Simulate read-heavy workload (90% reads, 10% writes)
            {
                let storage_guard = storage.read().await;
                for _ in 0..9 {
                    for vector in &vectors {
                        let _ = storage_guard.get_vector(black_box(&vector.id)).await;
                    }
                }
            }
        });
    });
    
    group.finish();
}

/// Benchmark storage tiering operations
pub fn benchmark_storage_tiering(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());
    
    let mut group = c.benchmark_group("storage_tiering");
    group.measurement_time(Duration::from_secs(25));
    
    let count = 5_000;
    let dimension = 512;
    
    // Hot tier access patterns
    group.throughput(Throughput::Elements(count as u64));
    group.bench_function("hot_tier_access", |b| {
        let vectors = generator.generate_vectors(count, dimension, 0.1, "tier_bench");
        
        b.to_async(&runner.runtime).iter(|| async {
            let storage = runner.create_test_storage().await;
            
            // Insert and immediately access (hot tier)
            {
                let mut storage_guard = storage.write().await;
                for vector in &vectors {
                    let _ = storage_guard.insert_vector(vector.clone()).await;
                    let _ = storage_guard.get_vector(black_box(&vector.id)).await;
                }
            }
        });
    });
    
    // Cold tier simulation
    group.bench_function("cold_tier_simulation", |b| {
        let vectors = generator.generate_vectors(count, dimension, 0.1, "tier_bench");
        
        b.to_async(&runner.runtime).iter(|| async {
            let storage = runner.create_test_storage().await;
            
            // Insert vectors and let them "cool down"
            {
                let mut storage_guard = storage.write().await;
                for vector in &vectors {
                    let _ = storage_guard.insert_vector(vector.clone()).await;
                }
            }
            
            // Simulate cold access after delay
            tokio::time::sleep(Duration::from_millis(10)).await;
            
            {
                let storage_guard = storage.read().await;
                for vector in &vectors {
                    let _ = storage_guard.get_vector(black_box(&vector.id)).await;
                }
            }
        });
    });
    
    group.finish();
}

/// Benchmark compression performance
pub fn benchmark_compression_performance(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());
    
    let mut group = c.benchmark_group("compression_performance");
    group.measurement_time(Duration::from_secs(20));
    
    let count = 10_000;
    let dimension = 1024; // Larger vectors for compression testing
    
    // Test different sparsity levels for compression effectiveness
    for &sparsity in &[0.0, 0.1, 0.5, 0.9] {
        group.throughput(Throughput::Elements(count as u64));
        
        group.bench_with_input(
            BenchmarkId::new("compression_write", format!("sparsity_{:.1}", sparsity)),
            &sparsity,
            |b, &sparsity| {
                let vectors = generator.generate_vectors(count, dimension, sparsity, "compression_bench");
                
                b.to_async(&runner.runtime).iter(|| async {
                    let storage = runner.create_test_storage().await;
                    let mut storage_guard = storage.write().await;
                    
                    for vector in &vectors {
                        let _ = storage_guard.insert_vector(black_box(vector.clone())).await;
                    }
                });
            },
        );
    }
    
    group.finish();
}

/// Benchmark concurrent storage operations
pub fn benchmark_concurrent_operations(c: &mut Criterion) {
    let config = BenchmarkConfig::default();
    let generator = BenchmarkDataGenerator::new(config.clone());
    let runner = BenchmarkRunner::new(config.clone());
    
    let mut group = c.benchmark_group("concurrent_operations");
    group.measurement_time(Duration::from_secs(30));
    
    let count = 1_000;
    let dimension = 256;
    let concurrent_tasks = vec![2, 4, 8, 16];
    
    for &tasks in &concurrent_tasks {
        group.throughput(Throughput::Elements((count * tasks) as u64));
        
        group.bench_with_input(
            BenchmarkId::new("concurrent_insert", format!("{}tasks", tasks)),
            &(count, tasks),
            |b, &(count, tasks)| {
                let vectors_per_task = count / tasks;
                
                b.to_async(&runner.runtime).iter(|| async {
                    let storage = runner.create_test_storage().await;
                    
                    let mut handles = Vec::new();
                    
                    for task_id in 0..tasks {
                        let vectors = generator.generate_vectors(
                            vectors_per_task,
                            dimension,
                            0.1,
                            &format!("concurrent_bench_{}", task_id)
                        );
                        let storage_clone = storage.clone();
                        
                        let handle = tokio::spawn(async move {
                            let mut storage_guard = storage_clone.write().await;
                            for vector in vectors {
                                let _ = storage_guard.insert_vector(vector).await;
                            }
                        });
                        
                        handles.push(handle);
                    }
                    
                    // Wait for all tasks to complete
                    for handle in handles {
                        let _ = handle.await;
                    }
                });
            },
        );
    }
    
    group.finish();
}

criterion_group!(
    storage_benches,
    benchmark_wal_performance,
    benchmark_memtable_operations,
    benchmark_lsm_operations,
    benchmark_storage_tiering,
    benchmark_compression_performance,
    benchmark_concurrent_operations
);

criterion_main!(storage_benches);