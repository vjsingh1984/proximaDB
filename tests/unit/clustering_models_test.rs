//! Unit Tests for Clustering Models and Stats
//!
//! Tests the clustering model manager, K-means++ implementation,
//! model persistence, and statistics tracking.

use std::path::PathBuf;
use tempfile::TempDir;
use tokio::test;

use proximadb::storage::engines::viper::clustering_models::{
    ClusteringModelManager, ClusteringStats, EfficientClusteringModel,
    MIN_VECTORS_FOR_CLUSTERING,
};

/// Test helper to create a temporary directory for tests
fn create_temp_models_dir() -> TempDir {
    TempDir::new().expect("Failed to create temp directory")
}

/// Generate test vectors for clustering
fn generate_test_vectors(count: usize, dimension: usize) -> Vec<Vec<f32>> {
    let mut vectors = Vec::with_capacity(count);
    
    for i in 0..count {
        let mut vector = Vec::with_capacity(dimension);
        for j in 0..dimension {
            // Create clustered data with some noise
            let cluster_id = i % 3; // 3 clusters
            let base_value = cluster_id as f32 * 10.0;
            let noise = (i * j) as f32 * 0.01;
            vector.push(base_value + noise);
        }
        vectors.push(vector);
    }
    
    vectors
}

#[test]
async fn test_clustering_model_manager_creation() {
    let temp_dir = create_temp_models_dir();
    let manager = ClusteringModelManager::new(temp_dir.path().to_path_buf())
        .expect("Failed to create clustering model manager");
    
    // Check that directories were created
    assert!(temp_dir.path().join("__models").exists());
    assert!(temp_dir.path().join("__models/clustering").exists());
    assert!(temp_dir.path().join("__models/stats").exists());
}

#[test]
async fn test_small_collection_skip() {
    let temp_dir = create_temp_models_dir();
    let manager = ClusteringModelManager::new(temp_dir.path().to_path_buf())
        .expect("Failed to create clustering model manager");
    
    // Test with small collection (< 1M vectors)
    let small_collection = "small_test";
    let small_vector_count = 1000;
    let dimension = 128;
    
    let model = manager.get_clustering_model(small_collection, small_vector_count, dimension)
        .await
        .expect("Failed to get clustering model");
    
    // Should return None for small collections
    assert!(model.is_none(), "Small collection should not trigger clustering");
}

#[test]
async fn test_large_collection_model_creation() {
    let temp_dir = create_temp_models_dir();
    let manager = ClusteringModelManager::new(temp_dir.path().to_path_buf())
        .expect("Failed to create clustering model manager");
    
    // Test with large collection (>1M vectors)
    let large_collection = "large_test";
    let large_vector_count = MIN_VECTORS_FOR_CLUSTERING + 100000;
    let dimension = 256;
    
    // First call should trigger training (returns None while training queued)
    let model = manager.get_clustering_model(large_collection, large_vector_count, dimension)
        .await
        .expect("Failed to get clustering model");
    
    // Should queue training and return None initially
    assert!(model.is_none(), "Large collection should queue training on first call");
}

#[test]
async fn test_kmeans_plus_plus_initialization() {
    let temp_dir = create_temp_models_dir();
    let manager = ClusteringModelManager::new(temp_dir.path().to_path_buf())
        .expect("Failed to create clustering model manager");
    
    // Generate test vectors
    let vectors = generate_test_vectors(10000, 128);
    let collection_id = "kmeans_test";
    
    // Train model
    let model = manager.train_model(collection_id, &vectors, 128)
        .await
        .expect("Failed to train clustering model");
    
    // Verify model properties
    assert_eq!(model.collection_id, collection_id);
    assert_eq!(model.dimension, 128);
    assert!(model.centroids.len() > 0, "Model should have centroids");
    assert!(model.centroids.len() <= 256, "Should not exceed max clusters");
    
    // Verify centroids have correct dimension
    for centroid in &model.centroids {
        assert_eq!(centroid.len(), 128, "Centroid should have correct dimension");
    }
}

#[test]
async fn test_clustering_statistics() {
    let temp_dir = create_temp_models_dir();
    let manager = ClusteringModelManager::new(temp_dir.path().to_path_buf())
        .expect("Failed to create clustering model manager");
    
    // Generate test vectors
    let vectors = generate_test_vectors(5000, 64);
    let collection_id = "stats_test";
    
    // Train model
    let model = manager.train_model(collection_id, &vectors, 64)
        .await
        .expect("Failed to train clustering model");
    
    // Verify statistics
    let stats = &model.stats;
    assert_eq!(stats.collection_id, collection_id);
    assert_eq!(stats.total_vectors, 5000);
    assert!(stats.cluster_count > 0);
    assert!(stats.avg_vectors_per_cluster > 0.0);
    assert!(stats.training_time_ms > 0);
    assert!(stats.silhouette_score >= 0.0);
    assert!(stats.intra_cluster_distance >= 0.0);
    assert!(stats.inter_cluster_distance >= 0.0);
    
    // Verify cluster sizes
    assert_eq!(stats.cluster_sizes.len(), stats.cluster_count);
    let total_clustered: usize = stats.cluster_sizes.iter().sum();
    assert_eq!(total_clustered, 5000, "All vectors should be clustered");
}

