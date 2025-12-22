//! Integration tests for Unified Spatial Clustering Infrastructure
//!
//! Tests the complete pipeline: PCA → Spatial Encoding → Block Clustering → Pruning
//! for SST (Z-order), HELIX (Hilbert), and SWIFT (AdaCurve) engines.

use proximadb::proto::proximadb_v1::VectorRecord;
use proximadb::storage::engines::core::formats::proximablocks::spatial_encoding::SpatialCode;
use proximadb::storage::engines::core::formats::proximablocks::spatial_pruning::{
    BlockPruningInfo, PruningConfig, PruningMode, SpatialPruner,
};
use proximadb::storage::engines::core::formats::proximablocks::spatial_traits::{
    CurveType, SpatialCurveEncoder, SpatialEncoderFactory,
};
use proximadb::storage::engines::core::pca::{
    BlockInfo, ClusteringConfig, EnhancedPCAModel, SpatialClusteringPipeline,
};

use std::collections::HashMap;

/// Generate clustered test vectors with known spatial distribution
fn generate_clustered_vectors(
    num_clusters: usize,
    vectors_per_cluster: usize,
    dimension: usize,
) -> Vec<VectorRecord> {
    let mut records = Vec::new();
    let mut rng_seed = 42u64;

    for cluster_id in 0..num_clusters {
        // Cluster center
        let center: Vec<f32> = (0..dimension)
            .map(|d| {
                let base = (cluster_id as f32 / num_clusters as f32) * 10.0;
                base + (d as f32 / dimension as f32) * 0.5
            })
            .collect();

        // Generate vectors around center
        for i in 0..vectors_per_cluster {
            rng_seed = rng_seed.wrapping_mul(1103515245).wrapping_add(12345);
            let noise_scale = 0.1;

            let vector: Vec<f32> = center
                .iter()
                .enumerate()
                .map(|(d, &c)| {
                    let noise = ((rng_seed.wrapping_add(d as u64) % 1000) as f32 / 1000.0 - 0.5)
                        * noise_scale;
                    c + noise
                })
                .collect();

            records.push(VectorRecord {
                id: format!("cluster{}_vec{}", cluster_id, i),
                vector,
                metadata: HashMap::new(),
                timestamp: Some(0),
                updated_at: None,
                expires_at: None,
                version: Some(1),
                source: None,
            });
        }
    }

    records
}

/// Generate random test vectors
fn generate_random_vectors(num_vectors: usize, dimension: usize) -> Vec<VectorRecord> {
    let mut rng_seed = 12345u64;

    (0..num_vectors)
        .map(|i| {
            let vector: Vec<f32> = (0..dimension)
                .map(|d| {
                    rng_seed = rng_seed.wrapping_mul(1103515245).wrapping_add(12345);
                    (rng_seed % 1000) as f32 / 1000.0
                })
                .collect();

            VectorRecord {
                id: format!("vec_{}", i),
                vector,
                metadata: HashMap::new(),
                timestamp: Some(0),
                updated_at: None,
                expires_at: None,
                version: Some(1),
                source: None,
            }
        })
        .collect()
}

#[test]
fn test_pca_model_training_and_projection() {
    // Test PCA model training with clustered data
    let records = generate_clustered_vectors(5, 100, 128);

    // Train PCA model
    let model = EnhancedPCAModel::train(&records, 8).unwrap();

    assert_eq!(model.n_components, 8);
    assert_eq!(model.original_dim, 128);
    assert!(model.variance_explained() > 0.5); // Should explain significant variance

    // Test projection
    let test_vector = &records[0].vector;
    let projected = model.project(test_vector).unwrap();
    assert_eq!(projected.len(), 8);

    // Test reconstruction
    let reconstructed = model.reconstruct(&projected).unwrap();
    assert_eq!(reconstructed.len(), 128);

    // Reconstruction error should be reasonable
    let error = model.reconstruction_error(test_vector).unwrap();
    assert!(error < 1.0); // Error should be small
}

#[test]
fn test_spatial_encoder_zorder() {
    let encoder = SpatialEncoderFactory::create(CurveType::ZOrder, 8, 8);

    assert_eq!(encoder.curve_type(), CurveType::ZOrder);
    assert_eq!(encoder.dimensions(), 8);

    // Test encoding
    let coords = vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];
    let code = encoder.encode(&coords);

    // Verify code type
    assert!(matches!(code, SpatialCode::Code64(_)));

    // Test decode roundtrip
    let decoded = encoder.decode(&code);
    assert_eq!(decoded.len(), 8);

    // Should be approximately equal (quantization error expected)
    for (orig, dec) in coords.iter().zip(decoded.iter()) {
        assert!((orig - dec).abs() < 0.01);
    }
}

