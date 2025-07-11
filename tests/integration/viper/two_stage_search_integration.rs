//! Two-Stage Search Integration Tests
//!
//! End-to-end tests for the two-stage search functionality that verify
//! the complete search pipeline from quantized filtering to FP32 refinement.

use proximadb::compute::{
    UnifiedDistanceCompute, UnifiedQuantizationEngine, UnifiedQuantizationLevel,
    InMemoryCodebookStore, DistanceMetric, 
};
use proximadb::core::{CollectionId, Collection, VectorRecord};
use proximadb::core::search::SearchParams;
use proximadb::services::collection_service::CollectionService;
use proximadb::storage::engines::viper::{
    ViperEngine, ViperConfig, TwoStageSearchBuilder,
    tests::test_data_generator::{TestDataGenerator, TestDataConfig},
};
use proximadb::storage::persistence::filesystem::FilesystemFactory;
use proximadb::storage::traits::UnifiedStorageEngine;

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::RwLock;

/// Helper to create a test collection
async fn create_test_collection(
    collection_service: &Arc<RwLock<CollectionService>>,
    id: &str,
    dimension: usize,
) -> Result<()> {
    let collection = Collection {
        id: CollectionId(id.to_string()),
        dimension,
        metadata: HashMap::new(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };
    
    collection_service.write().await.create_collection(collection).await?;
    Ok(())
}

#[tokio::test]
async fn test_two_stage_search_basic_flow() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let data_dir = temp_dir.path().join("data");
    std::fs::create_dir_all(&data_dir)?;
    
    // Create test data
    let config = TestDataConfig {
        num_vectors: 1000,
        dimension: 128,
        num_collections: 1,
        include_pq8: true,
        include_pq4: false,
        include_binary: false,
        include_int8: false,
        ..Default::default()
    };
    let mut generator = TestDataGenerator::new(config);
    
    let parquet_path = data_dir.join("test_vectors.parquet");
    generator.create_parquet_file(parquet_path.to_str().unwrap())?;
    
    // Setup VIPER engine
    let filesystem = Arc::new(FilesystemFactory::new(Default::default()));
    let viper_config = ViperConfig {
        data_dir: data_dir.to_str().unwrap().to_string(),
        ..Default::default()
    };
    let viper_engine = Arc::new(ViperEngine::new(viper_config, filesystem.clone()).await?);
    
    // Setup collection service
    let collection_service = Arc::new(RwLock::new(CollectionService::new(
        Arc::new(RwLock::new(HashMap::new())),
        filesystem.clone(),
    )));
    viper_engine.set_collection_service(collection_service.clone()).await;
    
    // Create collection
    create_test_collection(&collection_service, "collection_0", 128).await?;
    
    // Setup two-stage search
    let distance_compute = Arc::new(UnifiedDistanceCompute::default());
    let quantization_engine = Arc::new(UnifiedQuantizationEngine::new(
        distance_compute.clone(),
        Arc::new(InMemoryCodebookStore::new()),
    ));
    
    let two_stage_engine = TwoStageSearchBuilder::new()
        .candidate_multiplier(3.0)
        .min_candidates(50)
        .max_candidates(500)
        .build(distance_compute, quantization_engine);
    
    // Create a query vector
    let query_vector = vec![0.5; 128];
    
    // Search with two-stage enabled
    let search_params = SearchParams {
        top_k: Some(10),
        enable_two_stage: Some(true),
        quantization_hint: Some(UnifiedQuantizationLevel::pq8(16)),
        ..Default::default()
    };
    
    let results = viper_engine.search(
        &CollectionId("collection_0".to_string()),
        &query_vector,
        search_params,
    ).await?;
    
    // Verify results
    assert!(!results.is_empty());
    assert!(results.len() <= 10);
    
    // Verify results are sorted by distance
    for i in 1..results.len() {
        assert!(results[i-1].score <= results[i].score);
    }
    
    Ok(())
}

