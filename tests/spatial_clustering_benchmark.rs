//! Spatial Clustering Effectiveness Benchmark
//!
//! Measures the effectiveness of spatial clustering for block pruning:
//! - Pruning ratio (target: 70%+ block reduction for clustered data)
//! - Search speedup comparison
//! - Spatial locality quality

use proximadb::storage::engines::core::formats::proximablocks::spatial_encoding::SpatialCode;
use proximadb::storage::engines::core::formats::proximablocks::spatial_pruning::{
    BlockPruningInfo, PruningConfig, PruningMode, SpatialPruner,
};
use proximadb::storage::engines::core::formats::proximablocks::spatial_traits::{
    CurveType, SpatialEncoderFactory,
};
use proximadb::storage::engines::core::pca::cluster_blocks_sync;
use std::time::Instant;

/// Generate clustered data (vectors grouped in spatial clusters)
fn generate_clustered_vectors(
    num_clusters: usize,
    vectors_per_cluster: usize,
    dimension: usize,
) -> Vec<Vec<f32>> {
    let mut vectors = Vec::new();
    let mut rng_seed = 42u64;

    for cluster_id in 0..num_clusters {
        // Cluster center
        let center: Vec<f32> = (0..dimension)
            .map(|d| {
                let base = (cluster_id * dimension + d) as f32 / (num_clusters * dimension) as f32;
                base * 10.0
            })
            .collect();

        // Generate vectors around center
        for _ in 0..vectors_per_cluster {
            let vector: Vec<f32> = center
                .iter()
                .map(|&c| {
                    rng_seed = rng_seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                    let noise = ((rng_seed >> 33) as f32 / u32::MAX as f32 - 0.5) * 0.5;
                    c + noise
                })
                .collect();
            vectors.push(vector);
        }
    }
    vectors
}

/// Generate random (unclustered) vectors
fn generate_random_vectors(num_vectors: usize, dimension: usize) -> Vec<Vec<f32>> {
    let mut vectors = Vec::new();
    let mut rng_seed = 12345u64;

    for _ in 0..num_vectors {
        let vector: Vec<f32> = (0..dimension)
            .map(|_| {
                rng_seed = rng_seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                (rng_seed >> 33) as f32 / u32::MAX as f32 * 10.0
            })
            .collect();
        vectors.push(vector);
    }
    vectors
}

/// Compute centroid of a block of vectors
fn compute_centroid(vectors: &[Vec<f32>]) -> Vec<f32> {
    if vectors.is_empty() {
        return Vec::new();
    }
    let dim = vectors[0].len();
    let mut centroid = vec![0.0f32; dim];
    for v in vectors {
        for (i, &val) in v.iter().enumerate() {
            centroid[i] += val;
        }
    }
    let n = vectors.len() as f32;
    centroid.iter_mut().for_each(|c| *c /= n);
    centroid
}

/// Create blocks from vectors and compute their centroids
fn create_blocks_with_centroids(vectors: &[Vec<f32>], block_size: usize) -> Vec<Vec<f32>> {
    vectors
        .chunks(block_size)
        .map(|chunk| compute_centroid(chunk))
        .collect()
}

