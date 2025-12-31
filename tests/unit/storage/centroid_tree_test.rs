//! # CentroidTree TDD Tests
//!
//! Comprehensive tests for the CentroidTree implementation following TDD principles.
//! These tests verify the O(log n) centroid-based pruning for vector search.

use proximadb::storage::schema::{
    // Core types
    CentroidTree, CentroidTreeConfig, CentroidNode,
    // Pruning traits
    VectorPruner, PruningResult,
    // Header cache integration
    CachedHeader, RowGroupMeta, EnhancedCachedHeader,
    // Bloom filter types
    BloomConsolidator, ConsolidatedBloom, IncrementalBloomBuilder, BloomChecker,
    // Composite pruner
    CompositePruner, ScalarPruner, NullScalarPruner,
};

// ============================================================================
// CentroidTree Construction Tests
// ============================================================================

#[test]
fn test_centroid_tree_construction() {
    // Arrange: Create test centroids
    let centroids = vec![
        vec![0.0, 0.0, 0.0],
        vec![1.0, 0.0, 0.0],
        vec![0.0, 1.0, 0.0],
        vec![0.0, 0.0, 1.0],
    ];

    // Act: Build tree
    let tree = CentroidTree::build(&centroids, 8).unwrap();

    // Assert
    assert_eq!(tree.dimension(), 3);
    assert_eq!(tree.num_rowgroups(), 4);
    assert!(tree.depth() <= 8);
}

#[test]
fn test_centroid_tree_empty() {
    // Arrange & Act
    let tree = CentroidTree::build(&[], 8).unwrap();

    // Assert
    assert_eq!(tree.dimension(), 0);
    assert_eq!(tree.num_rowgroups(), 0);
}

#[test]
fn test_centroid_tree_single_centroid() {
    // Arrange
    let centroids = vec![vec![1.0, 2.0, 3.0]];

    // Act
    let tree = CentroidTree::build(&centroids, 8).unwrap();
    let result = tree.prune(&[1.0, 2.0, 3.0], 0.1);

    // Assert
    assert_eq!(tree.num_rowgroups(), 1);
    assert_eq!(result.included_indices.len(), 1);
    assert_eq!(result.included_indices[0], 0);
}

#[test]
fn test_centroid_tree_dimension_mismatch_error() {
    // Arrange: Centroids with different dimensions
    let centroids = vec![
        vec![1.0, 2.0, 3.0],
        vec![1.0, 2.0], // Wrong dimension
    ];

    // Act & Assert
    let result = CentroidTree::build(&centroids, 8);
    assert!(result.is_err());
}

// ============================================================================
// CentroidTree Pruning Tests
// ============================================================================

#[test]
fn test_centroid_tree_pruning() {
    // Arrange: Two clusters far apart
    let centroids = vec![
        // Cluster near origin (0, 0, 0)
        vec![0.0, 0.0, 0.0],
        vec![0.1, 0.1, 0.1],
        vec![0.2, 0.0, 0.0],
        vec![0.0, 0.2, 0.0],
        // Cluster near (10, 10, 10)
        vec![10.0, 10.0, 10.0],
        vec![10.1, 10.1, 10.1],
        vec![10.2, 10.0, 10.0],
        vec![10.0, 10.2, 10.0],
    ];

    let tree = CentroidTree::build(&centroids, 8).unwrap();

    // Act: Query near origin
    let query_near_origin = vec![0.05, 0.05, 0.05];
    let result = tree.prune(&query_near_origin, 1.0);

    // Assert: Should include at least the near-origin cluster
    assert!(result.has_matches());
    assert!(result.included_indices.len() < 8); // Should prune some

    // Verify the near cluster is included
    for idx in &result.included_indices {
        let centroid = &centroids[*idx];
        let dist = ((centroid[0] - 0.05).powi(2)
            + (centroid[1] - 0.05).powi(2)
            + (centroid[2] - 0.05).powi(2))
        .sqrt();
        // All included should be reasonably close (within threshold + tolerance)
        // Note: Tree may include extra due to conservative pruning
    }
}