#[test]
fn test_spatial_encoder_hilbert() {
    let encoder = SpatialEncoderFactory::create(CurveType::Hilbert, 4, 8);

    assert_eq!(encoder.curve_type(), CurveType::Hilbert);

    // Test encoding
    let coords = vec![0.25, 0.5, 0.75, 1.0];
    let code = encoder.encode(&coords);

    // Hilbert should produce valid codes
    assert!(matches!(code, SpatialCode::Code64(_)));
}

#[test]
fn test_spatial_locality_via_block_selection() {
    // Test that block selection prefers spatially close blocks
    // Note: Z-order curves guarantee hierarchical locality, not linear code proximity
    let encoder = SpatialEncoderFactory::create(CurveType::ZOrder, 4, 8);

    // Create blocks in a grid pattern
    let block_centroids: Vec<Vec<f32>> = vec![
        vec![0.0, 0.0, 0.0, 0.0], // Origin
        vec![0.1, 0.1, 0.1, 0.1], // Near origin
        vec![0.5, 0.5, 0.5, 0.5], // Center
        vec![0.6, 0.6, 0.6, 0.6], // Near center
        vec![0.9, 0.9, 0.9, 0.9], // Far corner
    ];

    let block_codes: Vec<SpatialCode> = block_centroids
        .iter()
        .map(|c| encoder.encode(c))
        .collect();

    // Query near the center
    let query = vec![0.55, 0.55, 0.55, 0.55];
    let query_code = encoder.encode(&query);

    // Select top 2 blocks
    let selected = encoder.select_blocks(&query_code, &block_codes, 2);

    // Center and near-center blocks should be preferred (indices 2 and 3)
    // The selection uses code distance which should prefer nearby grid cells
    assert_eq!(selected.len(), 2);
    // Either center (2) or near-center (3) should be in the selection
    let has_center_region = selected.contains(&2) || selected.contains(&3);
    assert!(
        has_center_region,
        "Block selection should prefer nearby blocks, got: {:?}",
        selected
    );
}

#[test]
fn test_encode_decode_consistency() {
    // Verify encode/decode roundtrip consistency
    let encoder = SpatialEncoderFactory::create(CurveType::ZOrder, 4, 8);

    let original = vec![0.25, 0.5, 0.75, 1.0];
    let code = encoder.encode(&original);
    let decoded = encoder.decode(&code);

    // With 8 bits per dimension, quantization error is 1/256 ≈ 0.004
    for (orig, dec) in original.iter().zip(decoded.iter()) {
        let error = (orig - dec).abs();
        assert!(
            error < 0.01,
            "Decode should be close to original: {} vs {} (error: {})",
            orig,
            dec,
            error
        );
    }
}

/// Helper function for comparing spatial codes (64-bit only for tests)
fn code_distance(a: &SpatialCode, b: &SpatialCode) -> u64 {
    match (a, b) {
        (SpatialCode::Code64(a), SpatialCode::Code64(b)) => a.abs_diff(*b),
        (SpatialCode::Code128(a), SpatialCode::Code128(b)) => a.abs_diff(*b) as u64,
        _ => u64::MAX, // Different or complex code types
    }
}

#[test]
fn test_spatial_pruner_sqrt_mode() {
    let config = PruningConfig {
        mode: PruningMode::Sqrt { min_blocks: 3 },
        ..Default::default()
    };
    let pruner = SpatialPruner::new(config);

    // Create 100 blocks
    let query_code = SpatialCode::Code64(500);
    let block_codes: Vec<SpatialCode> = (0..100)
        .map(|i| SpatialCode::Code64(i * 10))
        .collect();

    let selected = pruner.select_blocks_by_code(&query_code, &block_codes);

    // Should select sqrt(100) = 10 blocks
    assert_eq!(selected.len(), 10);

    // Block at index 50 (code 500) should be selected (closest to query)
    assert!(selected.contains(&50));
}