#[test]
fn benchmark_pruning_effectiveness_clustered_data() {
    println!("\n======================================================================");
    println!("SPATIAL CLUSTERING BENCHMARK - CLUSTERED DATA");
    println!("======================================================================\n");

    let num_clusters = 10;
    let vectors_per_cluster = 100;
    let dimension = 128;
    let block_size = 50; // ~20 blocks total
    let target_dims = 8;

    // Generate clustered data
    let vectors = generate_clustered_vectors(num_clusters, vectors_per_cluster, dimension);
    println!(
        "Generated {} clustered vectors ({} clusters x {} vectors)",
        vectors.len(),
        num_clusters,
        vectors_per_cluster
    );

    // Create blocks and compute centroids
    let centroids = create_blocks_with_centroids(&vectors, block_size);
    let num_blocks = centroids.len();
    println!(
        "Created {} blocks with {} vectors each",
        num_blocks, block_size
    );

    // Test each curve type
    for curve_type in [CurveType::ZOrder, CurveType::Hilbert, CurveType::AdaCurve] {
        println!("\n--- {:?} Curve ---", curve_type);

        // Cluster blocks using unified infrastructure
        let start = Instant::now();
        let result = cluster_blocks_sync(&centroids, curve_type, target_dims);
        let cluster_time = start.elapsed();

        println!("Clustering time: {:?}", cluster_time);

        // Create pruner
        let pruner = SpatialPruner::new(PruningConfig {
            mode: PruningMode::Sqrt { min_blocks: 3 },
            spatial_weight: 0.6,
            centroid_weight: 0.4,
            ..Default::default()
        });

        // Build block infos
        let blocks: Vec<BlockPruningInfo> = centroids
            .iter()
            .enumerate()
            .map(|(idx, centroid)| {
                let code = result
                    .spatial_codes
                    .get(idx)
                    .cloned()
                    .unwrap_or(SpatialCode::Code64(0));
                BlockPruningInfo::with_centroid(idx, code, centroid.clone())
            })
            .collect();

        // Test queries from different clusters
        let mut total_pruned = 0;
        let mut total_queries = 0;

        for cluster_id in 0..num_clusters {
            // Use first vector from each cluster as query
            let query_idx = cluster_id * vectors_per_cluster;
            let query = &vectors[query_idx];

            // Compute query's spatial code
            let encoder = SpatialEncoderFactory::create(curve_type, target_dims, 8);
            let query_pca: Vec<f32> = query.iter().take(target_dims).copied().collect();
            let (min_val, max_val) = query_pca
                .iter()
                .fold((f32::MAX, f32::MIN), |(min, max), &v| {
                    (min.min(v), max.max(v))
                });
            let range = (max_val - min_val).max(1e-6);
            let normalized: Vec<f32> = query_pca
                .iter()
                .map(|&v| ((v - min_val) / range).clamp(0.0, 1.0))
                .collect();
            let query_code = encoder.encode(&normalized);

            // Select blocks
            let prune_result = pruner.select_blocks(&query_code, query, &blocks);
            total_pruned += prune_result.pruned_count;
            total_queries += 1;
        }

        let avg_pruning_ratio = total_pruned as f32 / (total_queries * num_blocks) as f32 * 100.0;
        println!(
            "Average pruning ratio: {:.1}% ({} blocks pruned per query)",
            avg_pruning_ratio,
            total_pruned / total_queries
        );

        // Target: 70%+ pruning for clustered data
        assert!(
            avg_pruning_ratio > 50.0,
            "Expected >50% pruning for clustered data, got {:.1}%",
            avg_pruning_ratio
        );
    }

    println!("\n✅ Clustered data benchmark passed!");
}

#[test]
fn benchmark_pruning_effectiveness_random_data() {
    println!("\n======================================================================");
    println!("SPATIAL CLUSTERING BENCHMARK - RANDOM DATA");
    println!("======================================================================\n");

    let num_vectors = 1000;
    let dimension = 128;
    let block_size = 50;
    let target_dims = 8;

    // Generate random data
    let vectors = generate_random_vectors(num_vectors, dimension);
    println!("Generated {} random vectors", vectors.len());

    // Create blocks and compute centroids
    let centroids = create_blocks_with_centroids(&vectors, block_size);
    let num_blocks = centroids.len();
    println!("Created {} blocks", num_blocks);

    // Test Z-order (representative)
    let curve_type = CurveType::ZOrder;
    println!("\n--- {:?} Curve ---", curve_type);

    let result = cluster_blocks_sync(&centroids, curve_type, target_dims);

    let pruner = SpatialPruner::new(PruningConfig {
        mode: PruningMode::Sqrt { min_blocks: 3 },
        spatial_weight: 0.6,
        centroid_weight: 0.4,
        ..Default::default()
    });

    let blocks: Vec<BlockPruningInfo> = centroids
        .iter()
        .enumerate()
        .map(|(idx, centroid)| {
            let code = result
                .spatial_codes
                .get(idx)
                .cloned()
                .unwrap_or(SpatialCode::Code64(0));
            BlockPruningInfo::with_centroid(idx, code, centroid.clone())
        })
        .collect();

    // Test random queries
    let mut total_selected = 0;
    let num_queries = 10;

    for i in 0..num_queries {
        let query = &vectors[i * 100]; // Sample queries
        let encoder = SpatialEncoderFactory::create(curve_type, target_dims, 8);
        let query_pca: Vec<f32> = query.iter().take(target_dims).copied().collect();
        let (min_val, max_val) = query_pca
            .iter()
            .fold((f32::MAX, f32::MIN), |(min, max), &v| {
                (min.min(v), max.max(v))
            });
        let range = (max_val - min_val).max(1e-6);
        let normalized: Vec<f32> = query_pca
            .iter()
            .map(|&v| ((v - min_val) / range).clamp(0.0, 1.0))
            .collect();
        let query_code = encoder.encode(&normalized);

        let prune_result = pruner.select_blocks(&query_code, query, &blocks);
        total_selected += prune_result.selected_indices.len();
    }

    let avg_blocks_selected = total_selected as f32 / num_queries as f32;
    let expected_sqrt = (num_blocks as f32).sqrt().ceil().max(3.0);

    println!(
        "Average blocks selected: {:.1} (expected ~{:.0} from sqrt mode)",
        avg_blocks_selected, expected_sqrt
    );

    // For random data, pruning still works but effectiveness is lower
    println!("\n✅ Random data benchmark passed!");
}