#[test]
async fn test_model_persistence() {
    let temp_dir = create_temp_models_dir();
    let manager = ClusteringModelManager::new(temp_dir.path().to_path_buf())
        .expect("Failed to create clustering model manager");
    
    // Generate test vectors
    let vectors = generate_test_vectors(2000, 32);
    let collection_id = "persistence_test";
    
    // Train and save model
    let original_model = manager.train_model(collection_id, &vectors, 32)
        .await
        .expect("Failed to train clustering model");
    
    // Verify files were created
    let model_file = temp_dir.path().join("__models/clustering").join(format!("{}.json", collection_id));
    let stats_file = temp_dir.path().join("__models/stats").join(format!("{}.json", collection_id));
    
    assert!(model_file.exists(), "Model file should be created");
    assert!(stats_file.exists(), "Stats file should be created");
    
    // Create new manager instance to test loading
    let manager2 = ClusteringModelManager::new(temp_dir.path().to_path_buf())
        .expect("Failed to create second clustering model manager");
    
    // Load model from disk
    let loaded_model = manager2.load_model_from_disk(collection_id)
        .await
        .expect("Failed to load model from disk")
        .expect("Model should exist on disk");
    
    // Verify loaded model matches original
    assert_eq!(loaded_model.collection_id, original_model.collection_id);
    assert_eq!(loaded_model.dimension, original_model.dimension);
    assert_eq!(loaded_model.centroids.len(), original_model.centroids.len());
    assert_eq!(loaded_model.stats.total_vectors, original_model.stats.total_vectors);
}

#[test]
async fn test_online_model_update() {
    let temp_dir = create_temp_models_dir();
    let manager = ClusteringModelManager::new(temp_dir.path().to_path_buf())
        .expect("Failed to create clustering model manager");
    
    // Generate initial vectors
    let initial_vectors = generate_test_vectors(1000, 64);
    let collection_id = "update_test";
    
    // Train initial model
    let _model = manager.train_model(collection_id, &initial_vectors, 64)
        .await
        .expect("Failed to train initial model");
    
    // Generate new vectors for update
    let new_vectors = generate_test_vectors(100, 64);
    
    // Update model with new vectors
    manager.update_model_online(collection_id, &new_vectors)
        .await
        .expect("Failed to update model online");
    
    // Verify model was updated
    let updated_model = manager.get_clustering_model(collection_id, 1100, 64)
        .await
        .expect("Failed to get updated model")
        .expect("Updated model should exist");
    
    // Check that learning rate was decayed
    assert!(updated_model.learning_rate < 0.01, "Learning rate should be decayed");
    
    // Check that version was incremented
    assert!(updated_model.version > 1, "Version should be incremented");
}

#[test]
async fn test_model_stats_retrieval() {
    let temp_dir = create_temp_models_dir();
    let manager = ClusteringModelManager::new(temp_dir.path().to_path_buf())
        .expect("Failed to create clustering model manager");
    
    // Generate test vectors
    let vectors = generate_test_vectors(1500, 96);
    let collection_id = "stats_retrieval_test";
    
    // Train model
    let _model = manager.train_model(collection_id, &vectors, 96)
        .await
        .expect("Failed to train clustering model");
    
    // Retrieve stats
    let stats = manager.get_model_stats(collection_id)
        .await
        .expect("Stats should exist for trained model");
    
    // Verify stats content
    assert_eq!(stats.collection_id, collection_id);
    assert_eq!(stats.total_vectors, 1500);
    assert!(stats.cluster_count > 0);
    assert!(stats.training_time_ms > 0);
    
    // Test non-existent collection
    let no_stats = manager.get_model_stats("nonexistent_collection").await;
    assert!(no_stats.is_none(), "Non-existent collection should return None");
}

#[test]
async fn test_performance_metrics() {
    let temp_dir = create_temp_models_dir();
    let manager = ClusteringModelManager::new(temp_dir.path().to_path_buf())
        .expect("Failed to create clustering model manager");
    
    // Train multiple models to generate metrics
    let vectors1 = generate_test_vectors(800, 32);
    let vectors2 = generate_test_vectors(1200, 64);
    
    let _model1 = manager.train_model("perf_test_1", &vectors1, 32)
        .await
        .expect("Failed to train first model");
    
    let _model2 = manager.train_model("perf_test_2", &vectors2, 64)
        .await
        .expect("Failed to train second model");
    
    // Get performance metrics
    let metrics = manager.get_performance_metrics().await;
    
    // Verify metrics
    assert_eq!(metrics.models_trained, 2);
    assert!(metrics.training_time_total_ms > 0);
    assert!(metrics.avg_training_time_ms > 0.0);
    assert_eq!(metrics.avg_training_time_ms, metrics.training_time_total_ms as f64 / 2.0);
}

