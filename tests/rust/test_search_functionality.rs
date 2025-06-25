//! Search functionality integration tests

use super::common::*;
use anyhow::Result;
use proximadb::core::*;
use std::collections::HashMap;

#[cfg(test)]
mod search_tests {
    use super::*;

    #[tokio::test]
    async fn test_search_response_structure() -> Result<()> {
        init_test_env();
        
        // Create mock search results
        let search_results = vec![
            SearchResult {
                id: "result_001".to_string(),
                vector_id: Some("result_001".to_string()),
                score: 0.95,
                vector: Some(generate_random_vector(384)),
                metadata: {
                    let mut meta = HashMap::new();
                    meta.insert("category".to_string(), serde_json::Value::String("AI".to_string()));
                    meta
                },
                rank: Some(1),
                distance: Some(0.05),
                collection_id: Some("test_collection".to_string()),
                created_at: Some(chrono::Utc::now().timestamp_millis()),
                algorithm_used: Some("HNSW".to_string()),
                processing_time_us: Some(1000),
            },
            SearchResult {
                id: "result_002".to_string(),
                vector_id: Some("result_002".to_string()),
                score: 0.87,
                vector: None, // Test optional vector
                metadata: {
                    let mut meta = HashMap::new();
                    meta.insert("category".to_string(), serde_json::Value::String("ML".to_string()));
                    meta
                },
                rank: Some(2),
                distance: Some(0.13),
                collection_id: Some("test_collection".to_string()),
                created_at: Some(chrono::Utc::now().timestamp_millis()),
                algorithm_used: Some("HNSW".to_string()),
                processing_time_us: Some(1200),
            },
        ];
        
        let search_metadata = SearchMetadata {
            algorithm_used: "HNSW".to_string(),
            query_id: Some("test_query_001".to_string()),
            query_complexity: 0.5,
            total_results: 2,
            search_time_ms: 15.5,
            performance_hint: Some("Consider using more specific filters".to_string()),
            index_stats: Some(IndexStats {
                total_vectors: 1000,
                vectors_compared: 100,
                vectors_scanned: 50,
                distance_calculations: 100,
                nodes_visited: 10,
                filter_efficiency: 0.5,
                cache_hits: 5,
                cache_misses: 5,
            }),
        };
        
        let response = VectorSearchResponse {
            success: true,
            results: search_results,
            total_count: 2,
            total_found: 2,
            processing_time_us: 15500,
            algorithm_used: "HNSW".to_string(),
            error_message: None,
            search_metadata,
            debug_info: Some(SearchDebugInfo {
                search_steps: vec!["index_lookup".to_string(), "similarity_calc".to_string()],
                clusters_searched: vec!["cluster_0".to_string(), "cluster_1".to_string()],
                filter_pushdown_enabled: true,
                parquet_columns_scanned: vec!["category".to_string(), "author".to_string()],
                timing_breakdown: {
                    let mut timing = HashMap::new();
                    timing.insert("index_lookup".to_string(), 10.0);
                    timing.insert("similarity_calc".to_string(), 5.5);
                    timing
                },
                memory_usage_mb: Some(2.5),
                estimated_total_cost: Some(15.5),
                actual_cost: Some(15.5),
                cost_breakdown: Some({
                    let mut cost = HashMap::new();
                    cost.insert("cpu".to_string(), 10.0);
                    cost.insert("memory".to_string(), 5.5);
                    cost
                }),
            }),
        };
        
        // Validate response structure
        assert_eq!(response.results.len(), 2);
        assert_eq!(response.total_found, 2);
        assert!(response.processing_time_us > 0);
        assert!(response.debug_info.is_some());
        
        // Test serialization
        let json_str = serde_json::to_string(&response)?;
        let deserialized: VectorSearchResponse = serde_json::from_str(&json_str)?;
        assert_eq!(response.results.len(), deserialized.results.len());
        
        println!("✅ Search response structure validation successful");
        Ok(())
    }

    #[tokio::test]
    async fn test_distance_calculations() -> Result<()> {
        init_test_env();
        
        let vec1 = vec![1.0, 0.0, 0.0, 0.0];
        let vec2 = vec![0.0, 1.0, 0.0, 0.0];
        let vec3 = vec![1.0, 0.0, 0.0, 0.0]; // Same as vec1
        
        // Test cosine similarity
        let calculator = proximadb::compute::distance::create_distance_calculator(
            proximadb::compute::distance::DistanceMetric::Cosine
        );
        let sim_orthogonal = calculator.distance(&vec1, &vec2);
        let sim_identical = calculator.distance(&vec1, &vec3);
        
        assert!((sim_orthogonal - 1.0).abs() < 1e-6); // Should be 1 (distance) for orthogonal vectors
        assert!((sim_identical - 0.0).abs() < 1e-6);  // Should be 0 (distance) for identical vectors
        
        println!("✅ Distance calculations test successful");
        Ok(())
    }