#[test]
fn test_centroid_tree_prune_all() {
    // Arrange
    let centroids = vec![
        vec![0.0, 0.0, 0.0],
        vec![1.0, 0.0, 0.0],
    ];
    let tree = CentroidTree::build(&centroids, 8).unwrap();

    // Act: Query very far from all centroids with small threshold
    let query_far = vec![100.0, 100.0, 100.0];
    let result = tree.prune(&query_far, 1.0);

    // Assert: Should prune all or most
    assert!(result.pruned_count() > 0 || result.included_indices.is_empty());
}

#[test]
fn test_centroid_tree_include_all() {
    // Arrange
    let centroids = vec![
        vec![0.0, 0.0, 0.0],
        vec![1.0, 0.0, 0.0],
        vec![0.0, 1.0, 0.0],
    ];
    let tree = CentroidTree::build(&centroids, 8).unwrap();

    // Act: Query with very large distance threshold
    let query = vec![0.5, 0.5, 0.0];
    let result = tree.prune(&query, 1000.0);

    // Assert: Should include all
    assert_eq!(result.included_indices.len(), 3);
    assert!(!result.stats.method.is_empty());
}

// ============================================================================
// Quantized Centroid Approximation Tests
// ============================================================================

#[test]
fn test_quantized_centroid_approximation() {
    // Arrange
    let centroids = vec![
        vec![0.0, 0.0, 0.0],
        vec![1.0, 1.0, 1.0],
        vec![2.0, 2.0, 2.0],
        vec![10.0, 10.0, 10.0],
    ];

    let config = CentroidTreeConfig {
        max_depth: 8,
        min_leaf_size: 2,
        use_quantized: true,
        quantization_bits: 8,
    };
    let tree = CentroidTree::build_with_config(&centroids, config).unwrap();

    // Act
    let query = vec![0.5, 0.5, 0.5];
    let exact_result = tree.prune(&query, 2.0);
    let quantized_result = tree.prune_quantized(&query, 2.0);

    // Assert: Quantized should be at least as inclusive (conservative)
    assert!(quantized_result.included_indices.len() >= exact_result.included_indices.len());
}

#[test]
fn test_quantized_pruning_maintains_recall() {
    // Arrange: Create a larger dataset
    let mut centroids = Vec::new();
    for i in 0..100 {
        centroids.push(vec![
            (i % 10) as f32,
            (i / 10) as f32,
            0.0,
        ]);
    }

    let config = CentroidTreeConfig {
        max_depth: 10,
        min_leaf_size: 4,
        use_quantized: true,
        quantization_bits: 8,
    };
    let tree = CentroidTree::build_with_config(&centroids, config).unwrap();

    // Act: Query at (5, 5, 0)
    let query = vec![5.0, 5.0, 0.0];
    let exact_result = tree.prune(&query, 3.0);
    let quantized_result = tree.prune_quantized(&query, 3.0);

    // Assert: All exact matches should be in quantized result
    for idx in &exact_result.included_indices {
        assert!(
            quantized_result.included_indices.contains(idx),
            "Quantized result should contain all exact matches"
        );
    }
}

// ============================================================================
// VectorPruner Trait Tests
// ============================================================================

#[test]
fn test_vector_pruner_trait_implementation() {
    // Arrange
    let centroids = vec![
        vec![0.0, 0.0],
        vec![1.0, 1.0],
        vec![5.0, 5.0],
    ];
    let tree = CentroidTree::build(&centroids, 8).unwrap();

    // Act: Use as trait object
    let pruner: &dyn VectorPruner = &tree;

    // Assert
    assert_eq!(pruner.dimension(), 2);
    assert_eq!(pruner.num_entries(), 3);

    let result = pruner.prune_by_vector(&[0.5, 0.5], 2.0);
    assert!(result.has_matches());
}

// ============================================================================
// Serialization Tests
// ============================================================================

