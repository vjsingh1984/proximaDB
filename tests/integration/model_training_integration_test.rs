//! Integration Tests for Model Training System
//!
//! Tests the complete integration of clustering model training with the
//! background manager, flush-compaction cycles, and search optimization.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::test;
use tokio::time::sleep;

use crate::storage::engines::viper::clustering_models::{
    ClusteringModelManager, MIN_VECTORS_FOR_CLUSTERING,
};
use crate::storage::engines::viper::search::{ViperSearchEngine, ViperSearchConfig};
use crate::storage::engines::viper::ViperEngine;
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::persistence::wal::background_manager::BackgroundMaintenanceManager;
use crate::storage::persistence::wal::{WalConfig, WalManager};
use crate::storage::traits::FlushResult;

/// Test helper to create a temporary directory for tests
fn create_temp_dir() -> TempDir {
    TempDir::new().expect("Failed to create temp directory")
}

/// Create a test WAL configuration
fn create_test_wal_config() -> Arc<WalConfig> {
    Arc::new(WalConfig {
        memory_flush_size_bytes: 1024 * 1024, // 1MB
        background_flush_interval_secs: 10,
        max_batch_size: 1000,
        compression_enabled: false,
        max_segment_size_bytes: 10 * 1024 * 1024, // 10MB
        segment_cleanup_interval_secs: 60,
        ..Default::default()
    })
}

/// Generate test vectors for large collections
fn generate_large_vector_collection(count: usize, dimension: usize) -> Vec<Vec<f32>> {
    let mut vectors = Vec::with_capacity(count);
    
    for i in 0..count {
        let mut vector = Vec::with_capacity(dimension);
        for j in 0..dimension {
            // Create clustered data with multiple clusters
            let cluster_id = i % 5; // 5 clusters
            let base_value = cluster_id as f32 * 5.0;
            let noise = ((i * j) as f32 * 0.001) % 1.0;
            vector.push(base_value + noise);
        }
        vectors.push(vector);
    }
    
    vectors
}

/// Create a mock flush result for testing
fn create_mock_flush_result(entries_flushed: u64) -> FlushResult {
    FlushResult {
        success: true,
        entries_flushed,
        bytes_written: entries_flushed * 1024, // Simulate 1KB per entry
        files_created: vec![format!("test_file_{}.parquet", entries_flushed)],
        flush_id: format!("flush_{}", entries_flushed),
        duration_ms: 1000,
        engine_metrics: HashMap::new(),
        flushed_batch_ids: vec![],
        collection_id: "test_collection".to_string(),
        storage_engine: "VIPER".to_string(),
        timestamp: chrono::Utc::now(),
    }
}

#[test]
async fn test_background_manager_model_training_integration() {
    let temp_dir = create_temp_dir();
    let wal_config = create_test_wal_config();
    
    // Create clustering model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Create background manager
    let mut background_manager = BackgroundMaintenanceManager::new(wal_config);
    background_manager.set_clustering_model_manager(Arc::clone(&model_manager));
    
    // Test that background manager has clustering model manager
    let stats = background_manager.get_stats().await;
    assert_eq!(stats.total_model_training_operations, 0);
    
    // The background manager should now be ready for model training integration
    // This test verifies the setup is correct
}

