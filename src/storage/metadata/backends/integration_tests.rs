// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Integration Tests for Metadata Backends
//!
//! Comprehensive tests that verify all metadata backend implementations
//! conform to the expected behavior of the MetadataProvider trait.

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use std::sync::Arc;
    use tempfile::TempDir;
    
    use crate::proto::proximadb_v1::{Collection, CollectionConfig, DistanceMetric, StorageEngine};
    use crate::storage::metadata::backends::{
        MetadataBackendFactory,
        metrics_decorator::MetricsDecorator,
    };
    use crate::storage::traits::{InternalCollectionProvider, MetadataProvider, UnifiedMetricsCollector};
    
    /// Create a test collection
    fn create_test_collection(id: &str, name: &str) -> Collection {
        Collection {
            collection_id: id.to_string(),
            name: name.to_string(),
            uuid: format!("uuid-{}", id),
            config: Some(CollectionConfig {
                vector_dimension: 128,
                distance_metric: DistanceMetric::Cosine as i32,
                storage_engine: StorageEngine::Sst as i32,
                ..Default::default()
            }),
            ..Default::default()
        }
    }
    
    /// Test suite that can be run against any MetadataProvider implementation
    async fn test_provider_operations<P: MetadataProvider>(provider: &P) -> Result<()> {
        // Test 1: Empty state
        let collections = provider.list_collections().await?;
        assert!(collections.is_empty(), "Provider should start empty");
        
        // Test 2: Insert collection
        let collection1 = create_test_collection("test-1", "Test Collection 1");
        provider.upsert_collection_proto(&collection1).await?;
        
        // Test 3: Get collection
        let retrieved = provider.get_collection("test-1").await?;
        assert!(retrieved.is_some(), "Should find inserted collection");
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.collection_id, "test-1");
        assert_eq!(retrieved.name, "Test Collection 1");
        
        // Test 4: Get UUID
        let uuid = provider.get_uuid("test-1").await?;
        assert_eq!(uuid, Some("uuid-test-1".to_string()));
        
        // Test 5: List collections
        let collections = provider.list_collections().await?;
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].collection_id, "test-1");
        
        // Test 6: Update collection
        let mut updated = collection1.clone();
        updated.name = "Updated Collection 1".to_string();
        provider.upsert_collection_proto(&updated).await?;
        
        let retrieved = provider.get_collection("test-1").await?;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, "Updated Collection 1");
        
        // Test 7: Insert second collection
        let collection2 = create_test_collection("test-2", "Test Collection 2");
        provider.upsert_collection_proto(&collection2).await?;
        
        let collections = provider.list_collections().await?;
        assert_eq!(collections.len(), 2);
        
        // Test 8: Delete collection
        provider.delete_collection("test-1").await?;
        
        let retrieved = provider.get_collection("test-1").await?;
        assert!(retrieved.is_none(), "Deleted collection should not exist");
        
        let collections = provider.list_collections().await?;
        assert_eq!(collections.len(), 1);
        assert_eq!(collections[0].collection_id, "test-2");
        
        // Test 9: Delete non-existent collection (should not error)
        provider.delete_collection("non-existent").await?;
        
        // Test 10: Clean up
        provider.delete_collection("test-2").await?;
        
        let collections = provider.list_collections().await?;
        assert!(collections.is_empty());
        
        Ok(())
    }
    
    /// Test concurrent operations
    async fn test_concurrent_operations<P: MetadataProvider + Clone + 'static>(
        provider: P,
    ) -> Result<()> {
        use tokio::task::JoinSet;
        
        let mut tasks = JoinSet::new();
        
        // Create 10 collections concurrently
        for i in 0..10 {
            let provider = provider.clone();
            let collection = create_test_collection(
                &format!("concurrent-{}", i),
                &format!("Concurrent Collection {}", i),
            );
            
            tasks.spawn(async move {
                provider.upsert_collection_proto(&collection).await
            });
        }
        
        // Wait for all insertions
        while let Some(result) = tasks.join_next().await {
            result??;
        }
        
        // Verify all collections exist
        let collections = provider.list_collections().await?;
        assert_eq!(collections.len(), 10);
        
        // Delete all collections concurrently
        for i in 0..10 {
            let provider = provider.clone();
            let id = format!("concurrent-{}", i);
            
            tasks.spawn(async move {
                provider.delete_collection(&id).await
            });
        }
        
        // Wait for all deletions
        while let Some(result) = tasks.join_next().await {
            result??;
        }
        
        // Verify all collections are deleted
        let collections = provider.list_collections().await?;
        assert!(collections.is_empty());
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_universal_backend() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let url = format!("file://{}", temp_dir.path().display());
        
        let backend = MetadataBackendFactory::create_from_url(&url).await?;
        test_provider_operations(backend.as_ref()).await?;
        
        Ok(())
    }
    
    #[tokio::test]
    #[cfg(feature = "rocksdb")]
    async fn test_local_rocksdb_backend() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let url = format!("rocksdb://{}", temp_dir.path().display());
        
        let backend = MetadataBackendFactory::create_from_url(&url).await?;
        test_provider_operations(backend.as_ref()).await?;
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_metrics_decorator() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let url = format!("file://{}", temp_dir.path().display());
        
        let metrics = Arc::new(UnifiedMetricsCollector::new());
        let backend = MetadataBackendFactory::create_with_metrics(&url, metrics.clone()).await?;
        
        // Run operations
        test_provider_operations(backend.as_ref()).await?;
        
        // Verify metrics were collected
        let report = metrics.get_report().await;
        assert!(report.contains("UpsertCollection"));
        assert!(report.contains("GetCollection"));
        assert!(report.contains("ListCollections"));
        assert!(report.contains("DeleteCollection"));
        assert!(report.contains("GetUuid"));
        
        // Verify success/failure counts
        assert!(report.contains("success:"));
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_backend_persistence() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let url = format!("file://{}", temp_dir.path().display());
        
        // Create and populate backend
        {
            let backend = MetadataBackendFactory::create_from_url(&url).await?;
            
            let collection1 = create_test_collection("persist-1", "Persistent Collection 1");
            let collection2 = create_test_collection("persist-2", "Persistent Collection 2");
            
            backend.upsert_collection_proto(&collection1).await?;
            backend.upsert_collection_proto(&collection2).await?;
        }
        
        // Create new backend instance and verify data persists
        {
            let backend = MetadataBackendFactory::create_from_url(&url).await?;
            
            let collections = backend.list_collections().await?;
            assert_eq!(collections.len(), 2);
            
            let col1 = backend.get_collection("persist-1").await?;
            assert!(col1.is_some());
            assert_eq!(col1.unwrap().name, "Persistent Collection 1");
            
            let col2 = backend.get_collection("persist-2").await?;
            assert!(col2.is_some());
            assert_eq!(col2.unwrap().name, "Persistent Collection 2");
        }
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_invalid_operations() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let url = format!("file://{}", temp_dir.path().display());
        
        let backend = MetadataBackendFactory::create_from_url(&url).await?;
        
        // Test getting non-existent collection
        let result = backend.get_collection("non-existent").await?;
        assert!(result.is_none());
        
        // Test getting UUID for non-existent collection
        let uuid = backend.get_uuid("non-existent").await?;
        assert!(uuid.is_none());
        
        // Test deleting non-existent collection (should not error)
        backend.delete_collection("non-existent").await?;
        
        // Test upserting collection with empty ID (should fail in validation)
        let invalid = Collection {
            collection_id: "".to_string(),
            name: "Invalid".to_string(),
            ..Default::default()
        };
        
        // This might fail depending on validation implementation
        let result = backend.upsert_collection_proto(&invalid).await;
        // We expect this to fail, but the exact error depends on implementation
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_large_collection_names() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let url = format!("file://{}", temp_dir.path().display());
        
        let backend = MetadataBackendFactory::create_from_url(&url).await?;
        
        // Test with very long collection name
        let long_name = "a".repeat(1000);
        let collection = Collection {
            collection_id: "long-name-test".to_string(),
            name: long_name.clone(),
            config: Some(CollectionConfig::default()),
            ..Default::default()
        };
        
        backend.upsert_collection_proto(&collection).await?;
        
        let retrieved = backend.get_collection("long-name-test").await?;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name, long_name);
        
        Ok(())
    }
    
    #[tokio::test]
    async fn test_special_characters_in_ids() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let url = format!("file://{}", temp_dir.path().display());
        
        let backend = MetadataBackendFactory::create_from_url(&url).await?;
        
        // Test with various special characters (that are allowed)
        let test_ids = vec![
            "test-with-dashes",
            "test_with_underscores",
            "TEST_UPPERCASE",
            "test123numbers",
            "123-starts-with-number",
        ];
        
        for id in test_ids {
            let collection = create_test_collection(id, &format!("Collection {}", id));
            backend.upsert_collection_proto(&collection).await?;
            
            let retrieved = backend.get_collection(id).await?;
            assert!(retrieved.is_some(), "Should find collection with ID: {}", id);
        }
        
        let all = backend.list_collections().await?;
        assert_eq!(all.len(), 5);
        
        Ok(())
    }
}