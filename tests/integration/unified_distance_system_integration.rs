//! Comprehensive integration test for the unified distance system
//!
//! This test verifies the complete integration of the unified distance system
//! across all ProximaDB components: WAL, Storage, Memtable, and API layers.

use std::sync::Arc;
use std::collections::HashMap;
use anyhow::Result;
use serde_json::json;
use tempfile::TempDir;

use proximadb::core::{CollectionId, VectorId, VectorRecord};
use proximadb::compute::distance::DistanceMetric;
use proximadb::compute::unified_distance::{UnifiedDistanceCompute, DistanceComputeProvider};
use proximadb::services::vector_service::VectorService;
use proximadb::storage::memtable::core::MemtableManager;
use proximadb::storage::persistence::wal::{WalManager, WalConfig, WalFactory};
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};

/// Comprehensive integration test fixture
struct UnifiedDistanceIntegrationTest {
    service: Arc<VectorService>,
    wal_manager: Arc<WalManager>,
    collection_id: String,
    _temp_dir: TempDir,
}

impl UnifiedDistanceIntegrationTest {
    async fn new() -> Result<Self> {
        let temp_dir = TempDir::new()?;
        
        // Create WAL with unified distance support
        let mut wal_config = WalConfig::default();
        wal_config.multi_disk.data_directories = vec![temp_dir.path().to_string_lossy().to_string()];
        
        let filesystem_config = FilesystemConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(filesystem_config).await?);
        
        let wal_strategy = WalFactory::create_from_config(&wal_config, filesystem).await?;
        let wal_manager = Arc::new(WalManager::new(wal_strategy, wal_config).await?);
        
        // Create memtable with unified distance support
        let memtable_manager = Arc::new(MemtableManager::new_unified_for_wal());
        
        // Create unified service
        let service = Arc::new(VectorService::new(
            wal_manager.clone(),
            memtable_manager,
            None,
        ));
        
        Ok(Self {
            service,
            wal_manager,
            collection_id: "unified_distance_test".to_string(),
            _temp_dir: temp_dir,
        })
    }

    async fn insert_comprehensive_test_data(&self) -> Result<()> {
        // Insert vectors that test all distance metrics comprehensively
        let test_vectors = vec![
            // Geometric relationships for cosine/euclidean/dot product
            ("unit_x", vec![1.0, 0.0, 0.0, 0.0]),
            ("unit_y", vec![0.0, 1.0, 0.0, 0.0]),
            ("unit_z", vec![0.0, 0.0, 1.0, 0.0]),
            ("unit_w", vec![0.0, 0.0, 0.0, 1.0]),
            ("diagonal_xy", vec![0.707, 0.707, 0.0, 0.0]),
            ("diagonal_xyz", vec![0.577, 0.577, 0.577, 0.0]),
            ("opposite_x", vec![-1.0, 0.0, 0.0, 0.0]),
            ("scaled_x", vec![2.0, 0.0, 0.0, 0.0]),
            
            // Binary patterns for Hamming distance
            ("binary_0000", vec![0.0, 0.0, 0.0, 0.0]),
            ("binary_0001", vec![0.0, 0.0, 0.0, 1.0]),
            ("binary_0011", vec![0.0, 0.0, 1.0, 1.0]),
            ("binary_0111", vec![0.0, 1.0, 1.0, 1.0]),
            ("binary_1111", vec![1.0, 1.0, 1.0, 1.0]),
            
            // Set patterns for Jaccard distance
            ("set_empty", vec![0.0, 0.0, 0.0, 0.0]),
            ("set_single", vec![1.0, 0.0, 0.0, 0.0]),
            ("set_pair", vec![1.0, 1.0, 0.0, 0.0]),
            ("set_triple", vec![1.0, 1.0, 1.0, 0.0]),
            ("set_full", vec![1.0, 1.0, 1.0, 1.0]),
            
            // Manhattan distance test vectors
            ("manhattan_0", vec![0.0, 0.0, 0.0, 0.0]),
            ("manhattan_1", vec![1.0, 0.0, 0.0, 0.0]),
            ("manhattan_2", vec![1.0, 1.0, 0.0, 0.0]),
            ("manhattan_4", vec![1.0, 1.0, 1.0, 1.0]),
        ];

        for (id, vector) in test_vectors {
            let vector_record = VectorRecord {
                id: id.to_string(),
                collection_id: self.collection_id.clone(),
                vector,
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
                VectorId::from(id.to_string()),
                vector_record,
            ).await?;
        }

        Ok(())
    }

    async fn search_with_metric(&self, query: Vec<f32>, metric: DistanceMetric, k: usize) -> Result<serde_json::Value> {
        let metric_str = match metric {
            DistanceMetric::Cosine => "cosine",
            DistanceMetric::Euclidean => "euclidean",
            DistanceMetric::Manhattan => "manhattan",
            DistanceMetric::DotProduct => "dot_product",
            DistanceMetric::Hamming => "hamming",
            DistanceMetric::Jaccard => "jaccard",
            DistanceMetric::Custom(ref name) => name,
        };

        let search_request = json!({
            "collection_id": self.collection_id,
            "query_vector": query,
            "k": k,
            "distance_metric": metric_str
        });

        self.service.search_vectors_polymorphic(search_request).await
    }
}

