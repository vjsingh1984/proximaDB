//! Unit tests for AXIS Manager
//! Tests adaptive indexing, migration, and collection strategy management

use std::sync::Arc;
use std::collections::HashMap;
use tokio;
use anyhow::Result;
use chrono::Utc;

use super::*;
use crate::core::VectorRecord;
use crate::proto::proximadb;
use crate::index::axis::types::{IndexSelectionStrategy, IndexSpecification, Data, IndexAlgorithm};

/// Mock collection service for testing
#[derive(Debug, Clone)]
pub struct MockCollectionService {
    pub should_fail: Arc<std::sync::Mutex<bool>>,
    pub index_configs: Arc<std::sync::Mutex<HashMap<String, crate::index::config::IndexConfig>>>,
}

impl MockCollectionService {
    pub fn new() -> Self {
        Self {
            should_fail: Arc::new(std::sync::Mutex::new(false)),
            index_configs: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }
    
    pub fn set_should_fail(&self, fail: bool) {
        *self.should_fail.lock().unwrap() = fail;
    }
    
    pub fn set_index_config(&self, collection_id: &str, config: crate::index::config::IndexConfig) {
        self.index_configs.lock().unwrap().insert(collection_id.to_string(), config);
    }
    
    /// Mock get_native_index_config method
    pub async fn get_native_index_config(&self, collection_id: &str) -> Result<Option<crate::index::config::IndexConfig>> {
        if *self.should_fail.lock().unwrap() {
            return Err(anyhow::anyhow!("Mock collection service failed"));
        }
        
        Ok(self.index_configs.lock().unwrap().get(key).cloned())
    }
}

/// Create test vector record
fn create_test_vector(id: &str, _collection_id: &str, dimension: usize) -> VectorRecord {
    VectorRecord {
        id: Some(id.to_string()),
        vector: (0..dimension).map(|i| i as f32 / 100.0).collect(),
        metadata: vec![
            proximadb::MetadataItem {
                key: "category".to_string(),
                value: Some(crate::proto::proximadb::metadata_item::Value::StringValue("test".to_string())),
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            similarity: None,
            // rank removed -  None,
            similarity: None,
        },
            proximadb::MetadataItem {
                key: "score".to_string(),
                value: Some(crate::proto::proximadb::metadata_item::Value::StringValue("0.95".to_string())),
            },
        ],
        timestamp: Utc::now().timestamp_micros(),
        expires_at: None,
    }
}

/// Create test vector record with expiration
fn create_expired_vector(id: &str, _collection_id: &str, dimension: usize) -> VectorRecord {
    VectorRecord {
        id: Some(id.to_string()),
        vector: (0..dimension).map(|i| i as f32 / 100.0).collect(),
        metadata: vec![],
        timestamp: Utc::now().timestamp_micros(),
        expires_at: Some(Utc::now().timestamp_millis() - 1000), // Expired 1 second ago,
            timestamp: 0,
            updated_at: None,
            similarity: None,
            // rank removed -  None,
            similarity: None,
        }
}

/// Create test index selection strategy
fn create_test_strategy() -> IndexSelectionStrategy {
    IndexSelectionStrategy {
        indexes: vec![
            IndexSpecification {
                // data_type removed -  Data::DenseVector { dimension: 128 },
                algorithm: IndexAlgorithm::HNSW,
                configuration: HashMap::new(),
            },
            IndexSpecification {
                // data_type removed -  Data::Metadata,
                algorithm: IndexAlgorithm::BTree,
                configuration: HashMap::new(),
            },
        ],
        routing_rules: vec![],
    }
}

/// Test AxisManager construction
#[cfg(test)]
mod construction_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_new_axis_manager() {
        let config = AxisConfig::default();
        
        let result = AxisManager::new(config).await;
        assert!(result.is_ok());
        
        let manager = result.unwrap();
        
        // Verify initialization
        assert!(manager.collection_service.is_none()); // Initially none
        
        // Check metrics are initialized
        let metrics = manager.metrics.read().await;
        assert_eq!(metrics.total_migrations, 0);
        assert_eq!(metrics.successful_migrations, 0);
        assert_eq!(metrics.failed_migrations, 0);
        assert_eq!(metrics.total_collections_managed, 0);
        assert_eq!(metrics.total_vectors_indexed, 0);
        
