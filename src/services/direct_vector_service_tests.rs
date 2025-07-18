#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb::{VectorRecord, MetadataItem};
    use crate::services::direct_vector_service::DirectVectorService;
    use crate::compute::distance::DistanceMetric;
    use std::collections::HashMap;
    
    /// Helper to create a test vector record
    fn create_test_vector_record(id: &str, vector: Vec<f32>, metadata: Vec<(&str, &str)>) -> VectorRecord {
        VectorRecord {
            id: Some(id.to_string()),
            vector,
            metadata: metadata.into_iter().map(|(k, v)| MetadataItem {
                key: k.to_string(),
                value: v.to_string(),
            }).collect(),
            timestamp: 1000,
            created_at: 1000,
            updated_at: 1000,
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        }
    }
    
    #[tokio::test]
    async fn test_apply_metadata_filter_with_id() {
        // Test that ID filtering works in apply_metadata_filter
        let service = DirectVectorService::new(/* mock dependencies */);
        
        let vector_record = create_test_vector_record(
            "test_vector_1",
            vec![1.0, 2.0, 3.0],
            vec![("category", "test"), ("type", "example")]
        );
        
        // Test exact ID match
        let mut filters = HashMap::new();
        filters.insert("id".to_string(), serde_json::Value::String("test_vector_1".to_string()));
        
        assert!(service.apply_metadata_filter(&vector_record, &filters));
        
        // Test ID mismatch
        filters.insert("id".to_string(), serde_json::Value::String("wrong_id".to_string()));
        assert!(!service.apply_metadata_filter(&vector_record, &filters));
        
        // Test __id variant
        filters.clear();
        filters.insert("__id".to_string(), serde_json::Value::String("test_vector_1".to_string()));
        assert!(service.apply_metadata_filter(&vector_record, &filters));
    }
    
    #[tokio::test]
    async fn test_apply_metadata_filter_with_id_and_metadata() {
        let service = DirectVectorService::new(/* mock dependencies */);
        
        let vector_record = create_test_vector_record(
            "test_vector_1",
            vec![1.0, 2.0, 3.0],
            vec![("category", "test"), ("type", "example")]
        );
        
        // Test ID + metadata match (AND logic)
        let mut filters = HashMap::new();
        filters.insert("id".to_string(), serde_json::Value::String("test_vector_1".to_string()));
        filters.insert("category".to_string(), serde_json::Value::String("test".to_string()));
        
        assert!(service.apply_metadata_filter(&vector_record, &filters));
        
        // Test ID match but metadata mismatch
        filters.insert("category".to_string(), serde_json::Value::String("wrong".to_string()));
        assert!(!service.apply_metadata_filter(&vector_record, &filters));
    }
    
    #[tokio::test]
    async fn test_get_vector_by_id_found() {
        let service = DirectVectorService::new(/* mock dependencies */);
        
        // Mock setup: Insert a vector first
        let test_vector = vec![1.0, 2.0, 3.0, 4.0];
        let metadata = vec![("category", "test")];
        
        // Insert vector
        service.insert_vectors(
            "test_collection",
            vec![create_test_vector_record("test_id_1", test_vector.clone(), metadata)],
        ).await.unwrap();
        
        // Get vector by ID
        let result = service.get_vector_by_id(
            "test_collection",
            "test_id_1",
            true,  // include_vector
            true,  // include_metadata
        ).await.unwrap();
        
        assert!(result.is_some());
        let search_result = result.unwrap();
        assert_eq!(search_result.id, "test_id_1");
        assert_eq!(search_result.vector, Some(test_vector));
        assert!(search_result.metadata.contains_key("category"));
    }
    
    #[tokio::test]
    async fn test_get_vector_by_id_not_found() {
        let service = DirectVectorService::new(/* mock dependencies */);
        
        // Get non-existent vector by ID
        let result = service.get_vector_by_id(
            "test_collection",
            "non_existent_id",
            true,
            true,
        ).await.unwrap();
        
        assert!(result.is_none());
    }
    
    #[tokio::test]
    async fn test_get_vector_by_id_without_vector_data() {
        let service = DirectVectorService::new(/* mock dependencies */);
        
        // Insert a vector first
        service.insert_vectors(
            "test_collection",
            vec![create_test_vector_record("test_id_2", vec![1.0, 2.0], vec![])],
        ).await.unwrap();
        
        // Get vector by ID without vector data
        let result = service.get_vector_by_id(
            "test_collection",
            "test_id_2",
            false, // include_vector = false
            true,  // include_metadata
        ).await.unwrap();
        
        assert!(result.is_some());
        let search_result = result.unwrap();
        assert_eq!(search_result.id, "test_id_2");
        assert!(search_result.vector.is_none());
    }
    
    #[tokio::test]
    async fn test_get_vector_by_id_without_metadata() {
        let service = DirectVectorService::new(/* mock dependencies */);
        
        // Insert a vector with metadata
        service.insert_vectors(
            "test_collection",
            vec![create_test_vector_record("test_id_3", vec![1.0], vec![("key", "value")])],
        ).await.unwrap();
        
        // Get vector by ID without metadata
        let result = service.get_vector_by_id(
            "test_collection",
            "test_id_3",
            true,  // include_vector
            false, // include_metadata = false
        ).await.unwrap();
        
        assert!(result.is_some());
        let search_result = result.unwrap();
        assert_eq!(search_result.id, "test_id_3");
        assert!(search_result.metadata.is_empty());
    }
}