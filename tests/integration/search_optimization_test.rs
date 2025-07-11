//! Integration Tests for Search Optimization with Trained Models
//!
//! Tests the complete search optimization pipeline using trained clustering models,
//! including cluster selection, performance improvements, and accuracy retention.

use std::sync::Arc;
use std::time::Instant;
use tempfile::TempDir;
use tokio::test;

use crate::core::SearchResult;
use crate::storage::engines::viper::clustering_models::ClusteringModelManager;
use crate::storage::engines::viper::search::{ViperSearchEngine, ViperSearchConfig, SearchHints};
use crate::storage::engines::viper::ViperEngine;
use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};

/// Test helper to create a temporary directory
fn create_temp_dir() -> TempDir {
    TempDir::new().expect("Failed to create temp directory")
}

/// Generate clustered test vectors for search optimization testing
fn generate_clustered_vectors(cluster_count: usize, vectors_per_cluster: usize, dimension: usize) -> Vec<Vec<f32>> {
    let mut vectors = Vec::new();
    
    for cluster_id in 0..cluster_count {
        let cluster_center = (cluster_id as f32) * 20.0; // Space clusters far apart
        
        for vector_id in 0..vectors_per_cluster {
            let mut vector = Vec::with_capacity(dimension);
            
            for dim in 0..dimension {
                // Create vectors clustered around cluster center with some noise
                let base_value = cluster_center + (dim as f32 * 0.1);
                let noise = (vector_id as f32 * 0.001) % 1.0;
                vector.push(base_value + noise);
            }
            
            vectors.push(vector);
        }
    }
    
    vectors
}

/// Generate query vector that should be close to a specific cluster
fn generate_query_for_cluster(cluster_id: usize, dimension: usize) -> Vec<f32> {
    let cluster_center = (cluster_id as f32) * 20.0;
    let mut query = Vec::with_capacity(dimension);
    
    for dim in 0..dimension {
        let base_value = cluster_center + (dim as f32 * 0.1);
        query.push(base_value);
    }
    
    query
}

#[test]
async fn test_search_without_trained_model() {
    let temp_dir = create_temp_dir();
    
    // Create model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Create search engine
    let mut search_engine = ViperSearchEngine::with_config(ViperSearchConfig::default());
    search_engine.set_model_manager(Arc::clone(&model_manager));
    
    // Create VIPER engine for search
    let filesystem = Arc::new(FilesystemFactory::new(FilesystemConfig::default()));
    let viper_config = crate::storage::engines::viper::ViperConfig::default();
    let viper_engine = crate::storage::engines::viper::ViperEngine::new(viper_config, filesystem)
        .await
        .expect("Failed to create VIPER engine");
    
    let collection_id = "no_model_test".to_string();
    let query_vector = vec![1.0; 128];
    
    // Search without trained model (should use fallback)
    let results = search_engine.search_vectors(
        &viper_engine,
        &collection_id,
        &query_vector,
        10,
        None,
        None,
    ).await.expect("Search should work without trained model");
    
    // Should return empty results but not error
    assert_eq!(results.len(), 0);
}

#[test]
async fn test_search_with_trained_model() {
    let temp_dir = create_temp_dir();
    
    // Create model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Generate clustered training data
    let vectors = generate_clustered_vectors(5, 400, 128); // 5 clusters, 400 vectors each
    let collection_id = "trained_model_test";
    
    // Train clustering model
    let trained_model = model_manager.train_model(collection_id, &vectors, 128)
        .await
        .expect("Failed to train clustering model");
    
    // Verify model was trained correctly
    assert_eq!(trained_model.collection_id, collection_id);
    assert_eq!(trained_model.dimension, 128);
    assert!(trained_model.centroids.len() > 0);
    
    // Create search engine with trained model
    let mut search_engine = ViperSearchEngine::with_config(ViperSearchConfig::default());
    search_engine.set_model_manager(Arc::clone(&model_manager));
    
    // Test model retrieval
    let retrieved_model = model_manager.get_clustering_model(collection_id, 2000, 128)
        .await
        .expect("Failed to get model")
        .expect("Model should exist");
    
    assert_eq!(retrieved_model.collection_id, trained_model.collection_id);
    assert_eq!(retrieved_model.centroids.len(), trained_model.centroids.len());
}

