//! Real performance benchmark for ProximaDB

use proximadb::compute::distance_computation::{DistanceMetric};
use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
use proximadb::index::axis::ivf_unified::{UnifiedIvfIndex, UnifiedIvfConfig, IvfClusteringMethod, CentroidConfig, PostingListConfig};
use proximadb::index::axis::lsh_index::{AxisLshIndex, AxisLshConfig};
use proximadb::core::VectorRecord;
use std::sync::Arc;
use std::time::Instant;
use std::collections::HashMap;
use tracing::{debug, error, info};

fn benchmark_distance_metrics() -> HashMap<(usize, DistanceMetric), f64> {
    debug!("\n=== Distance Computation Benchmarks ===");
    debug!("Platform: UnifiedDistanceCompute with automatic hardware detection");
    
    let dimensions = vec![128, 256, 512, 1024, 2048];
    let iterations = 100_000;
    
    let mut results = HashMap::new();
    
    for dim in dimensions {
        debug!("\nDimension: {}", dim);
        
        let a: Vec<f32> = (0..dim).map(|i| (i as f32).sin()).collect();
        let b: Vec<f32> = (0..dim).map(|i| (i as f32).cos()).collect();
        
        for metric in [DistanceMetric::Cosine, DistanceMetric::Euclidean, DistanceMetric::DotProduct] {
            let calc = UnifiedDistanceCompute::new(metric);
            
            // Warmup
            for _ in 0..1000 {
                let _ = calc.calculate_distance(&a, &b, &metric);
            }
            
            // Benchmark
            let start = Instant::now();
            for _ in 0..iterations {
                let _ = calc.calculate_distance(&a, &b, &metric);
            }
            let elapsed = start.elapsed();
            
            let ops_per_sec = iterations as f64 / elapsed.as_secs_f64();
            results.insert((dim, metric), ops_per_sec);
            
            debug!("  {:?}: {:.0} ops/sec ({:.2} ns/op)", 
                     metric, ops_per_sec, 
                     elapsed.as_nanos() as f64 / iterations as f64);
        }
    }
    
    results
}

async fn benchmark_indexing() {
    debug!("\n=== Indexing Algorithm Benchmarks ===");
    
    let dataset_sizes = vec![10_000, 50_000, 100_000];
    let dimension = 128;
    
    for size in dataset_sizes {
        debug!("\nDataset size: {}", size);
        
        // Generate random data
        let vectors: Vec<Vec<f32>> = (0..size)
            .map(|i| (0..dimension).map(|j| ((i * j) as f32).sin()).collect())
            .collect();
        
        // Unified IVF benchmark
        {
            let num_clusters = ((size as f64).sqrt() as usize).max(10);
            
            let config = UnifiedIvfConfig {
                n_clusters: num_clusters,
                n_probe: 5,
                dimension,
                distance_metric: DistanceMetric::Cosine,
                quantization_bits: 0,
                use_pq: false,
                pq_subspaces: 0,
                clustering_method: IvfClusteringMethod::KMeans,
                train_on_insert: false,
                min_train_size: 100,
                max_iterations: 20,
                tolerance: 0.01,
                n_init: 1,
                centroid_config: CentroidConfig::default(),
                posting_list_config: PostingListConfig::default(),
            };
            
            let start = Instant::now();
            let mut index = UnifiedIvfIndex::new("benchmark".to_string(), config).unwrap();
            
            // Train the index
            let training_data = vectors[..1000.min(size)].to_vec();
            index.train(training_data).await.unwrap();
            
            // Add vectors
            for (i, vec) in vectors.iter().enumerate() {
                index.add_vector(
                    format!("vec_{}", i), 
                    vec.clone(),
                    None  // No metadata for benchmark
                ).await.unwrap();
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
            
            debug!("  Unified IVF: build={:.2}s, QPS={:.0}, clusters={}", 
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
                    error!("  LSH add failed: {}", e);
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
            
            debug!("  LSH: build={:.2}s, QPS={:.0}", build_time.as_secs_f64(), qps);
        }
    }
}

fn benchmark_concurrent_operations() {
    debug!("\n=== Concurrent Operation Benchmarks ===");
    
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
                let metric = DistanceMetric::Cosine;
                let calc = UnifiedDistanceCompute::new(metric);
                let a: Vec<f32> = (0..128).map(|i| (i as f32).sin()).collect();
                let b: Vec<f32> = (0..128).map(|i| (i as f32).cos()).collect();
                
                for _ in 0..operations_per_thread {
                    let _ = calc.calculate_distance(&a, &b, &metric);
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
        
        debug!("  {} threads: {:.0} ops/sec", num_threads, ops_per_sec);
    }
}

#[tokio::main]
async fn main() {
    debug!("ProximaDB Real Performance Benchmarks");
    debug!("=====================================");
    
    benchmark_distance_metrics();
    benchmark_indexing().await;
    benchmark_concurrent_operations();
    
    info!("\n✅ Benchmarks completed!");
}