#[test]
fn test_spatial_pruner_with_centroids() {
    let config = PruningConfig {
        mode: PruningMode::Fixed { k: 3 },
        use_centroid_distance: true,
        spatial_weight: 0.5,
        centroid_weight: 0.5,
        distance_metric: proximadb::compute::distance_computation::DistanceMetric::Euclidean,
    };
    let pruner = SpatialPruner::new(config);

    let query_code = SpatialCode::Code64(100);
    let query_vector = vec![1.0, 0.0, 0.0, 0.0];

    let blocks = vec![
        BlockPruningInfo::with_centroid(
            0,
            SpatialCode::Code64(50),
            vec![0.9, 0.1, 0.0, 0.0], // Close centroid
        ),
        BlockPruningInfo::with_centroid(
            1,
            SpatialCode::Code64(200),
            vec![0.0, 1.0, 0.0, 0.0], // Far centroid
        ),
        BlockPruningInfo::with_centroid(
            2,
            SpatialCode::Code64(110),
            vec![0.95, 0.05, 0.0, 0.0], // Closest centroid
        ),
        BlockPruningInfo::with_centroid(
            3,
            SpatialCode::Code64(90),
            vec![0.5, 0.5, 0.0, 0.0], // Medium
        ),
    ];

    let result = pruner.select_blocks(&query_code, &query_vector, &blocks);

    assert_eq!(result.selected_indices.len(), 3);
    assert_eq!(result.pruned_count, 1);

    // Block 2 should be in top 3 (closest centroid and close code)
    assert!(result.selected_indices.contains(&2));
}

#[tokio::test]
async fn test_clustering_pipeline_zorder() {
    let config = ClusteringConfig::for_engine(CurveType::ZOrder);
    let pipeline = SpatialClusteringPipeline::new_in_memory(config);

    // Create blocks from clustered data
    let records = generate_clustered_vectors(5, 20, 32);
    let mut blocks: Vec<BlockInfo> = (0..5)
        .map(|i| {
            let start = i * 20;
            let end = start + 20;
            let vectors: Vec<Vec<f32>> = records[start..end]
                .iter()
                .map(|r| r.vector.clone())
                .collect();
            BlockInfo::from_vectors(i, vectors)
        })
        .collect();

    // Cluster blocks
    let result = pipeline.cluster_blocks(&mut blocks).await.unwrap();

    assert!(result.clustering_applied);
    assert_eq!(result.sorted_indices.len(), 5);
    assert_eq!(result.spatial_codes.len(), 5);
    assert_eq!(result.curve_type, CurveType::ZOrder);

    // All indices should be present
    let mut sorted = result.sorted_indices.clone();
    sorted.sort();
    assert_eq!(sorted, vec![0, 1, 2, 3, 4]);
}

#[tokio::test]
async fn test_clustering_pipeline_hilbert() {
    let config = ClusteringConfig::for_engine(CurveType::Hilbert);
    let pipeline = SpatialClusteringPipeline::new_in_memory(config);

    let records = generate_clustered_vectors(4, 25, 64);
    let mut blocks: Vec<BlockInfo> = (0..4)
        .map(|i| {
            let start = i * 25;
            let end = start + 25;
            let vectors: Vec<Vec<f32>> = records[start..end]
                .iter()
                .map(|r| r.vector.clone())
                .collect();
            BlockInfo::from_vectors(i, vectors)
        })
        .collect();

    let result = pipeline.cluster_blocks(&mut blocks).await.unwrap();

    assert!(result.clustering_applied);
    assert_eq!(result.curve_type, CurveType::Hilbert);
}

