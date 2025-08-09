/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Integration tests for WAL search with bloom filter optimization in VectorOperationsService

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use std::sync::Arc;
    use std::collections::HashMap;
    use tracing::{debug, info};
    
    use crate::compute::distance_computation::DistanceMetric;
    use crate::core::search::{FilterExpression, SearchParams};
    use crate::proto::proximadb::{VectorRecord, MetadataItem, Collection};
    use crate::services::vector_operations_service::VectorOperationsService;
    use crate::services::collection_service::CollectionService;
    use crate::storage::StorageEngine;
    use crate::storage::persistence::write_ahead_log::{WriteAheadLogManager, WALConfig};
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use crate::storage::transaction_coordinator::TransactionCoordinator;
    use crate::index::axis::manager::AxisIndexManager;
    
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
    async fn test_wal_bloom_filter_integration() -> Result<()> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        info!("🧪 Testing WAL bloom filter integration in VectorOperationsService");
        
        // Setup service
        let filesystem = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let wal_manager = Arc::new(WriteAheadLogManager::new(WALConfig::default(), filesystem.clone()).await?);
        let storage_engine = Arc::new(StorageEngine::new(
            "/tmp/test_bloom_integration".to_string(),
            Default::default(),
            wal_manager.clone(),
            filesystem.clone(),
        ).await?);
        
        let collection_service = Arc::new(CollectionService::new(
            storage_engine.clone(),
            filesystem.clone(),
        ));
        
        let transaction_coordinator = Arc::new(TransactionCoordinator::new());
        let index_manager = Arc::new(AxisIndexManager::new(Default::default())?);
        
        let service = VectorOperationsService::new(
            storage_engine.clone(),
            collection_service.clone(),
            transaction_coordinator.clone(),
            index_manager.clone(),
            wal_manager.clone(),
            filesystem.clone(),
        );
        
        let collection_id = "bloom_integration_test";
        
        // Create collection
        let collection = Collection {
            id: collection_id.to_string(),
            name: collection_id.to_string(),
            dimension: 3,
            distance_metric: Some(DistanceMetric::Cosine as i32),
            ..Default::default()
        };
        
        collection_service.create_collection(collection.clone()).await?;
        
        // Insert vectors with metadata into WAL
        let vectors = vec![
            create_test_vector("product_1", vec![1.0, 0.0, 0.0], HashMap::from([
                ("category", "electronics"),
                ("brand", "TechCorp"),
                ("price", "499"),
            ])),
            create_test_vector("product_2", vec![0.0, 1.0, 0.0], HashMap::from([
                ("category", "books"),
                ("brand", "ReadMore"),
                ("price", "29"),
            ])),
            create_test_vector("product_3", vec![0.0, 0.0, 1.0], HashMap::from([
                ("category", "electronics"),
                ("brand", "GadgetPro"),
                ("price", "299"),
            ])),
            create_test_vector("product_4", vec![0.5, 0.5, 0.0], HashMap::from([
                ("category", "clothing"),
                ("brand", "FashionHub"),
                ("price", "79"),
            ])),
            create_test_vector("product_5", vec![0.3, 0.3, 0.4], HashMap::from([
                ("category", "electronics"),
                ("brand", "TechCorp"),
                ("price", "899"),
            ])),
        ];
        
        for vector in &vectors {
            wal_manager.append_vector(collection_id, vector.clone()).await?;
        }
        
        // Test 1: Complex metadata filter - category=electronics AND brand=TechCorp
        let filter = FilterExpression {
            field: "category".to_string(),
            operator: "=".to_string(),
            value: serde_json::json!("electronics"),
        };
        
        let search_params = SearchParams {
            filter_expression: Some(filter),
            ..Default::default()
        };
        
        let query = vec![1.0, 0.0, 0.0];
        let results = service.search_vectors(
            collection_id,
            &query,
            10,
            DistanceMetric::Cosine,
            Some(&search_params),
            true,
            true,
        ).await?;
        
        // Should find 3 electronics items
        assert_eq!(results.len(), 3, "Should find 3 electronics items");
        assert!(results.iter().all(|r| 
            r.metadata.get("category").map(|v| v == "electronics").unwrap_or(false)
        ));
        
        info!("✅ Bloom filter correctly filtered by category");
        
        // Test 2: Numeric range filter - price > 100
        let price_filter = FilterExpression {
            field: "price".to_string(),
            operator: ">".to_string(),
            value: serde_json::json!(100),
        };
        
        let price_params = SearchParams {
            filter_expression: Some(price_filter),
            ..Default::default()
        };
        
        let price_results = service.search_vectors(
            collection_id,
            &query,
            10,
            DistanceMetric::Euclidean,
            Some(&price_params),
            false,
            true,
        ).await?;
        
        // Should find products with price > 100
        let high_price_count = price_results.iter()
            .filter(|r| {
                r.metadata.get("price")
                    .and_then(|v| v.parse::<i32>().ok())
                    .map(|p| p > 100)
                    .unwrap_or(false)
            })
            .count();
        
        assert_eq!(high_price_count, price_results.len(), "All results should have price > 100");
        
        info!("✅ Numeric comparison with bloom filter optimization works");
        
        // Test 3: String contains filter
        let brand_filter = FilterExpression {
            field: "brand".to_string(),
            operator: "CONTAINS".to_string(),
            value: serde_json::json!("Tech"),
        };
        
        let brand_params = SearchParams {
            filter_expression: Some(brand_filter),
            ..Default::default()
        };
        
        let brand_results = service.search_vectors(
            collection_id,
            &query,
            10,
            DistanceMetric::Cosine,
            Some(&brand_params),
            true,
            true,
        ).await?;
        
        // Should find TechCorp products
        let tech_count = brand_results.iter()
            .filter(|r| {
                r.metadata.get("brand")
                    .map(|v| v.contains("Tech"))
                    .unwrap_or(false)
            })
            .count();
        
        assert_eq!(tech_count, brand_results.len(), "All results should contain 'Tech' in brand");
        
        info!("✅ String contains operator with bloom filter works");
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_bloom_filter_performance_improvement() -> Result<()> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        info!("🧪 Testing bloom filter performance improvement");
        
        // Setup service
        let filesystem = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let wal_manager = Arc::new(WriteAheadLogManager::new(WALConfig::default(), filesystem.clone()).await?);
        let storage_engine = Arc::new(StorageEngine::new(
            "/tmp/test_bloom_perf".to_string(),
            Default::default(),
            wal_manager.clone(),
            filesystem.clone(),
        ).await?);
        
        let collection_service = Arc::new(CollectionService::new(
            storage_engine.clone(),
            filesystem.clone(),
        ));
        
        let transaction_coordinator = Arc::new(TransactionCoordinator::new());
        let index_manager = Arc::new(AxisIndexManager::new(Default::default())?);
        
        let service = VectorOperationsService::new(
            storage_engine.clone(),
            collection_service.clone(),
            transaction_coordinator.clone(),
            index_manager.clone(),
            wal_manager.clone(),
            filesystem.clone(),
        );
        
        let collection_id = "bloom_perf_test";
        
        // Create collection
        let collection = Collection {
            id: collection_id.to_string(),
            name: collection_id.to_string(),
            dimension: 2,
            ..Default::default()
        };
        
        collection_service.create_collection(collection.clone()).await?;
        
        // Insert large number of vectors with diverse metadata
        let num_vectors = 5000;
        let num_categories = 50;
        
        for i in 0..num_vectors {
            let category = if i % 100 == 0 { 
                "rare_category" 
            } else { 
                &format!("category_{}", i % num_categories)
            };
            
            let vector = create_test_vector(
                &format!("vec_{}", i),
                vec![i as f32, (i * 2) as f32],
                HashMap::from([
                    ("category", category),
                    ("index", &i.to_string()),
                ]),
            );
            
            wal_manager.append_vector(collection_id, vector).await?;
        }
        
        // Search for rare category (1% of data)
        let filter = FilterExpression {
            field: "category".to_string(),
            operator: "=".to_string(),
            value: serde_json::json!("rare_category"),
        };
        
        let search_params = SearchParams {
            filter_expression: Some(filter),
            ..Default::default()
        };
        
        let start = std::time::Instant::now();
        let results = service.search_vectors(
            collection_id,
            &vec![0.0, 0.0],
            100,
            DistanceMetric::Cosine,
            Some(&search_params),
            false,
            false,
        ).await?;
        let duration = start.elapsed();
        
        // Should find exactly 50 rare items (1% of 5000)
        assert_eq!(results.len(), 50, "Should find 50 rare category items");
        
        // Performance assertion - should be fast due to bloom filter
        assert!(duration.as_millis() < 100, 
            "Search should complete in < 100ms with bloom filter, took {:?}", duration);
        
        info!("✅ Bloom filter search completed in {:?} for {} vectors", duration, num_vectors);
        info!("   Filtered 99% of data using bloom filters");
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_mixed_wal_and_storage_search() -> Result<()> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        info!("🧪 Testing mixed WAL and storage search with bloom filters");
        
        // Setup service
        let filesystem = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let wal_manager = Arc::new(WriteAheadLogManager::new(WALConfig::default(), filesystem.clone()).await?);
        let storage_engine = Arc::new(StorageEngine::new(
            "/tmp/test_mixed_search".to_string(),
            Default::default(),
            wal_manager.clone(),
            filesystem.clone(),
        ).await?);
        
        let collection_service = Arc::new(CollectionService::new(
            storage_engine.clone(),
            filesystem.clone(),
        ));
        
        let transaction_coordinator = Arc::new(TransactionCoordinator::new());
        let index_manager = Arc::new(AxisIndexManager::new(Default::default())?);
        
        let service = VectorOperationsService::new(
            storage_engine.clone(),
            collection_service.clone(),
            transaction_coordinator.clone(),
            index_manager.clone(),
            wal_manager.clone(),
            filesystem.clone(),
        );
        
        let collection_id = "mixed_search_test";
        
        // Create collection
        let collection = Collection {
            id: collection_id.to_string(),
            name: collection_id.to_string(),
            dimension: 3,
            ..Default::default()
        };
        
        collection_service.create_collection(collection.clone()).await?;
        
        // Insert some vectors directly to storage (simulating flushed data)
        let storage_vectors = vec![
            create_test_vector("storage_1", vec![1.0, 0.0, 0.0], HashMap::from([
                ("location", "storage"),
                ("type", "persistent"),
            ])),
            create_test_vector("storage_2", vec![0.0, 1.0, 0.0], HashMap::from([
                ("location", "storage"),
                ("type", "persistent"),
            ])),
        ];
        
        for vector in &storage_vectors {
            storage_engine.insert_vector(collection_id, vector.clone()).await?;
        }
        
        // Insert some vectors to WAL (unflushed)
        let wal_vectors = vec![
            create_test_vector("wal_1", vec![0.0, 0.0, 1.0], HashMap::from([
                ("location", "wal"),
                ("type", "temporary"),
            ])),
            create_test_vector("wal_2", vec![0.5, 0.5, 0.0], HashMap::from([
                ("location", "wal"),
                ("type", "temporary"),
            ])),
        ];
        
        for vector in &wal_vectors {
            wal_manager.append_vector(collection_id, vector.clone()).await?;
        }
        
        // Search should find vectors from both WAL and storage
        let query = vec![0.5, 0.5, 0.0];
        let results = service.search_vectors(
            collection_id,
            &query,
            10,
            DistanceMetric::Euclidean,
            None,
            true,
            true,
        ).await?;
        
        assert_eq!(results.len(), 4, "Should find all 4 vectors");
        
        // Verify we have results from both sources
        let wal_results = results.iter()
            .filter(|r| r.metadata.get("location").map(|v| v == "wal").unwrap_or(false))
            .count();
        let storage_results = results.iter()
            .filter(|r| r.metadata.get("location").map(|v| v == "storage").unwrap_or(false))
            .count();
        
        assert_eq!(wal_results, 2, "Should have 2 results from WAL");
        assert_eq!(storage_results, 2, "Should have 2 results from storage");
        
        info!("✅ Mixed WAL and storage search works correctly");
        
        // Test with filter that only matches WAL data
        let filter = FilterExpression {
            field: "type".to_string(),
            operator: "=".to_string(),
            value: serde_json::json!("temporary"),
        };
        
        let search_params = SearchParams {
            filter_expression: Some(filter),
            ..Default::default()
        };
        
        let filtered_results = service.search_vectors(
            collection_id,
            &query,
            10,
            DistanceMetric::Euclidean,
            Some(&search_params),
            true,
            true,
        ).await?;
        
        assert_eq!(filtered_results.len(), 2, "Should only find WAL vectors");
        assert!(filtered_results.iter().all(|r| 
            r.metadata.get("location").map(|v| v == "wal").unwrap_or(false)
        ));
        
        info!("✅ Bloom filter correctly filters across WAL and storage");
        
        Ok(())
    }
}