#[tokio::test]
async fn test_two_stage_search_with_filters() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let data_dir = temp_dir.path().join("data");
    std::fs::create_dir_all(&data_dir)?;
    
    // Create test data with metadata
    let config = TestDataConfig {
        num_vectors: 500,
        dimension: 64,
        num_collections: 1,
        num_categories: 3,
        include_pq8: true,
        include_binary: true,
        ..Default::default()
    };
    let mut generator = TestDataGenerator::new(config);
    
    let parquet_path = data_dir.join("test_vectors.parquet");
    generator.create_parquet_file(parquet_path.to_str().unwrap())?;
    
    // Setup engines
    let filesystem = Arc::new(FilesystemFactory::new(Default::default()));
    let viper_config = ViperConfig {
        data_dir: data_dir.to_str().unwrap().to_string(),
        ..Default::default()
    };
    let viper_engine = Arc::new(ViperEngine::new(viper_config, filesystem.clone()).await?);
    
    let collection_service = Arc::new(RwLock::new(CollectionService::new(
        Arc::new(RwLock::new(HashMap::new())),
        filesystem.clone(),
    )));
    viper_engine.set_collection_service(collection_service.clone()).await;
    
    create_test_collection(&collection_service, "collection_0", 64).await?;
    
    // Search with metadata filters
    let query_vector = vec![0.3; 64];
    let search_params = SearchParams {
        top_k: Some(5),
        filters: Some(HashMap::from([
            ("category".to_string(), serde_json::Value::String("category_0".to_string())),
            ("price".to_string(), serde_json::Value::Number(150.into())),
        ])),
        enable_two_stage: Some(true),
        quantization_hint: Some(UnifiedQuantizationLevel::pq8(8)),
        ..Default::default()
    };
    
    let results = viper_engine.search(
        &CollectionId("collection_0".to_string()),
        &query_vector,
        search_params,
    ).await?;
    
    // Verify filtered results
    assert!(!results.is_empty());
    for result in &results {
        // Check metadata matches filters
        if let Some(category) = result.metadata.get("category") {
            assert_eq!(category.as_str().unwrap(), "category_0");
        }
        if let Some(price) = result.metadata.get("price") {
            assert_eq!(price.as_i64().unwrap(), 150);
        }
    }
    
    Ok(())
}

