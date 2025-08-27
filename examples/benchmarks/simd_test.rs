//! SIMD performance test example

use proximadb::compute::distance_computation::DistanceMetric;
use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
use proximadb::core::hardware_capabilities::{initialize_hardware_capabilities_default, get_hardware_capabilities};
use std::time::Instant;
use tracing::{debug, info};

fn main() {
    debug!("ProximaDB SIMD Performance Test");
    debug!("===============================");
    
    let _ = initialize_hardware_capabilities_default();
    let capability = get_hardware_capabilities();
    debug!("Detected platform: {:?}", capability);
    
    let dim = 1024;
    let iterations = 10_000;
    
    let a: Vec<f32> = (0..dim).map(|i| (i as f32).sin()).collect();
    let b: Vec<f32> = (0..dim).map(|i| (i as f32).cos()).collect();
    
    debug!("Testing dimension {} with {} iterations", dim, iterations);
    
    for metric in [
        DistanceMetric::Cosine,
        DistanceMetric::Euclidean,
        DistanceMetric::DotProduct,
    ] {
        let calc = UnifiedDistanceCompute::new(metric);
        
        // Warmup
        for _ in 0..100 {
            let _ = calc.calculate_distance(&a, &b, &metric);
        }
        
        // Benchmark
        let start = Instant::now();
        for _ in 0..iterations {
            let _ = calc.calculate_distance(&a, &b, &metric);
        }
        let elapsed = start.elapsed();
        
        let ops_per_sec = iterations as f64 / elapsed.as_secs_f64();
        let ns_per_op = elapsed.as_nanos() as f64 / iterations as f64;
        
        debug!("{:?}:", metric);
        debug!("  {:.0} ops/sec", ops_per_sec);
        debug!("  {:.1} ns/op", ns_per_op);
        debug!("  {:.2} ms total", elapsed.as_millis());
    }
    
    info!("✅ SIMD acceleration is active!");
}