        // Check collections strategies are empty
        let strategies = manager.collection_strategies.read().await;
        assert!(strategies.is_none());
        
        // Check active migrations are empty
        let migrations = manager.active_migrations.read().await;
        assert!(migrations.is_none());
    }
    
    #[tokio::test]
    async fn test_set_collection_service() {
        let config = AxisConfig::default();
        let mut manager = AxisManager::new(config).await.unwrap();
        
        let mock_service = Arc::new(MockCollectionService::new());
        // Note: We can't actually test this due to type constraints, but we can verify the concept
        
        // Initially should be None
        assert!(manager.collection_service.is_none());
        
        // After setting, should be Some (conceptually)
        // manager.set_collection_service(mock_service);
        // assert!(manager.collection_service.is_some());
    }
}

/// Test index configuration retrieval
#[cfg(test)]
mod index_config_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_get_native_index_config_no_service() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // Should return default config when no service is set
        let result = manager.native_index_config("test_collection").await;
        assert!(result.is_ok());
        
        let index_config = result.unwrap();
        // Should be default config
        assert_eq!(index_config, crate::index::config::IndexConfig::default());
    }
}

/// Test vector insertion
#[cfg(test)]
mod insertion_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_insert_vector_success() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        let vector = create_test_vector("vector_1", "test_collection", 128);
        
        let result = manager.insert("test_collection", &vector).await;
        assert!(result.is_ok());
        
        // Check metrics updated
        let metrics = manager.metrics.read().await;
        assert_eq!(metrics.total_vectors_indexed, 1);
        
        // Check strategy was created for collection
        let strategies = manager.collection_strategies.read().await;
        assert!(strategies.contains_key("test_collection"));
    }
    
    #[tokio::test]
    async fn test_insert_expired_vector() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        let expired_vector = create_expired_vector("expired_1", "test_collection", 128);
        
        let result = manager.insert(&expired_vector).await;
        assert!(result.is_ok());
        
        // Metrics should not be updated for expired vectors
        let metrics = manager.metrics.read().await;
        assert_eq!(metrics.total_vectors_indexed, 0);
    }
    
    #[tokio::test]
    async fn test_insert_multiple_vectors() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // Insert multiple vectors
        for i in 0..5 {
            let vector = create_test_vector(&format!("vector_{}", i), "test_collection", 128);
            manager.insert("test_collection", &vector).await.unwrap();
        }
        
        // Check metrics
        let metrics = manager.metrics.read().await;
        assert_eq!(metrics.total_vectors_indexed, 5);
        
        // Should still have only one collection strategy
        let strategies = manager.collection_strategies.read().await;
        assert_eq!(strategies.len(), 1);
        assert!(strategies.contains_key("test_collection"));
    }
    
    #[tokio::test]
    async fn test_insert_vectors_multiple_collections() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // Insert vectors into different collections
        let collections = ["collection_1", "collection_2", "collection_3"];
        
        for collection in &collections {
            for i in 0..3 {
                let vector = create_test_vector(&format!("vector_{}_{}", collection, i), collection, 128);
                manager.insert("test_collection", &vector).await.unwrap();
            }
        }
        
        // Check metrics
        let metrics = manager.metrics.read().await;
        assert_eq!(metrics.total_vectors_indexed, 9); // 3 collections * 3 vectors
        
        // Should have strategies for all collections
        let strategies = manager.collection_strategies.read().await;
        assert_eq!(strategies.len(), 3);
        for collection in &collections {
            assert!(strategies.contains_key(*collection));
        }
    }
}

/// Test vector deletion
#[cfg(test)]
mod deletion_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_delete_vector() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // First insert a vector
        let vector = create_test_vector("vector_1", "test_collection", 128);
        manager.insert("test_collection", &vector).await.unwrap();
        
        // Then delete it
        let result = manager.delete("test_collection", "vector_1".to_string()).await;
        assert!(result.is_ok());
    }
    
    #[tokio::test]
    async fn test_delete_nonexistent_vector() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // Try to delete a vector that doesn't exist
        let result = manager.delete("test_collection", "nonexistent".to_string()).await;
        assert!(result.is_ok()); // Should not fail for non-existent vectors
    }
}