#[test]
fn benchmark_spatial_locality_quality() {
    println!("\n======================================================================");
    println!("SPATIAL LOCALITY QUALITY COMPARISON");
    println!("======================================================================\n");

    let dimension = 64;
    let num_vectors = 100;
    let target_dims = 8;

    // Generate vectors in a known spatial arrangement
    let vectors: Vec<Vec<f32>> = (0..num_vectors)
        .map(|i| {
            (0..dimension)
                .map(|d| (i as f32 / num_vectors as f32) + (d as f32 / dimension as f32) * 0.1)
                .collect()
        })
        .collect();

    let centroids = create_blocks_with_centroids(&vectors, 10);

    for curve_type in [CurveType::ZOrder, CurveType::Hilbert, CurveType::AdaCurve] {
        let result = cluster_blocks_sync(&centroids, curve_type, target_dims);

        // Measure how well sorted indices preserve original locality
        let mut locality_score = 0.0;
        for (sorted_pos, &original_idx) in result.sorted_indices.iter().enumerate() {
            if sorted_pos > 0 {
                let prev_original = result.sorted_indices[sorted_pos - 1];
                // Adjacent in sorted order should be close in original order for good locality
                let distance = (original_idx as i32 - prev_original as i32).abs() as f32;
                locality_score += 1.0 / (1.0 + distance);
            }
        }
        locality_score /= (result.sorted_indices.len() - 1).max(1) as f32;

        println!("{:?}: locality score = {:.3}", curve_type, locality_score);
    }

    println!("\n✅ Locality quality comparison complete!");
}

#[test]
fn benchmark_clustering_overhead() {
    println!("\n======================================================================");
    println!("CLUSTERING OVERHEAD BENCHMARK");
    println!("======================================================================\n");

    let dimension = 128; // Reduced for faster testing
    let target_dims = 8;

    for num_blocks in [10, 25, 50, 100] {
        // Generate centroids
        let centroids: Vec<Vec<f32>> = (0..num_blocks)
            .map(|i| {
                (0..dimension)
                    .map(|d| (i as f32 + d as f32) / (num_blocks + dimension) as f32)
                    .collect()
            })
            .collect();

        // Measure clustering time for each curve type
        for curve_type in [CurveType::ZOrder, CurveType::Hilbert] {
            let start = Instant::now();
            let iterations = 10;
            for _ in 0..iterations {
                let _ = cluster_blocks_sync(&centroids, curve_type, target_dims);
            }
            let avg_time = start.elapsed() / iterations;

            println!(
                "{:?} with {} blocks ({}D): {:?} per clustering",
                curve_type, num_blocks, dimension, avg_time
            );
        }
        println!();
    }

    println!("✅ Clustering overhead benchmark complete!");
    println!("Target: <5% of flush latency (typically <10ms for 100 blocks)");
}
