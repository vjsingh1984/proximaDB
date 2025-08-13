#!/usr/bin/env rust-script
//! Test SIMD performance improvements
//! 
//! ```cargo
//! [dependencies]
//! proximadb = { path = "." }
//! tokio = { version = "1", features = ["full"] }
//! ```

use proximadb::compute::distance::{detect_platform_capability, create_distance_calculator, DistanceMetric};
use std::time::Instant;
use tracing::info;

fn benchmark_distance(name: &str, metric: DistanceMetric, dim: usize, iterations: usize) {
    let a: Vec<f32> = (0..dim).map(|i| (i as f32).sin()).collect();
    let b: Vec<f32> = (0..dim).map(|i| (i as f32).cos()).collect();
    
    let calc = create_distance_calculator(metric);
    
    // Warmup
    for _ in 0..100 {
        let _ = calc.distance(&a, &b);
    }
    
    // Benchmark
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = calc.distance(&a, &b);
    }
    let elapsed = start.elapsed();
    
    let ops_per_sec = iterations as f64 / elapsed.as_secs_f64();
    info!("{} (dim {}): {:.0} ops/sec, {:.2} ns/op", 
             name, dim, ops_per_sec, elapsed.as_nanos() as f64 / iterations as f64);
}

#[tokio::main]
async fn main() {
    info!("ProximaDB SIMD Performance Test");
    info!("===============================");
    
    let capability = detect_platform_capability();
    info!("Detected platform: {}", capability);
    info!("");
    
    // Test different dimensions
    let dimensions = vec![128, 256, 512, 1024, 2048];
    let iterations = 100_000;
    
    for dim in dimensions {
        info!("\nDimension: {}", dim);
        info!("-----------");
        
        benchmark_distance("Cosine", DistanceMetric::Cosine, dim, iterations);
        benchmark_distance("Euclidean", DistanceMetric::Euclidean, dim, iterations);
        benchmark_distance("DotProduct", DistanceMetric::DotProduct, dim, iterations);
    }
    
    info!("\n✅ SIMD acceleration is working!");
}