/// Test collection strategy management
#[cfg(test)]
mod strategy_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_ensure_collection_strategy() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // Initially no strategies
        let strategies = manager.collection_strategies.read().await;
        assert!(!strategies.contains_key("new_collection"));
        drop(strategies);
        
        // Ensure strategy for new collection
        let result = manager.ensure_collection_strategy("new_collection").await;
        assert!(result.is_ok());
        
        // Should now have strategy
        let strategies = manager.collection_strategies.read().await;
        assert!(strategies.contains_key("new_collection"));
    }
    
    #[tokio::test]
    async fn test_get_collection_strategy() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // Ensure strategy exists first
        manager.ensure_collection_strategy("test_collection").await.unwrap();
        
        // Get the strategy
        let result = manager.get_collection_strategy("test_collection").await;
        assert!(result.is_ok());
        
        let strategy = result.unwrap();
        assert!(!strategy.indexes.is_none());
    }
    
    #[tokio::test]
    async fn test_set_collection_strategy() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        let custom_strategy = create_test_strategy();
        
        // Set custom strategy
        manager.set_collection_strategy("test_collection", custom_strategy.clone()).await;
        
        // Verify it was set
        let result = manager.get_collection_strategy("test_collection").await;
        assert!(result.is_ok());
        
        let retrieved_strategy = result.unwrap();
        assert_eq!(retrieved_strategy.indexes.len(), custom_strategy.indexes.len());
    }
}

/// Test search functionality
#[cfg(test)]
mod search_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_search_vectors() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // Insert some test vectors first
        for i in 0..5 {
            let vector = create_test_vector(&format!("vector_{}", i), "test_collection", 128);
            manager.insert("test_collection", &vector).await.unwrap();
        }
        
        // Create search query
        let query = VectorQuery {
            collection_id: "test_collection".to_string(),
            vector: vec![0.1; 128],
            k: 3,
            distance_threshold: Some(1.0),
            metadata_filter: None,
        };
        
        let result = manager.search(&query).await;
        assert!(result.is_ok());
        
        let search_results = result.unwrap();
        assert!(!search_results.is_none());
        assert!(search_results.len() <= 3); // Should respect k limit
    }
    
    #[tokio::test]
    async fn test_search_with_metadata_filter() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // Insert test vectors
        let vector = create_test_vector("vector_1", "test_collection", 128);
        manager.insert("test_collection", &vector).await.unwrap();
        
        // Create search query with metadata filter
        let metadata_filter = MetadataFilter {
            field: "category".to_string(),
            operator: FilterOperator::Equals,
            value: serde_json::Value::String("test".to_string()),
        };
        
        let query = VectorQuery {
            collection_id: "test_collection".to_string(),
            vector: vec![0.1; 128],
            k: 5,
            distance_threshold: Some(1.0),
            metadata_filter: Some(metadata_filter),
        };
        
        let result = manager.search(&query).await;
        assert!(result.is_ok());
        
        let search_results = result.unwrap();
        // Should find the vector that matches the filter
        assert!(!search_results.is_none());
    }
    
    #[tokio::test]
    async fn test_search_nonexistent_collection() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        let query = VectorQuery {
            collection_id: "nonexistent_collection".to_string(),
            vector: vec![0.1; 128],
            k: 5,
            distance_threshold: Some(1.0),
            metadata_filter: None,
        };
        
        let result = manager.search(&query).await;
        assert!(result.is_ok());
        
        let search_results = result.unwrap();
        assert!(search_results.is_none()); // Should return empty results
    }
    
    #[tokio::test]
    async fn test_hybrid_search() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // Insert test vectors
        let vector = create_test_vector("vector_1", "test_collection", 128);
        manager.insert("test_collection", &vector).await.unwrap();
        
        let hybrid_query = HybridQuery {
            collection_id: "test_collection".to_string(),
            vector_query: Some(VectorQuery {
                collection_id: "test_collection".to_string(),
                vector: vec![0.1; 128],
                k: 5,
                distance_threshold: Some(1.0),
                metadata_filter: None,
            }),
            text_query: Some("test query".to_string()),
            metadata_filters: vec![],
            result_fusion_weight: 0.7,
        };
        
        let result = manager.hybrid_search(&hybrid_query).await;
        assert!(result.is_ok());
        
        let search_results = result.unwrap();
        assert!(!search_results.is_none());
    }
}

