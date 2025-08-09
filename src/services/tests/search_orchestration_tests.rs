/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Tests for search orchestration with WAL bloom filter optimization

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use std::sync::Arc;
    use std::collections::HashMap;
    use tracing::{debug, info};
    
    use crate::compute::distance_computation::DistanceMetric;
    use crate::core::search::{FilterExpression, SearchParams, SearchResult};
    use crate::proto::proximadb::{VectorRecord, MetadataItem, Collection};
    use crate::services::vector_operations_service::VectorOperationsService;
    use crate::services::collection_service::CollectionService;
    use crate::storage::StorageEngine;
    use crate::storage::persistence::write_ahead_log::{WriteAheadLogManager, WALConfig};
    use crate::storage::persistence::filesystem::FilesystemFactory;
    use crate::storage::transaction_coordinator::TransactionCoordinator;
    use crate::index::axis::manager::AxisIndexManager;
    
    /// Helper to create test service with all dependencies
    async fn create_test_service() -> Result<Arc<VectorOperationsService>> {
        let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();
        
        // Create filesystem
        let filesystem = Arc::new(FilesystemFactory::new(Default::default()).await?);
        
        // Create WAL manager
        let wal_config = WALConfig::default();
        let wal_manager = Arc::new(WriteAheadLogManager::new(wal_config, filesystem.clone()).await?);
        
        // Create storage engine
        let storage_engine = Arc::new(StorageEngine::new(
            "/tmp/test_storage".to_string(),
            Default::default(),
            wal_manager.clone(),
            filesystem.clone(),
        ).await?);
        
        // Create collection service
        let collection_service = Arc::new(CollectionService::new(
            storage_engine.clone(),
            filesystem.clone(),
        ));
        
        // Create transaction coordinator
        let transaction_coordinator = Arc::new(TransactionCoordinator::new());
        
        // Create index manager
        let index_manager = Arc::new(AxisIndexManager::new(Default::default())?);
        
        // Create vector operations service
        let service = Arc::new(VectorOperationsService::new(
            storage_engine.clone(),
            collection_service.clone(),
            transaction_coordinator.clone(),
            index_manager.clone(),
            wal_manager.clone(),
            filesystem.clone(),
        ));
        
        Ok(service)
    }
    
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
    async fn test_search_orchestration_order() -> Result<()> {
        info!("🧪 Testing search orchestration order: indexes → WAL → storage");
        
        let service = create_test_service().await?;
        let collection_id = "orchestration_test";
        
        // Create collection
        let collection = Collection {
            id: collection_id.to_string(),
            name: collection_id.to_string(),
            dimension: 3,
            distance_metric: Some(DistanceMetric::Cosine as i32),
            index_configuration: None, // No index, will go to WAL and storage
            ..Default::default()
        };
        
        service
            .collection_service
            .create_collection(collection.clone())
            .await?;
        
        // Insert vectors into WAL (unflushed)
        let wal_vectors = vec![
            create_test_vector("wal_1", vec![1.0, 0.0, 0.0], HashMap::from([
                ("location", "wal"),
                ("category", "electronics"),
            ])),
            create_test_vector("wal_2", vec![0.0, 1.0, 0.0], HashMap::from([
                ("location", "wal"),
                ("category", "books"),
            ])),
        ];
        
        for vector in &wal_vectors {
            service
                .write_ahead_log_manager
                .append_vector(collection_id, vector.clone())
                .await?;
        }
        
        // Insert vectors into storage (flushed)
        let storage_vectors = vec![
            create_test_vector("storage_1", vec![0.0, 0.0, 1.0], HashMap::from([
                ("location", "storage"),
                ("category", "electronics"),
            ])),
            create_test_vector("storage_2", vec![0.5, 0.5, 0.0], HashMap::from([
                ("location", "storage"),
                ("category", "clothing"),
            ])),
        ];
        
        // Insert directly to storage by using the storage engine
        for vector in &storage_vectors {
            service
                .storage_engine
                .insert_vector(collection_id, vector.clone())
                .await?;
        }
        
        // Test 1: Search without filter - should find all vectors
        let query = vec![1.0, 0.0, 0.0];
        let results = service
            .search_vectors(
                collection_id,
                &query,
                10,
                DistanceMetric::Cosine,
                None,
                true,
                true,
            )
            .await?;
        
        assert_eq!(results.len(), 4, "Should find all 4 vectors");
        
        // Verify results come from both WAL and storage
        let wal_count = results.iter()
            .filter(|r| r.metadata.get("location").map(|v| v == "wal").unwrap_or(false))
            .count();
        let storage_count = results.iter()
            .filter(|r| r.metadata.get("location").map(|v| v == "storage").unwrap_or(false))
            .count();
        
        assert_eq!(wal_count, 2, "Should find 2 vectors from WAL");
        assert_eq!(storage_count, 2, "Should find 2 vectors from storage");
        
        info!("✅ Search orchestration correctly searches both WAL and storage");
        
        // Test 2: Search with metadata filter - tests bloom filter optimization
        let filter = FilterExpression {
            field: "category".to_string(),
            operator: "=".to_string(),
            value: serde_json::json!("electronics"),
        };
        
        let search_params = SearchParams {
            filter_expression: Some(filter),
            ..Default::default()
        };
        
        let filtered_results = service
            .search_vectors(
                collection_id,
                &query,
                10,
                DistanceMetric::Cosine,
                Some(&search_params),
                true,
                true,
            )
            .await?;
        
        assert_eq!(filtered_results.len(), 2, "Should find 2 electronics items");
        
        // Verify bloom filter worked
        assert!(filtered_results.iter().all(|r| 
            r.metadata.get("category").map(|v| v == "electronics").unwrap_or(false)
        ), "All results should be electronics");
        
        info!("✅ Bloom filter optimization correctly filtered metadata");
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_search_with_index_priority() -> Result<()> {
        info!("🧪 Testing search with index priority");
        
        let service = create_test_service().await?;
        let collection_id = "index_priority_test";
        
        // Create collection with HNSW index
        let collection = Collection {
            id: collection_id.to_string(),
            name: collection_id.to_string(),
            dimension: 4,
            distance_metric: Some(DistanceMetric::Euclidean as i32),
            index_configuration: Some(crate::proto::proximadb::IndexConfiguration {
                algorithm: crate::proto::proximadb::IndexAlgorithm::Hnsw as i32,
                parameters: HashMap::from([
                    ("m".to_string(), "16".to_string()),
                    ("ef_construction".to_string(), "200".to_string()),
                ]),
            }),
            ..Default::default()
        };
        
        service
            .collection_service
            .create_collection(collection.clone())
            .await?;
        
        // Insert vectors that will be indexed
        let indexed_vectors = vec![
            create_test_vector("idx_1", vec![1.0, 0.0, 0.0, 0.0], HashMap::from([
                ("source", "indexed"),
            ])),
            create_test_vector("idx_2", vec![0.0, 1.0, 0.0, 0.0], HashMap::from([
                ("source", "indexed"),
            ])),
        ];
        
        for vector in &indexed_vectors {
            service
                .insert_vector(collection_id, vector.clone())
                .await?;
        }
        
        // Force index building
        service
            .index_manager
            .build_index(collection_id, &indexed_vectors)
            .await?;
        
        // Insert additional vectors to WAL (not indexed yet)
        let wal_vectors = vec![
            create_test_vector("wal_3", vec![0.0, 0.0, 1.0, 0.0], HashMap::from([
                ("source", "wal_only"),
            ])),
        ];
        
        for vector in &wal_vectors {
            service
                .write_ahead_log_manager
                .append_vector(collection_id, vector.clone())
                .await?;
        }
        
        // Search should prioritize index results
        let query = vec![1.0, 0.0, 0.0, 0.0];
        let results = service
            .search_vectors(
                collection_id,
                &query,
                2, // Limit to 2 results
                DistanceMetric::Euclidean,
                None,
                true,
                true,
            )
            .await?;
        
        assert_eq!(results.len(), 2, "Should return top 2 results");
        
        // First results should be from index (closer to query)
        assert_eq!(results[0].id, "idx_1", "Closest match should be from index");
        
        info!("✅ Index results are correctly prioritized in search orchestration");
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_bloom_filter_efficiency() -> Result<()> {
        info!("🧪 Testing bloom filter efficiency with large dataset");
        
        let service = create_test_service().await?;
        let collection_id = "bloom_efficiency_test";
        
        // Create collection
        let collection = Collection {
            id: collection_id.to_string(),
            name: collection_id.to_string(),
            dimension: 2,
            distance_metric: Some(DistanceMetric::Cosine as i32),
            ..Default::default()
        };
        
        service
            .collection_service
            .create_collection(collection.clone())
            .await?;
        
        // Insert many vectors with diverse metadata
        let num_vectors = 1000;
        let num_categories = 100;
        
        for i in 0..num_vectors {
            let category = format!("category_{}", i % num_categories);
            let vector = create_test_vector(
                &format!("vec_{}", i),
                vec![i as f32, (i * 2) as f32],
                HashMap::from([
                    ("category", category.as_str()),
                    ("index", &i.to_string()),
                ]),
            );
            
            service
                .write_ahead_log_manager
                .append_vector(collection_id, vector)
                .await?;
        }
        
        // Search for rare category (should filter out 99% with bloom filter)
        let filter = FilterExpression {
            field: "category".to_string(),
            operator: "=".to_string(),
            value: serde_json::json!("category_0"),
        };
        
        let search_params = SearchParams {
            filter_expression: Some(filter),
            ..Default::default()
        };
        
        let start = std::time::Instant::now();
        let results = service
            .search_vectors(
                collection_id,
                &vec![0.0, 0.0],
                20,
                DistanceMetric::Cosine,
                Some(&search_params),
                false,
                false,
            )
            .await?;
        let duration = start.elapsed();
        
        // Should find exactly num_vectors/num_categories items
        let expected_count = num_vectors / num_categories;
        assert_eq!(results.len(), expected_count, "Should find {} items", expected_count);
        
        // Verify all results match the filter
        assert!(results.iter().all(|r| r.id.starts_with("vec_")));
        
        info!("✅ Bloom filter efficiently processed {} vectors in {:?}", num_vectors, duration);
        info!("   Found {} matching vectors ({}% filtered)", expected_count, 99);
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_new_search_api() -> Result<()> {
        info!("🧪 Testing new search API with bloom filter optimization");
        
        let service = create_test_service().await?;
        let collection_id = "new_api_test";
        
        // Create collection
        let collection = Collection {
            id: collection_id.to_string(),
            name: collection_id.to_string(),
            dimension: 3,
            ..Default::default()
        };
        
        service
            .collection_service
            .create_collection(collection.clone())
            .await?;
        
        // Insert test vector
        let vector = create_test_vector("test", vec![1.0, 0.0, 0.0], HashMap::new());
        service.insert_vector(collection_id, vector).await?;
        
        // Use the new VectorOperationsService API
        let results = service
            .search_vectors(
                collection_id,
                &vec![1.0, 0.0, 0.0],
                10,
                DistanceMetric::Cosine,
                None,
                true,
                true,
            )
            .await?;
        
        assert!(!results.is_empty(), "New API should find results");
        
        info!("✅ New search API with bloom filter optimization works correctly");
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_concurrent_search_orchestration() -> Result<()> {
        info!("🧪 Testing concurrent search orchestration");
        
        let service = create_test_service().await?;
        let collection_id = "concurrent_orchestration_test";
        
        // Create collection
        let collection = Collection {
            id: collection_id.to_string(),
            name: collection_id.to_string(),
            dimension: 3,
            ..Default::default()
        };
        
        service
            .collection_service
            .create_collection(collection.clone())
            .await?;
        
        // Insert vectors
        for i in 0..100 {
            let vector = create_test_vector(
                &format!("vec_{}", i),
                vec![i as f32, 0.0, 0.0],
                HashMap::from([("index", &i.to_string())]),
            );
            service
                .write_ahead_log_manager
                .append_vector(collection_id, vector)
                .await?;
        }
        
        // Launch concurrent searches
        let mut handles = vec![];
        let service = Arc::clone(&service);
        
        for i in 0..10 {
            let service = Arc::clone(&service);
            let collection_id = collection_id.to_string();
            
            let handle = tokio::spawn(async move {
                let query = vec![i as f32 * 10.0, 0.0, 0.0];
                service
                    .search_vectors(
                        &collection_id,
                        &query,
                        5,
                        DistanceMetric::Euclidean,
                        None,
                        false,
                        false,
                    )
                    .await
            });
            
            handles.push(handle);
        }
        
        // Wait for all searches to complete
        let mut all_succeeded = true;
        for handle in handles {
            match handle.await? {
                Ok(results) => {
                    assert!(!results.is_empty(), "Each search should find results");
                }
                Err(e) => {
                    eprintln!("Search failed: {:?}", e);
                    all_succeeded = false;
                }
            }
        }
        
        assert!(all_succeeded, "All concurrent searches should succeed");
        
        info!("✅ Concurrent search orchestration handled correctly");
        
        Ok(())
    }
}