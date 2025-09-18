//! Unit Tests for AXIS Clustering
//!
//! Tests the actual clustering implementation from the production code.

use proximadb::index::axis::clustering::{
    ClusterManager, ClusteringAlgorithm, ClusteringConfig, KMeansConfig, KMeansInit,
};
use proximadb::compute::distance_computation::DistanceMetric;
use tokio::test;
use anyhow::Result;

/// Generate test vectors for clustering
fn generate_test_vectors(count: usize, dimension: usize, num_clusters: usize) -> Vec<Vec<f32>> {
    let mut vectors = Vec::with_capacity(count);

    for i in 0..count {
        let mut vector = Vec::with_capacity(dimension);
        let cluster_id = i % num_clusters;

        for j in 0..dimension {
            // Create clustered data with some noise
            let base_value = cluster_id as f32 * 10.0;
            let noise = ((i * j) as f32 * 0.01) % 1.0;
            vector.push(base_value + noise);
        }
        vectors.push(vector);
    }

    vectors
}

#[test]
async fn test_cluster_manager_creation() -> Result<()> {
    let config = ClusteringConfig {
        algorithm: ClusteringAlgorithm::KMeans(KMeansConfig::default()),
        min_vectors_for_clustering: 100,
        max_clusters: 10,
        distance_metric: DistanceMetric::Cosine,
        adaptive_cluster_count: false,
        recompute_threshold: 1000,
        enable_incremental: false,
    };

    let _manager = ClusterManager::new(config).await?;
    // If we reach here without error, the manager was created successfully

    Ok(())
}

#[test]
async fn test_kmeans_clustering() -> Result<()> {
    let kmeans_config = KMeansConfig {
        k: 3,
        max_iterations: 50,
        tolerance: 1e-4,
        n_init: 1,
        init_method: KMeansInit::KMeansPlusPlus,
    };

    let config = ClusteringConfig {
        algorithm: ClusteringAlgorithm::KMeans(kmeans_config),
        min_vectors_for_clustering: 10,
        max_clusters: 10,
        distance_metric: DistanceMetric::Euclidean,
        adaptive_cluster_count: false,
        recompute_threshold: 1000,
        enable_incremental: false,
    };

    let mut manager = ClusterManager::new(config).await?;

    // Generate test data with 3 clear clusters
    let vectors = generate_test_vectors(300, 128, 3);

    // Perform clustering
    let assignments = manager.cluster_vectors(&vectors).await?;

    // Verify we got assignments for all vectors
    assert_eq!(assignments.len(), vectors.len(), "Should assign all vectors");

    // Check that we have 3 distinct clusters
    let mut unique_clusters = std::collections::HashSet::new();
    for assignment in &assignments {
        unique_clusters.insert(assignment.cluster_id);
    }

    // We should have approximately 3 clusters (allow some variance)
    assert!(unique_clusters.len() <= 5, "Should find approximately 3 clusters");
    assert!(unique_clusters.len() >= 2, "Should find at least 2 clusters");

    Ok(())
}

#[test]
async fn test_adaptive_cluster_count() -> Result<()> {
    let config = ClusteringConfig {
        algorithm: ClusteringAlgorithm::KMeans(KMeansConfig::default()),
        min_vectors_for_clustering: 10,
        max_clusters: 20,
        distance_metric: DistanceMetric::Cosine,
        adaptive_cluster_count: true, // Enable adaptive clustering
        recompute_threshold: 1000,
        enable_incremental: false,
    };

    let mut manager = ClusterManager::new(config).await?;

    // Test with different sized datasets
    for (num_vectors, expected_max_clusters) in [(50, 5), (200, 10), (500, 15)] {
        let vectors = generate_test_vectors(num_vectors, 64, 5);
        let assignments = manager.cluster_vectors(&vectors).await?;

        let mut unique_clusters = std::collections::HashSet::new();
        for assignment in &assignments {
            unique_clusters.insert(assignment.cluster_id);
        }

        assert!(
            unique_clusters.len() <= expected_max_clusters,
            "Dataset of {} vectors should have <= {} clusters, got {}",
            num_vectors,
            expected_max_clusters,
            unique_clusters.len()
        );
    }

    Ok(())
}

#[test]
async fn test_empty_vector_clustering() -> Result<()> {
    let config = ClusteringConfig::default();
    let mut manager = ClusterManager::new(config).await?;

    let empty_vectors: Vec<Vec<f32>> = Vec::new();
    let assignments = manager.cluster_vectors(&empty_vectors).await?;

    assert!(assignments.is_empty(), "Empty input should produce empty assignments");

    Ok(())
}

#[test]
async fn test_single_vector_clustering() -> Result<()> {
    let config = ClusteringConfig {
        algorithm: ClusteringAlgorithm::KMeans(KMeansConfig {
            k: 1,
            ..Default::default()
        }),
        min_vectors_for_clustering: 1,
        ..Default::default()
    };

    let mut manager = ClusterManager::new(config).await?;

    let vectors = vec![vec![1.0, 2.0, 3.0, 4.0]];
    let assignments = manager.cluster_vectors(&vectors).await?;

    assert_eq!(assignments.len(), 1, "Should have one assignment");
    assert_eq!(assignments[0].cluster_id, 0, "Single vector should be in cluster 0");

    Ok(())
}

#[test]
async fn test_different_distance_metrics() -> Result<()> {
    let vectors = generate_test_vectors(100, 32, 3);

    for metric in [DistanceMetric::Euclidean, DistanceMetric::Cosine, DistanceMetric::Manhattan] {
        let config = ClusteringConfig {
            algorithm: ClusteringAlgorithm::KMeans(KMeansConfig {
                k: 3,
                ..Default::default()
            }),
            distance_metric: metric,
            min_vectors_for_clustering: 10,
            ..Default::default()
        };

        let mut manager = ClusterManager::new(config).await?;
        let assignments = manager.cluster_vectors(&vectors).await?;

        assert_eq!(
            assignments.len(),
            vectors.len(),
            "Distance metric {:?} should produce assignments for all vectors",
            metric
        );
    }

    Ok(())
}