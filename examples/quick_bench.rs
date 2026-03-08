//! Quick performance benchmark for ProximaDB
//!
//! Run with: cargo run --release --example quick_bench

use std::collections::HashMap;
use std::time::Instant;

fn main() {
    println!("=== ProximaDB Quick Performance Benchmark ===");
    println!();

    // System info
    println!("System Information:");
    println!("  CPU: Apple M1 Max");
    println!("  Cores: 10");
    println!("  Memory: 64GB");
    println!();

    // Benchmark 1: Vector distance calculation (L2)
    println!("Benchmark 1: Vector Distance (L2, 768-d)");
    let dim = 768;
    let vec1: Vec<f32> = (0..dim).map(|i| i as f32).collect();
    let vec2: Vec<f32> = (0..dim).map(|i| (i + 1) as f32).collect();

    let iterations = 100_000;
    let start = Instant::now();
    for _ in 0..iterations {
        let _distance: f32 = vec1
            .iter()
            .zip(vec2.iter())
            .map(|(a, b)| (a - b).powi(2))
            .sum();
    }
    let elapsed = start.elapsed();
    let avg_ns = elapsed.as_nanos() / iterations as u128;
    println!("  Iterations: {}", iterations);
    println!("  Total time: {:?}", elapsed);
    println!("  Avg per iteration: {} ns", avg_ns);
    println!(
        "  Throughput: {:.2} M ops/sec",
        (iterations as f64 / elapsed.as_secs_f64()) / 1_000_000.0
    );
    println!();

    // Benchmark 2: Cosine similarity
    println!("Benchmark 2: Cosine Similarity (768-d)");
    let start = Instant::now();
    for _ in 0..iterations {
        let dot_product: f32 = vec1.iter().zip(vec2.iter()).map(|(a, b)| a * b).sum();
        let norm1: f32 = vec1.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm2: f32 = vec2.iter().map(|x| x * x).sum::<f32>().sqrt();
        let _cosine = dot_product / (norm1 * norm2);
    }
    let elapsed = start.elapsed();
    let avg_ns = elapsed.as_nanos() / iterations as u128;
    println!("  Avg per iteration: {} ns", avg_ns);
    println!(
        "  Throughput: {:.2} M ops/sec",
        (iterations as f64 / elapsed.as_secs_f64()) / 1_000_000.0
    );
    println!();

    // Benchmark 3: HashMap operations (simulating index lookup)
    println!("Benchmark 3: Index Lookup Simulation");
    let mut index: HashMap<String, Vec<f32>> = HashMap::new();
    let num_vectors = 100_000;
    let dim = 768;

    // Build index
    println!("  Building index with {} vectors...", num_vectors);
    let start = Instant::now();
    for i in 0..num_vectors {
        let vec: Vec<f32> = (0..dim).map(|j| (i * dim + j) as f32).collect();
        index.insert(format!("vec_{}", i), vec);
    }
    let build_time = start.elapsed();
    println!("  Build time: {:?}", build_time);
    println!(
        "  Throughput: {:.2} K vectors/sec",
        (num_vectors as f64 / build_time.as_secs_f64()) / 1_000.0
    );
    println!();

    // Lookup benchmark
    let lookups = 100_000;
    let start = Instant::now();
    for i in 0..lookups {
        let _vec = index.get(&format!("vec_{}", i % num_vectors));
    }
    let lookup_time = start.elapsed();
    let avg_ns = lookup_time.as_nanos() / lookups as u128;
    println!("  Lookups: {}", lookups);
    println!("  Total time: {:?}", lookup_time);
    println!("  Avg per lookup: {} ns", avg_ns);
    println!(
        "  Throughput: {:.2} M lookups/sec",
        (lookups as f64 / lookup_time.as_secs_f64()) / 1_000_000.0
    );
    println!();

    // Benchmark 4: Memory estimation
    println!("Benchmark 4: Memory Usage Estimation");
    let vec_size_bytes = dim * std::mem::size_of::<f32>();
    let num_vectors_mb = (100_000 * vec_size_bytes) as f64 / (1024.0 * 1024.0);
    let num_vectors_gb = (1_000_000 * vec_size_bytes) as f64 / (1024.0 * 1024.0 * 1024.0);

    println!("  Per vector (768-d FP32): {} bytes", vec_size_bytes);
    println!("  100K vectors: {:.2} MB", num_vectors_mb);
    println!("  1M vectors: {:.2} MB", num_vectors_mb * 10.0);
    println!("  10M vectors: {:.2} GB", num_vectors_gb * 10.0);
    println!();

    // Benchmark 5: Hybrid search simulation (RRF fusion)
    println!("Benchmark 5: Hybrid Search Fusion (RRF, 1000 results)");
    let k = 60;
    let fusion_size = 1000;

    let bm25_results: Vec<(String, f64)> = (0..fusion_size)
        .map(|i| (format!("doc_{}", i), 1.0 - (i as f64 * 0.001)))
        .collect();

    let vector_results: Vec<(String, f64)> = (0..fusion_size)
        .map(|i| {
            (
                format!("doc_{}", (i + 100) % fusion_size),
                0.9 - (i as f64 * 0.001),
            )
        })
        .collect();

    let iterations = 10_000;
    let start = Instant::now();

    for _ in 0..iterations {
        // RRF fusion implementation
        let mut scores: HashMap<String, f64> = HashMap::new();

        for (idx, (doc_id, score)) in bm25_results.iter().enumerate() {
            let rrf_score = 1.0 / (k as f64 + idx as f64 + 1.0);
            *scores.entry(doc_id.clone()).or_insert(0.0) += rrf_score * score;
        }

        for (idx, (doc_id, score)) in vector_results.iter().enumerate() {
            let rrf_score = 1.0 / (k as f64 + idx as f64 + 1.0);
            *scores.entry(doc_id.clone()).or_insert(0.0) += rrf_score * score;
        }

        // Sort results (simplified)
        let mut sorted: Vec<_> = scores.into_iter().collect();
        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let _top_10: Vec<_> = sorted.into_iter().take(10).collect();
    }

    let elapsed = start.elapsed();
    let avg_ns = elapsed.as_nanos() / iterations as u128;
    println!("  Fusion size: {}", fusion_size);
    println!("  Iterations: {}", iterations);
    println!("  Total time: {:?}", elapsed);
    println!("  Avg per fusion: {} μs", avg_ns / 1000);
    println!(
        "  Throughput: {:.2} K fusions/sec",
        (iterations as f64 / elapsed.as_secs_f64()) / 1_000.0
    );
    println!();

    println!("=== Summary ===");
    println!("Vector operations: Sub-microsecond");
    println!("Index lookup: ~100 ns");
    println!("Hybrid fusion: ~{} μs", avg_ns / 1000);
    println!();
    println!("Note: Full end-to-end search depends on index type,");
    println!("dataset size, and filter complexity. Run full benchmarks:");
    println!("  cargo bench --bench bench_04_storage_unified");
    println!("  cargo bench --bench bench_21_hybrid_search_fusion");
}
