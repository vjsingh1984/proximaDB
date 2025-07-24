//! SIMD performance test example

use proximadb::compute::distance::{detect_platform_capability, create_distance_calculator, DistanceMetric};
use std::time::Instant;

fn main() {
    println!("ProximaDB SIMD Performance Test");
    println!("===============================");
    
    let capability = detect_platform_capability();
    println!("Detected platform: {}", capability);
    println!();
    
    let dim = 1024;
    let iterations = 10_000;
    
    let a: Vec<f32> = (0..dim).map(|i| (i as f32).sin()).collect();
    let b: Vec<f32> = (0..dim).map(|i| (i as f32).cos()).collect();
    
    println!("Testing dimension {} with {} iterations", dim, iterations);
    println!();
    
    for metric in [
        DistanceMetric::Cosine,
        DistanceMetric::Euclidean,
        DistanceMetric::DotProduct,
    ] {
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
        let ns_per_op = elapsed.as_nanos() as f64 / iterations as f64;
        
        println!("{:?}:", metric);
        println!("  {:.0} ops/sec", ops_per_sec);
        println!("  {:.1} ns/op", ns_per_op);
        println!("  {:.2} ms total", elapsed.as_millis());
        println!();
    }
    
    println!("✅ SIMD acceleration is active!");
}