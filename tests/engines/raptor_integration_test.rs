//! Integration tests for RAPTOR storage engine
//! 
//! Tests the complete RAPTOR workflow including:
//! - Matrix Trinity architecture (P² + K² + P×K)
//! - Boundary detection with K² matrix
//! - Spillover detection with P×K matrix
//! - 5-component distance boosting
//! - Zero-copy I/O integration
//! - Bloom filter ID lookups

use proximadb::storage::engines::raptor::{RaptorEngine, RaptorConfig};
use proximadb::storage::engines::raptor::common::{
    AccuracyLevel, PxKStrategy, RaptorCompressionCodec
};
use proximadb::core::models::VectorRecord;
use proximadb::core::distance::DistanceMetric;
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn test_raptor_write_read_cycle() {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    
    // Create temp directory for storage
    let temp_dir = TempDir::new().unwrap();
    let storage_path = temp_dir.path().to_str().unwrap();
    
    // Create RAPTOR engine with Matrix Trinity configuration
    let config = RaptorConfig {
        rowgroup_size: 256,  // Small for testing
        enable_clustering: true,
        compression: RaptorCompressionCodec::Zstd(3),
        accuracy_level: AccuracyLevel::Balanced,
        pxk_strategy: PxKStrategy::Adaptive,
        enable_bloom_filter: true,
        boundary_ratio: 0.8,
        spillover_threshold: 0.15,
        ..Default::default()
    };
    
    let engine = RaptorEngine::new(
        "test_collection",
        storage_path,
        config,
    ).await.unwrap();
    
    // Generate test vectors (3 clusters)
    let mut vectors = Vec::new();
    let dimension = 128;
    
    // Cluster 1: Around [1, 0, 0, ...]
    for i in 0..100 {
        let mut vec = vec![0.0; dimension];
        vec[0] = 1.0 + (i as f32 * 0.01);
        vec[1] = i as f32 * 0.001;
        
        vectors.push(VectorRecord {
            id: Some(format!("cluster1_{}", i)),
            vector: vec,
            metadata: Some(serde_json::json!({
                "cluster": 1,
                "index": i,
            })),
            source: None,
        });
    }
    
    // Cluster 2: Around [0, 1, 0, ...]
    for i in 0..100 {
        let mut vec = vec![0.0; dimension];
        vec[1] = 1.0 + (i as f32 * 0.01);
        vec[2] = i as f32 * 0.001;
        
        vectors.push(VectorRecord {
            id: Some(format!("cluster2_{}", i)),
            vector: vec,
            metadata: Some(serde_json::json!({
                "cluster": 2,
                "index": i,
            })),
            source: None,
        });
    }
    
    // Cluster 3: Around [0, 0, 1, ...]
    for i in 0..100 {
        let mut vec = vec![0.0; dimension];
        vec[2] = 1.0 + (i as f32 * 0.01);
        vec[3] = i as f32 * 0.001;
        
        vectors.push(VectorRecord {
            id: Some(format!("cluster3_{}", i)),
            vector: vec,
            metadata: Some(serde_json::json!({
                "cluster": 3,
                "index": i,
            })),
            source: None,
        });
    }
    
    // Test batch insertion
    println!("Inserting {} vectors into RAPTOR engine", vectors.len());
    engine.insert_batch(vectors.clone()).await.unwrap();
    
    // Force flush to create rowgroups with matrices
    engine.flush().await.unwrap();
    
    // Test search with boundary detection
    let mut query_vector = vec![0.0; dimension];
    query_vector[0] = 0.9;  // Near cluster 1 boundary
    query_vector[1] = 0.1;  // Slight spillover to cluster 2
    
    let search_options = proximadb::storage::engines::raptor::SearchOptions {
        use_boundary_detection: true,
        use_spillover_detection: true,
        boost_components: true,
    };
    
    let results = engine.search(
        &query_vector,
        10,
        search_options,
    ).await.unwrap();
    
    // Verify results
    assert!(!results.is_empty(), "Search should return results");
    assert!(results.len() <= 10, "Should return at most 10 results");
    
    // Check that results are primarily from cluster 1 with possible spillover
    let cluster1_count = results.iter()
        .filter(|r| r.id.as_ref().unwrap().starts_with("cluster1_"))
        .count();
    
    println!("Found {} results from cluster 1 out of {}", cluster1_count, results.len());
    assert!(cluster1_count >= 5, "Should find mostly cluster 1 vectors");
    
    // Test bloom filter ID lookup
    let retrieved = engine.get_vector("cluster1_50").await.unwrap();
    assert!(retrieved.is_some(), "Should retrieve vector by ID using bloom filter");
    
    // Test metadata filtering with predicates
    let filtered_results = engine.search_with_filter(
        &query_vector,
        10,
        vec![("cluster", "1")],  // Only cluster 1
        search_options,
    ).await.unwrap();
    
    assert!(filtered_results.iter().all(|r| {
        r.metadata.as_ref()
            .and_then(|m| m.get("cluster"))
            .and_then(|v| v.as_i64())
            .map(|c| c == 1)
            .unwrap_or(false)
    }), "All results should be from cluster 1");
}