#[tokio::test]
async fn test_two_stage_accuracy_vs_speed_tradeoff() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let data_dir = temp_dir.path().join("data");
    std::fs::create_dir_all(&data_dir)?;
    
    // Create larger dataset
    let config = TestDataConfig {
        num_vectors: 5000,
        dimension: 256,
        num_collections: 1,
        include_pq8: true,
        include_pq4: true,
        pq_num_subvectors: 32,
        ..Default::default()
    };
    let mut generator = TestDataGenerator::new(config);
    
    let parquet_path = data_dir.join("large_dataset.parquet");
    generator.create_parquet_file(parquet_path.to_str().unwrap())?;
    
    // Setup engines
    let filesystem = Arc::new(FilesystemFactory::new(Default::default()));
    let viper_config = ViperConfig {
        data_dir: data_dir.to_str().unwrap().to_string(),
        ..Default::default()
    };
    let viper_engine = Arc::new(ViperEngine::new(viper_config, filesystem.clone()).await?);
    
    let collection_service = Arc::new(RwLock::new(CollectionService::new(
        Arc::new(RwLock::new(HashMap::new())),
        filesystem.clone(),
    )));
    viper_engine.set_collection_service(collection_service.clone()).await;
    
    create_test_collection(&collection_service, "collection_0", 256).await?;
    
    let query_vector = vec![0.7; 256];
    
    // Test 1: FP32-only search (baseline)
    let start = std::time::Instant::now();
    let fp32_params = SearchParams {
        top_k: Some(20),
        enable_two_stage: Some(false),
        ..Default::default()
    };
    let fp32_results = viper_engine.search(
        &CollectionId("collection_0".to_string()),
        &query_vector,
        fp32_params,
    ).await?;
    let fp32_time = start.elapsed();
    
    // Test 2: Two-stage with PQ8
    let start = std::time::Instant::now();
    let pq8_params = SearchParams {
        top_k: Some(20),
        enable_two_stage: Some(true),
        quantization_hint: Some(UnifiedQuantizationLevel::pq8(32)),
        ..Default::default()
    };
    let pq8_results = viper_engine.search(
        &CollectionId("collection_0".to_string()),
        &query_vector,
        pq8_params,
    ).await?;
    let pq8_time = start.elapsed();
    
    // Test 3: Two-stage with PQ4 (faster but less accurate)
    let start = std::time::Instant::now();
    let pq4_params = SearchParams {
        top_k: Some(20),
        enable_two_stage: Some(true),
        quantization_hint: Some(UnifiedQuantizationLevel::pq4(32)),
        ..Default::default()
    };
    let pq4_results = viper_engine.search(
        &CollectionId("collection_0".to_string()),
        &query_vector,
        pq4_params,
    ).await?;
    let pq4_time = start.elapsed();
    
    // Verify performance characteristics
    println!("Search times - FP32: {:?}, PQ8: {:?}, PQ4: {:?}", 
             fp32_time, pq8_time, pq4_time);
    
    // Two-stage should be faster
    assert!(pq8_time < fp32_time || pq8_time.as_millis() < 100);
    assert!(pq4_time < pq8_time || pq4_time.as_millis() < 100);
    
    // Calculate recall (how many results match the FP32 baseline)
    let fp32_ids: std::collections::HashSet<_> = fp32_results.iter()
        .map(|r| &r.id)
        .collect();
    
    let pq8_recall = pq8_results.iter()
        .filter(|r| fp32_ids.contains(&r.id))
        .count() as f32 / fp32_results.len() as f32;
    
    let pq4_recall = pq4_results.iter()
        .filter(|r| fp32_ids.contains(&r.id))
        .count() as f32 / fp32_results.len() as f32;
    
    println!("Recall - PQ8: {:.2}%, PQ4: {:.2}%", 
             pq8_recall * 100.0, pq4_recall * 100.0);
    
    // PQ8 should have better recall than PQ4
    assert!(pq8_recall >= pq4_recall);
    
    Ok(())
}

#[tokio::test]
async fn test_two_stage_search_multi_file() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let data_dir = temp_dir.path().join("data");
    std::fs::create_dir_all(&data_dir)?;
    
    // Create multiple Parquet files
    let config = TestDataConfig {
        num_vectors: 200,
        dimension: 64,
        num_collections: 1,
        include_pq8: true,
        ..Default::default()
    };
    let mut generator = TestDataGenerator::new(config);
    
    let file_paths = generator.create_multi_file_dataset(
        data_dir.to_str().unwrap(),
        5
    )?;
    
    // Setup engines
    let filesystem = Arc::new(FilesystemFactory::new(Default::default()));
    let viper_config = ViperConfig {
        data_dir: data_dir.to_str().unwrap().to_string(),
        ..Default::default()
    };
    let viper_engine = Arc::new(ViperEngine::new(viper_config, filesystem.clone()).await?);
    
    let collection_service = Arc::new(RwLock::new(CollectionService::new(
        Arc::new(RwLock::new(HashMap::new())),
        filesystem.clone(),
    )));
    viper_engine.set_collection_service(collection_service.clone()).await;
    
    create_test_collection(&collection_service, "collection_0", 64).await?;
    
    // Search across multiple files
    let query_vector = vec![0.5; 64];
    let search_params = SearchParams {
        top_k: Some(50),
        enable_two_stage: Some(true),
        quantization_hint: Some(UnifiedQuantizationLevel::pq8(8)),
        ..Default::default()
    };
    
    let results = viper_engine.search(
        &CollectionId("collection_0".to_string()),
        &query_vector,
        search_params,
    ).await?;
    
    // Verify we get results from multiple files
    assert!(!results.is_empty());
    assert!(results.len() <= 50);
    
    // Check that results come from different vector ID ranges
    let id_numbers: Vec<usize> = results.iter()
        .filter_map(|r| r.id.strip_prefix("vec_"))
        .filter_map(|num_str| num_str.parse().ok())
        .collect();
    
    let min_id = id_numbers.iter().min().unwrap();
    let max_id = id_numbers.iter().max().unwrap();
    
    // Should span multiple files (each file has 200 vectors)
    assert!(max_id - min_id > 200);
    
    Ok(())
}