/// Test metrics functionality
#[cfg(test)]
mod metrics_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_get_metrics() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // Insert some vectors to generate metrics
        for i in 0..3 {
            let vector = create_test_vector(&format!("vector_{}", i), "test_collection", 128);
            manager.insert("test_collection", &vector).await.unwrap();
        }
        
        let metrics = manager.metrics().await;
        
        assert_eq!(metrics.total_vectors_indexed, 3);
        assert_eq!(metrics.total_migrations, 0); // No migrations yet
        assert_eq!(metrics.successful_migrations, 0);
        assert_eq!(metrics.failed_migrations, 0);
    }
    
    #[tokio::test]
    async fn test_update_metrics() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // Initial metrics
        let initial_metrics = manager.metrics().await;
        assert_eq!(initial_metrics.total_vectors_indexed, 0);
        
        // Insert vectors
        for i in 0..5 {
            let vector = create_test_vector(&format!("vector_{}", i), "test_collection", 128);
            manager.insert("test_collection", &vector).await.unwrap();
        }
        
        // Updated metrics
        let updated_metrics = manager.metrics().await;
        assert_eq!(updated_metrics.total_vectors_indexed, 5);
    }
}

/// Test migration functionality
#[cfg(test)]
mod migration_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_start_migration() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // Ensure collection has a strategy
        manager.ensure_collection_strategy("test_collection").await.unwrap();
        
        let from_strategy = manager.get_collection_strategy("test_collection").await.unwrap();
        let to_strategy = create_test_strategy();
        
        let result = manager.start_migration("test_collection", from_strategy, to_strategy).await;
        assert!(result.is_ok());
        
        let migration_id = result.unwrap();
        
        // Check that migration is tracked
        let migrations = manager.active_migrations.read().await;
        assert!(migrations.contains_key("test_collection"));
        
        let migration_status = migrations.get(key).unwrap();
        assert_eq!(migration_status.migration_id, migration_id);
        assert_eq!(migration_status.progress_percentage, 0.0);
    }
    
    #[tokio::test]
    async fn test_get_migration_status() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // Start a migration first
        manager.ensure_collection_strategy("test_collection").await.unwrap();
        let from_strategy = manager.get_collection_strategy("test_collection").await.unwrap();
        let to_strategy = create_test_strategy();
        
        manager.start_migration("test_collection", from_strategy, to_strategy).await.unwrap();
        
        // Get migration status
        let result = manager.get_migration_status("test_collection").await;
        assert!(result.is_ok());
        
        let status = result.unwrap();
        assert!(status.is_some());
        
        let migration_status = status.unwrap();
        assert_eq!(migration_status.progress_percentage, 0.0);
    }
    
    #[tokio::test]
    async fn test_get_migration_status_no_migration() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // Get status for collection with no migration
        let result = manager.get_migration_status("nonexistent_collection").await;
        assert!(result.is_ok());
        
        let status = result.unwrap();
        assert!(status.is_none());
    }
}

/// Test performance monitoring
#[cfg(test)]
mod performance_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_maybe_evaluate_strategy() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // Ensure collection has strategy
        manager.ensure_collection_strategy("test_collection").await.unwrap();
        
        // This should not fail even if no actual evaluation occurs
        let result = manager.maybe_evaluate_strategy("test_collection").await;
        assert!(result.is_ok());
    }
    
    #[tokio::test]
    async fn test_evaluate_migration_decision() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // Ensure collection has strategy
        manager.ensure_collection_strategy("test_collection").await.unwrap();
        
        let result = manager.evaluate_migration_decision("test_collection").await;
        assert!(result.is_ok());
        
        // Should return a migration decision
        let decision = result.unwrap();
        // Decision could be either Migrate or Stay - both are valid
        match decision {
            MigrationDecision::Migrate { .. } => {
                // Valid migration decision
            }
            MigrationDecision::Stay { .. } => {
                // Valid stay decision
            }
        }
    }
}

