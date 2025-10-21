//! SIMD performance benchmarks

#[cfg(test)]
mod tests {
    use super::super::*;
    use crate::core::hardware_capabilities::get_hardware_capabilities;
    use std::time::Instant;
    use tracing::debug;

    #[test]
    fn benchmark_simd_performance() {
        debug!("\n=== SIMD Performance Benchmark ===");

        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        let capability = get_hardware_capabilities();
        debug!("Platform: {:?}", capability.preferred_backend());

        let dimensions = vec![128, 256, 512, 1024];
        let iterations = 10_000;

        for dim in dimensions {
            debug!("\nDimension: {}", dim);

            let a: Vec<f32> = (0..dim).map(|i| (i as f32).sin()).collect();
            let b: Vec<f32> = (0..dim).map(|i| (i as f32).cos()).collect();

            // Benchmark each distance metric
            for metric in [
                DistanceMetric::Cosine,
                DistanceMetric::Euclidean,
                DistanceMetric::DotProduct,
            ] {
                let calc =
                    crate::compute::distance_computation::engine::UnifiedDistanceCompute::new(
                        metric,
                    );

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

                debug!(
                    "  {:?}: {:.0} ops/sec ({:.1} ns/op)",
                    metric, ops_per_sec, ns_per_op
                );
            }
        }
    }
}