#[tokio::test]
async fn test_matrix_trinity_architecture() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    
    let temp_dir = TempDir::new().unwrap();
    let storage_path = temp_dir.path().to_str().unwrap();
    
    let config = RaptorConfig {
        rowgroup_size: 64,  // Very small for testing matrix creation
        enable_clustering: true,
        compression: RaptorCompressionCodec::None,  // No compression for testing
        accuracy_level: AccuracyLevel::High,
        pxk_strategy: PxKStrategy::Full,  // Full P×K coverage for testing
        ..Default::default()
    };
    
    let engine = RaptorEngine::new(
        "matrix_test",
        storage_path,
        config,
    ).await.unwrap();
    
    // Create distinct clusters for matrix testing
    let mut vectors = Vec::new();
    let dimension = 16;  // Small dimension for testing
    
    // Create 3 very distinct clusters
    for cluster_id in 0..3 {
        for i in 0..50 {
            let mut vec = vec![0.0; dimension];
            vec[cluster_id * 4] = 10.0;  // Strong signal in different dimensions
            vec[cluster_id * 4 + 1] = 5.0 + (i as f32 * 0.1);
            
            vectors.push(VectorRecord {
                id: Some(format!("vec_{}_{}", cluster_id, i)),
                vector: vec,
                metadata: None,
                source: None,
            });
        }
    }
    
    engine.insert_batch(vectors).await.unwrap();
    engine.flush().await.unwrap();
    
    // Test boundary detection between clusters
    let mut boundary_query = vec![0.0; dimension];
    boundary_query[0] = 5.0;  // Between cluster 0 and 1
    boundary_query[4] = 5.0;
    
    let results = engine.search(
        &boundary_query,
        20,
        proximadb::storage::engines::raptor::SearchOptions {
            use_boundary_detection: true,
            use_spillover_detection: true,
            boost_components: false,  // Test without boosting
        },
    ).await.unwrap();
    
    // Should find vectors from both clusters due to boundary detection
    let cluster0_count = results.iter()
        .filter(|r| r.id.as_ref().unwrap().starts_with("vec_0_"))
        .count();
    let cluster1_count = results.iter()
        .filter(|r| r.id.as_ref().unwrap().starts_with("vec_1_"))
        .count();
    
    println!("Boundary search found: {} from cluster 0, {} from cluster 1", 
             cluster0_count, cluster1_count);
    
    assert!(cluster0_count > 0 && cluster1_count > 0,
            "Boundary detection should find vectors from multiple clusters");
}

#[tokio::test]
async fn test_adaptive_pxk_coverage() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    
    let temp_dir = TempDir::new().unwrap();
    let storage_path = temp_dir.path().to_str().unwrap();
    
    // Test with different P×K strategies
    for strategy in [PxKStrategy::Sparse, PxKStrategy::Hierarchical, PxKStrategy::Full] {
        let config = RaptorConfig {
            rowgroup_size: 128,
            enable_clustering: true,
            pxk_strategy: strategy.clone(),
            ..Default::default()
        };
        
        let engine = RaptorEngine::new(
            &format!("pxk_test_{:?}", strategy),
            storage_path,
            config,
        ).await.unwrap();
        
        // Create test data with varying cluster density
        let mut vectors = Vec::new();
        let dimension = 64;
        
        for i in 0..200 {
            let cluster_id = i / 50;  // 4 clusters
            let mut vec = vec![0.0; dimension];
            
            // Create clusters with different densities
            let spread = match cluster_id {
                0 => 0.1,  // Dense cluster
                1 => 0.5,  // Medium density
                2 => 1.0,  // Sparse cluster
                _ => 2.0,  // Very sparse
            };
            
            for j in 0..dimension {
                vec[j] = (cluster_id as f32) + (rand::random::<f32>() - 0.5) * spread;
            }
            
            vectors.push(VectorRecord {
                id: Some(format!("vec_{}", i)),
                vector: vec,
                metadata: None,
                source: None,
            });
        }
        
        engine.insert_batch(vectors).await.unwrap();
        engine.flush().await.unwrap();
        
        // Test spillover detection with each strategy
        let mut query = vec![0.5; dimension];  // Between clusters
        
        let results = engine.search(
            &query,
            10,
            proximadb::storage::engines::raptor::SearchOptions {
                use_boundary_detection: false,
                use_spillover_detection: true,  // Focus on spillover
                boost_components: false,
            },
        ).await.unwrap();
        
        println!("P×K strategy {:?} found {} results", strategy, results.len());
        assert!(!results.is_empty(), 
                "P×K strategy {:?} should find results", strategy);
    }
}

