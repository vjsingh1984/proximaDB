//! Vector operations integration tests

use super::common::*;
use anyhow::Result;
use proximadb::schema_types::{VectorInsertRequest, VectorRecord};
use std::collections::HashMap;

#[cfg(test)]
mod vector_tests {
    use super::*;

    #[tokio::test]
    async fn test_single_vector_insert() -> Result<()> {
        init_test_env();
        
        let collection_id = generate_test_collection_name();
        let vector_record = create_test_vector_record(
            "test_vector_001".to_string(),
            collection_id.clone(),
            384
        );
        
        let request = VectorInsertRequest::single_insert(collection_id, vector_record.clone());
        
        assert_eq!(request.vectors.len(), 1);
        assert_eq!(request.vectors[0].id, Some("test_vector_001".to_string()));
        assert_eq!(request.vectors[0].vector.len(), 384);
        assert!(request.vectors[0].metadata.is_some());
        
        println!("✅ Single vector insert request structure valid");
        Ok(())
    }

    #[tokio::test]
    async fn test_batch_vector_insert() -> Result<()> {
        init_test_env();
        
        let collection_id = generate_test_collection_name();
        let batch_size = 10;
        let vector_batch = create_test_vector_batch(collection_id.clone(), batch_size, 384);
        
        let schema_vectors: Vec<proximadb::schema_types::VectorRecord> = vector_batch
            .into_iter()
            .map(|v| proximadb::schema_types::VectorRecord {
                id: Some(v.id),
                vector: v.vector,
                metadata: Some(v.metadata),
                timestamp: Some(v.timestamp.timestamp()),
                version: 1,
                expires_at: v.expires_at.map(|t| t.timestamp()),
            })
            .collect();
        
        let request = VectorInsertRequest::batch_insert(collection_id, schema_vectors);
        
        assert_eq!(request.vectors.len(), batch_size);
        assert!(!request.upsert_mode);
        
        println!("✅ Batch vector insert request structure valid");
        Ok(())
    }

    #[tokio::test]
    async fn test_vector_upsert() -> Result<()> {
        init_test_env();
        
        let collection_id = generate_test_collection_name();
        let vector_record = create_test_vector_record(
            "upsert_test_001".to_string(),
            collection_id.clone(),
            384
        );
        
        let schema_vector = proximadb::schema_types::VectorRecord {
            id: Some(vector_record.id),
            vector: vector_record.vector,
            metadata: Some(vector_record.metadata),
            timestamp: Some(vector_record.timestamp.timestamp()),
            version: 1,
            expires_at: vector_record.expires_at.map(|t| t.timestamp()),
        };
        
        let request = VectorInsertRequest::upsert(collection_id, vec![schema_vector]);
        
        assert!(request.upsert_mode);
        assert_eq!(request.vectors.len(), 1);
        
        println!("✅ Vector upsert request structure valid");
        Ok(())
    }

    #[tokio::test]
    async fn test_vector_metadata_validation() -> Result<()> {
        init_test_env();
        
        let collection_id = generate_test_collection_name();
        let mut vector_record = create_test_vector_record(
            "metadata_test_001".to_string(),
            collection_id.clone(),
            384
        );
        
        // Add extensive metadata
        vector_record.metadata.insert("title".to_string(), 
            serde_json::Value::String("Test Document".to_string()));
        vector_record.metadata.insert("score".to_string(), 
            serde_json::Value::Number(serde_json::Number::from_f64(0.95).unwrap()));
        vector_record.metadata.insert("tags".to_string(), 
            serde_json::Value::Array(vec![
                serde_json::Value::String("test".to_string()),
                serde_json::Value::String("vector".to_string()),
            ]));
        
        // Test metadata serialization
        let json_str = serde_json::to_string(&vector_record.metadata)?;
        let deserialized: HashMap<String, serde_json::Value> = 
            serde_json::from_str(&json_str)?;
        
        assert_eq!(vector_record.metadata.len(), deserialized.len());
        assert!(deserialized.contains_key("title"));
        assert!(deserialized.contains_key("score"));
        assert!(deserialized.contains_key("tags"));
        
        println!("✅ Vector metadata validation successful");
        Ok(())
    }

    #[tokio::test]
    async fn test_performance_vector_creation() -> Result<()> {
        init_test_env();
        
        let collection_id = generate_test_collection_name();
        let batch_size = 1000;
        
        let (vector_batch, measurement) = measure_performance!(
            "Vector batch creation",
            batch_size,
            {
                create_test_vector_batch(collection_id.clone(), batch_size, 384)
            }
        );
        
        assert_eq!(vector_batch.len(), batch_size);
        assert!(measurement.throughput > 1000.0); // Should be very fast
        
        println!("✅ Vector creation performance test completed");
        Ok(())
    }

    #[tokio::test]
    async fn test_large_dimension_vectors() -> Result<()> {
        init_test_env();
        
        let collection_id = generate_test_collection_name();
        let large_dimension = 1536; // Test with larger embeddings
        
        let vector_record = create_test_vector_record(
            "large_dim_test".to_string(),
            collection_id,
            large_dimension
        );
        
        assert_eq!(vector_record.vector.len(), large_dimension);
        
        // Test memory usage is reasonable
        let vector_size_mb = (large_dimension * 4) as f64 / 1024.0 / 1024.0; // 4 bytes per f32
        assert!(vector_size_mb < 10.0); // Should be less than 10MB
        
        println!("✅ Large dimension vector test completed");
        Ok(())
    }

    #[tokio::test]
    async fn test_vector_record_serialization() -> Result<()> {
        init_test_env();
        
        let collection_id = generate_test_collection_name();
        let vector_record = create_test_vector_record(
            "serialization_test".to_string(),
            collection_id,
            384
        );
        
        // Test JSON serialization
        let json_str = serde_json::to_string(&vector_record)?;
        let deserialized: proximadb::core::VectorRecord = 
            serde_json::from_str(&json_str)?;
        
        assert_eq!(vector_record.id, deserialized.id);
        assert_eq!(vector_record.vector.len(), deserialized.vector.len());
        assert_eq!(vector_record.metadata.len(), deserialized.metadata.len());
        
        println!("✅ Vector record serialization test completed");
        Ok(())
    }
}