#[tokio::test]
async fn test_unified_distance_system_comprehensive() -> Result<()> {
    let test = UnifiedDistanceIntegrationTest::new().await?;
    
    // Verify that all components implement DistanceComputeProvider
    let wal_distance_compute = test.wal_manager.distance_compute();
    assert_eq!(wal_distance_compute.system_default(), &DistanceMetric::Cosine);
    
    // Insert comprehensive test data
    test.insert_comprehensive_test_data().await?;
    
    println!("✅ Inserted comprehensive test data with unified distance support");
    
    // Test all distance metrics with appropriate query vectors
    let test_cases = vec![
        (
            "Cosine Similarity",
            vec![1.0, 0.0, 0.0, 0.0],
            DistanceMetric::Cosine,
            "unit_x", // Expected best match
            1.0,      // Expected score
        ),
        (
            "Euclidean Distance",
            vec![0.0, 0.0, 0.0, 0.0],
            DistanceMetric::Euclidean,
            "manhattan_0", // Expected best match (same as query)
            0.0,           // Expected score
        ),
        (
            "Manhattan Distance",
            vec![0.0, 0.0, 0.0, 0.0],
            DistanceMetric::Manhattan,
            "manhattan_0", // Expected best match
            0.0,           // Expected score
        ),
        (
            "Dot Product",
            vec![1.0, 0.0, 0.0, 0.0],
            DistanceMetric::DotProduct,
            "scaled_x",    // Expected best match (highest dot product)
            2.0,           // Expected score
        ),
        (
            "Hamming Distance",
            vec![1.0, 1.0, 1.0, 1.0],
            DistanceMetric::Hamming,
            "binary_1111", // Expected best match
            0.0,           // Expected score
        ),
        (
            "Jaccard Distance",
            vec![1.0, 1.0, 1.0, 1.0],
            DistanceMetric::Jaccard,
            "set_full",    // Expected best match
            0.0,           // Expected score
        ),
    ];

    for (test_name, query, metric, expected_best, expected_score) in test_cases {
        println!("🧪 Testing {}", test_name);
        
        let results = test.search_with_metric(query, metric, 5).await?;
        let search_results = results["results"].as_array().unwrap();
        
        assert!(!search_results.is_empty(), "{} should return results", test_name);
        
        let best_result = &search_results[0];
        let best_id = best_result["id"].as_str().unwrap();
        let best_score = best_result["score"].as_f64().unwrap();
        
        assert_eq!(
            best_id, expected_best,
            "{}: Expected best match '{}', got '{}'",
            test_name, expected_best, best_id
        );
        
        assert!(
            (best_score - expected_score).abs() < 1e-5,
            "{}: Expected score {}, got {}",
            test_name, expected_score, best_score
        );
        
        println!("✅ {} passed: best='{}', score={:.6}", test_name, best_id, best_score);
    }
    
    Ok(())
}

#[tokio::test]
async fn test_cross_component_distance_consistency() -> Result<()> {
    let test = UnifiedDistanceIntegrationTest::new().await?;
    
    // Test that the same distance calculation is consistent across components
    let query_vector = vec![1.0, 0.0, 0.0];
    let test_vector = vec![0.0, 1.0, 0.0];
    
    // Get distance computations from different components
    let wal_distance_compute = test.wal_manager.distance_compute();
    let service_distance_compute = test.service.distance_compute();
    
    // Test all metrics for consistency
    let metrics = vec![
        DistanceMetric::Cosine,
        DistanceMetric::Euclidean,
        DistanceMetric::Manhattan,
        DistanceMetric::DotProduct,
        DistanceMetric::Hamming,
        DistanceMetric::Jaccard,
    ];
    
    for metric in metrics {
        let wal_distance = wal_distance_compute.calculate_distance(&query_vector, &test_vector, &metric);
        let service_distance = service_distance_compute.calculate_distance(&query_vector, &test_vector, &metric);
        
        assert!(
            (wal_distance - service_distance).abs() < 1e-10,
            "WAL and Service should produce identical distances for {:?}: {} vs {}",
            metric, wal_distance, service_distance
        );
        
        println!("✅ {:?}: WAL={:.6}, Service={:.6}", metric, wal_distance, service_distance);
    }
    
    Ok(())
}