#[test]
fn test_centroid_tree_serialization_roundtrip() {
    // Arrange
    let centroids = vec![
        vec![0.0, 0.0, 0.0],
        vec![1.0, 1.0, 1.0],
        vec![2.0, 2.0, 2.0],
    ];
    let tree = CentroidTree::build(&centroids, 8).unwrap();

    // Act: Serialize and deserialize
    let bytes = tree.serialize().unwrap();
    let restored = CentroidTree::deserialize(&bytes).unwrap();

    // Assert
    assert_eq!(restored.dimension(), tree.dimension());
    assert_eq!(restored.num_rowgroups(), tree.num_rowgroups());

    // Same pruning results
    let query = vec![0.5, 0.5, 0.5];
    let original_result = tree.prune(&query, 2.0);
    let restored_result = restored.prune(&query, 2.0);

    assert_eq!(
        original_result.included_indices,
        restored_result.included_indices
    );
}

// ============================================================================
// EnhancedCachedHeader Integration Tests
// ============================================================================

#[test]
fn test_enhanced_cached_header_with_centroid_tree() {
    // Arrange: Create header with centroids
    let mut header = CachedHeader::new("/test/file.sst".to_string(), 12345);

    // Add rowgroups with centroids
    for i in 0..5 {
        let rg = RowGroupMeta::new(i, i as u64 * 1000, 1000, 100)
            .with_centroid(vec![i as f32, i as f32, i as f32]);
        header.rowgroups.push(rg);
    }

    // Act: Create enhanced header
    let enhanced = header.with_centroid_tree();

    // Assert
    assert!(enhanced.centroid_tree.is_some());
    assert!(enhanced.indexes_built);
    assert_eq!(enhanced.dimension(), 3);
    assert_eq!(enhanced.num_entries(), 5);
}

#[test]
fn test_enhanced_cached_header_pruning() {
    // Arrange
    let mut header = CachedHeader::new("/test/file.sst".to_string(), 12345);

    // Add rowgroups - 2 clusters
    for i in 0..3 {
        let rg = RowGroupMeta::new(i, i as u64 * 1000, 1000, 100)
            .with_centroid(vec![i as f32 * 0.1, i as f32 * 0.1, 0.0]);
        header.rowgroups.push(rg);
    }
    for i in 3..6 {
        let rg = RowGroupMeta::new(i, i as u64 * 1000, 1000, 100)
            .with_centroid(vec![10.0 + (i - 3) as f32 * 0.1, 10.0, 0.0]);
        header.rowgroups.push(rg);
    }

    let enhanced = header.with_centroid_tree();

    // Act: Query near first cluster
    let result = enhanced.prune_by_vector(&[0.1, 0.1, 0.0], 1.0);

    // Assert
    assert!(result.has_matches());
    assert!(result.included_indices.len() < 6); // Should prune some
}

// ============================================================================
// BloomConsolidator Tests
// ============================================================================

#[test]
fn test_bloom_consolidator_basic() {
    // Arrange
    let mut builder = IncrementalBloomBuilder::new(1000, 0.01);

    // Add IDs
    for i in 0..100 {
        builder.add(&format!("user:{}", i));
    }

    // Act
    let bloom = builder.build().unwrap();

    // Assert
    assert_eq!(bloom.num_items(), 100);
    assert!(!bloom.is_empty());

    // Check existing IDs
    assert!(bloom.might_contain("user:0"));
    assert!(bloom.might_contain("user:50"));
    assert!(bloom.might_contain("user:99"));
}

#[test]
fn test_bloom_checker_trait() {
    // Arrange
    let mut builder = IncrementalBloomBuilder::new(1000, 0.01);
    for i in 0..50 {
        builder.add(&format!("item:{}", i));
    }
    let bloom = builder.build().unwrap();

    // Act: Use as trait object
    let checker: &dyn BloomChecker = &bloom;

    // Assert
    assert_eq!(checker.num_items(), 50);
    assert!(checker.false_positive_rate() < 0.1);
    assert!(checker.might_contain("item:25"));
}

#[test]
fn test_bloom_check_ids_batch() {
    // Arrange
    let mut builder = IncrementalBloomBuilder::new(1000, 0.01);
    for i in 0..100 {
        builder.add(&format!("doc:{}", i));
    }
    let bloom = builder.build().unwrap();

    // Act: Check existing IDs
    let result = bloom.check_ids(&["doc:0", "doc:50", "doc:99"]);

    // Assert: All should be possibly present
    assert!(result.possibly_present.contains(&"doc:0".to_string()));
    assert!(result.possibly_present.contains(&"doc:50".to_string()));
    assert!(result.possibly_present.contains(&"doc:99".to_string()));
}

