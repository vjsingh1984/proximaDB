//! Basic functionality tests for ProximaDB core components
//!
//! These tests validate that core components can be instantiated and work together
//! without requiring complex setup or external dependencies.

use anyhow::Result;
use std::collections::HashMap;

use proximadb::{
    core::{VectorRecord, SearchResult},
    compute::distance::DistanceMetric,
    storage::strategy::StorageEngineType,
};

#[tokio::test]
async fn test_basic_vector_record_creation() -> Result<()> {
    let vector_record = VectorRecord {
        id: "test_vector_1".to_string(),
        collection_id: "test_collection".to_string(),
        vector: vec![1.0, 2.0, 3.0, 4.0],
        metadata: {
            let mut meta = HashMap::new();
            meta.insert("category".to_string(), serde_json::Value::String("test".to_string()));
            meta
        },
        timestamp: chrono::Utc::now().timestamp_millis(),
        created_at: chrono::Utc::now().timestamp_millis(),
        updated_at: chrono::Utc::now().timestamp_millis(),
        expires_at: None,
        version: 1,
        rank: None,
        score: None,
        distance: None,
    };

    assert_eq!(vector_record.id, "test_vector_1");
    assert_eq!(vector_record.vector.len(), 4);
    assert!(vector_record.metadata.contains_key("category"));

    println!("✅ Basic VectorRecord creation test passed");
    Ok(())
}

#[tokio::test]
async fn test_search_result_creation() -> Result<()> {
    let search_result = SearchResult {
        id: "result_vector_1".to_string(),
        vector_id: Some("result_vector_1".to_string()),
        score: 0.95,
        distance: Some(0.05),
        rank: Some(1),
        vector: Some(vec![1.0, 2.0, 3.0, 4.0]),
        metadata: {
            let mut meta = HashMap::new();
            meta.insert("type".to_string(), serde_json::Value::String("search_result".to_string()));
            meta
        },
        collection_id: Some("test_collection".to_string()),
        created_at: Some(chrono::Utc::now().timestamp_millis()),
        algorithm_used: Some("cosine".to_string()),
        processing_time_us: Some(1000),
    };

    assert_eq!(search_result.id, "result_vector_1");
    assert_eq!(search_result.score, 0.95);
    assert_eq!(search_result.distance, Some(0.05));

    println!("✅ Basic SearchResult creation test passed");
    Ok(())
}

#[tokio::test]
async fn test_distance_metric_enum() -> Result<()> {
    let cosine = DistanceMetric::Cosine;
    let euclidean = DistanceMetric::Euclidean;
    let manhattan = DistanceMetric::Manhattan;

    assert_eq!(cosine, DistanceMetric::Cosine);
    assert_ne!(euclidean, DistanceMetric::Cosine);

    // Test default
    let default_metric = DistanceMetric::default();
    assert_eq!(default_metric, DistanceMetric::Cosine);

    println!("✅ DistanceMetric enum tests passed");
    Ok(())
}

#[tokio::test]
async fn test_storage_engine_type_enum() -> Result<()> {
    let viper = StorageEngineType::Viper;
    let lsm = StorageEngineType::Lsm;
    let mmap = StorageEngineType::Mmap;

    assert_eq!(viper, StorageEngineType::Viper);
    assert_ne!(lsm, StorageEngineType::Viper);

    // Test default
    let default_engine = StorageEngineType::default();
    assert_eq!(default_engine, StorageEngineType::Viper);

    println!("✅ StorageEngineType enum tests passed");
    Ok(())
}

#[tokio::test]
async fn test_vector_operations() -> Result<()> {
    let mut vectors = Vec::new();
    
    // Create a batch of test vectors
    for i in 0..10 {
        let vector = VectorRecord {
            id: format!("vector_{}", i),
            collection_id: "test_collection".to_string(),
            vector: vec![i as f32, (i + 1) as f32, (i + 2) as f32, (i + 3) as f32],
            metadata: {
                let mut meta = HashMap::new();
                meta.insert("index".to_string(), serde_json::Value::Number(serde_json::Number::from(i)));
                meta
            },
            timestamp: chrono::Utc::now().timestamp_millis(),
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            expires_at: None,
            version: 1,
            rank: None,
            score: None,
            distance: None,
        };
        vectors.push(vector);
    }

    assert_eq!(vectors.len(), 10);
    assert_eq!(vectors[0].id, "vector_0");
    assert_eq!(vectors[9].id, "vector_9");

    // Test vector metadata access
    for (i, vector) in vectors.iter().enumerate() {
        let index = vector.metadata.get("index").unwrap().as_u64().unwrap();
        assert_eq!(index, i as u64);
    }

    println!("✅ Vector operations test passed");
    Ok(())
}

#[tokio::test]
async fn test_avro_serialization_available() -> Result<()> {
    // Just test that we can access the AvroSerializationManager type
    use proximadb::core::avro_serialization::AvroSerializationManager;
    
    let manager = AvroSerializationManager::new();
    
    // Basic test - just verify the manager was created
    // We don't need to test actual serialization as that would require more setup
    println!("✅ AvroSerializationManager creation test passed");
    Ok(())
}

#[tokio::test]
async fn test_multi_tier_deduplication_types() -> Result<()> {
    use proximadb::core::search::{StorageTier, TieredSearchResult, DeduplicationStorageEngine};
    use proximadb::core::VectorRecord;
    
    // Test storage tier ordering
    assert!(StorageTier::Unflushed > StorageTier::Flushed);
    assert!(StorageTier::Flushed > StorageTier::Compacted);
    
    // Create a TieredSearchResult
    let vector_record = VectorRecord {
        id: "tiered_test_vector".to_string(),
        collection_id: "test_collection".to_string(),
        vector: vec![1.0, 2.0, 3.0, 4.0],
        metadata: HashMap::new(),
        timestamp: chrono::Utc::now().timestamp_millis(),
        created_at: chrono::Utc::now().timestamp_millis(),
        updated_at: chrono::Utc::now().timestamp_millis(),
        expires_at: None,
        version: 1,
        rank: None,
        score: Some(0.9),
        distance: None,
    };

    let tiered_result = TieredSearchResult {
        vector_record,
        score: 0.9,
        tier: StorageTier::Unflushed,
        engine: DeduplicationStorageEngine::WAL,
        timestamp: chrono::Utc::now(),
        sequence: 1,
        file_path: None,
    };

    assert_eq!(tiered_result.tier, StorageTier::Unflushed);
    assert_eq!(tiered_result.engine, DeduplicationStorageEngine::WAL);
    assert_eq!(tiered_result.score, 0.9);

    println!("✅ Multi-tier deduplication types test passed");
    Ok(())
}