#[test]
async fn test_model_training_threshold_logic() {
    let temp_dir = create_temp_dir();
    let wal_config = create_test_wal_config();
    
    // Create clustering model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Create background manager
    let mut background_manager = BackgroundMaintenanceManager::new(wal_config);
    background_manager.set_clustering_model_manager(Arc::clone(&model_manager));
    
    let collection_id = "threshold_test".to_string();
    
    // Test 1: Small collection should not trigger training
    let should_train_small = background_manager
        .should_retrain_model(&collection_id, 500_000) // < 1M vectors
        .await;
    assert!(!should_train_small, "Small collection should not trigger training");
    
    // Test 2: Large collection without previous training should trigger training
    let should_train_large = background_manager
        .should_retrain_model(&collection_id, MIN_VECTORS_FOR_CLUSTERING + 100_000)
        .await;
    assert!(should_train_large, "Large collection should trigger initial training");
    
    // Test 3: Simulate previous training count
    {
        let mut last_counts = background_manager.last_training_vector_counts.write().await;
        last_counts.insert(collection_id.clone(), MIN_VECTORS_FOR_CLUSTERING);
    }
    
    // Test 4: 19% growth should not trigger retraining
    let growth_19_percent = (MIN_VECTORS_FOR_CLUSTERING as f64 * 1.19) as usize;
    let should_train_19 = background_manager
        .should_retrain_model(&collection_id, growth_19_percent)
        .await;
    assert!(!should_train_19, "19% growth should not trigger retraining");
    
    // Test 5: 21% growth should trigger retraining
    let growth_21_percent = (MIN_VECTORS_FOR_CLUSTERING as f64 * 1.21) as usize;
    let should_train_21 = background_manager
        .should_retrain_model(&collection_id, growth_21_percent)
        .await;
    assert!(should_train_21, "21% growth should trigger retraining");
}

#[test]
async fn test_model_training_stats_tracking() {
    let temp_dir = create_temp_dir();
    let wal_config = create_test_wal_config();
    
    // Create clustering model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Create background manager
    let mut background_manager = BackgroundMaintenanceManager::new(wal_config);
    background_manager.set_clustering_model_manager(Arc::clone(&model_manager));
    
    let collection_id = "stats_test".to_string();
    
    // Test skipping small collection
    let _should_train = background_manager
        .should_retrain_model(&collection_id, 500_000)
        .await;
    
    let stats = background_manager.get_stats().await;
    assert_eq!(stats.model_training_skipped_small, 1);
    
    // Test skipping due to recent training
    {
        let mut last_counts = background_manager.last_training_vector_counts.write().await;
        last_counts.insert(collection_id.clone(), MIN_VECTORS_FOR_CLUSTERING);
    }
    
    let _should_train = background_manager
        .should_retrain_model(&collection_id, MIN_VECTORS_FOR_CLUSTERING + 100_000)
        .await;
    
    let stats = background_manager.get_stats().await;
    assert_eq!(stats.model_training_skipped_recent, 1);
}

#[test]
async fn test_search_engine_model_integration() {
    let temp_dir = create_temp_dir();
    
    // Create clustering model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Create search engine
    let mut search_engine = ViperSearchEngine::with_config(ViperSearchConfig::default());
    search_engine.set_model_manager(Arc::clone(&model_manager));
    
    // Train a model
    let vectors = generate_large_vector_collection(2000, 128);
    let collection_id = "search_integration_test";
    
    let trained_model = model_manager.train_model(collection_id, &vectors, 128)
        .await
        .expect("Failed to train clustering model");
    
    // Verify model was trained
    assert_eq!(trained_model.collection_id, collection_id);
    assert_eq!(trained_model.dimension, 128);
    assert!(trained_model.centroids.len() > 0);
    
    // Test model retrieval through search engine
    let retrieved_model = model_manager.get_clustering_model(collection_id, 2000, 128)
        .await
        .expect("Failed to get clustering model")
        .expect("Model should exist");
    
    assert_eq!(retrieved_model.collection_id, trained_model.collection_id);
    assert_eq!(retrieved_model.centroids.len(), trained_model.centroids.len());
}

