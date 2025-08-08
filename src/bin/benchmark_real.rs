//! Real performance benchmark for ProximaDB

use proximadb::compute::distance_computation::{DistanceMetric};
use proximadb::compute::distance_computation::core::{detect_platform_capability, create_distance_calculator};
use proximadb::index::axis::{AxisIvfIndex, AxisIvfConfig, AxisLshIndex, AxisLshConfig};
use proximadb::core::VectorRecord;
use std::sync::Arc;
use std::time::Instant;
use std::collections::HashMap;

fn benchmark_distance_metrics() -> HashMap<(usize, DistanceMetric), f64> {
    println!("\n=== Distance Computation Benchmarks ===");
    println!("Platform: {}", detect_platform_capability());
    
    let dimensions = vec![128, 256, 512, 1024, 2048];
    let iterations = 100_000;
    
    let mut results = HashMap::new();
    
    for dim in dimensions {
        println!("\nDimension: {}", dim);
        
        let a: Vec<f32> = (0..dim).map(|i| (i as f32).sin()).collect();
        let b: Vec<f32> = (0..dim).map(|i| (i as f32).cos()).collect();
        
        for metric in [DistanceMetric::Cosine, DistanceMetric::Euclidean, DistanceMetric::DotProduct] {
            let calc = create_distance_calculator(metric);
            
            // Warmup
            for _ in 0..1000 {
                let _ = calc.distance(&a, &b);
            }
            
            // Benchmark
            let start = Instant::now();
            for _ in 0..iterations {
                let _ = calc.distance(&a, &b);
            }
            let elapsed = start.elapsed();
            
            let ops_per_sec = iterations as f64 / elapsed.as_secs_f64();
            results.insert((dim, metric), ops_per_sec);
            
            println!("  {:?}: {:.0} ops/sec ({:.2} ns/op)", 
                     metric, ops_per_sec, 
                     elapsed.as_nanos() as f64 / iterations as f64);
        }
    }
    
    results
}

async fn benchmark_indexing() {
    println!("\n=== Indexing Algorithm Benchmarks ===");
    
    let dataset_sizes = vec![10_000, 50_000, 100_000];
    let dimension = 128;
    
    for size in dataset_sizes {
        println!("\nDataset size: {}", size);
        
        // Generate random data
        let vectors: Vec<Vec<f32>> = (0..size)
            .map(|i| (0..dimension).map(|j| ((i * j) as f32).sin()).collect())
            .collect();
        
        // AXIS IVF benchmark
        {
            let num_clusters = ((size as f64).sqrt() as usize).max(10);
            
            let config = AxisIvfConfig {
                n_clusters: num_clusters,
                n_probe: 5,
                train_size: 0, // Auto-calculate
                max_iterations: 20,
                distance_metric: DistanceMetric::Cosine,
                enable_pq: false,
                pq_subquantizers: 8,
            };
            
            let start = Instant::now();
            let mut index = AxisIvfIndex::new(config, dimension);
            
            // Train the index
            if let Err(e) = index.train(&vectors[..1000.min(size)]).await {
                println!("  IVF training failed: {}", e);
                continue;
            }
            
            // Add vectors
            for (i, vec) in vectors.iter().enumerate() {
                let record = VectorRecord {
                    id: Some(format!("vec_{}", i)),
                    vector: vec.clone(),
                    metadata: vec![],
                    timestamp: 0,
                    updated_at: Some(0),
                    expires_at: Some(0),
                    version: Some(1),
                    rank: Some(0),
                    score: Some(0.0),
                    distance: Some(0.0),
                };
                if let Err(e) = index.add(format!("vec_{}", i), Arc::new(record)).await {
                    println!("  IVF add failed: {}", e);
                    break;
                }
            }
            let build_time = start.elapsed();
            
            // Search benchmark
            let query = &vectors[0];
            let search_iterations = 1000;
            let start = Instant::now();
            for _ in 0..search_iterations {
                let _ = index.search(query, 10, None).await;
            }
            let search_time = start.elapsed();
            let qps = search_iterations as f64 / search_time.as_secs_f64();
            
            println!("  AXIS IVF: build={:.2}s, QPS={:.0}, clusters={}", 
                     build_time.as_secs_f64(), qps, num_clusters);
        }
        
        // AXIS LSH benchmark
        {
            let config = AxisLshConfig {
                n_tables: 10,
                n_hashes: 8,
                hash_width: 1.0,
                seed: 42,
                distance_metric: DistanceMetric::Cosine,
                binary_mode: false,
            };
            
            let start = Instant::now();
            let index = AxisLshIndex::new(config, dimension);
            
            // Add vectors
            for (i, vec) in vectors.iter().enumerate() {
                let record = VectorRecord {
                    id: Some(format!("vec_{}", i)),
                    vector: vec.clone(),
                    metadata: vec![],
                    timestamp: 0,
                    updated_at: Some(0),
                    expires_at: Some(0),
                    version: Some(1),
                    rank: Some(0),
                    score: Some(0.0),
                    distance: Some(0.0),
                };
                if let Err(e) = index.add(format!("vec_{}", i), Arc::new(record)).await {
                    println!("  LSH add failed: {}", e);
                    break;
                }
            }
            let build_time = start.elapsed();
            
            // Search benchmark
            let query = &vectors[0];
            let search_iterations = 1000;
            let start = Instant::now();
            for _ in 0..search_iterations {
                let _ = index.search(query, 10, None).await;
            }
            let search_time = start.elapsed();
            let qps = search_iterations as f64 / search_time.as_secs_f64();
            
            println!("  LSH: build={:.2}s, QPS={:.0}", build_time.as_secs_f64(), qps);
        }
    }
}

fn benchmark_concurrent_operations() {
    println!("\n=== Concurrent Operation Benchmarks ===");
    
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    
    let thread_counts = vec![1, 10, 50, 100];
    let operations_per_thread = 10_000;
    
    for num_threads in thread_counts {
        let counter = Arc::new(AtomicU64::new(0));
        let start = Instant::now();
        
        let mut handles = vec![];
        
        for _ in 0..num_threads {
            let counter_clone = counter.clone();
            let handle = thread::spawn(move || {
                let calc = create_distance_calculator(DistanceMetric::Cosine);
                let a: Vec<f32> = (0..128).map(|i| (i as f32).sin()).collect();
                let b: Vec<f32> = (0..128).map(|i| (i as f32).cos()).collect();
                
                for _ in 0..operations_per_thread {
                    let _ = calc.distance(&a, &b);
                    counter_clone.fetch_add(1, Ordering::Relaxed);
                }
            });
            handles.push(handle);
        }
        
        for handle in handles {
            handle.join().unwrap();
        }
        
        let elapsed = start.elapsed();
        let total_ops = counter.load(Ordering::Relaxed);
        let ops_per_sec = total_ops as f64 / elapsed.as_secs_f64();
        
        println!("  {} threads: {:.0} ops/sec", num_threads, ops_per_sec);
    }
}

#[tokio::main]
async fn main() {
    println!("ProximaDB Real Performance Benchmarks");
    println!("=====================================");
    
    benchmark_distance_metrics();
    benchmark_indexing().await;
    benchmark_concurrent_operations();
    
    println!("\n✅ Benchmarks completed!");
}