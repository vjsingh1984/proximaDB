//! Integration tests for search with configurable distance metrics
//!
//! This module tests the complete search pipeline from API request through
//! storage and WAL to final merged results, ensuring consistent distance
//! calculations across all storage tiers.

use std::sync::Arc;
use std::collections::HashMap;
use anyhow::Result;
use serde_json::json;
use tempfile::TempDir;

use proximadb::core::{CollectionId, VectorId, VectorRecord};
use proximadb::compute::distance::DistanceMetric;
use proximadb::services::vector_service::VectorService;
use proximadb::storage::memtable::core::MemtableManager;
use proximadb::storage::persistence::wal::{WalManager, WalConfig, WalBatchFactory, WalStrategyType};
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};

/// Integration test fixture for search testing
struct SearchIntegrationTest {
    service: Arc<VectorService>,
    collection_id: String,
    _temp_dir: TempDir,
}

impl SearchIntegrationTest {
    /// Create a new integration test fixture
    async fn new(collection_name: &str) -> Result<Self> {
        let temp_dir = TempDir::new()?;
        
        // Create WAL configuration
        let mut wal_config = WalConfig::default();
        wal_config.multi_disk.data_directories = vec![temp_dir.path().to_string_lossy().to_string()];
        
        // Create filesystem
        let filesystem_config = FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await?);
        
        // Create WAL manager
        let wal_manager = Arc::new(WalManager::create_with_batch_factory(
            WalStrategyType::Avro,
            wal_config,
            filesystem
        ).await?);
        
        // Create memtable manager
        let memtable_manager = Arc::new(MemtableManager::new_unified_for_wal());
        
        // Create unified service
        let service = Arc::new(VectorService::new(
            wal_manager,
            memtable_manager,
            None, // No storage engine for pure WAL testing
        ));
        
        Ok(Self {
            service,
            collection_id: collection_name.to_string(),
            _temp_dir: temp_dir,
        })
    }

    /// Insert test vectors into the collection
    async fn insert_test_vectors(&self, vectors: Vec<(String, Vec<f32>)>) -> Result<()> {
        for (vector_id, vector_data) in vectors {
            let vector_record = VectorRecord {
                id: vector_id.clone(),
                collection_id: self.collection_id.clone(),
                vector: vector_data,
                metadata: HashMap::new(),
                timestamp: chrono::Utc::now().timestamp_millis(),
                created_at: chrono::Utc::now().timestamp_millis(),
                updated_at: chrono::Utc::now().timestamp_millis(),
                expires_at: None,
                version: 1,
                rank: None,
                score: None,
                distance: None,
            };

            self.service.handle_vector_insert(
                CollectionId::from(self.collection_id.clone()),
                VectorId::from(vector_id),
                vector_record,
            ).await?;
        }
        Ok(())
    }

    /// Perform search with specified distance metric
    async fn search_with_metric(
        &self,
        query_vector: Vec<f32>,
        k: usize,
        distance_metric: Option<DistanceMetric>,
    ) -> Result<serde_json::Value> {
        let mut search_request = json!({
            "collection_id": self.collection_id,
            "query_vector": query_vector,
            "k": k
        });

        if let Some(metric) = distance_metric {
            let metric_str = match metric {
                DistanceMetric::Cosine => "cosine",
                DistanceMetric::Euclidean => "euclidean",
                DistanceMetric::Manhattan => "manhattan",
                DistanceMetric::DotProduct => "dot_product",
                DistanceMetric::Hamming => "hamming",
                DistanceMetric::Jaccard => "jaccard",
                DistanceMetric::Custom(name) => &name,
            };
            search_request["distance_metric"] = json!(metric_str);
        }

        self.service.search_vectors_polymorphic(search_request).await
    }
}

