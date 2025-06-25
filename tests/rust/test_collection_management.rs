//! Collection management integration tests

use super::common::*;
use anyhow::Result;
use proximadb::core::{CollectionRequest, CollectionOperation};

#[cfg(test)]
mod collection_tests {
    use super::*;

    #[tokio::test]
    async fn test_create_collection() -> Result<()> {
        init_test_env();
        
        let collection_name = generate_test_collection_name();
        let config = create_test_collection_config(collection_name.clone(), 384);
        
        // Create collection request
        let request = CollectionRequest::create_collection(config);
        
        // This would normally use a real CollectionService instance
        // For now, we'll test the data structures
        assert_eq!(request.operation, CollectionOperation::Create);
        assert!(request.collection_config.is_some());
        
        println!("✅ Collection creation request structure valid");
        Ok(())
    }

    #[tokio::test]
    async fn test_list_collections() -> Result<()> {
        init_test_env();
        
        let request = CollectionRequest::list_collections();
        
        assert_eq!(request.operation, CollectionOperation::List);
        assert!(request.collection_id.is_none());
        assert!(request.collection_config.is_none());
        
        println!("✅ Collection list request structure valid");
        Ok(())
    }

    #[tokio::test]
    async fn test_get_collection() -> Result<()> {
        init_test_env();
        
        let collection_name = generate_test_collection_name();
        let request = CollectionRequest::get_collection(collection_name.clone());
        
        assert_eq!(request.operation, CollectionOperation::Get);
        assert_eq!(request.collection_id, Some(collection_name));
        
        println!("✅ Collection get request structure valid");
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_collection() -> Result<()> {
        init_test_env();
        
        let collection_name = generate_test_collection_name();
        let request = CollectionRequest::delete_collection(collection_name.clone());
        
        assert_eq!(request.operation, CollectionOperation::Delete);
        assert_eq!(request.collection_id, Some(collection_name));
        
        println!("✅ Collection delete request structure valid");
        Ok(())
    }

    #[tokio::test]
    async fn test_collection_config_serialization() -> Result<()> {
        init_test_env();
        
        let collection_name = generate_test_collection_name();
        let config = create_test_collection_config(collection_name, 768);
        
        // Test JSON serialization
        let json_str = serde_json::to_string(&config)?;
        let deserialized: proximadb::core::CollectionConfig = 
            serde_json::from_str(&json_str)?;
        
        assert_eq!(config.dimension, deserialized.dimension);
        assert_eq!(config.distance_metric, deserialized.distance_metric);
        assert_eq!(config.storage_engine, deserialized.storage_engine);
        
        println!("✅ Collection config serialization works");
        Ok(())
    }

    #[tokio::test]
    async fn test_filterable_metadata_fields() -> Result<()> {
        init_test_env();
        
        let collection_name = generate_test_collection_name();
        let mut config = create_test_collection_config(collection_name, 384);
        
        // Add filterable metadata fields
        config.filterable_metadata_fields = vec![
            "category".to_string(),
            "author".to_string(),
            "doc_type".to_string(),
            "year".to_string(),
            "length".to_string(),
        ];
        
        assert_eq!(config.filterable_metadata_fields.len(), 5);
        assert!(config.filterable_metadata_fields.contains(&"category".to_string()));
        
        println!("✅ Filterable metadata fields configuration works");
        Ok(())
    }
}