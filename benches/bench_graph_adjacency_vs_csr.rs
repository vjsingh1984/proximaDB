//! Benchmark for comparing Graph Adjacency Table vs CSR Projection access paths.
//!
//! This benchmark validates the cost rules implemented in `optimizer_support.rs`
//! by measuring the latency difference between random-access row lookups (Adjacency Table)
//! and sequential-access neighbor fetches (CSR).
//!
//! Run with: `cargo bench --bench bench_graph_adjacency_vs_csr`

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use proximadb_query::graph_traversal_access_path;
use rand::prelude::*;
use std::hint::black_box;

// ---------------------------------------------------------------------------
// Simulated Traversal Logic
// ---------------------------------------------------------------------------

/// Simulates Adjacency Table lookup: O(degree) random memory accesses.
/// This mimics looking up edges in a relational table with a non-clustered index.
fn simulate_adjacency_table_lookup(degree: usize, data: &[u32], indices: &[usize]) -> u32 {
    let mut sum = 0;
    for &idx in indices.iter().take(degree) {
        sum += data[idx]; // Random access (cache miss likely)
    }
    sum
}

/// Simulates CSR lookup: O(degree) sequential memory accesses.
/// This mimics fetching neighbors from a contiguous memory block.
fn simulate_csr_lookup(degree: usize, data: &[u32], start_offset: usize) -> u32 {
    let mut sum = 0;
    for i in 0..degree {
        sum += data[start_offset + i]; // Sequential access (cache-line friendly)
    }
    sum
}

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

fn bench_traversal_paths(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph_traversal_access_paths");

    // Setup: 10M elements (~40MB) to exceed L3 cache on most systems and ensure random vs sequential matters.
    let data_size = 10_000_000;
    let data: Vec<u32> = (0..data_size as u32).collect();

    // Prepare random indices for adjacency table simulation
    let mut rng = StdRng::seed_from_u64(42);
    let max_degree = 200;
    let indices: Vec<usize> = (0..max_degree)
        .map(|_| rng.gen_range(0..data_size))
        .collect();

    for degree in [2usize, 10, 50, 200] {
        // Measure Adjacency Table (Random)
        group.bench_with_input(
            BenchmarkId::new("adjacency_table", degree),
            &degree,
            |b, &d| {
                b.iter(|| black_box(simulate_adjacency_table_lookup(d, &data, &indices)));
            },
        );

        // Measure CSR (Sequential)
        group.bench_with_input(
            BenchmarkId::new("csr_projection", degree),
            &degree,
            |b, &d| {
                b.iter(|| black_box(simulate_csr_lookup(d, &data, 5_000_000)));
            },
        );

        // Print optimizer decision for validation
        let est = graph_traversal_access_path(60, 2, degree as f64);
        println!(
            "\n[Validation] Degree {}: Optimizer choice = {:?} (Cost: {:.2})\nReason: {}",
            degree, est.path, est.cost, est.reason
        );
    }
    group.finish();
}

criterion_group!(benches, bench_traversal_paths);
criterion_main!(benches);
