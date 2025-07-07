//! Integration tests for AXIS + HNSW functionality
//!
//! Tests the integration between AXIS adaptive indexing and HNSW vector search
//! to ensure optimal performance across different collection characteristics.

use std::collections::HashMap;
use serde_json::json;

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb::index::axis::{
        AxisHnswConfig, AxisHnswManager, PartitionedHnswIndex, IndexType, IndexStrategy
    };
    use proximadb::compute::DistanceMetric;
    use proximadb::core::{VectorRecord, MetadataQuery, FieldQuery, ComparisonOperator};
    use chrono::Utc;

    /// Create a test vector record with metadata
    fn create_test_vector(id: &str, vector: Vec<f32>, category: &str, price: f64) -> VectorRecord {
        let mut metadata = HashMap::new();
        metadata.insert("category".to_string(), json!(category));
        metadata.insert("price".to_string(), json!(price));
        metadata.insert("created_date".to_string(), json!("2024-01-01"));
        
        VectorRecord {
            id: id.to_string(),
            collection_id: "test_collection".to_string(),
            vector,
            metadata,
            timestamp: Utc::now().timestamp_millis(),
            created_at: Utc::now().timestamp_millis(),
            updated_at: Utc::now().timestamp_millis(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        }
    }

    /// Generate test vectors with known similarity patterns
    fn generate_test_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
        let mut vectors = Vec::new();
        
        for i in 0..count {
            // Create vectors with different patterns
            let vector = match i % 4 {
                0 => {
                    // Electronics cluster
                    let mut v = vec![1.0; dimension];
                    for j in 0..dimension {
                        v[j] = 1.0 + (j as f32) * 0.1;
                    }
                    v
                }
                1 => {
                    // Books cluster
                    let mut v = vec![0.5; dimension];
                    for j in 0..dimension {
                        v[j] = 0.5 + (j as f32) * 0.05;
                    }
                    v
                }
                2 => {
                    // Clothing cluster
                    let mut v = vec![-0.5; dimension];
                    for j in 0..dimension {
                        v[j] = -0.5 + (j as f32) * 0.02;
                    }
                    v
                }
                _ => {
                    // Random cluster
                    (0..dimension).map(|j| (i + j) as f32 * 0.001).collect()
                }
            };
            
            let category = match i % 4 {
                0 => "electronics",
                1 => "books",
                2 => "clothing",
                _ => "misc",
            };
            
            let price = 10.0 + (i as f64) * 5.0;
            
            vectors.push(create_test_vector(&format!("vec_{}", i), vector, category, price));
        }
        
        vectors
    }

    #[test]
    fn test_axis_hnsw_config_defaults() {
        let config = AxisHnswConfig::default();
        
        assert_eq!(config.m, 16);
        assert_eq!(config.ef_construction, 200);
        assert_eq!(config.ef_search, 50);
        assert_eq!(config.max_partition_size, 100_000);
        assert!(config.adaptive_parameters);
        assert!(config.use_simd);
    }

    #[test]
    fn test_partitioned_hnsw_creation() {
        let config = AxisHnswConfig::default();
        let index = PartitionedHnswIndex::new(config, DistanceMetric::Cosine);
        
        let stats = index.get_stats();
        assert_eq!(stats.total_vectors, 0);
        assert_eq!(stats.num_partitions, 0);
        assert_eq!(stats.memory_usage_mb, 0.0);
    }

    #[test]
    fn test_vector_addition_and_partitioning() {
        let mut config = AxisHnswConfig::default();
        config.max_partition_size = 5; // Small partitions for testing
        
        let mut index = PartitionedHnswIndex::new(config, DistanceMetric::Cosine);
        
        // Add 10 vectors (should create 2 partitions)
        let test_vectors = generate_test_vectors(10, 128);
        for vector in &test_vectors {
            let result = index.add_vector(vector);
            assert!(result.is_ok(), "Failed to add vector {}: {:?}", vector.id, result);
        }
        
        let stats = index.get_stats();
        assert_eq!(stats.total_vectors, 10);
        assert!(stats.num_partitions >= 2); // Should have created multiple partitions
        assert!(stats.memory_usage_mb > 0.0);
        
        println!("Stats after adding 10 vectors: {:?}", stats);
    }

    #[test]
    fn test_hnsw_search_basic() {
        let config = AxisHnswConfig::default();
        let mut index = PartitionedHnswIndex::new(config, DistanceMetric::Cosine);
        
        // Add test vectors
        let test_vectors = generate_test_vectors(50, 64);
        for vector in &test_vectors {
            index.add_vector(vector).unwrap();
        }
        
        // Search for similar vector (should find the electronics cluster)
        let query_vector = {
            let dimension = 64;
            let mut v = vec![1.0; dimension];
            for j in 0..dimension {
                v[j] = 1.0 + (j as f32) * 0.1;
            }
            v
        };
        
        let results = index.search(&query_vector, 5).unwrap();
        assert!(!results.is_empty(), "Search should return results");
        assert!(results.len() <= 5, "Should not return more than k results");
        
        // Results should be sorted by score (highest first)
        for i in 1..results.len() {
            if let (Some(score1), Some(score2)) = (results[i-1].score, results[i].score) {
                assert!(score1 >= score2, "Results should be sorted by score");
            }
        }
        
        println!("Search returned {} results", results.len());
        for (i, result) in results.iter().enumerate() {
            println!("  {}: {} (score: {:?})", i, result.id, result.score);
        }
    }

    #[test]
    fn test_hnsw_search_with_metadata_filter() {
        let config = AxisHnswConfig::default();
        let mut index = PartitionedHnswIndex::new(config, DistanceMetric::Cosine);
        
        // Add test vectors
        let test_vectors = generate_test_vectors(20, 32);
        for vector in &test_vectors {
            index.add_vector(vector).unwrap();
        }
        
        // Create metadata query: category = "electronics" AND price < 50
        let metadata_query = MetadataQuery::And(vec![
            MetadataQuery::field_eq("category", json!("electronics")),
            MetadataQuery::Field(FieldQuery {
                field: "price".to_string(),
                operator: ComparisonOperator::LessThan,
                value: json!(50.0),
            }),
        ]);
        
        // Search with filter
        let query_vector = vec![1.0; 32];
        let results = index.search_with_filter(&query_vector, 10, Some(&metadata_query)).unwrap();
        
        // Verify all results match the filter
        for result in &results {
            let category = result.metadata.get("category").unwrap();
            let price = result.metadata.get("price").unwrap().as_f64().unwrap();
            
            assert_eq!(category, &json!("electronics"));
            assert!(price < 50.0);
        }
        
        println!("Filtered search returned {} results", results.len());
    }

    #[test]
    fn test_vector_removal() {
        let config = AxisHnswConfig::default();
        let mut index = PartitionedHnswIndex::new(config, DistanceMetric::Cosine);
        
        // Add test vectors
        let test_vectors = generate_test_vectors(10, 16);
        for vector in &test_vectors {
            index.add_vector(vector).unwrap();
        }
        
        let stats_before = index.get_stats();
        assert_eq!(stats_before.total_vectors, 10);
        
        // Remove a vector
        let removed = index.remove_vector("vec_0").unwrap();
        assert!(removed, "Should have removed the vector");
        
        let stats_after = index.get_stats();
        assert_eq!(stats_after.total_vectors, 9);
        
        // Try to remove non-existent vector
        let not_removed = index.remove_vector("non_existent").unwrap();
        assert!(!not_removed, "Should not have removed non-existent vector");
    }

    #[test]
    fn test_index_optimization() {
        let config = AxisHnswConfig::default();
        let mut index = PartitionedHnswIndex::new(config, DistanceMetric::Cosine);
        
        // Add test vectors
        let test_vectors = generate_test_vectors(30, 64);
        for vector in &test_vectors {
            index.add_vector(vector).unwrap();
        }
        
        // Optimize the index
        let result = index.optimize();
        assert!(result.is_ok(), "Index optimization should succeed");
        
        // Index should still work after optimization
        let query_vector = vec![1.0; 64];
        let results = index.search(&query_vector, 5).unwrap();
        assert!(!results.is_empty(), "Search should work after optimization");
    }

    #[tokio::test]
    async fn test_axis_hnsw_manager() {
        let config = AxisHnswConfig::default();
        let mut manager = AxisHnswManager::new(config);
        
        // Add vectors to a collection
        let test_vectors = generate_test_vectors(20, 128);
        for vector in &test_vectors {
            let result = manager.add_vector("collection1", vector).await;
            assert!(result.is_ok(), "Failed to add vector: {:?}", result);
        }
        
        // Search the collection
        let query_vector = vec![1.0; 128];
        let results = manager.search("collection1", &query_vector, 5, None).await.unwrap();
        assert!(!results.is_empty(), "Search should return results");
        
        // Get statistics
        let stats = manager.get_all_stats().await;
        assert!(stats.contains_key("collection1"));
        
        let collection_stats = stats.get("collection1").unwrap();
        assert_eq!(collection_stats.total_vectors, 20);
        
        println!("Collection stats: {:?}", collection_stats);
    }

    #[tokio::test]
    async fn test_multi_collection_hnsw_manager() {
        let config = AxisHnswConfig::default();
        let mut manager = AxisHnswManager::new(config);
        
        // Add vectors to multiple collections
        let test_vectors1 = generate_test_vectors(15, 64);
        let test_vectors2 = generate_test_vectors(25, 64);
        
        for vector in &test_vectors1 {
            manager.add_vector("electronics", vector).await.unwrap();
        }
        
        for vector in &test_vectors2 {
            manager.add_vector("books", vector).await.unwrap();
        }
        
        // Search each collection
        let query_vector = vec![1.0; 64];
        
        let electronics_results = manager.search("electronics", &query_vector, 10, None).await.unwrap();
        let books_results = manager.search("books", &query_vector, 10, None).await.unwrap();
        
        assert!(!electronics_results.is_empty());
        assert!(!books_results.is_empty());
        
        // Get all statistics
        let all_stats = manager.get_all_stats().await;
        assert_eq!(all_stats.len(), 2);
        assert!(all_stats.contains_key("electronics"));
        assert!(all_stats.contains_key("books"));
        
        println!("Electronics collection: {} vectors", all_stats["electronics"].total_vectors);
        println!("Books collection: {} vectors", all_stats["books"].total_vectors);
    }

    #[tokio::test]
    async fn test_hnsw_manager_optimization() {
        let config = AxisHnswConfig::default();
        let mut manager = AxisHnswManager::new(config);
        
        // Add vectors to collection
        let test_vectors = generate_test_vectors(50, 32);
        for vector in &test_vectors {
            manager.add_vector("test_collection", vector).await.unwrap();
        }
        
        // Optimize all indices
        let result = manager.optimize_all().await;
        assert!(result.is_ok(), "Optimization should succeed");
        
        // Verify indices still work after optimization
        let query_vector = vec![1.0; 32];
        let results = manager.search("test_collection", &query_vector, 5, None).await.unwrap();
        assert!(!results.is_empty(), "Search should work after optimization");
    }

    #[test]
    fn test_adaptive_parameter_selection() {
        // Test different configurations for different data sizes
        let small_config = AxisHnswConfig {
            m: 8,  // Smaller M for less memory usage
            ef_construction: 100,
            ef_search: 25,
            max_partition_size: 10_000,
            adaptive_parameters: true,
            use_simd: true,
            memory_limit_mb: 128,
            lazy_loading: false,
        };
        
        let large_config = AxisHnswConfig {
            m: 32, // Larger M for better connectivity
            ef_construction: 400,
            ef_search: 100,
            max_partition_size: 1_000_000,
            adaptive_parameters: true,
            use_simd: true,
            memory_limit_mb: 2048,
            lazy_loading: true,
        };
        
        // Small dataset
        let mut small_index = PartitionedHnswIndex::new(small_config, DistanceMetric::Cosine);
        let small_vectors = generate_test_vectors(100, 16);
        for vector in &small_vectors {
            small_index.add_vector(vector).unwrap();
        }
        
        // Large dataset
        let mut large_index = PartitionedHnswIndex::new(large_config, DistanceMetric::Cosine);
        let large_vectors = generate_test_vectors(1000, 256);
        for vector in &large_vectors {
            large_index.add_vector(vector).unwrap();
        }
        
        let small_stats = small_index.get_stats();
        let large_stats = large_index.get_stats();
        
        assert!(small_stats.memory_usage_mb < large_stats.memory_usage_mb);
        assert_eq!(small_stats.total_vectors, 100);
        assert_eq!(large_stats.total_vectors, 1000);
        
        println!("Small index: {:?}", small_stats);
        println!("Large index: {:?}", large_stats);
    }

    #[test]
    fn test_distance_metric_compatibility() {
        let metrics = vec![
            DistanceMetric::Cosine,
            DistanceMetric::Euclidean,
            DistanceMetric::Manhattan,
            DistanceMetric::Dot,
        ];
        
        for metric in metrics {
            let config = AxisHnswConfig::default();
            let mut index = PartitionedHnswIndex::new(config, metric);
            
            // Add a few test vectors
            let test_vectors = generate_test_vectors(5, 32);
            for vector in &test_vectors {
                let result = index.add_vector(vector);
                assert!(result.is_ok(), "Failed to add vector with metric {:?}: {:?}", metric, result);
            }
            
            // Search should work
            let query_vector = vec![1.0; 32];
            let results = index.search(&query_vector, 3);
            assert!(results.is_ok(), "Search failed with metric {:?}: {:?}", metric, results);
            
            println!("Metric {:?}: {} results", metric, results.unwrap().len());
        }
    }

    #[test]
    fn test_hnsw_memory_usage_estimation() {
        let config = AxisHnswConfig::default();
        let mut index = PartitionedHnswIndex::new(config, DistanceMetric::Cosine);
        
        let initial_stats = index.get_stats();
        assert_eq!(initial_stats.memory_usage_mb, 0.0);
        
        // Add vectors and check memory growth
        let test_vectors = generate_test_vectors(100, 128);
        for (i, vector) in test_vectors.iter().enumerate() {
            index.add_vector(vector).unwrap();
            
            if (i + 1) % 25 == 0 {
                let stats = index.get_stats();
                println!("After {} vectors: {:.2} MB", i + 1, stats.memory_usage_mb);
                assert!(stats.memory_usage_mb > 0.0);
            }
        }
        
        let final_stats = index.get_stats();
        assert!(final_stats.memory_usage_mb > initial_stats.memory_usage_mb);
        assert_eq!(final_stats.total_vectors, 100);
    }

    #[test]
    fn test_performance_with_large_vectors() {
        let config = AxisHnswConfig {
            m: 16,
            ef_construction: 100, // Lower for faster construction
            ef_search: 50,
            max_partition_size: 50_000,
            adaptive_parameters: false,
            use_simd: true,
            memory_limit_mb: 1024,
            lazy_loading: false,
        };
        
        let mut index = PartitionedHnswIndex::new(config, DistanceMetric::Cosine);
        
        // Test with high-dimensional vectors (like BERT embeddings)
        let dimension = 768;
        let num_vectors = 100;
        
        let start_time = std::time::Instant::now();
        
        // Add vectors
        for i in 0..num_vectors {
            let vector: Vec<f32> = (0..dimension)
                .map(|j| (i * dimension + j) as f32 * 0.001)
                .collect();
            let vector_record = create_test_vector(&format!("bert_vec_{}", i), vector, "embeddings", 0.0);
            index.add_vector(&vector_record).unwrap();
        }
        
        let build_time = start_time.elapsed();
        
        // Search performance
        let query_vector: Vec<f32> = (0..dimension).map(|j| j as f32 * 0.001).collect();
        
        let search_start = std::time::Instant::now();
        let results = index.search(&query_vector, 10).unwrap();
        let search_time = search_start.elapsed();
        
        println!("Performance test with {} dimensional vectors:", dimension);
        println!("  Build time: {:?}", build_time);
        println!("  Search time: {:?}", search_time);
        println!("  Results: {}", results.len());
        
        // Verify reasonable performance
        assert!(build_time.as_secs() < 30, "Build time should be reasonable");
        assert!(search_time.as_millis() < 100, "Search time should be fast");
        assert!(!results.is_empty(), "Should return results");
    }
}