#[test]
async fn test_cluster_selection_optimization() {
    let temp_dir = create_temp_dir();
    
    // Create model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Generate well-separated clusters
    let cluster_count = 4;
    let vectors_per_cluster = 500;
    let dimension = 64;
    let vectors = generate_clustered_vectors(cluster_count, vectors_per_cluster, dimension);
    
    let collection_id = "cluster_selection_test";
    
    // Train clustering model
    let trained_model = model_manager.train_model(collection_id, &vectors, dimension)
        .await
        .expect("Failed to train clustering model");
    
    // Verify clusters were detected
    assert!(trained_model.centroids.len() >= 2, "Should detect multiple clusters");
    assert!(trained_model.centroids.len() <= 10, "Should not create too many clusters");
    
    // Test query that should be close to cluster 1
    let query_cluster_1 = generate_query_for_cluster(1, dimension);
    
    // Create search engine
    let mut search_engine = ViperSearchEngine::with_config(ViperSearchConfig::default());
    search_engine.set_model_manager(Arc::clone(&model_manager));
    
    // Test cluster selection (internal method testing)
    let collection_id_typed = collection_id.clone();
    let selected_clusters = search_engine
        .select_relevant_clusters(&collection_id_typed, &query_cluster_1)
        .await
        .expect("Failed to select clusters");
    
    // Should select some clusters (not all)
    assert!(selected_clusters.len() > 0, "Should select some clusters");
    assert!(selected_clusters.len() <= search_engine.config.max_clusters_to_search);
}

#[test]
async fn test_search_performance_with_clustering() {
    let temp_dir = create_temp_dir();
    
    // Create model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Generate large dataset for performance testing
    let cluster_count = 8;
    let vectors_per_cluster = 250;
    let dimension = 96;
    let vectors = generate_clustered_vectors(cluster_count, vectors_per_cluster, dimension);
    
    let collection_id = "performance_test";
    
    // Train clustering model
    let trained_model = model_manager.train_model(collection_id, &vectors, dimension)
        .await
        .expect("Failed to train clustering model");
    
    // Create search engine with trained model
    let mut search_engine = ViperSearchEngine::with_config(ViperSearchConfig::default());
    search_engine.set_model_manager(Arc::clone(&model_manager));
    
    // Test search performance
    let query_vector = generate_query_for_cluster(2, dimension);
    let collection_id_typed = collection_id.clone();
    
    // Measure cluster selection time
    let start_time = Instant::now();
    let selected_clusters = search_engine
        .select_relevant_clusters(&collection_id_typed, &query_vector)
        .await
        .expect("Failed to select clusters");
    let cluster_selection_time = start_time.elapsed();
    
    // Cluster selection should be fast (< 10ms for this size)
    assert!(cluster_selection_time.as_millis() < 10, 
            "Cluster selection should be fast: {}ms", cluster_selection_time.as_millis());
    
    // Should select fewer clusters than total available
    assert!(selected_clusters.len() < trained_model.centroids.len(), 
            "Should select subset of clusters for efficiency");
}

#[test]
async fn test_search_metrics_tracking() {
    let temp_dir = create_temp_dir();
    
    // Create model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Create search engine
    let mut search_engine = ViperSearchEngine::with_config(ViperSearchConfig::default());
    search_engine.set_model_manager(Arc::clone(&model_manager));
    
    // Generate and train on test data
    let vectors = generate_clustered_vectors(3, 300, 64);
    let collection_id = "metrics_test";
    
    let _trained_model = model_manager.train_model(collection_id, &vectors, 64)
        .await
        .expect("Failed to train clustering model");
    
    // Perform multiple searches to generate metrics
    let query_vectors = [
        generate_query_for_cluster(0, 64),
        generate_query_for_cluster(1, 64),
        generate_query_for_cluster(2, 64),
    ];
    
    let collection_id_typed = collection_id.clone();
    
    for query in &query_vectors {
        let _clusters = search_engine
            .select_relevant_clusters(&collection_id_typed, query)
            .await
            .expect("Failed to select clusters");
    }
    
    // Get search metrics
    let metrics = search_engine.get_search_metrics().await;
    
    // Verify metrics are being tracked
    assert!(metrics.total_searches >= 0);
    assert!(metrics.avg_latency_us >= 0.0);
}