#[tokio::test]
async fn test_two_stage_search_edge_cases() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let data_dir = temp_dir.path().join("data");
    std::fs::create_dir_all(&data_dir)?;
    
    // Create test data with edge cases
    let config = TestDataConfig {
        num_vectors: 100,
        dimension: 32,
        expiry_rate: 0.5, // 50% expired vectors
        include_pq8: true,
        ..Default::default()
    };
    let mut generator = TestDataGenerator::new(config);
    
    let parquet_path = data_dir.join("edge_cases.parquet");
    generator.create_parquet_file(parquet_path.to_str().unwrap())?;
    
    // Setup engines
    let filesystem = Arc::new(FilesystemFactory::new(Default::default()));
    let viper_config = ViperConfig {
        data_dir: data_dir.to_str().unwrap().to_string(),
        ..Default::default()
    };
    let viper_engine = Arc::new(ViperEngine::new(viper_config, filesystem.clone()).await?);
    
    let collection_service = Arc::new(RwLock::new(CollectionService::new(
        Arc::new(RwLock::new(HashMap::new())),
        filesystem.clone(),
    )));
    viper_engine.set_collection_service(collection_service.clone()).await;
    
    create_test_collection(&collection_service, "collection_0", 32).await?;
    
    // Test 1: Search with no results due to filters
    let query_vector = vec![0.5; 32];
    let no_match_params = SearchParams {
        top_k: Some(10),
        filters: Some(HashMap::from([
            ("category".to_string(), serde_json::Value::String("nonexistent".to_string())),
        ])),
        enable_two_stage: Some(true),
        ..Default::default()
    };
    
    let no_results = viper_engine.search(
        &CollectionId("collection_0".to_string()),
        &query_vector,
        no_match_params,
    ).await?;
    
    assert!(no_results.is_empty());
    
    // Test 2: Search with very high k
    let high_k_params = SearchParams {
        top_k: Some(1000), // More than available vectors
        enable_two_stage: Some(true),
        ..Default::default()
    };
    
    let many_results = viper_engine.search(
        &CollectionId("collection_0".to_string()),
        &query_vector,
        high_k_params,
    ).await?;
    
    // Should return at most the number of non-expired vectors
    assert!(many_results.len() <= 50); // ~50% are expired
    
    // Test 3: Search with include_expired
    let include_expired_params = SearchParams {
        top_k: Some(100),
        include_expired: Some(true),
        enable_two_stage: Some(true),
        ..Default::default()
    };
    
    let all_results = viper_engine.search(
        &CollectionId("collection_0".to_string()),
        &query_vector,
        include_expired_params,
    ).await?;
    
    // Should return more results when including expired
    assert!(all_results.len() > many_results.len());
    
    Ok(())
}

/// Helper function to measure search recall
fn calculate_recall(baseline: &[String], results: &[String]) -> f32 {
    let baseline_set: std::collections::HashSet<_> = baseline.iter().collect();
    let matches = results.iter().filter(|id| baseline_set.contains(id)).count();
    matches as f32 / baseline.len().min(results.len()) as f32
}