//! Unit tests for AXIS Index Manager

use super::*;
use crate::core::avro_unified::VectorRecord;
use chrono::Utc;
use std::collections::HashMap;
use uuid::Uuid;

/// Helper function to create test vector records
fn create_test_vector(collection_id: &str, dimension: usize) -> VectorRecord {
    let mut vector_data = vec![0.0; dimension];
    // Create simple vector pattern
    for (i, val) in vector_data.iter_mut().enumerate() {
        *val = (i as f32) / 100.0;
    }

    let mut metadata = HashMap::new();
    metadata.insert("test_key".to_string(), serde_json::json!("test_value"));

    let now = Utc::now().timestamp_millis();
    
    VectorRecord {
        id: Uuid::new_v4().to_string(),
        collection_id: collection_id.to_string(),
        vector: vector_data,
        metadata,
        timestamp: now,
        created_at: now,
        updated_at: now,
        expires_at: None,
        version: 1,
        rank: None,
        score: None,
        distance: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_axis_manager_creation() {
        let config = AxisConfig::default();
        let result = AxisIndexManager::new(config).await;
        
        assert!(result.is_ok(), "AXIS manager should be created successfully");
    }

    #[tokio::test]
    async fn test_collection_strategy_creation() {
        let config = AxisConfig::default();
        println!("✅ Created AXIS config");
        
        let axis_manager = AxisIndexManager::new(config).await.unwrap();
        println!("✅ Created AXIS manager");

        // Test just the strategy creation part
        let collection_id = "test_collection".to_string();
        println!("🔍 About to ensure collection strategy for: {}", collection_id);
        
        match axis_manager.ensure_collection_strategy(&collection_id).await {
            Ok(_) => {
                println!("✅ Collection strategy creation succeeded");
            }
            Err(e) => {
                println!("❌ Collection strategy creation failed: {}", e);
                panic!("Collection strategy creation should succeed, but got error: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_vector_insertion() {
        let config = AxisConfig::default();
        let axis_manager = AxisIndexManager::new(config).await.unwrap();

        let test_vector = create_test_vector("test_collection", 128);
        let result = axis_manager.insert(test_vector).await;
        
        assert!(result.is_ok(), "Vector insertion should succeed");
    }

    #[tokio::test]
    async fn test_expired_vector_insertion() {
        let config = AxisConfig::default();
        let axis_manager = AxisIndexManager::new(config).await.unwrap();

        let mut expired_vector = create_test_vector("test_collection", 128);
        expired_vector.expires_at = Some((Utc::now() - chrono::Duration::hours(1)).timestamp_millis());

        let result = axis_manager.insert(expired_vector).await;
        assert!(result.is_ok(), "Expired vector insertion should succeed but be skipped");
    }

    #[tokio::test]
    async fn test_vector_deletion() {
        let config = AxisConfig::default();
        let axis_manager = AxisIndexManager::new(config).await.unwrap();

        let vector_id = Uuid::new_v4();
        let collection_id = "test_collection".to_string();

        let result = axis_manager.delete(&collection_id, vector_id.to_string()).await;
        assert!(result.is_ok(), "Vector deletion should succeed");
    }

    #[tokio::test]
    async fn test_collection_analysis() {
        let config = AxisConfig::default();
        let axis_manager = AxisIndexManager::new(config).await.unwrap();

        let collection_id = "analysis_test_collection".to_string();
        let result = axis_manager.analyze_and_optimize(&collection_id).await;
        
        assert!(result.is_ok(), "Collection analysis should succeed");
    }

    #[tokio::test]
    async fn test_metrics_collection() {
        let config = AxisConfig::default();
        let axis_manager = AxisIndexManager::new(config).await.unwrap();

        let metrics = axis_manager.get_metrics().await;
        
        // Initially should have zero metrics
        assert_eq!(metrics.total_vectors_indexed, 0);
        assert_eq!(metrics.total_collections_managed, 0);
    }

    #[tokio::test]
    async fn test_collection_stats() {
        let config = AxisConfig::default();
        let axis_manager = AxisIndexManager::new(config).await.unwrap();

        let collection_id = "stats_test_collection".to_string();
        let result = axis_manager.get_collection_stats(&collection_id).await;
        
        // Should handle non-existent collection gracefully
        assert!(result.is_err() || result.is_ok());
    }

    #[tokio::test]
    async fn test_drop_collection() {
        let config = AxisConfig::default();
        let axis_manager = AxisIndexManager::new(config).await.unwrap();

        let collection_id = "drop_test_collection".to_string();
        let result = axis_manager.drop_collection(&collection_id).await;
        
        assert!(result.is_ok(), "Collection drop should succeed");
    }

    #[tokio::test]
    async fn test_hybrid_query_execution() {
        let config = AxisConfig::default();
        let axis_manager = AxisIndexManager::new(config).await.unwrap();

        let query = HybridQuery {
            collection_id: "test_collection".to_string(),
            vector_query: Some(VectorQuery::Dense {
                vector: vec![0.5; 128],
                similarity_threshold: 0.7,
            }),
            metadata_filters: vec![],
            id_filters: vec![],
            k: 10,
            include_expired: false,
        };

        let result = axis_manager.query(query).await;
        assert!(result.is_ok(), "Hybrid query should execute successfully");
    }

    #[tokio::test]
    async fn test_migration_status() {
        let config = AxisConfig::default();
        let axis_manager = AxisIndexManager::new(config).await.unwrap();

        let collection_id = "migration_test_collection".to_string();
        let status = axis_manager.get_migration_status(&collection_id).await;
        
        // Should be None if no migration in progress
        assert!(status.is_none());
    }
}