#[tokio::test]
async fn test_end_to_end_cosine_similarity_search() -> Result<()> {
    let test = SearchIntegrationTest::new("cosine_test_collection").await?;
    
    // Insert vectors with known cosine relationships
    let test_vectors = vec![
        ("unit_x".to_string(), vec![1.0, 0.0, 0.0]),      // Query vector
        ("unit_y".to_string(), vec![0.0, 1.0, 0.0]),      // Orthogonal (similarity = 0)
        ("unit_z".to_string(), vec![0.0, 0.0, 1.0]),      // Orthogonal (similarity = 0)
        ("diagonal".to_string(), vec![0.707, 0.707, 0.0]), // 45° angle (similarity ≈ 0.707)
        ("opposite".to_string(), vec![-1.0, 0.0, 0.0]),   // Opposite (similarity = -1)
        ("scaled".to_string(), vec![2.0, 0.0, 0.0]),      // Same direction, scaled (similarity = 1)
    ];
    
    test.insert_test_vectors(test_vectors).await?;
    
    // Search with cosine similarity
    let query_vector = vec![1.0, 0.0, 0.0];
    let results = test.search_with_metric(query_vector, 5, Some(DistanceMetric::Cosine)).await?;
    
    // Parse results
    let search_results = results["results"].as_array().unwrap();
    assert_eq!(search_results.len(), 5, "Should return top 5 results");
    
    // Verify ordering by cosine similarity (higher is better)
    let first_result = &search_results[0];
    let first_id = first_result["id"].as_str().unwrap();
    let first_score = first_result["score"].as_f64().unwrap();
    
    // Best matches should be identical or same-direction vectors
    assert!(first_id == "unit_x" || first_id == "scaled", "Best match should be unit_x or scaled");
    assert!((first_score - 1.0).abs() < 1e-6, "Best cosine similarity should be ≈ 1.0");
    
    // Verify that diagonal vector has expected similarity
    let diagonal_result = search_results.iter()
        .find(|r| r["id"].as_str().unwrap() == "diagonal")
        .unwrap();
    let diagonal_score = diagonal_result["score"].as_f64().unwrap();
    assert!((diagonal_score - 0.707).abs() < 0.01, "Diagonal vector should have similarity ≈ 0.707");
    
    // Verify worst result is opposite vector
    let last_result = &search_results[search_results.len() - 1];
    let last_id = last_result["id"].as_str().unwrap();
    let last_score = last_result["score"].as_f64().unwrap();
    assert_eq!(last_id, "opposite", "Worst result should be opposite vector");
    assert!((last_score - (-1.0)).abs() < 1e-6, "Opposite vector should have similarity ≈ -1.0");
    
    Ok(())
}

#[tokio::test]
async fn test_end_to_end_euclidean_distance_search() -> Result<()> {
    let test = SearchIntegrationTest::new("euclidean_test_collection").await?;
    
    // Insert vectors with known Euclidean distances
    let test_vectors = vec![
        ("origin".to_string(), vec![0.0, 0.0, 0.0]),       // Distance 0 from query
        ("unit_away".to_string(), vec![1.0, 0.0, 0.0]),    // Distance 1 from query
        ("sqrt2_away".to_string(), vec![1.0, 1.0, 0.0]),   // Distance √2 from query
        ("sqrt3_away".to_string(), vec![1.0, 1.0, 1.0]),   // Distance √3 from query
        ("far_away".to_string(), vec![3.0, 4.0, 0.0]),     // Distance 5 from query (3-4-5 triangle)
    ];
    
    test.insert_test_vectors(test_vectors).await?;
    
    // Search with Euclidean distance from origin
    let query_vector = vec![0.0, 0.0, 0.0];
    let results = test.search_with_metric(query_vector, 5, Some(DistanceMetric::Euclidean)).await?;
    
    let search_results = results["results"].as_array().unwrap();
    assert_eq!(search_results.len(), 5, "Should return all 5 results");
    
    // Verify ordering by Euclidean distance (lower is better)
    let result_distances: Vec<f64> = search_results.iter()
        .map(|r| r["score"].as_f64().unwrap())
        .collect();
    
    // Check that distances are sorted in ascending order
    for i in 1..result_distances.len() {
        assert!(
            result_distances[i-1] <= result_distances[i],
            "Euclidean distances should be sorted ascending"
        );
    }
    
    // Verify specific distance values
    let first_result = &search_results[0];
    assert_eq!(first_result["id"].as_str().unwrap(), "origin");
    assert!((first_result["score"].as_f64().unwrap() - 0.0).abs() < 1e-6);
    
    let last_result = &search_results[4];
    assert_eq!(last_result["id"].as_str().unwrap(), "far_away");
    assert!((last_result["score"].as_f64().unwrap() - 5.0).abs() < 1e-6);
    
    Ok(())
}