#[tokio::test]
async fn test_hardware_acceleration_integration() -> Result<()> {
    let test = UnifiedDistanceIntegrationTest::new().await?;
    
    // Create high-dimensional vectors to test SIMD acceleration
    let dimension = 512;
    let num_vectors = 100;
    
    // Generate test vectors with patterns that benefit from SIMD
    for i in 0..num_vectors {
        let mut vector = vec![0.0; dimension];
        for j in 0..dimension {
            vector[j] = ((i * j) % 256) as f32 / 256.0;
        }
        
        let vector_record = VectorRecord {
            id: format!("hvec_{}", i),
            collection_id: test.collection_id.clone(),
            vector,
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

        test.service.handle_vector_insert(
            CollectionId::from(test.collection_id.clone()),
            VectorId::from(format!("hvec_{}", i)),
            vector_record,
        ).await?;
    }
    
    // Create query vector
    let query_vector: Vec<f32> = (0..dimension)
        .map(|i| (i % 256) as f32 / 256.0)
        .collect();
    
    // Test search performance with hardware acceleration
    let start = std::time::Instant::now();
    let results = test.search_with_metric(query_vector, DistanceMetric::DotProduct, 50).await?;
    let elapsed = start.elapsed();
    
    let search_results = results["results"].as_array().unwrap();
    assert_eq!(search_results.len(), 50, "Should return top 50 results");
    
    println!(
        "🚀 Hardware-accelerated search on {} {}-dimensional vectors took {:?}",
        num_vectors, dimension, elapsed
    );
    
    // Verify results are properly ordered
    let scores: Vec<f64> = search_results.iter()
        .map(|r| r["score"].as_f64().unwrap())
        .collect();
    
    for i in 1..scores.len() {
        assert!(
            scores[i-1] >= scores[i],
            "Dot product results should be ordered descending"
        );
    }
    
    println!("✅ Hardware acceleration test passed with proper result ordering");
    
    Ok(())
}

#[tokio::test]
async fn test_distance_metric_hierarchy_end_to_end() -> Result<()> {
    let test = UnifiedDistanceIntegrationTest::new().await?;
    
    // Insert test data
    let vector_record = VectorRecord {
        id: "test_vector".to_string(),
        collection_id: test.collection_id.clone(),
        vector: vec![1.0, 0.0, 0.0],
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

    test.service.handle_vector_insert(
        CollectionId::from(test.collection_id.clone()),
        VectorId::from("test_vector".to_string()),
        vector_record,
    ).await?;
    
    let query_vector = vec![1.0, 0.0, 0.0];
    
    // Test 1: Request override (should use Euclidean)
    let euclidean_request = json!({
        "collection_id": test.collection_id,
        "query_vector": query_vector,
        "k": 1,
        "distance_metric": "euclidean"
    });
    
    let euclidean_results = test.service.search_vectors_polymorphic(euclidean_request).await?;
    let euclidean_score = euclidean_results["results"][0]["score"].as_f64().unwrap();
    assert!((euclidean_score - 0.0).abs() < 1e-6, "Should use Euclidean distance");
    
    // Test 2: No distance metric (should use system default - Cosine)
    let default_request = json!({
        "collection_id": test.collection_id,
        "query_vector": query_vector,
        "k": 1
    });
    
    let default_results = test.service.search_vectors_polymorphic(default_request).await?;
    let default_score = default_results["results"][0]["score"].as_f64().unwrap();
    assert!((default_score - 1.0).abs() < 1e-6, "Should use default Cosine similarity");
    
    // Test 3: Custom metric (should fall back to Cosine)
    let custom_request = json!({
        "collection_id": test.collection_id,
        "query_vector": query_vector,
        "k": 1,
        "distance_metric": "my_custom_metric"
    });
    
    let custom_results = test.service.search_vectors_polymorphic(custom_request).await?;
    let custom_score = custom_results["results"][0]["score"].as_f64().unwrap();
    assert!((custom_score - 1.0).abs() < 1e-6, "Custom metric should fall back to Cosine");
    
    println!("✅ Distance metric hierarchy test passed:");
    println!("   - Request override: Euclidean distance = {:.6}", euclidean_score);
    println!("   - System default: Cosine similarity = {:.6}", default_score);
    println!("   - Custom fallback: Cosine similarity = {:.6}", custom_score);
    
    Ok(())
}

#[tokio::test]
async fn test_batch_operations_with_unified_distance() -> Result<()> {
    let test = UnifiedDistanceIntegrationTest::new().await?;
    
    // Insert vectors in batches to test batch distance calculations
    let batch_size = 100;
    let num_batches = 5;
    
    for batch in 0..num_batches {
        let mut batch_vectors = Vec::new();
        
        for i in 0..batch_size {
            let vector_id = format!("batch_{}_{}", batch, i);
            let vector_data = vec![
                (batch as f32) / (num_batches as f32),
                (i as f32) / (batch_size as f32),
                0.0,
            ];
            
            let vector_record = VectorRecord {
                id: vector_id.clone(),
                collection_id: test.collection_id.clone(),
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
            
            batch_vectors.push((VectorId::from(vector_id), vector_record));
        }
        
        // Insert batch
        test.wal_manager.insert_batch(
            CollectionId::from(test.collection_id.clone()),
            batch_vectors,
        ).await?;
    }
    
    // Test search across all batches
    let query_vector = vec![0.5, 0.5, 0.0];
    let start = std::time::Instant::now();
    let results = test.search_with_metric(query_vector, DistanceMetric::Euclidean, 50).await?;
    let elapsed = start.elapsed();
    
    let search_results = results["results"].as_array().unwrap();
    assert_eq!(search_results.len(), 50, "Should return top 50 from {} vectors", batch_size * num_batches);
    
    println!(
        "🔄 Batch search on {} vectors across {} batches took {:?}",
        batch_size * num_batches, num_batches, elapsed
    );
    
    // Verify results are properly ordered
    let distances: Vec<f64> = search_results.iter()
        .map(|r| r["score"].as_f64().unwrap())
        .collect();
    
    for i in 1..distances.len() {
        assert!(
            distances[i-1] <= distances[i],
            "Euclidean distances should be ordered ascending"
        );
    }
    
    println!("✅ Batch operations with unified distance test passed");
    
    Ok(())
}

#[tokio::test]
async fn test_error_handling_and_fallback() -> Result<()> {
    let test = UnifiedDistanceIntegrationTest::new().await?;
    
    // Test dimension mismatch handling
    let vector_3d = VectorRecord {
        id: "vector_3d".to_string(),
        collection_id: test.collection_id.clone(),
        vector: vec![1.0, 2.0, 3.0],
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

    test.service.handle_vector_insert(
        CollectionId::from(test.collection_id.clone()),
        VectorId::from("vector_3d".to_string()),
        vector_3d,
    ).await?;
    
    // Search with mismatched dimensions
    let query_2d = vec![1.0, 2.0];
    let results = test.search_with_metric(query_2d, DistanceMetric::Cosine, 1).await;
    
    // Should handle gracefully
    assert!(results.is_ok(), "Should handle dimension mismatch gracefully");
    
    let search_results = results.unwrap()["results"].as_array().unwrap();
    if !search_results.is_empty() {
        let score = search_results[0]["score"].as_f64().unwrap();
        // For cosine with dimension mismatch, should return fallback value
        assert_eq!(score, 0.0, "Dimension mismatch should return fallback value");
    }
    
    // Test invalid distance metric handling
    let invalid_metric_request = json!({
        "collection_id": test.collection_id,
        "query_vector": vec![1.0, 2.0, 3.0],
        "k": 1,
        "distance_metric": "invalid_metric"
    });
    
    let invalid_results = test.service.search_vectors_polymorphic(invalid_metric_request).await;
    
    // Should either handle gracefully or return appropriate error
    if let Ok(results) = invalid_results {
        // If it succeeds, it should fall back to default metric
        let search_results = results["results"].as_array().unwrap();
        if !search_results.is_empty() {
            // Should have used fallback metric (default Cosine)
            println!("✅ Invalid metric handled with fallback");
        }
    } else {
        // If it fails, that's also acceptable behavior
        println!("✅ Invalid metric properly rejected");
    }
    
    println!("✅ Error handling and fallback test completed");
    
    Ok(())
}