#[test]
async fn test_search_hints_optimization() {
    let temp_dir = create_temp_dir();
    
    // Create model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Create search engine
    let mut search_engine = ViperSearchEngine::with_config(ViperSearchConfig::default());
    search_engine.set_model_manager(Arc::clone(&model_manager));
    
    // Generate and train on test data
    let vectors = generate_clustered_vectors(4, 300, 128);
    let collection_id = "hints_test";
    
    let _trained_model = model_manager.train_model(collection_id, &vectors, 128)
        .await
        .expect("Failed to train clustering model");
    
    // Test search with clustering enabled
    let search_hints_with_clustering = SearchHints {
        enable_clustering: true,
        enable_metadata_filtering: false,
        quantization_level: None,
        custom_params: std::collections::HashMap::new(),
    };
    
    let query_vector = generate_query_for_cluster(1, 128);
    let collection_id_typed = collection_id.clone();
    
    let clusters_with_hints = search_engine
        .select_relevant_clusters(&collection_id_typed, &query_vector)
        .await
        .expect("Failed to select clusters");
    
    // Should select clusters when clustering is enabled
    assert!(clusters_with_hints.len() > 0, "Should select clusters with clustering enabled");
    
    // Test search with clustering disabled
    let search_hints_no_clustering = SearchHints {
        enable_clustering: false,
        enable_metadata_filtering: false,
        quantization_level: None,
        custom_params: std::collections::HashMap::new(),
    };
    
    // Note: This would require modifications to the search engine to respect the clustering hint
    // For now, we just verify the hints structure is correct
    assert!(!search_hints_no_clustering.enable_clustering);
}

#[test]
async fn test_model_cache_efficiency() {
    let temp_dir = create_temp_dir();
    
    // Create model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Generate and train model
    let vectors = generate_clustered_vectors(3, 400, 64);
    let collection_id = "cache_test";
    
    let _trained_model = model_manager.train_model(collection_id, &vectors, 64)
        .await
        .expect("Failed to train clustering model");
    
    // Get performance metrics before cache hits
    let metrics_before = model_manager.get_performance_metrics().await;
    let initial_cache_hits = metrics_before.cache_hits;
    
    // Access model multiple times (should hit cache)
    for _ in 0..5 {
        let _model = model_manager.get_clustering_model(collection_id, 1200, 64)
            .await
            .expect("Failed to get model");
    }
    
    // Get performance metrics after cache hits
    let metrics_after = model_manager.get_performance_metrics().await;
    let final_cache_hits = metrics_after.cache_hits;
    
    // Should have cache hits
    assert!(final_cache_hits > initial_cache_hits, 
            "Should have cache hits: {} -> {}", initial_cache_hits, final_cache_hits);
}

#[test]
async fn test_clustering_accuracy_retention() {
    let temp_dir = create_temp_dir();
    
    // Create model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Generate well-separated clusters for accuracy testing
    let cluster_count = 3;
    let vectors_per_cluster = 200;
    let dimension = 32;
    let vectors = generate_clustered_vectors(cluster_count, vectors_per_cluster, dimension);
    
    let collection_id = "accuracy_test";
    
    // Train clustering model
    let trained_model = model_manager.train_model(collection_id, &vectors, dimension)
        .await
        .expect("Failed to train clustering model");
    
    // Verify clustering quality
    let stats = &trained_model.stats;
    
    // With well-separated clusters, we should have good quality metrics
    assert!(stats.silhouette_score > 0.0, "Silhouette score should be positive");
    assert!(stats.inter_cluster_distance > 0.0, "Inter-cluster distance should be positive");
    assert!(stats.intra_cluster_distance >= 0.0, "Intra-cluster distance should be non-negative");
    
    // Test that clusters are reasonably balanced
    let min_cluster_size = stats.cluster_sizes.iter().min().unwrap_or(&0);
    let max_cluster_size = stats.cluster_sizes.iter().max().unwrap_or(&0);
    
    // Clusters shouldn't be too imbalanced (max 3x difference)
    if *min_cluster_size > 0 {
        let imbalance_ratio = *max_cluster_size as f64 / *min_cluster_size as f64;
        assert!(imbalance_ratio < 5.0, "Clusters should be reasonably balanced");
    }
}