#[tokio::test]
async fn test_end_to_end_manhattan_distance_search() -> Result<()> {
    let test = SearchIntegrationTest::new("manhattan_test_collection").await?;
    
    // Insert vectors with known Manhattan distances
    let test_vectors = vec![
        ("origin".to_string(), vec![0.0, 0.0, 0.0]),           // Distance 0
        ("one_step".to_string(), vec![1.0, 0.0, 0.0]),         // Distance 1
        ("two_steps".to_string(), vec![1.0, 1.0, 0.0]),        // Distance 2
        ("three_steps".to_string(), vec![1.0, 1.0, 1.0]),      // Distance 3
        ("six_steps".to_string(), vec![2.0, 2.0, 2.0]),        // Distance 6
    ];
    
    test.insert_test_vectors(test_vectors).await?;
    
    let query_vector = vec![0.0, 0.0, 0.0];
    let results = test.search_with_metric(query_vector, 5, Some(DistanceMetric::Manhattan)).await?;
    
    let search_results = results["results"].as_array().unwrap();
    assert_eq!(search_results.len(), 5);
    
    // Verify Manhattan distance calculations
    let expected_distances = vec![0.0, 1.0, 2.0, 3.0, 6.0];
    for (i, expected_dist) in expected_distances.iter().enumerate() {
        let actual_dist = search_results[i]["score"].as_f64().unwrap();
        assert!(
            (actual_dist - expected_dist).abs() < 1e-6,
            "Manhattan distance {} should be {}, got {}",
            i, expected_dist, actual_dist
        );
    }
    
    Ok(())
}

#[tokio::test]
async fn test_end_to_end_dot_product_search() -> Result<()> {
    let test = SearchIntegrationTest::new("dot_product_test_collection").await?;
    
    // Insert vectors with known dot products
    let test_vectors = vec![
        ("same_direction".to_string(), vec![2.0, 0.0, 0.0]),   // Dot product = 2
        ("orthogonal".to_string(), vec![0.0, 1.0, 0.0]),       // Dot product = 0
        ("opposite".to_string(), vec![-1.0, 0.0, 0.0]),        // Dot product = -1
        ("diagonal".to_string(), vec![1.0, 1.0, 0.0]),         // Dot product = 1
        ("mixed".to_string(), vec![0.5, -0.5, 1.0]),           // Dot product = 0.5
    ];
    
    test.insert_test_vectors(test_vectors).await?;
    
    let query_vector = vec![1.0, 0.0, 0.0];
    let results = test.search_with_metric(query_vector, 5, Some(DistanceMetric::DotProduct)).await?;
    
    let search_results = results["results"].as_array().unwrap();
    assert_eq!(search_results.len(), 5);
    
    // For dot product, higher values should come first
    let result_scores: Vec<f64> = search_results.iter()
        .map(|r| r["score"].as_f64().unwrap())
        .collect();
    
    for i in 1..result_scores.len() {
        assert!(
            result_scores[i-1] >= result_scores[i],
            "Dot product scores should be sorted descending"
        );
    }
    
    // Verify specific dot product values
    let best_result = &search_results[0];
    assert_eq!(best_result["id"].as_str().unwrap(), "same_direction");
    assert!((best_result["score"].as_f64().unwrap() - 2.0).abs() < 1e-6);
    
    let worst_result = &search_results[4];
    assert_eq!(worst_result["id"].as_str().unwrap(), "opposite");
    assert!((worst_result["score"].as_f64().unwrap() - (-1.0)).abs() < 1e-6);
    
    Ok(())
}

#[tokio::test]
async fn test_end_to_end_hamming_distance_search() -> Result<()> {
    let test = SearchIntegrationTest::new("hamming_test_collection").await?;
    
    // Insert binary-like vectors for Hamming distance
    let test_vectors = vec![
        ("identical".to_string(), vec![1.0, 0.0, 1.0, 0.0]),      // Hamming distance = 0
        ("one_diff".to_string(), vec![1.0, 1.0, 1.0, 0.0]),       // Hamming distance = 1
        ("two_diff".to_string(), vec![1.0, 1.0, 0.0, 0.0]),       // Hamming distance = 2
        ("three_diff".to_string(), vec![0.0, 1.0, 0.0, 0.0]),     // Hamming distance = 3
        ("all_diff".to_string(), vec![0.0, 1.0, 0.0, 1.0]),       // Hamming distance = 4
    ];
    
    test.insert_test_vectors(test_vectors).await?;
    
    let query_vector = vec![1.0, 0.0, 1.0, 0.0];
    let results = test.search_with_metric(query_vector, 5, Some(DistanceMetric::Hamming)).await?;
    
    let search_results = results["results"].as_array().unwrap();
    assert_eq!(search_results.len(), 5);
    
    // Verify Hamming distance calculations
    let expected_distances = vec![0.0, 1.0, 2.0, 3.0, 4.0];
    for (i, expected_dist) in expected_distances.iter().enumerate() {
        let actual_dist = search_results[i]["score"].as_f64().unwrap();
        assert!(
            (actual_dist - expected_dist).abs() < 1e-6,
            "Hamming distance {} should be {}, got {}",
            i, expected_dist, actual_dist
        );
    }
    
    Ok(())
}