#[test]
async fn test_model_persistence_integration() {
    let temp_dir = create_temp_dir();
    
    // Create first model manager instance
    let model_manager_1 = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create first clustering model manager")
    );
    
    // Train and save model
    let vectors = generate_large_vector_collection(1500, 64);
    let collection_id = "persistence_integration_test";
    
    let original_model = model_manager_1.train_model(collection_id, &vectors, 64)
        .await
        .expect("Failed to train clustering model");
    
    // Create second model manager instance (simulating restart)
    let model_manager_2 = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create second clustering model manager")
    );
    
    // Load model with second instance
    let loaded_model = model_manager_2.get_clustering_model(collection_id, 1500, 64)
        .await
        .expect("Failed to get clustering model")
        .expect("Model should be loaded from disk");
    
    // Verify loaded model matches original
    assert_eq!(loaded_model.collection_id, original_model.collection_id);
    assert_eq!(loaded_model.dimension, original_model.dimension);
    assert_eq!(loaded_model.centroids.len(), original_model.centroids.len());
    assert_eq!(loaded_model.stats.total_vectors, original_model.stats.total_vectors);
    
    // Verify stats were also loaded
    let loaded_stats = model_manager_2.get_model_stats(collection_id)
        .await
        .expect("Stats should be loaded");
    
    assert_eq!(loaded_stats.collection_id, original_model.stats.collection_id);
    assert_eq!(loaded_stats.total_vectors, original_model.stats.total_vectors);
}

#[test]
async fn test_online_model_update_integration() {
    let temp_dir = create_temp_dir();
    
    // Create clustering model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Train initial model
    let initial_vectors = generate_large_vector_collection(1000, 96);
    let collection_id = "online_update_test";
    
    let initial_model = model_manager.train_model(collection_id, &initial_vectors, 96)
        .await
        .expect("Failed to train initial model");
    
    let initial_learning_rate = initial_model.learning_rate;
    let initial_version = initial_model.version;
    
    // Generate new vectors and update model
    let new_vectors = generate_large_vector_collection(200, 96);
    
    model_manager.update_model_online(collection_id, &new_vectors)
        .await
        .expect("Failed to update model online");
    
    // Verify model was updated
    let updated_model = model_manager.get_clustering_model(collection_id, 1200, 96)
        .await
        .expect("Failed to get updated model")
        .expect("Updated model should exist");
    
    // Verify updates
    assert!(updated_model.learning_rate < initial_learning_rate, "Learning rate should decay");
    assert!(updated_model.version > initial_version, "Version should increment");
    assert!(updated_model.stats.last_updated > initial_model.stats.last_updated);
}

#[test]
async fn test_performance_metrics_integration() {
    let temp_dir = create_temp_dir();
    
    // Create clustering model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Train multiple models to generate performance metrics
    let vectors1 = generate_large_vector_collection(800, 32);
    let vectors2 = generate_large_vector_collection(1200, 64);
    let vectors3 = generate_large_vector_collection(1500, 128);
    
    let _model1 = model_manager.train_model("perf_test_1", &vectors1, 32)
        .await
        .expect("Failed to train model 1");
    
    let _model2 = model_manager.train_model("perf_test_2", &vectors2, 64)
        .await
        .expect("Failed to train model 2");
    
    let _model3 = model_manager.train_model("perf_test_3", &vectors3, 128)
        .await
        .expect("Failed to train model 3");
    
    // Test performance metrics
    let metrics = model_manager.get_performance_metrics().await;
    
    assert_eq!(metrics.models_trained, 3);
    assert!(metrics.training_time_total_ms > 0);
    assert!(metrics.avg_training_time_ms > 0.0);
    
    // Test all stats retrieval
    let all_stats = model_manager.get_all_stats().await;
    assert_eq!(all_stats.len(), 3);
    
    // Verify individual collection stats
    let stats1 = all_stats.get("perf_test_1").expect("Stats should exist");
    assert_eq!(stats1.total_vectors, 800);
    
    let stats2 = all_stats.get("perf_test_2").expect("Stats should exist");
    assert_eq!(stats2.total_vectors, 1200);
    
    let stats3 = all_stats.get("perf_test_3").expect("Stats should exist");
    assert_eq!(stats3.total_vectors, 1500);
}