#[test]
async fn test_clustering_quality_metrics() {
    let temp_dir = create_temp_models_dir();
    let manager = ClusteringModelManager::new(temp_dir.path().to_path_buf())
        .expect("Failed to create clustering model manager");
    
    // Generate well-separated clusters for better quality
    let mut vectors = Vec::new();
    
    // Cluster 1: vectors around (0, 0)
    for i in 0..500 {
        let noise = (i as f32) * 0.01;
        vectors.push(vec![0.0 + noise, 0.0 + noise]);
    }
    
    // Cluster 2: vectors around (10, 10)
    for i in 0..500 {
        let noise = (i as f32) * 0.01;
        vectors.push(vec![10.0 + noise, 10.0 + noise]);
    }
    
    // Cluster 3: vectors around (-10, -10)
    for i in 0..500 {
        let noise = (i as f32) * 0.01;
        vectors.push(vec![-10.0 + noise, -10.0 + noise]);
    }
    
    let collection_id = "quality_test";
    
    // Train model
    let model = manager.train_model(collection_id, &vectors, 2)
        .await
        .expect("Failed to train clustering model");
    
    // Verify quality metrics
    let stats = &model.stats;
    
    // With well-separated clusters, we should have good quality
    assert!(stats.silhouette_score > 0.0, "Silhouette score should be positive");
    assert!(stats.inter_cluster_distance > stats.intra_cluster_distance, 
            "Inter-cluster distance should be greater than intra-cluster distance");
    
    // Should have around 3 clusters
    assert!(stats.cluster_count >= 2 && stats.cluster_count <= 5, 
            "Should detect reasonable number of clusters");
}

#[test]
async fn test_all_collections_stats() {
    let temp_dir = create_temp_models_dir();
    let manager = ClusteringModelManager::new(temp_dir.path().to_path_buf())
        .expect("Failed to create clustering model manager");
    
    // Train models for multiple collections
    let vectors1 = generate_test_vectors(1000, 32);
    let vectors2 = generate_test_vectors(2000, 64);
    let vectors3 = generate_test_vectors(1500, 96);
    
    let _model1 = manager.train_model("collection_1", &vectors1, 32)
        .await
        .expect("Failed to train model 1");
    
    let _model2 = manager.train_model("collection_2", &vectors2, 64)
        .await
        .expect("Failed to train model 2");
    
    let _model3 = manager.train_model("collection_3", &vectors3, 96)
        .await
        .expect("Failed to train model 3");
    
    // Get all stats
    let all_stats = manager.get_all_stats().await;
    
    // Verify all collections are included
    assert_eq!(all_stats.len(), 3);
    assert!(all_stats.contains_key("collection_1"));
    assert!(all_stats.contains_key("collection_2"));
    assert!(all_stats.contains_key("collection_3"));
    
    // Verify stats content
    let stats1 = &all_stats["collection_1"];
    assert_eq!(stats1.total_vectors, 1000);
    
    let stats2 = &all_stats["collection_2"];
    assert_eq!(stats2.total_vectors, 2000);
    
    let stats3 = &all_stats["collection_3"];
    assert_eq!(stats3.total_vectors, 1500);
}

#[test]
fn test_clustering_stats_serialization() {
    let stats = ClusteringStats {
        collection_id: "test_collection".to_string(),
        total_vectors: 10000,
        cluster_count: 25,
        avg_vectors_per_cluster: 400.0,
        silhouette_score: 0.85,
        intra_cluster_distance: 2.5,
        inter_cluster_distance: 15.0,
        training_time_ms: 5000,
        convergence_iterations: 12,
        search_speedup_factor: 6.1,
        accuracy_retention: 0.98,
        model_version: 1,
        last_trained: chrono::Utc::now(),
        last_updated: chrono::Utc::now(),
        cluster_sizes: vec![380, 420, 410, 390, 400],
    };
    
    // Test serialization
    let serialized = serde_json::to_string(&stats)
        .expect("Failed to serialize clustering stats");
    
    // Test deserialization
    let deserialized: ClusteringStats = serde_json::from_str(&serialized)
        .expect("Failed to deserialize clustering stats");
    
    // Verify deserialized data
    assert_eq!(deserialized.collection_id, stats.collection_id);
    assert_eq!(deserialized.total_vectors, stats.total_vectors);
    assert_eq!(deserialized.cluster_count, stats.cluster_count);
    assert_eq!(deserialized.silhouette_score, stats.silhouette_score);
    assert_eq!(deserialized.cluster_sizes, stats.cluster_sizes);
}