#[tokio::test]
async fn test_end_to_end_jaccard_distance_search() -> Result<()> {
    let test = SearchIntegrationTest::new("jaccard_test_collection").await?;
    
    // Insert set-like vectors for Jaccard distance
    let test_vectors = vec![
        ("identical".to_string(), vec![1.0, 0.0, 1.0, 1.0]),      // Jaccard distance = 0
        ("subset".to_string(), vec![1.0, 0.0, 1.0, 0.0]),         // Jaccard distance = 1/3
        ("overlap".to_string(), vec![1.0, 1.0, 0.0, 1.0]),        // Jaccard distance = 0.5
        ("minimal_overlap".to_string(), vec![0.0, 0.0, 1.0, 0.0]), // Jaccard distance = 2/3
        ("disjoint".to_string(), vec![0.0, 1.0, 0.0, 0.0]),       // Jaccard distance = 1.0
    ];
    
    test.insert_test_vectors(test_vectors).await?;
    
    let query_vector = vec![1.0, 0.0, 1.0, 1.0];
    let results = test.search_with_metric(query_vector, 5, Some(DistanceMetric::Jaccard)).await?;
    
    let search_results = results["results"].as_array().unwrap();
    assert_eq!(search_results.len(), 5);
    
    // Verify Jaccard distance ordering (lower is better)
    let result_distances: Vec<f64> = search_results.iter()
        .map(|r| r["score"].as_f64().unwrap())
        .collect();
    
    for i in 1..result_distances.len() {
        assert!(
            result_distances[i-1] <= result_distances[i],
            "Jaccard distances should be sorted ascending"
        );
    }
    
    // Verify specific Jaccard values
    let best_result = &search_results[0];
    assert_eq!(best_result["id"].as_str().unwrap(), "identical");
    assert!((best_result["score"].as_f64().unwrap() - 0.0).abs() < 1e-6);
    
    let worst_result = &search_results[4];
    assert_eq!(worst_result["id"].as_str().unwrap(), "disjoint");
    assert!((worst_result["score"].as_f64().unwrap() - 1.0).abs() < 1e-6);
    
    Ok(())
}

#[tokio::test]
async fn test_distance_metric_hierarchy_integration() -> Result<()> {
    let test = SearchIntegrationTest::new("hierarchy_test_collection").await?;
    
    // Insert test data
    let test_vectors = vec![
        ("vector_1".to_string(), vec![1.0, 0.0, 0.0]),
        ("vector_2".to_string(), vec![0.0, 1.0, 0.0]),
    ];
    
    test.insert_test_vectors(test_vectors).await?;
    
    let query_vector = vec![1.0, 0.0, 0.0];
    
    // Test 1: Explicit distance metric override
    let euclidean_results = test.search_with_metric(
        query_vector.clone(),
        2,
        Some(DistanceMetric::Euclidean)
    ).await?;
    
    let euclidean_score = euclidean_results["results"][0]["score"].as_f64().unwrap();
    assert!((euclidean_score - 0.0).abs() < 1e-6, "Should use Euclidean distance");
    
    // Test 2: No distance metric (should use system default - Cosine)
    let default_results = test.search_with_metric(
        query_vector.clone(),
        2,
        None
    ).await?;
    
    let default_score = default_results["results"][0]["score"].as_f64().unwrap();
    assert!((default_score - 1.0).abs() < 1e-6, "Should use default Cosine similarity");
    
    // Test 3: Custom distance metric (should fall back to Cosine)
    let custom_results = test.search_with_metric(
        query_vector,
        2,
        Some(DistanceMetric::Custom("my_custom_metric".to_string()))
    ).await?;
    
    let custom_score = custom_results["results"][0]["score"].as_f64().unwrap();
    assert!((custom_score - 1.0).abs() < 1e-6, "Custom metric should fall back to Cosine");
    
    Ok(())
}