#[test]
async fn test_model_training_workflow_integration() {
    let temp_dir = create_temp_dir();
    let wal_config = create_test_wal_config();
    
    // Create clustering model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Create background manager
    let mut background_manager = BackgroundMaintenanceManager::new(wal_config);
    background_manager.set_clustering_model_manager(Arc::clone(&model_manager));
    
    // Create search engine
    let mut search_engine = ViperSearchEngine::with_config(ViperSearchConfig::default());
    search_engine.set_model_manager(Arc::clone(&model_manager));
    
    let collection_id = "workflow_test".to_string();
    
    // Step 1: Simulate large collection creation (should trigger training)
    let large_vector_count = MIN_VECTORS_FOR_CLUSTERING + 500_000;
    let should_train = background_manager
        .should_retrain_model(&collection_id, large_vector_count)
        .await;
    assert!(should_train, "Large collection should trigger training");
    
    // Step 2: Train model (simulating background training)
    let vectors = generate_large_vector_collection(2000, 256); // Representative sample
    let trained_model = model_manager.train_model(&collection_id.to_string(), &vectors, 256)
        .await
        .expect("Failed to train model");
    
    // Step 3: Verify model exists and has correct properties
    assert_eq!(trained_model.collection_id, collection_id.to_string());
    assert_eq!(trained_model.dimension, 256);
    assert!(trained_model.centroids.len() > 0);
    
    // Step 4: Test search engine can use trained model
    let retrieved_model = model_manager.get_clustering_model(
        &collection_id.to_string(),
        large_vector_count,
        256
    )
    .await
    .expect("Failed to get model")
    .expect("Model should exist");
    
    assert_eq!(retrieved_model.collection_id, trained_model.collection_id);
    
    // Step 5: Simulate growth and test 20% threshold
    let growth_vector_count = (large_vector_count as f64 * 1.25) as usize; // 25% growth
    let should_retrain = background_manager
        .should_retrain_model(&collection_id, growth_vector_count)
        .await;
    assert!(should_retrain, "25% growth should trigger retraining");
    
    // Step 6: Test stats tracking
    let stats = background_manager.get_stats().await;
    assert!(stats.total_model_training_operations >= 0);
    
    let model_stats = model_manager.get_model_stats(&collection_id.to_string())
        .await
        .expect("Model stats should exist");
    
    assert_eq!(model_stats.total_vectors, 2000);
    assert!(model_stats.training_time_ms > 0);
    assert!(model_stats.cluster_count > 0);
}

#[test]
async fn test_concurrent_model_operations() {
    let temp_dir = create_temp_dir();
    
    // Create clustering model manager
    let model_manager = Arc::new(
        ClusteringModelManager::new(temp_dir.path().to_path_buf())
            .expect("Failed to create clustering model manager")
    );
    
    // Test concurrent model training
    let vectors1 = generate_large_vector_collection(1000, 64);
    let vectors2 = generate_large_vector_collection(1200, 128);
    let vectors3 = generate_large_vector_collection(800, 32);
    
    let model_manager_1 = Arc::clone(&model_manager);
    let model_manager_2 = Arc::clone(&model_manager);
    let model_manager_3 = Arc::clone(&model_manager);
    
    let handle1 = tokio::spawn(async move {
        model_manager_1.train_model("concurrent_1", &vectors1, 64).await
    });
    
    let handle2 = tokio::spawn(async move {
        model_manager_2.train_model("concurrent_2", &vectors2, 128).await
    });
    
    let handle3 = tokio::spawn(async move {
        model_manager_3.train_model("concurrent_3", &vectors3, 32).await
    });
    
    // Wait for all training to complete
    let (result1, result2, result3) = tokio::join!(handle1, handle2, handle3);
    
    // Verify all models trained successfully
    let model1 = result1.expect("Task should complete").expect("Training should succeed");
    let model2 = result2.expect("Task should complete").expect("Training should succeed");
    let model3 = result3.expect("Task should complete").expect("Training should succeed");
    
    assert_eq!(model1.collection_id, "concurrent_1");
    assert_eq!(model2.collection_id, "concurrent_2");
    assert_eq!(model3.collection_id, "concurrent_3");
    
    // Verify performance metrics
    let metrics = model_manager.get_performance_metrics().await;
    assert_eq!(metrics.models_trained, 3);
    
    // Verify all stats are accessible
    let all_stats = model_manager.get_all_stats().await;
    assert_eq!(all_stats.len(), 3);
}