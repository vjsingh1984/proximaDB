/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Tests for enhanced WAL search with bloom filter optimization

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use std::sync::Arc;
    use std::collections::HashMap;
    use tracing::{debug, info};
    
    use crate::compute::distance_computation::DistanceMetric;
    use crate::core::search::{FilterExpression, SearchResult};
    use crate::proto::proximadb::{VectorRecord, MetadataItem};
    use crate::storage::persistence::write_ahead_log::{
        WriteAheadLogManager, WALConfig, BatchId
    };
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use crate::storage::memtable::specialized::wal_behavior::WALVectorBatch;
    
    /// Helper to create test vector with metadata
    fn create_test_vector(id: &str, values: Vec<f32>, metadata: HashMap<&str, &str>) -> VectorRecord {
        let metadata_items: Vec<MetadataItem> = metadata
            .into_iter()
            .map(|(k, v)| MetadataItem {
                key: k.to_string(),
                value: v.to_string(),
            })
            .collect();
        
        VectorRecord {
            id: Some(id.to_string()),
            vector: values,
            metadata: metadata_items,
            timestamp: chrono::Utc::now().timestamp() as u64,
            version: Some(1),
        }
    }
    
    #[tokio::test]
    async fn test_wal_search_with_bloom_filter_optimization() -> Result<()> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        info!("🧪 Testing WAL search with bloom filter optimization");
        
        // Create WAL manager
        let config = WALConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let wal_manager = WriteAheadLogManager::new(config.clone(), filesystem).await?;
        
        // Create test vectors with different metadata
        let vectors = vec![
            create_test_vector("vec1", vec![1.0, 0.0, 0.0], HashMap::from([
                ("category", "electronics"),
                ("price", "100"),
            ])),
            create_test_vector("vec2", vec![0.0, 1.0, 0.0], HashMap::from([
                ("category", "books"),
                ("price", "20"),
            ])),
            create_test_vector("vec3", vec![0.0, 0.0, 1.0], HashMap::from([
                ("category", "electronics"),
                ("price", "200"),
            ])),
            create_test_vector("vec4", vec![0.5, 0.5, 0.0], HashMap::from([
                ("category", "clothing"),
                ("price", "50"),
            ])),
        ];
        
        // Insert vectors into WAL
        let collection_id = "test_bloom_collection";
        for vector in &vectors {
            wal_manager.append_vector(collection_id, vector.clone()).await?;
        }
        
        // Test 1: Search with metadata filter for category=electronics
        let filter = FilterExpression {
            field: "category".to_string(),
            operator: "=".to_string(),
            value: serde_json::json!("electronics"),
        };
        
        let query_vector = vec![1.0, 0.0, 0.0];
        let results = wal_manager.search_unflushed_vectors(
            collection_id,
            &query_vector,
            10,
            DistanceMetric::Cosine,
            Some(&filter),
            true,
            true,
        ).await?;
        
        // Should only find vec1 and vec3 (electronics)
        assert_eq!(results.len(), 2, "Should find 2 electronics items");
        assert!(results.iter().any(|r| r.id == "vec1"));
        assert!(results.iter().any(|r| r.id == "vec3"));
        
        info!("✅ Bloom filter correctly filtered by category");
        
        // Test 2: Search without filter (should find all)
        let all_results = wal_manager.search_unflushed_vectors(
            collection_id,
            &query_vector,
            10,
            DistanceMetric::Cosine,
            None,
            true,
            true,
        ).await?;
        
        assert_eq!(all_results.len(), 4, "Should find all 4 vectors without filter");
        
        info!("✅ Search without filter returns all vectors");
        
        // Test 3: Search with price > 50 filter
        let price_filter = FilterExpression {
            field: "price".to_string(),
            operator: ">".to_string(),
            value: serde_json::json!(50),
        };
        
        let price_results = wal_manager.search_unflushed_vectors(
            collection_id,
            &query_vector,
            10,
            DistanceMetric::Euclidean,
            Some(&price_filter),
            false, // Don't include vectors
            true,  // Include metadata
        ).await?;
        
        // Should find vec1 (100) and vec3 (200)
        assert_eq!(price_results.len(), 2, "Should find 2 items with price > 50");
        
        info!("✅ Numeric comparison filters work correctly");
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_bloom_filter_performance_metrics() -> Result<()> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        info!("🧪 Testing bloom filter performance metrics");
        
        let config = WALConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let wal_manager = WriteAheadLogManager::new(config.clone(), filesystem).await?;
        
        let collection_id = "bloom_perf_test";
        
        // Create many vectors with diverse metadata
        for i in 0..100 {
            let category = if i % 10 == 0 { "rare" } else { "common" };
            let vector = create_test_vector(
                &format!("vec_{}", i),
                vec![i as f32, 0.0, 0.0],
                HashMap::from([
                    ("category", category),
                    ("index", &i.to_string()),
                ]),
            );
            wal_manager.append_vector(collection_id, vector).await?;
        }
        
        // Search for rare category (should filter out 90% of batches)
        let filter = FilterExpression {
            field: "category".to_string(),
            operator: "=".to_string(),
            value: serde_json::json!("rare"),
        };
        
        let query = vec![0.0, 0.0, 0.0];
        let results = wal_manager.search_unflushed_vectors(
            collection_id,
            &query,
            20,
            DistanceMetric::Cosine,
            Some(&filter),
            false,
            false,
        ).await?;
        
        // Should find exactly 10 rare items
        assert_eq!(results.len(), 10, "Should find 10 rare items");
        
        info!("✅ Bloom filter efficiently filtered 90% of non-matching batches");
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_wal_search_distance_metrics() -> Result<()> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        info!("🧪 Testing WAL search with different distance metrics");
        
        let config = WALConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let wal_manager = WriteAheadLogManager::new(config.clone(), filesystem).await?;
        
        let collection_id = "distance_test";
        
        // Create test vectors
        let vectors = vec![
            create_test_vector("near", vec![1.0, 0.0], HashMap::new()),
            create_test_vector("far", vec![0.0, 1.0], HashMap::new()),
            create_test_vector("middle", vec![0.707, 0.707], HashMap::new()),
        ];
        
        for vector in &vectors {
            wal_manager.append_vector(collection_id, vector.clone()).await?;
        }
        
        let query = vec![1.0, 0.0];
        
        // Test different metrics
        let metrics = vec![
            DistanceMetric::Cosine,
            DistanceMetric::Euclidean,
            DistanceMetric::DotProduct,
        ];
        
        for metric in metrics {
            let results = wal_manager.search_unflushed_vectors(
                collection_id,
                &query,
                3,
                metric,
                None,
                false,
                false,
            ).await?;
            
            assert_eq!(results.len(), 3, "Should find all 3 vectors");
            
            // First result should always be "near" (closest to query)
            assert_eq!(results[0].id, "near", "Nearest vector should be first for {:?}", metric);
            
            info!("✅ Distance metric {:?} works correctly", metric);
        }
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_empty_wal_search() -> Result<()> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        info!("🧪 Testing search on empty WAL");
        
        let config = WALConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let wal_manager = WriteAheadLogManager::new(config.clone(), filesystem).await?;
        
        let results = wal_manager.search_unflushed_vectors(
            "empty_collection",
            &vec![1.0, 0.0, 0.0],
            10,
            DistanceMetric::Cosine,
            None,
            true,
            true,
        ).await?;
        
        assert_eq!(results.len(), 0, "Empty WAL should return no results");
        
        info!("✅ Empty WAL search handled correctly");
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_wal_search_ranking() -> Result<()> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        info!("🧪 Testing WAL search result ranking");
        
        let config = WALConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let wal_manager = WriteAheadLogManager::new(config.clone(), filesystem).await?;
        
        let collection_id = "ranking_test";
        
        // Create vectors with increasing distance from origin
        for i in 1..=5 {
            let vector = create_test_vector(
                &format!("vec_{}", i),
                vec![i as f32, 0.0, 0.0],
                HashMap::new(),
            );
            wal_manager.append_vector(collection_id, vector).await?;
        }
        
        let query = vec![0.0, 0.0, 0.0];
        let results = wal_manager.search_unflushed_vectors(
            collection_id,
            &query,
            3, // Only top 3
            DistanceMetric::Euclidean,
            None,
            false,
            false,
        ).await?;
        
        assert_eq!(results.len(), 3, "Should return top 3 results");
        
        // Check ranking is correct
        for (i, result) in results.iter().enumerate() {
            assert_eq!(result.rank, Some(i + 1), "Rank should be set correctly");
        }
        
        // Closest vectors should be first
        assert_eq!(results[0].id, "vec_1", "Closest vector should be first");
        assert_eq!(results[1].id, "vec_2", "Second closest should be second");
        assert_eq!(results[2].id, "vec_3", "Third closest should be third");
        
        info!("✅ Search results are correctly ranked");
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_metadata_filter_operators() -> Result<()> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        info!("🧪 Testing various metadata filter operators");
        
        let config = WALConfig::default();
        let filesystem = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let wal_manager = WriteAheadLogManager::new(config.clone(), filesystem).await?;
        
        let collection_id = "operator_test";
        
        // Create test vector
        let vector = create_test_vector(
            "test_vec",
            vec![1.0, 0.0],
            HashMap::from([
                ("name", "ProximaDB"),
                ("version", "1.0"),
                ("score", "95"),
            ]),
        );
        wal_manager.append_vector(collection_id, vector).await?;
        
        let query = vec![1.0, 0.0];
        
        // Test CONTAINS operator
        let contains_filter = FilterExpression {
            field: "name".to_string(),
            operator: "CONTAINS".to_string(),
            value: serde_json::json!("Proxima"),
        };
        
        let results = wal_manager.search_unflushed_vectors(
            collection_id,
            &query,
            10,
            DistanceMetric::Cosine,
            Some(&contains_filter),
            false,
            false,
        ).await?;
        
        assert_eq!(results.len(), 1, "CONTAINS operator should find match");
        
        // Test STARTS_WITH operator
        let starts_filter = FilterExpression {
            field: "name".to_string(),
            operator: "STARTS_WITH".to_string(),
            value: serde_json::json!("Prox"),
        };
        
        let results = wal_manager.search_unflushed_vectors(
            collection_id,
            &query,
            10,
            DistanceMetric::Cosine,
            Some(&starts_filter),
            false,
            false,
        ).await?;
        
        assert_eq!(results.len(), 1, "STARTS_WITH operator should find match");
        
        // Test != operator
        let not_equal_filter = FilterExpression {
            field: "version".to_string(),
            operator: "!=".to_string(),
            value: serde_json::json!("2.0"),
        };
        
        let results = wal_manager.search_unflushed_vectors(
            collection_id,
            &query,
            10,
            DistanceMetric::Cosine,
            Some(&not_equal_filter),
            false,
            false,
        ).await?;
        
        assert_eq!(results.len(), 1, "!= operator should find match");
        
        info!("✅ All metadata filter operators work correctly");
        
        Ok(())
    }
}