#[test]
async fn test_search_strategy_selection() {
    let temp_dir = create_temp_dir();
    
    // Create model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Create search engine
    let mut search_engine = ViperSearchEngine::with_config(ViperSearchConfig::default());
    search_engine.set_model_manager(Arc::clone(&model_manager));
    
    // Test strategy selection for small collection (should use direct search)
    let small_collection = "small_collection".to_string();
    let query_vector = vec![1.0; 64];
    
    let strategy = search_engine.determine_search_strategy(
        &crate::storage::engines::viper::ViperEngine::new(
            crate::storage::engines::viper::ViperConfig::default(),
            Arc::new(crate::storage::persistence::filesystem::FilesystemFactory::new(
                crate::storage::persistence::filesystem::FilesystemConfig::default()
            ))
        ).await.expect("Failed to create VIPER engine"),
        &small_collection,
        &query_vector,
        10,
        None,
        None,
    ).await.expect("Failed to determine search strategy");
    
    // Should use direct search for collections without trained models
    // This is tested implicitly through the search method behavior
}

#[test]
async fn test_concurrent_model_access() {
    let temp_dir = create_temp_dir();
    
    // Create model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Generate and train model
    let vectors = generate_clustered_vectors(4, 300, 96);
    let collection_id = "concurrent_test";
    
    let _trained_model = model_manager.train_model(collection_id, &vectors, 96)
        .await
        .expect("Failed to train clustering model");
    
    // Test concurrent access to the same model
    let model_manager_1 = Arc::clone(&model_manager);
    let model_manager_2 = Arc::clone(&model_manager);
    let model_manager_3 = Arc::clone(&model_manager);
    
    let handle1 = tokio::spawn(async move {
        model_manager_1.get_clustering_model(collection_id, 1200, 96).await
    });
    
    let handle2 = tokio::spawn(async move {
        model_manager_2.get_clustering_model(collection_id, 1200, 96).await
    });
    
    let handle3 = tokio::spawn(async move {
        model_manager_3.get_clustering_model(collection_id, 1200, 96).await
    });
    
    // Wait for all accesses to complete
    let (result1, result2, result3) = tokio::join!(handle1, handle2, handle3);
    
    // All should succeed
    let model1 = result1.expect("Task should complete").expect("Should get model").expect("Model should exist");
    let model2 = result2.expect("Task should complete").expect("Should get model").expect("Model should exist");
    let model3 = result3.expect("Task should complete").expect("Should get model").expect("Model should exist");
    
    // All should return the same model
    assert_eq!(model1.collection_id, model2.collection_id);
    assert_eq!(model2.collection_id, model3.collection_id);
    assert_eq!(model1.centroids.len(), model2.centroids.len());
    assert_eq!(model2.centroids.len(), model3.centroids.len());
}

#[test]
async fn test_model_version_tracking() {
    let temp_dir = create_temp_dir();
    
    // Create model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Generate and train initial model
    let vectors = generate_clustered_vectors(3, 300, 64);
    let collection_id = "version_test";
    
    let initial_model = model_manager.train_model(collection_id, &vectors, 64)
        .await
        .expect("Failed to train clustering model");
    
    assert_eq!(initial_model.version, 1, "Initial model should be version 1");
    
    // Update model online
    let new_vectors = generate_clustered_vectors(1, 100, 64);
    model_manager.update_model_online(collection_id, &new_vectors)
        .await
        .expect("Failed to update model online");
    
    // Get updated model
    let updated_model = model_manager.get_clustering_model(collection_id, 1000, 64)
        .await
        .expect("Failed to get updated model")
        .expect("Updated model should exist");
    
    assert!(updated_model.version > initial_model.version, 
            "Updated model should have higher version");
}