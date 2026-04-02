//! FP16 Centroid Performance Benchmark
//!
//! Measures the real-world performance impact of FP16 centroids:
//! 1. Memory usage (50% reduction verified)
//! 2. Cache efficiency (fit 2x more centroids in cache)
//! 3. Distance computation overhead (FP16→FP32 conversion cost)
//! 4. Block selection latency (end-to-end impact)
//!
//! Expected results:
//! - Storage: 50% reduction ✓
//! - Memory: 50% reduction ✓
//! - Conversion overhead: <5% CPU time
//! - Cache hit rate: TBD (depends on workload)
//! - Overall latency: Neutral to slight improvement

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use proximadb::compute::distance_computation::DistanceMetric;
use proximadb::compute::distance_computation::engine::UnifiedDistanceCompute;
use proximadb::storage::engines::impls::sst::{fp16_to_fp32, fp32_to_fp16};
use rand::Rng;

/// Benchmark FP32 ↔ FP16 conversion overhead
fn bench_conversion(c: &mut Criterion) {
    let mut group = c.benchmark_group("fp16_conversion");

    for dimension in [128, 256, 512, 1024, 1536] {
        let vector: Vec<f32> = (0..dimension)
            .map(|i| (i as f32) / dimension as f32)
            .collect();

        group.throughput(Throughput::Elements(dimension as u64));

        group.bench_with_input(
            BenchmarkId::new("fp32_to_fp16", dimension),
            &vector,
            |b, v| {
                b.iter(|| {
                    let fp16 = fp32_to_fp16(black_box(v));
                    black_box(fp16);
                });
            },
        );

        let fp16_vector = fp32_to_fp16(&vector);
        group.bench_with_input(
            BenchmarkId::new("fp16_to_fp32", dimension),
            &fp16_vector,
            |b, v| {
                b.iter(|| {
                    let fp32 = fp16_to_fp32(black_box(v));
                    black_box(fp32);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("round_trip", dimension),
            &vector,
            |b, v| {
                b.iter(|| {
                    let fp16 = fp32_to_fp16(black_box(v));
                    let fp32 = fp16_to_fp32(&fp16);
                    black_box(fp32);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark distance computation: FP32-only vs FP16-path
fn bench_distance_computation(c: &mut Criterion) {
    let mut group = c.benchmark_group("fp16_distance");

    let distance_compute = UnifiedDistanceCompute::default();
    let metric = DistanceMetric::Cosine;

    for dimension in [128, 256, 512, 1024, 1536] {
        let query: Vec<f32> = (0..dimension)
            .map(|i| (i as f32) / dimension as f32)
            .collect();
        let centroid_fp32: Vec<f32> = (0..dimension)
            .map(|i| ((i + 10) as f32) / dimension as f32)
            .collect();
        let centroid_fp16 = fp32_to_fp16(&centroid_fp32);

        group.throughput(Throughput::Elements(dimension as u64));

        // Baseline: FP32 distance
        group.bench_with_input(
            BenchmarkId::new("fp32_baseline", dimension),
            &(&query, &centroid_fp32),
            |b, (q, c)| {
                b.iter(|| {
                    let dist =
                        distance_compute.distance_with_metric(black_box(q), black_box(c), &metric);
                    black_box(dist);
                });
            },
        );

        // FP16 path: convert + distance
        group.bench_with_input(
            BenchmarkId::new("fp16_path", dimension),
            &(&query, &centroid_fp16),
            |b, (q, c)| {
                b.iter(|| {
                    let centroid_fp32 = fp16_to_fp32(black_box(c));
                    let dist = distance_compute.distance_with_metric(
                        black_box(q),
                        &centroid_fp32,
                        &metric,
                    );
                    black_box(dist);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark block selection with FP16 vs FP32 centroids
fn bench_block_selection(c: &mut Criterion) {
    let mut group = c.benchmark_group("fp16_block_selection");
    let mut rng = rand::thread_rng();

    let distance_compute = UnifiedDistanceCompute::default();
    let metric = DistanceMetric::Cosine;

    for num_blocks in [100, 500, 1000, 5000, 10000] {
        let dimension = 128;
        let top_k = 10;

        let query: Vec<f32> = (0..dimension).map(|_| rng.gen_range(0.0..1.0)).collect();

        // Generate centroids in both FP32 and FP16
        let centroids_fp32: Vec<Vec<f32>> = (0..num_blocks)
            .map(|_| (0..dimension).map(|_| rng.gen_range(0.0..1.0)).collect())
            .collect();

        let centroids_fp16: Vec<Vec<u16>> =
            centroids_fp32.iter().map(|c| fp32_to_fp16(c)).collect();

        group.throughput(Throughput::Elements(num_blocks as u64));

        // FP32 path: Direct distance computation
        group.bench_with_input(
            BenchmarkId::new("fp32_block_select", num_blocks),
            &(&query, &centroids_fp32),
            |b, (q, centroids)| {
                b.iter(|| {
                    let mut distances: Vec<(usize, f32)> = centroids
                        .iter()
                        .enumerate()
                        .map(|(idx, c)| {
                            let dist = distance_compute.distance_with_metric(q, c, &metric);
                            (idx, dist)
                        })
                        .collect();
                    distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
                    let top_k_indices: Vec<usize> =
                        distances.iter().take(top_k).map(|(idx, _)| *idx).collect();
                    black_box(top_k_indices);
                });
            },
        );

        // FP16 path: Convert + distance computation
        group.bench_with_input(
            BenchmarkId::new("fp16_block_select", num_blocks),
            &(&query, &centroids_fp16),
            |b, (q, centroids)| {
                b.iter(|| {
                    let mut distances: Vec<(usize, f32)> = centroids
                        .iter()
                        .enumerate()
                        .map(|(idx, c)| {
                            let c_fp32 = fp16_to_fp32(c);
                            let dist = distance_compute.distance_with_metric(q, &c_fp32, &metric);
                            (idx, dist)
                        })
                        .collect();
                    distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
                    let top_k_indices: Vec<usize> =
                        distances.iter().take(top_k).map(|(idx, _)| *idx).collect();
                    black_box(top_k_indices);
                });
            },
        );

        // Amortized FP16 path: Pre-convert all centroids (simulates caching)
        group.bench_with_input(
            BenchmarkId::new("fp16_block_select_cached", num_blocks),
            &(&query, &centroids_fp16),
            |b, (q, centroids)| {
                b.iter(|| {
                    // Pre-convert all centroids (simulates cache/memoization)
                    let centroids_fp32_converted: Vec<Vec<f32>> =
                        centroids.iter().map(|c| fp16_to_fp32(c)).collect();

                    let mut distances: Vec<(usize, f32)> = centroids_fp32_converted
                        .iter()
                        .enumerate()
                        .map(|(idx, c)| {
                            let dist = distance_compute.distance_with_metric(q, c, &metric);
                            (idx, dist)
                        })
                        .collect();
                    distances.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
                    let top_k_indices: Vec<usize> =
                        distances.iter().take(top_k).map(|(idx, _)| *idx).collect();
                    black_box(top_k_indices);
                });
            },
        );
    }

    group.finish();
}

/// Benchmark memory footprint (this is more of a measurement than a perf benchmark)
fn bench_memory_footprint(c: &mut Criterion) {
    let mut group = c.benchmark_group("fp16_memory");

    for num_centroids in [1000, 5000, 10000, 50000, 100000] {
        let dimension = 128;

        let centroids_fp32: Vec<Vec<f32>> = (0..num_centroids)
            .map(|i| (0..dimension).map(|j| (i + j) as f32).collect())
            .collect();

        let _centroids_fp16: Vec<Vec<u16>> =
            centroids_fp32.iter().map(|c| fp32_to_fp16(c)).collect();

        // Calculate memory usage
        let fp32_bytes = num_centroids * dimension * std::mem::size_of::<f32>();
        let fp16_bytes = num_centroids * dimension * std::mem::size_of::<u16>();
        let savings = fp32_bytes - fp16_bytes;
        let savings_pct = (savings as f64 / fp32_bytes as f64) * 100.0;

        println!(
            "\n=== Memory Footprint for {} centroids ({} dimensions) ===",
            num_centroids, dimension
        );
        println!("FP32: {} MB", fp32_bytes as f64 / 1_048_576.0);
        println!("FP16: {} MB", fp16_bytes as f64 / 1_048_576.0);
        println!(
            "Savings: {} MB ({:.1}%)",
            savings as f64 / 1_048_576.0,
            savings_pct
        );

        group.throughput(Throughput::Bytes(fp32_bytes as u64));

        group.bench_with_input(
            BenchmarkId::new("fp32_allocation", num_centroids),
            &dimension,
            |b, &dim| {
                b.iter(|| {
                    let centroids: Vec<Vec<f32>> = (0..num_centroids)
                        .map(|i| (0..dim).map(|j| (i + j) as f32).collect())
                        .collect();
                    black_box(centroids);
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("fp16_allocation", num_centroids),
            &dimension,
            |b, &dim| {
                b.iter(|| {
                    let centroids: Vec<Vec<u16>> = (0..num_centroids)
                        .map(|i| (0..dim).map(|j| ((i + j) as f32 * 100.0) as u16).collect())
                        .collect();
                    black_box(centroids);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_conversion,
    bench_distance_computation,
    bench_block_selection,
    bench_memory_footprint
);
criterion_main!(benches);