#[tokio::test]
async fn test_cross_metric_consistency() -> Result<()> {
    let test = SearchIntegrationTest::new("consistency_test_collection").await?;
    
    // Insert vectors that should have consistent ordering across metrics
    let test_vectors = vec![
        ("identical".to_string(), vec![1.0, 0.0, 0.0]),
        ("similar".to_string(), vec![0.9, 0.1, 0.0]),
        ("different".to_string(), vec![0.0, 1.0, 0.0]),
    ];
    
    test.insert_test_vectors(test_vectors).await?;
    
    let query_vector = vec![1.0, 0.0, 0.0];
    
    // Test with multiple metrics and verify consistent winner
    let metrics = vec![
        DistanceMetric::Cosine,
        DistanceMetric::Euclidean,
        DistanceMetric::DotProduct,
    ];
    
    for metric in metrics {
        let results = test.search_with_metric(query_vector.clone(), 3, Some(metric.clone())).await?;
        let best_result = &results["results"].as_array().unwrap()[0];
        let best_id = best_result["id"].as_str().unwrap();
        
        // Identical vector should always be the best match
        assert_eq!(
            best_id, "identical",
            "Identical vector should be best match for {:?}",
            metric
        );
    }
    
    Ok(())
}

#[tokio::test]
async fn test_large_scale_search_performance() -> Result<()> {
    let test = SearchIntegrationTest::new("performance_test_collection").await?;
    
    // Insert a large number of high-dimensional vectors
    let dimension = 256;
    let num_vectors = 1000;
    
    let mut test_vectors = Vec::new();
    for i in 0..num_vectors {
        let mut vector = vec![0.0; dimension];
        // Create diverse vectors with patterns
        for j in 0..dimension {
            vector[j] = ((i + j) % 100) as f32 / 100.0;
        }
        test_vectors.push((format!("vector_{}", i), vector));
    }
    
    test.insert_test_vectors(test_vectors).await?;
    
    // Create query vector
    let query_vector: Vec<f32> = (0..dimension)
        .map(|i| (i % 100) as f32 / 100.0)
        .collect();
    
    // Test search performance with different metrics
    let metrics = vec![
        DistanceMetric::Cosine,
        DistanceMetric::Euclidean,
        DistanceMetric::DotProduct,
    ];
    
    for metric in metrics {
        let start = std::time::Instant::now();
        let results = test.search_with_metric(query_vector.clone(), 100, Some(metric.clone())).await?;
        let elapsed = start.elapsed();
        
        let search_results = results["results"].as_array().unwrap();
        assert_eq!(search_results.len(), 100, "Should return top 100 results");
        
        println!(
            "Search with {:?} on {} {}-dimensional vectors took {:?}",
            metric, num_vectors, dimension, elapsed
        );
        
        // Verify results are properly ordered
        let scores: Vec<f64> = search_results.iter()
            .map(|r| r["score"].as_f64().unwrap())
            .collect();
        
        for i in 1..scores.len() {
            match metric {
                DistanceMetric::DotProduct => {
                    // Higher is better for similarity
                    assert!(scores[i-1] >= scores[i], "Dot product results should be descending");
                }
                _ => {
                    // Lower is better for distance
                    assert!(scores[i-1] <= scores[i], "Distance results should be ascending");
                }
            }
        }
    }
    
    Ok(())
}

#[tokio::test]
async fn test_dimension_mismatch_handling() -> Result<()> {
    let test = SearchIntegrationTest::new("dimension_mismatch_collection").await?;
    
    // Insert 3D vectors
    let test_vectors = vec![
        ("vector_3d".to_string(), vec![1.0, 2.0, 3.0]),
    ];
    
    test.insert_test_vectors(test_vectors).await?;
    
    // Search with 2D query (dimension mismatch)
    let query_vector = vec![1.0, 2.0];
    let results = test.search_with_metric(query_vector, 1, Some(DistanceMetric::Cosine)).await;
    
    // Should handle gracefully - either return empty results or with fallback values
    assert!(results.is_ok(), "Should handle dimension mismatch gracefully");
    
    let search_results = results.unwrap()["results"].as_array().unwrap();
    if !search_results.is_empty() {
        // If results returned, score should be the fallback value for dimension mismatch
        let score = search_results[0]["score"].as_f64().unwrap();
        // For cosine distance with dimension mismatch, fallback should be 0.0
        assert_eq!(score, 0.0, "Dimension mismatch should return fallback value");
    }
    
    Ok(())
}

#[tokio::test]
async fn test_empty_collection_search() -> Result<()> {
    let test = SearchIntegrationTest::new("empty_collection").await?;
    
    // Don't insert any vectors
    
    let query_vector = vec![1.0, 0.0, 0.0];
    let results = test.search_with_metric(query_vector, 5, Some(DistanceMetric::Cosine)).await?;
    
    let search_results = results["results"].as_array().unwrap();
    assert_eq!(search_results.len(), 0, "Empty collection should return no results");
    
    Ok(())
}