#[tokio::test]
async fn test_end_to_end_clustering_and_pruning() {
    // Test complete pipeline: training, clustering, then pruning

    // 1. Generate clustered data
    let records = generate_clustered_vectors(10, 50, 128);

    // 2. Train PCA model
    let pca_model = EnhancedPCAModel::train(&records, 8).unwrap();
    println!(
        "PCA variance explained: {:.2}%",
        pca_model.variance_explained() * 100.0
    );

    // 3. Create blocks and cluster them
    let config = ClusteringConfig::for_engine(CurveType::ZOrder);
    let pipeline = SpatialClusteringPipeline::new_in_memory(config);

    let mut blocks: Vec<BlockInfo> = (0..10)
        .map(|i| {
            let start = i * 50;
            let end = start + 50;
            let vectors: Vec<Vec<f32>> = records[start..end]
                .iter()
                .map(|r| r.vector.clone())
                .collect();
            BlockInfo::from_vectors(i, vectors)
        })
        .collect();

    let cluster_result = pipeline.cluster_blocks(&mut blocks).await.unwrap();
    println!("Clustered {} blocks", cluster_result.sorted_indices.len());

    // 4. Simulate search with pruning
    let pruner_config = PruningConfig {
        mode: PruningMode::Sqrt { min_blocks: 3 },
        ..Default::default()
    };
    let pruner = SpatialPruner::new(pruner_config);

    // Query from first cluster
    let query_vector = records[0].vector.clone();
    let query_projected = pca_model.project(&query_vector).unwrap();

    // Normalize for encoding
    let normalized: Vec<f32> = query_projected
        .iter()
        .map(|&v| ((v + 10.0) / 20.0).clamp(0.0, 1.0))
        .collect();

    let encoder = SpatialEncoderFactory::create(CurveType::ZOrder, 8, 8);
    let query_code = encoder.encode(&normalized);

    // Create pruning info from cluster result
    let pruning_blocks: Vec<BlockPruningInfo> = cluster_result
        .spatial_codes
        .iter()
        .enumerate()
        .zip(cluster_result.centroids.iter())
        .map(|((idx, code), centroid)| {
            BlockPruningInfo::with_centroid(idx, code.clone(), centroid.clone())
        })
        .collect();

    let prune_result = pruner.select_blocks(&query_code, &query_vector, &pruning_blocks);

    println!(
        "Selected {}/{} blocks, pruning ratio: {:.1}%",
        prune_result.selected_indices.len(),
        prune_result.total_blocks,
        prune_result.pruning_ratio * 100.0
    );

    // Sqrt(10) = ~3.16, min 3, so should select ~4 blocks
    assert!(prune_result.selected_indices.len() <= 5);
    assert!(prune_result.pruning_ratio > 0.5); // Should prune at least 50%
}

#[test]
fn test_curve_type_locality_comparison() {
    // Compare locality of Z-order vs Hilbert for same data

    let zorder = SpatialEncoderFactory::create(CurveType::ZOrder, 4, 8);
    let hilbert = SpatialEncoderFactory::create(CurveType::Hilbert, 4, 8);

    // Test vectors in a grid
    let test_points: Vec<Vec<f32>> = vec![
        vec![0.0, 0.0, 0.0, 0.0],
        vec![0.1, 0.0, 0.0, 0.0],
        vec![0.0, 0.1, 0.0, 0.0],
        vec![0.1, 0.1, 0.0, 0.0],
        vec![1.0, 1.0, 1.0, 1.0],
    ];

    let zorder_codes: Vec<SpatialCode> = test_points.iter().map(|p| zorder.encode(p)).collect();
    let hilbert_codes: Vec<SpatialCode> = test_points.iter().map(|p| hilbert.encode(p)).collect();

    // Locality quality: close points should have close codes
    // Both should work, but Hilbert typically has better locality
    println!("Z-order codes: {:?}", zorder_codes);
    println!("Hilbert codes: {:?}", hilbert_codes);

    // Basic sanity check: first 4 points should have closer codes than point 5
    let z_dist_01 = code_distance(&zorder_codes[0], &zorder_codes[1]);
    let z_dist_04 = code_distance(&zorder_codes[0], &zorder_codes[4]);
    assert!(z_dist_01 < z_dist_04, "Close points should have close Z-order codes");
}

#[test]
fn test_different_pruning_modes() {
    let block_codes: Vec<SpatialCode> = (0..100).map(|i| SpatialCode::Code64(i * 10)).collect();
    let query_code = SpatialCode::Code64(500);

    // Test sqrt mode
    let sqrt_pruner = SpatialPruner::new(PruningConfig {
        mode: PruningMode::Sqrt { min_blocks: 3 },
        ..Default::default()
    });
    let sqrt_result = sqrt_pruner.select_blocks_by_code(&query_code, &block_codes);
    assert_eq!(sqrt_result.len(), 10); // sqrt(100) = 10

    // Test ratio mode
    let ratio_pruner = SpatialPruner::new(PruningConfig {
        mode: PruningMode::Ratio {
            ratio: 0.2,
            min_blocks: 5,
        },
        ..Default::default()
    });
    let ratio_result = ratio_pruner.select_blocks_by_code(&query_code, &block_codes);
    assert_eq!(ratio_result.len(), 20); // 100 * 0.2 = 20

    // Test fixed mode
    let fixed_pruner = SpatialPruner::new(PruningConfig {
        mode: PruningMode::Fixed { k: 15 },
        ..Default::default()
    });
    let fixed_result = fixed_pruner.select_blocks_by_code(&query_code, &block_codes);
    assert_eq!(fixed_result.len(), 15);

    // Test exact mode
    let exact_pruner = SpatialPruner::new(PruningConfig::exact_mode());
    let exact_result = exact_pruner.select_blocks_by_code(&query_code, &block_codes);
    assert_eq!(exact_result.len(), 100); // All blocks
}