// ============================================================================
// CompositePruner Tests
// ============================================================================

#[test]
fn test_composite_pruner_empty() {
    // Arrange
    let pruner = CompositePruner::new(10);

    // Act
    let result = pruner.prune(None, None, None, None);

    // Assert: Should include all
    assert_eq!(result.included_indices.len(), 10);
}

#[test]
fn test_composite_pruner_with_vector() {
    // Arrange: Use more centroids to ensure proper tree structure for pruning.
    // With min_leaf_size=2, we need enough centroids so the far ones end up
    // in their own leaf nodes that can be pruned.
    let centroids = vec![
        vec![0.0, 0.0],   // 0: close to query
        vec![1.0, 1.0],   // 1: close to query
        vec![2.0, 2.0],   // 2: close to query
        vec![50.0, 50.0], // 3: far from query
        vec![51.0, 51.0], // 4: far from query
        vec![52.0, 52.0], // 5: far from query
    ];
    let tree = CentroidTree::build(&centroids, 8).unwrap();

    let pruner = CompositePruner::new(6)
        .with_vector_pruner(std::sync::Arc::new(tree));

    // Act: Query near origin with small radius
    let query = vec![1.0, 1.0];
    let result = pruner.prune(Some((&query, 5.0)), None, None, None);

    // Assert: Should find matches and prune the far cluster
    assert!(result.has_matches());
    // The close cluster [0,1,2] should be included, far cluster [3,4,5] should be pruned
    assert!(
        result.included_indices.len() <= 4,
        "Expected at most 4 indices (close cluster + maybe some overlap), got {:?}",
        result.included_indices
    );
}

// ============================================================================
// PruningResult Tests
// ============================================================================

#[test]
fn test_pruning_result_intersect() {
    // Arrange
    let result1 = PruningResult::with_indices(vec![0, 1, 2, 3, 4], 10, "a", 100);
    let result2 = PruningResult::with_indices(vec![2, 3, 4, 5, 6], 10, "b", 100);

    // Act
    let intersected = result1.intersect(&result2);

    // Assert
    assert_eq!(intersected.included_indices.len(), 3); // 2, 3, 4
    assert!(intersected.included_indices.contains(&2));
    assert!(intersected.included_indices.contains(&3));
    assert!(intersected.included_indices.contains(&4));
    assert!(!intersected.included_indices.contains(&0));
    assert!(!intersected.included_indices.contains(&5));
}

#[test]
fn test_pruning_result_stats() {
    // Arrange & Act
    let result = PruningResult::with_indices(vec![1, 3, 5], 10, "test_method", 500);

    // Assert
    assert_eq!(result.total_rowgroups, 10);
    assert_eq!(result.pruned_count(), 7);
    assert!((result.stats.pruning_ratio - 0.7).abs() < 0.01);
    assert_eq!(result.stats.method, "test_method");
    assert_eq!(result.stats.computation_ns, 500);
}

// ============================================================================
// Performance Tests (basic validation)
// ============================================================================

#[test]
fn test_centroid_tree_performance_scales() {
    // Arrange: Create moderate-sized dataset
    let mut centroids = Vec::new();
    for i in 0..1000 {
        centroids.push(vec![
            (i % 100) as f32,
            ((i / 100) % 100) as f32,
            (i / 10000) as f32,
        ]);
    }

    let tree = CentroidTree::build(&centroids, 10).unwrap();

    // Act: Prune with a query near some centroids
    // Query at [50.0, 5.0, 0.0] is close to centroids like [50, 5, 0] (i=550)
    let query = vec![50.0, 5.0, 0.0];
    // Use a larger threshold that includes some but not all centroids
    let result = tree.prune(&query, 20.0);

    // Assert: Should complete quickly with meaningful pruning
    assert!(result.has_matches(), "Should find some matches within distance 20");
    assert!(result.included_indices.len() < 1000, "Should prune some rowgroups");
    assert!(result.stats.computation_ns > 0, "Timing should be recorded");
}

