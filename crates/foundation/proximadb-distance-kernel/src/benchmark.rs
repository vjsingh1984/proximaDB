//! SIMD performance benchmarks

#[cfg(test)]
mod tests {
    use super::super::*;
    use std::time::Instant;
    use tracing::debug;

    #[test]
    fn benchmark_simd_performance() {
        debug!("\n=== SIMD Performance Benchmark ===");

        let simd = proximadb_hardware::best_simd_level();
        debug!("Platform SIMD level: {:?}", simd);

        let dimensions = vec![128, 256, 512, 1024];
        let iterations = 10_000;

        for dim in dimensions {
            debug!("\nDimension: {}", dim);

            let a: Vec<f32> = (0..dim).map(|i| (i as f32).sin()).collect();
            let b: Vec<f32> = (0..dim).map(|i| (i as f32).cos()).collect();

            for metric in [
                DistanceMetric::Cosine,
                DistanceMetric::Euclidean,
                DistanceMetric::DotProduct,
            ] {
                let calc = crate::engine::UnifiedDistanceCompute::new(metric);

                for _ in 0..100 {
                    let _ = calc.distance(&a, &b);
                }

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