#[tokio::test]
async fn test_5_component_boosting() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    
    let temp_dir = TempDir::new().unwrap();
    let storage_path = temp_dir.path().to_str().unwrap();
    
    let config = RaptorConfig {
        rowgroup_size: 100,
        enable_clustering: true,
        accuracy_level: AccuracyLevel::High,
        enable_5_component_boost: true,  // Enable boosting
        ..Default::default()
    };
    
    let engine = RaptorEngine::new(
        "boost_test",
        storage_path,
        config,
    ).await.unwrap();
    
    // Create vectors with specific patterns for boosting
    let mut vectors = Vec::new();
    let dimension = 32;
    
    // Create vectors with different characteristics
    for i in 0..150 {
        let mut vec = vec![0.0; dimension];
        
        // Vary characteristics for different boost components
        let pattern = i % 5;
        match pattern {
            0 => {
                // High runtime distance variation
                for j in 0..dimension {
                    vec[j] = (j as f32).sin() * (i as f32 * 0.1);
                }
            }
            1 => {
                // Low mean distance (clustered)
                vec[0] = 1.0;
                vec[1] = 0.1 * (i as f32);
            }
            2 => {
                // High density variation
                for j in 0..dimension/2 {
                    vec[j] = rand::random::<f32>();
                }
            }
            3 => {
                // Extreme min values
                vec[0] = -10.0;
                vec[dimension-1] = 10.0;
            }
            4 => {
                // Extreme max values
                for j in 0..dimension {
                    vec[j] = (j as f32) * 0.5;
                }
            }
            _ => {}
        }
        
        vectors.push(VectorRecord {
            id: Some(format!("boost_vec_{}", i)),
            vector: vec,
            metadata: Some(serde_json::json!({
                "pattern": pattern,
            })),
            source: None,
        });
    }
    
    engine.insert_batch(vectors).await.unwrap();
    engine.flush().await.unwrap();
    
    // Test with and without boosting
    let mut query = vec![0.5; dimension];
    query[0] = 1.0;
    
    // Search without boosting
    let results_no_boost = engine.search(
        &query,
        10,
        proximadb::storage::engines::raptor::SearchOptions {
            use_boundary_detection: false,
            use_spillover_detection: false,
            boost_components: false,
        },
    ).await.unwrap();
    
    // Search with boosting
    let results_with_boost = engine.search(
        &query,
        10,
        proximadb::storage::engines::raptor::SearchOptions {
            use_boundary_detection: false,
            use_spillover_detection: false,
            boost_components: true,
        },
    ).await.unwrap();
    
    // Compare result sets - they should be different due to boosting
    let ids_no_boost: Vec<_> = results_no_boost.iter()
        .map(|r| r.id.as_ref().unwrap().clone())
        .collect();
    let ids_with_boost: Vec<_> = results_with_boost.iter()
        .map(|r| r.id.as_ref().unwrap().clone())
        .collect();
    
    let overlap = ids_no_boost.iter()
        .filter(|id| ids_with_boost.contains(id))
        .count();
    
    println!("Overlap between boosted and non-boosted: {}/10", overlap);
    
    // Some overlap is expected but not complete overlap
    assert!(overlap < 10, "Boosting should change result ordering");
    assert!(overlap > 0, "Some top results should remain consistent");
}

#[tokio::test]
async fn test_bloom_filter_efficiency() {
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    
    let temp_dir = TempDir::new().unwrap();
    let storage_path = temp_dir.path().to_str().unwrap();
    
    let config = RaptorConfig {
        rowgroup_size: 1000,
        enable_bloom_filter: true,
        bloom_false_positive_rate: 0.01,  // 1% FPR
        ..Default::default()
    };
    
    let engine = RaptorEngine::new(
        "bloom_test",
        storage_path,
        config,
    ).await.unwrap();
    
    // Insert many vectors to test bloom filter
    let mut vectors = Vec::new();
    let num_vectors = 10000;
    let dimension = 128;
    
    for i in 0..num_vectors {
        let mut vec = vec![0.0; dimension];
        for j in 0..dimension {
            vec[j] = ((i * j) as f32).sin();
        }
        
        vectors.push(VectorRecord {
            id: Some(format!("bloom_{:06}", i)),
            vector: vec,
            metadata: None,
            source: None,
        });
    }
    
    engine.insert_batch(vectors).await.unwrap();
    engine.flush().await.unwrap();
    
    // Test ID lookups - should be fast with bloom filter
    let start = std::time::Instant::now();
    
    // Lookup existing IDs
    for i in (0..100).step_by(10) {
        let id = format!("bloom_{:06}", i * 100);
        let result = engine.get_vector(&id).await.unwrap();
        assert!(result.is_some(), "Should find existing ID: {}", id);
    }
    
    // Lookup non-existing IDs (bloom filter prevents disk access)
    for i in 0..10 {
        let id = format!("nonexistent_{}", i);
        let result = engine.get_vector(&id).await.unwrap();
        assert!(result.is_none(), "Should not find non-existing ID: {}", id);
    }
    
    let elapsed = start.elapsed();
    println!("20 ID lookups completed in {:?} with bloom filter", elapsed);
    
    // Should be very fast with bloom filter (< 100ms for 20 lookups)
    assert!(elapsed.as_millis() < 100, 
            "Bloom filter lookups should be fast, took {:?}", elapsed);
}