    #[tokio::test]
    async fn test_metadata_filter_structures() -> Result<()> {
        init_test_env();
        
        // Test various metadata filter structures
        let filters = vec![
            // Simple equality filter
            {
                let mut filter = HashMap::new();
                filter.insert("category".to_string(), serde_json::Value::String("AI".to_string()));
                filter
            },
            // Numeric filter
            {
                let mut filter = HashMap::new();
                filter.insert("year".to_string(), serde_json::Value::Number(serde_json::Number::from(2024)));
                filter
            },
            // Multiple filters
            {
                let mut filter = HashMap::new();
                filter.insert("category".to_string(), serde_json::Value::String("AI".to_string()));
                filter.insert("author".to_string(), serde_json::Value::String("Dr. Smith".to_string()));
                filter
            },
        ];
        
        for filter in filters {
            // Test filter serialization
            let json_str = serde_json::to_string(&filter)?;
            let deserialized: HashMap<String, serde_json::Value> = 
                serde_json::from_str(&json_str)?;
            assert_eq!(filter.len(), deserialized.len());
        }
        
        println!("✅ Metadata filter structures test successful");
        Ok(())
    }

    #[tokio::test]
    async fn test_search_algorithm_selection() -> Result<()> {
        init_test_env();
        
        // Test different indexing algorithms
        let algorithms = vec![
            IndexingAlgorithm::Hnsw,
            IndexingAlgorithm::Ivf,
            IndexingAlgorithm::Flat,
        ];
        
        for algorithm in algorithms {
            let collection_name = generate_test_collection_name();
            let mut config = create_test_collection_config(collection_name, 384);
            config.indexing_algorithm = algorithm.clone();
            
            // Test that configuration is valid
            let json_str = serde_json::to_string(&config)?;
            let deserialized: proximadb::core::CollectionConfig = 
                serde_json::from_str(&json_str)?;
            assert_eq!(config.indexing_algorithm, deserialized.indexing_algorithm);
        }
        
        println!("✅ Search algorithm selection test successful");
        Ok(())
    }

    #[tokio::test]
    async fn test_performance_cost_calculation() -> Result<()> {
        init_test_env();
        
        // Simulate cost calculation for different search scenarios
        let scenarios = vec![
            ("simple_search", 100, 10, 0.5),
            ("complex_filter", 1000, 50, 2.0),
            ("large_result_set", 10000, 100, 5.0),
        ];
        
        for (name, total_vectors, vectors_scanned, expected_cost_range) in scenarios {
            let filter_efficiency = vectors_scanned as f32 / total_vectors as f32;
            let estimated_cost = (total_vectors as f32 * 0.001) + (vectors_scanned as f32 * 0.01);
            
            assert!(filter_efficiency <= 1.0);
            assert!(estimated_cost > 0.0);
            assert!(estimated_cost < expected_cost_range * 3.0); // Rough bounds check
            
            println!("📊 {}: efficiency={:.3}, cost={:.3}", name, filter_efficiency, estimated_cost);
        }
        
        println!("✅ Performance cost calculation test successful");
        Ok(())
    }

    #[tokio::test]
    async fn test_search_result_ranking() -> Result<()> {
        init_test_env();
        
        // Create test search results with different scores
        let mut results = vec![
            SearchResult {
                id: "vec_001".to_string(),
                vector_id: Some("vec_001".to_string()),
                score: 0.85,
                distance: Some(0.15),
                rank: None,
                vector: None,
                metadata: HashMap::new(),
                collection_id: None,
                created_at: None,
                algorithm_used: None,
                processing_time_us: None,
            },
            SearchResult {
                id: "vec_002".to_string(),
                vector_id: Some("vec_002".to_string()),
                score: 0.92,
                distance: Some(0.08),
                rank: None,
                vector: None,
                metadata: HashMap::new(),
                collection_id: None,
                created_at: None,
                algorithm_used: None,
                processing_time_us: None,
            },
            SearchResult {
                id: "vec_003".to_string(),
                vector_id: Some("vec_003".to_string()),
                score: 0.78,
                distance: Some(0.22),
                rank: None,
                vector: None,
                metadata: HashMap::new(),
                collection_id: None,
                created_at: None,
                algorithm_used: None,
                processing_time_us: None,
            },
            SearchResult {
                id: "vec_004".to_string(),
                vector_id: Some("vec_004".to_string()),
                score: 0.95,
                distance: Some(0.05),
                rank: None,
                vector: None,
                metadata: HashMap::new(),
                collection_id: None,
                created_at: None,
                algorithm_used: None,
                processing_time_us: None,
            },
        ];
        
        // Sort by score (descending)
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        
        // Assign ranks
        for (i, result) in results.iter_mut().enumerate() {
            result.rank = Some((i + 1) as i32);
        }
        
        // Validate ranking
        assert_eq!(results[0].score, 0.95); // Highest score first
        assert_eq!(results[0].rank, Some(1));
        assert_eq!(results.last().unwrap().score, 0.78); // Lowest score last
        assert_eq!(results.last().unwrap().rank, Some(4));
        
        println!("✅ Search result ranking test successful");
        Ok(())
    }