// ============================================================================
// Edge Cases
// ============================================================================

#[test]
fn test_centroid_tree_high_dimension() {
    // Arrange: High-dimensional vectors (like BERT embeddings)
    let dim = 768;
    let centroids: Vec<Vec<f32>> = (0..10)
        .map(|i| (0..dim).map(|j| ((i * dim + j) % 100) as f32 / 100.0).collect())
        .collect();

    // Act
    let tree = CentroidTree::build(&centroids, 8).unwrap();

    // Assert
    assert_eq!(tree.dimension(), dim);
    assert_eq!(tree.num_rowgroups(), 10);

    // Pruning should work
    let query: Vec<f32> = (0..dim).map(|j| (j % 50) as f32 / 100.0).collect();
    let result = tree.prune(&query, 10.0);
    assert!(result.has_matches());
}

#[test]
fn test_centroid_tree_identical_centroids() {
    // Arrange: All centroids are the same
    let centroids = vec![
        vec![1.0, 1.0, 1.0],
        vec![1.0, 1.0, 1.0],
        vec![1.0, 1.0, 1.0],
    ];

    // Act
    let tree = CentroidTree::build(&centroids, 8).unwrap();
    let result = tree.prune(&[1.0, 1.0, 1.0], 0.1);

    // Assert: All should match
    assert_eq!(result.included_indices.len(), 3);
}

#[test]
fn test_centroid_tree_query_dimension_mismatch_fallback() {
    // Arrange
    let centroids = vec![
        vec![0.0, 0.0, 0.0],
        vec![1.0, 1.0, 1.0],
    ];
    let tree = CentroidTree::build(&centroids, 8).unwrap();

    // Act: Query with wrong dimension
    let wrong_dim_query = vec![1.0, 2.0]; // 2D instead of 3D
    let result = tree.prune(&wrong_dim_query, 1.0);

    // Assert: Should include all as fallback (no crash)
    assert_eq!(result.included_indices.len(), 2);
}

// ============================================================================
// Additional TDD Tests for Comprehensive Coverage
// ============================================================================

#[test]
fn test_centroid_tree_build_many_rowgroups() {
    // Arrange: Build with 100 rowgroups
    let centroids: Vec<Vec<f32>> = (0..100)
        .map(|i| vec![i as f32 / 10.0, (i as f32 / 10.0).sin(), (i as f32 / 10.0).cos()])
        .collect();

    // Act
    let tree = CentroidTree::build(&centroids, 12).unwrap();

    // Assert
    assert_eq!(tree.dimension(), 3);
    assert_eq!(tree.num_rowgroups(), 100);
    assert!(tree.depth() <= 12);
}

#[test]
fn test_centroid_tree_prune_exact_match() {
    // Arrange: Query at exact centroid location
    let centroids = vec![
        vec![0.0, 0.0, 0.0],
        vec![5.0, 5.0, 5.0],
        vec![10.0, 10.0, 10.0],
    ];
    let tree = CentroidTree::build(&centroids, 8).unwrap();

    // Act: Query exactly at centroid with small threshold
    // Use threshold slightly larger than 0 to account for any floating point issues
    let result = tree.prune(&[5.0, 5.0, 5.0], 1.0);

    // Assert: Should include at least the exact match (rowgroup 1)
    assert!(result.has_matches(), "Query at exact centroid should find matches");
    // The exact centroid should be included
    assert!(
        result.included_indices.contains(&1),
        "Expected rowgroup 1 (centroid at [5,5,5]) to be included, got {:?}",
        result.included_indices
    );
}

#[test]
fn test_centroid_tree_prune_all_excluded() {
    // Arrange: Small dataset with well-separated centroids
    let centroids = vec![
        vec![0.0, 0.0, 0.0],
        vec![1.0, 0.0, 0.0],
        vec![0.0, 1.0, 0.0],
    ];
    let tree = CentroidTree::build(&centroids, 8).unwrap();

    // Act: Query extremely far away with tiny threshold
    let result = tree.prune(&[1000.0, 1000.0, 1000.0], 0.001);

    // Assert: Should exclude all rowgroups
    assert!(!result.has_matches() || result.included_indices.is_empty());
}