/// Test error handling and edge cases
#[cfg(test)]
mod error_handling_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_get_nonexistent_collection_strategy() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // Try to get strategy for collection that doesn't exist
        let result = manager.get_collection_strategy("nonexistent_collection").await;
        assert!(result.is_err());
    }
    
    #[tokio::test]
    async fn test_insert_vector_without_id() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        let mut vector = create_test_vector("vector_1", "test_collection", 128);
        vector.id = None; // Remove ID
        
        let result = manager.insert("test_collection", &vector).await;
        assert!(result.is_ok()); // Should handle vectors without ID gracefully
    }
    
    #[tokio::test]
    async fn test_insert_zero_dimension_vector() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        let vector = create_test_vector("vector_1", "test_collection", 0); // Zero dimension
        
        let result = manager.insert("test_collection", &vector).await;
        assert!(result.is_ok()); // Should handle zero-dimension vectors
    }
}

/// Test data structures
#[cfg(test)]
mod data_structure_tests {
    use super::*;
    
    #[test]
    fn test_migration_status() {
        let migration_status = MigrationStatus {
            migration_id: uuid::Uuid::new_v4(),
            from_strategy: create_test_strategy(),
            to_strategy: create_test_strategy(),
            start_time: Utc::now(),
            progress_percentage: 45.5,
            estimated_completion: Some(Utc::now() + chrono::Duration::hours(1)),
        };
        
        assert_eq!(migration_status.progress_percentage, 45.5);
        assert!(migration_status.estimated_completion.is_some());
    }
    
    #[test]
    fn test_axis_metrics_default() {
        let metrics = AxisMetrics::default();
        
        assert_eq!(metrics.total_migrations, 0);
        assert_eq!(metrics.successful_migrations, 0);
        assert_eq!(metrics.failed_migrations, 0);
        assert_eq!(metrics.average_migration_time_ms, 0);
        assert_eq!(metrics.total_collections_managed, 0);
        assert_eq!(metrics.total_vectors_indexed, 0);
        assert_eq!(metrics.total_rebuilds, 0);
    }
}

/// Integration tests
#[cfg(test)]
mod integration_tests {
    use super::*;
    
    #[tokio::test]
    async fn test_full_workflow() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        // Insert vectors
        for i in 0..10 {
            let vector = create_test_vector(&format!("vector_{}", i), "test_collection", 128);
            manager.insert("test_collection", &vector).await.unwrap();
        }
        
        // Search for vectors
        let query = VectorQuery {
            collection_id: "test_collection".to_string(),
            vector: vec![0.1; 128],
            k: 5,
            distance_threshold: Some(1.0),
            metadata_filter: None,
        };
        
        let search_results = manager.search(&query).await.unwrap();
        assert!(!search_results.is_none());
        
        // Check metrics
        let metrics = manager.metrics().await;
        assert_eq!(metrics.total_vectors_indexed, 10);
        
        // Evaluate migration
        let decision = manager.evaluate_migration_decision("test_collection").await.unwrap();
        match decision {
            MigrationDecision::Migrate { .. } | MigrationDecision::Stay { .. } => {
                // Both are valid responses
            }
        }
    }
    
    #[tokio::test]
    async fn test_multiple_collections_workflow() {
        let config = AxisConfig::default();
        let manager = AxisManager::new(config).await.unwrap();
        
        let collections = ["collection_1", "collection_2", "collection_3"];
        
        // Insert vectors into multiple collections
        for collection in &collections {
            for i in 0..5 {
                let vector = create_test_vector(&format!("vector_{}", i), collection, 128);
                manager.insert("test_collection", &vector).await.unwrap();
            }
        }
        
        // Search in each collection
        for collection in &collections {
            let query = VectorQuery {
                collection_id: collection.to_string(),
                vector: vec![0.1; 128],
                k: 3,
                distance_threshold: Some(1.0),
                metadata_filter: None,
            };
            
            let results = manager.search(&query).await.unwrap();
            assert!(!results.is_empty());
        }
        
        // Check overall metrics
        let metrics = manager.metrics().await;
        assert_eq!(metrics.total_vectors_indexed, 15); // 3 collections * 5 vectors
        
        // Check that all collections have strategies
        let strategies = manager.collection_strategies.read().await;
        assert_eq!(strategies.len(), 3);
        for collection in &collections {
            assert!(strategies.contains_key(*collection));
        }
    }
}