    #[tokio::test]
    async fn test_hybrid_search_combination() -> Result<()> {
        init_test_env();
        
        // Test different search combination strategies
        let similarity_scores = vec![0.95, 0.87, 0.82, 0.79];
        let metadata_matches = vec![true, true, false, true];
        
        // Boost scores for metadata matches
        let hybrid_scores: Vec<f32> = similarity_scores
            .iter()
            .zip(metadata_matches.iter())
            .map(|(sim_score, meta_match)| {
                if *meta_match {
                    sim_score * 1.1 // 10% boost for metadata match
                } else {
                    *sim_score
                }
            })
            .collect();
        
        // Verify boosting logic
        assert!(hybrid_scores[0] > similarity_scores[0]); // Boosted
        assert!(hybrid_scores[1] > similarity_scores[1]); // Boosted
        assert_eq!(hybrid_scores[2], similarity_scores[2]); // Not boosted
        assert!(hybrid_scores[3] > similarity_scores[3]); // Boosted
        
        println!("✅ Hybrid search combination test successful");
        Ok(())
    }

    #[tokio::test]
    async fn test_error_handling_edge_cases() -> Result<()> {
        init_test_env();
        
        let calculator = proximadb::compute::distance::create_distance_calculator(
            proximadb::compute::distance::DistanceMetric::Cosine
        );
        
        // Test 1: Small vectors to test edge cases safely
        let small_vector1 = vec![1.0];
        let small_vector2 = vec![2.0];
        
        // This should work with single-element vectors
        let _result = calculator.distance(&small_vector1, &small_vector2);
        // If we get here, the implementation handles small vectors
        
        // Test 2: Large metadata object handling
        let mut large_metadata = HashMap::new();
        for i in 0..1000 {
            large_metadata.insert(
                format!("key_{}", i), 
                serde_json::Value::String(format!("value_{}", i))
            );
        }
        
        // Should handle large metadata without issues
        let json_str = serde_json::to_string(&large_metadata)?;
        assert!(json_str.len() > 10000); // Verify it's actually large
        
        // Test 3: Nested metadata structures
        let nested_metadata = serde_json::json!({
            "level1": {
                "level2": {
                    "level3": {
                        "data": [1, 2, 3, 4, 5],
                        "more_data": "test_string"
                    }
                }
            },
            "array": [
                {"id": 1, "value": 100},
                {"id": 2, "value": 200},
                {"id": 3, "value": 300}
            ]
        });
        
        let nested_str = serde_json::to_string(&nested_metadata)?;
        assert!(!nested_str.is_empty());
        
        println!("✅ Error handling edge cases test successful");
        Ok(())
    }

    #[tokio::test]
    async fn test_performance_edge_cases() -> Result<()> {
        init_test_env();
        
        // Test 1: Very high dimensional vectors
        let high_dim = 10000;
        let vec1 = generate_random_vector(high_dim);
        let vec2 = generate_random_vector(high_dim);
        
        let start = std::time::Instant::now();
        let calculator = proximadb::compute::distance::create_distance_calculator(
            proximadb::compute::distance::DistanceMetric::Cosine
        );
        let _distance = calculator.distance(&vec1, &vec2);
        let duration = start.elapsed();
        
        // Should complete within reasonable time
        assert!(duration.as_millis() < 1000); // Less than 1 second
        
        // Test 2: Zero vectors (cosine distance with zero vectors is mathematically undefined)
        let zero_vec = vec![0.0; 100];
        let normal_vec = generate_random_vector(100);
        
        let distance = calculator.distance(&zero_vec, &normal_vec);
        
        // Distance with zero vector might be NaN, infinity, or a specific value
        // depending on implementation - we just verify it doesn't crash
        let _ = distance; // Just verify the call doesn't panic
        
        // Test 3: Identical vectors
        let identical_distance = calculator.distance(&normal_vec, &normal_vec);
        
        // Distance between identical vectors should be 0
        assert!((identical_distance - 0.0).abs() < 1e-6);
        
        println!("✅ Performance edge cases test successful");
        Ok(())
    }

    #[tokio::test]
    async fn test_concurrent_operations_safety() -> Result<()> {
        init_test_env();
        
        // Test concurrent distance calculations
        let num_threads = 4;
        let calculations_per_thread = 100;
        
        let handles: Vec<_> = (0..num_threads)
            .map(|_| {
                tokio::spawn(async move {
                    let calculator = proximadb::compute::distance::create_distance_calculator(
                        proximadb::compute::distance::DistanceMetric::Cosine
                    );
                    
                    for _ in 0..calculations_per_thread {
                        let vec1 = generate_random_vector(384);
                        let vec2 = generate_random_vector(384);
                        let _distance = calculator.distance(&vec1, &vec2);
                    }
                    
                    calculations_per_thread
                })
            })
            .collect();
        
        // Wait for all threads to complete
        let mut total_calculations = 0;
        for handle in handles {
            total_calculations += handle.await.unwrap();
        }
        
        assert_eq!(total_calculations, num_threads * calculations_per_thread);
        
        println!("✅ Concurrent operations safety test successful");
        Ok(())
    }
}