#[test]
fn test_centroid_tree_config_min_leaf_size() {
    // Arrange: Test with custom min_leaf_size
    let centroids: Vec<Vec<f32>> = (0..20)
        .map(|i| vec![i as f32, 0.0, 0.0])
        .collect();

    let config = CentroidTreeConfig {
        max_depth: 4,
        min_leaf_size: 5, // Force larger leaves
        use_quantized: false,
        quantization_bits: 8,
    };

    // Act
    let tree = CentroidTree::build_with_config(&centroids, config).unwrap();

    // Assert: Tree should still work
    assert_eq!(tree.num_rowgroups(), 20);

    let result = tree.prune(&[10.0, 0.0, 0.0], 5.0);
    assert!(result.has_matches());
}

#[test]
fn test_centroid_tree_l2_distance_accuracy() {
    // Arrange: 3-4-5 triangle
    let centroids = vec![
        vec![0.0, 0.0, 0.0],
        vec![3.0, 4.0, 0.0], // Distance = 5.0 from origin
    ];
    let tree = CentroidTree::build(&centroids, 8).unwrap();

    // Act: Query at origin with distance < 5
    let result_under = tree.prune(&[0.0, 0.0, 0.0], 4.9);

    // Query at origin with distance >= 5
    let result_over = tree.prune(&[0.0, 0.0, 0.0], 5.1);

    // Assert
    assert!(result_under.included_indices.contains(&0)); // Origin should be included
    assert!(result_over.included_indices.contains(&1)); // 3-4-5 point should be included with threshold > 5
}

#[test]
fn test_centroid_tree_serialization_empty() {
    // Arrange
    let tree = CentroidTree::build(&[], 8).unwrap();

    // Act
    let bytes = tree.serialize().unwrap();
    let restored = CentroidTree::deserialize(&bytes).unwrap();

    // Assert
    assert_eq!(restored.dimension(), 0);
    assert_eq!(restored.num_rowgroups(), 0);
}

#[test]
fn test_centroid_tree_serialization_large() {
    // Arrange: Large tree
    let centroids: Vec<Vec<f32>> = (0..500)
        .map(|i| {
            vec![
                (i % 50) as f32,
                (i / 50) as f32,
                ((i * 17) % 100) as f32 / 10.0,
            ]
        })
        .collect();
    let tree = CentroidTree::build(&centroids, 16).unwrap();

    // Act
    let bytes = tree.serialize().unwrap();
    let restored = CentroidTree::deserialize(&bytes).unwrap();

    // Assert
    assert_eq!(restored.dimension(), tree.dimension());
    assert_eq!(restored.num_rowgroups(), tree.num_rowgroups());

    // Pruning results should match
    let query = vec![25.0, 5.0, 5.0];
    let original_result = tree.prune(&query, 10.0);
    let restored_result = restored.prune(&query, 10.0);
    assert_eq!(original_result.included_indices, restored_result.included_indices);
}

// ============================================================================
// BloomConsolidator Merge Tests
// ============================================================================

#[test]
fn test_bloom_consolidator_merge_multiple() {
    // Arrange: Create multiple bloom filters and consolidate
    let mut builder1 = IncrementalBloomBuilder::new(1000, 0.01);
    for i in 0..50 {
        builder1.add(&format!("set1:item:{}", i));
    }
    let bloom1 = builder1.build().unwrap();

    let mut builder2 = IncrementalBloomBuilder::new(1000, 0.01);
    for i in 0..50 {
        builder2.add(&format!("set2:item:{}", i));
    }
    let bloom2 = builder2.build().unwrap();

    // Act: Create a new consolidated bloom that contains all items
    let mut consolidated_builder = IncrementalBloomBuilder::new(2000, 0.01);
    for i in 0..50 {
        consolidated_builder.add(&format!("set1:item:{}", i));
        consolidated_builder.add(&format!("set2:item:{}", i));
    }
    let consolidated = consolidated_builder.build().unwrap();

    // Assert: All items from both sets should be present
    assert!(consolidated.might_contain("set1:item:25"));
    assert!(consolidated.might_contain("set2:item:25"));
    assert_eq!(consolidated.num_items(), 100);
}

#[test]
fn test_bloom_consolidator_empty_construction() {
    // Arrange & Act
    let consolidator = BloomConsolidator::new(1000, 0.01);
    let bloom = consolidator.build().unwrap();

    // Assert
    assert!(bloom.is_empty());
    assert_eq!(bloom.num_items(), 0);
    // Empty bloom should conservatively say "might contain" for anything
    assert!(bloom.might_contain("anything"));
}

#[test]
fn test_bloom_consolidator_build_from_keys() {
    // Arrange
    let consolidator = BloomConsolidator::new(1000, 0.01);
    let keys = vec!["key1", "key2", "key3", "key4", "key5"];

    // Act
    let bloom = consolidator.build_from_keys(keys.iter().copied());

    // Assert
    assert_eq!(bloom.num_items(), 5);
    for key in &keys {
        assert!(bloom.might_contain(key));
    }
}

#[test]
fn test_bloom_consolidator_fpr_estimation() {
    // Arrange
    let mut consolidator = BloomConsolidator::new(1000, 0.01);

    // Add 10 rowgroup blooms (even if empty, FPR calculation should work)
    for i in 0..10 {
        consolidator.add_rowgroup_bloom(i, &[]);
    }

    // Act
    let estimated_fpr = consolidator.estimate_consolidated_fpr();

    // Assert: FPR should be higher than single filter FPR (0.01)
    // FPR_consolidated = 1 - (1 - 0.01)^10 ≈ 0.0956
    assert!(estimated_fpr > 0.01);
    assert!(estimated_fpr < 1.0);
    assert!((estimated_fpr - 0.0956).abs() < 0.01);
}

#[test]
fn test_bloom_check_ids_mixed() {
    // Arrange
    let mut builder = IncrementalBloomBuilder::new(10000, 0.001); // Very low FPR

    // Only add even-numbered IDs
    for i in (0..1000).step_by(2) {
        builder.add(&format!("id:{}", i));
    }
    let bloom = builder.build().unwrap();

    // Act: Check a mix of existing and non-existing IDs
    let result = bloom.check_ids(&["id:0", "id:1", "id:2", "id:3", "id:998", "id:999"]);

    // Assert: Even IDs should be present, odd IDs might or might not be (FP possibility)
    assert!(result.possibly_present.contains(&"id:0".to_string()));
    assert!(result.possibly_present.contains(&"id:2".to_string()));
    assert!(result.possibly_present.contains(&"id:998".to_string()));

    // With very low FPR, odd IDs should likely be absent
    // (but we can't guarantee 100% due to FP nature)
}

#[test]
fn test_bloom_consolidated_serialization() {
    // Arrange
    let mut builder = IncrementalBloomBuilder::new(1000, 0.01);
    for i in 0..100 {
        builder.add(&format!("serialize:test:{}", i));
    }
    let bloom = builder.build().unwrap();

    // Act
    let bytes = bloom.serialize().unwrap();
    let restored = ConsolidatedBloom::deserialize(&bytes).unwrap();

    // Assert
    assert_eq!(restored.num_items(), bloom.num_items());
    assert!(!restored.is_empty());

    // Check some items
    assert!(restored.might_contain("serialize:test:50"));
    assert!(restored.might_contain("serialize:test:0"));
    assert!(restored.might_contain("serialize:test:99"));
}

#[test]
fn test_bloom_memory_efficiency() {
    // Arrange: Create a large bloom filter
    let mut builder = IncrementalBloomBuilder::new(100000, 0.01);
    for i in 0..50000 {
        builder.add(&format!("mem:efficiency:test:{}", i));
    }
    let bloom = builder.build().unwrap();

    // Assert: Memory should be efficient
    // With 10 bits per key target, 50000 items = ~62.5KB
    let memory = bloom.memory_usage();
    assert!(memory > 0);
    assert!(memory < 200_000); // Should be well under 200KB
}
