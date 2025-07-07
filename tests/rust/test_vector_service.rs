//! Comprehensive Unit Tests for VectorService
//! 
//! This test suite covers the unified VectorService architecture including:
//! - Upsert-only semantics with zero-copy Avro payloads
//! - Multi-tier deduplication across WAL, flushed, and compacted storage
//! - SearchEngineFactory with storage-aware polymorphic routing
//! - Batch coordination between WAL and storage engines
//! - Performance validation of unified operations

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use proximadb::{
    core::{
        avro_serialization::AvroSerializationManager,
        CollectionId, SearchResult, VectorRecord, FieldCondition, MetadataFilter,
    },
    services::{
        vector_service::{VectorService, UnifiedServiceConfig, OperationMode},
        collection_service::CollectionService,
    },
    storage::{
        engines::viper::core::ViperCoreEngine,
        engines::lsm::LsmTree,
        persistence::wal::{WalManager, WalStrategyType, config::WalConfig},
        StorageEngine, FilesystemFactory,
    },
    compute::distance::DistanceMetric,
};

/// Test fixture for VectorService testing
struct VectorServiceTestFixture {
    service: VectorService,
    collection_id: String,
    test_vectors: Vec<VectorRecord>,
    avro_manager: AvroSerializationManager,
}

impl VectorServiceTestFixture {
    /// Create a new test fixture with mock dependencies
    async fn new() -> Result<Self> {
        let collection_id = format!("test_collection_{}", Uuid::new_v4());
        
        // Create mock storage components
        let storage = Arc::new(RwLock::new(StorageEngine::VIPER));
        let wal = Arc::new(Self::create_mock_wal_manager().await?);
        let collection_service = Arc::new(Self::create_mock_collection_service().await?);
        
        // Create test configuration
        let config = UnifiedServiceConfig {
            wal_strategy: WalStrategyType::Avro,
            memtable_type: "SkipList".to_string(),
            avro_schema_version: 1,
            max_batch_size: 1000,
            enable_zero_copy: true,
            enable_polymorphic_search: true,
        };
        
        // Create VectorService
        let service = VectorService::new(storage, wal, collection_service, config).await?;
        
        // Create test vectors
        let test_vectors = Self::create_test_vectors(&collection_id, 10);
        
        // Create Avro serialization manager
        let avro_manager = AvroSerializationManager::new();
        
        Ok(Self {
            service,
            collection_id,
            test_vectors,
            avro_manager,
        })
    }
    
    /// Create mock WAL manager
    async fn create_mock_wal_manager() -> Result<WalManager> {
        let wal_config = WalConfig {
            strategy: WalStrategyType::Avro,
            base_url: "file:///tmp/test_wal".to_string(),
            max_segment_size: 1024 * 1024, // 1MB
            sync_mode: crate::storage::persistence::wal::config::SyncMode::PerBatch,
            compression_enabled: true,
            ..Default::default()
        };
        
        let filesystem = Arc::new(FilesystemFactory::new_local_filesystem().await?);
        WalManager::new(wal_config, filesystem).await
    }
    
    /// Create mock collection service
    async fn create_mock_collection_service() -> Result<CollectionService> {
        CollectionService::new_with_test_config().await
    }
    
    /// Create test vectors for the given collection
    fn create_test_vectors(collection_id: &str, count: usize) -> Vec<VectorRecord> {
        (0..count)
            .map(|i| VectorRecord {
                id: format!("vector_{}", i),
                collection_id: collection_id.to_string(),
                vector: vec![i as f32, (i + 1) as f32, (i + 2) as f32, (i + 3) as f32],
                metadata: {
                    let mut meta = HashMap::new();
                    meta.insert("index".to_string(), serde_json::Value::Number(serde_json::Number::from(i)));
                    meta.insert("category".to_string(), serde_json::Value::String(format!("category_{}", i % 3)));
                    meta
                },
                timestamp_utc: chrono::Utc::now(),
                ..Default::default()
            })
            .collect()
    }
    
    /// Serialize vectors to Avro payload
    async fn serialize_vectors_to_avro(&self, vectors: &[VectorRecord]) -> Result<Vec<u8>> {
        self.avro_manager.serialize_batch(vectors).await
    }
    
    /// Create a search query payload
    async fn create_search_payload(&self, query_vector: Vec<f32>, k: usize) -> Result<Vec<u8>> {
        let search_request = serde_json::json!({
            "collection_id": self.collection_id,
            "query_vector": query_vector,
            "k": k,
            "distance_metric": "Cosine",
            "metadata_filter": null
        });
        
        Ok(serde_json::to_vec(&search_request)?)
    }
}

#[tokio::test]
async fn test_vector_service_initialization() -> Result<()> {
    let fixture = VectorServiceTestFixture::new().await?;
    
    // Verify service is properly initialized
    assert!(!fixture.collection_id.is_empty());
    assert!(!fixture.test_vectors.is_empty());
    
    println!("✅ VectorService initialized successfully with {} test vectors", fixture.test_vectors.len());
    Ok(())
}

#[tokio::test]
async fn test_upsert_operation_with_avro_payload() -> Result<()> {
    let fixture = VectorServiceTestFixture::new().await?;
    
    // Serialize test vectors to Avro
    let avro_payload = fixture.serialize_vectors_to_avro(&fixture.test_vectors[0..3]).await?;
    
    // Test upsert operation
    let response = fixture.service.handle_vector_insert(
        &fixture.collection_id,
        avro_payload,
        false, // no immediate flush
    ).await?;
    
    // Verify response
    assert!(response.len() > 0, "Response should not be empty");
    
    // Deserialize response to check results
    let response_json: serde_json::Value = serde_json::from_slice(&response)?;
    assert_eq!(response_json["success"], true);
    assert_eq!(response_json["vectors_processed"], 3);
    
    println!("✅ Upsert operation completed successfully: {} vectors", response_json["vectors_processed"]);
    Ok(())
}

#[tokio::test]
async fn test_search_vectors_polymorphic() -> Result<()> {
    let fixture = VectorServiceTestFixture::new().await?;
    
    // First, insert some test vectors
    let avro_payload = fixture.serialize_vectors_to_avro(&fixture.test_vectors).await?;
    fixture.service.handle_vector_insert(
        &fixture.collection_id,
        avro_payload,
        false,
    ).await?;
    
    // Create search payload
    let query_vector = vec![1.0, 2.0, 3.0, 4.0];
    let search_payload = fixture.create_search_payload(query_vector, 5).await?;
    
    // Perform search
    let search_response = fixture.service.search_vectors_polymorphic(&search_payload).await?;
    
    // Verify search results
    assert!(search_response.len() > 0, "Search response should not be empty");
    
    let response_json: serde_json::Value = serde_json::from_slice(&search_response)?;
    assert!(response_json["results"].is_array());
    
    let results = response_json["results"].as_array().unwrap();
    assert!(results.len() <= 5, "Should return at most 5 results");
    assert!(results.len() > 0, "Should return at least 1 result");
    
    // Verify result structure
    if let Some(first_result) = results.first() {
        assert!(first_result["id"].is_string());
        assert!(first_result["score"].is_number());
        assert!(first_result["metadata"].is_object());
    }
    
    println!("✅ Polymorphic search completed: {} results found", results.len());
    Ok(())
}

#[tokio::test]
async fn test_multi_tier_deduplication() -> Result<()> {
    let fixture = VectorServiceTestFixture::new().await?;
    
    // Test scenario: Insert same vector multiple times to test deduplication
    let duplicate_vector = VectorRecord {
        id: "duplicate_vector".to_string(),
        collection_id: fixture.collection_id.clone(),
        vector: vec![1.0, 1.0, 1.0, 1.0],
        metadata: HashMap::new(),
        timestamp_utc: chrono::Utc::now(),
        ..Default::default()
    };
    
    // Insert the same vector 3 times (should be deduplicated)
    for i in 0..3 {
        let payload = fixture.serialize_vectors_to_avro(&[duplicate_vector.clone()]).await?;
        let response = fixture.service.handle_vector_insert(
            &fixture.collection_id,
            payload,
            false,
        ).await?;
        
        println!("Upsert iteration {}: {:?}", i + 1, String::from_utf8_lossy(&response));
    }
    
    // Search to verify only one instance exists
    let search_payload = fixture.create_search_payload(vec![1.0, 1.0, 1.0, 1.0], 10).await?;
    let search_response = fixture.service.search_vectors_polymorphic(&search_payload).await?;
    
    let response_json: serde_json::Value = serde_json::from_slice(&search_response)?;
    let results = response_json["results"].as_array().unwrap();
    
    // Count how many results match our duplicate vector ID
    let duplicate_count = results.iter()
        .filter(|r| r["id"].as_str() == Some("duplicate_vector"))
        .count();
    
    assert_eq!(duplicate_count, 1, "Should have exactly 1 instance after deduplication, found {}", duplicate_count);
    
    println!("✅ Multi-tier deduplication verified: {} unique instances", duplicate_count);
    Ok(())
}

#[tokio::test]
async fn test_metadata_filtering() -> Result<()> {
    let fixture = VectorServiceTestFixture::new().await?;
    
    // Insert test vectors with metadata
    let avro_payload = fixture.serialize_vectors_to_avro(&fixture.test_vectors).await?;
    fixture.service.handle_vector_insert(
        &fixture.collection_id,
        avro_payload,
        false,
    ).await?;
    
    // Create search with metadata filter
    let search_request = serde_json::json!({
        "collection_id": fixture.collection_id,
        "query_vector": [2.0, 3.0, 4.0, 5.0],
        "k": 10,
        "distance_metric": "Cosine",
        "metadata_filter": {
            "conditions": [
                {
                    "field": "category",
                    "operator": "equals",
                    "value": "category_1"
                }
            ],
            "logic": "AND"
        }
    });
    
    let search_payload = serde_json::to_vec(&search_request)?;
    let search_response = fixture.service.search_vectors_polymorphic(&search_payload).await?;
    
    let response_json: serde_json::Value = serde_json::from_slice(&search_response)?;
    let results = response_json["results"].as_array().unwrap();
    
    // Verify all results match the filter
    for result in results {
        let metadata = &result["metadata"];
        assert_eq!(metadata["category"], "category_1", "All results should match category filter");
    }
    
    println!("✅ Metadata filtering verified: {} filtered results", results.len());
    Ok(())
}

#[tokio::test]
async fn test_batch_coordination() -> Result<()> {
    let fixture = VectorServiceTestFixture::new().await?;
    
    // Test large batch to verify coordination between WAL and storage
    let large_batch: Vec<VectorRecord> = (0..100)
        .map(|i| VectorRecord {
            id: format!("batch_vector_{}", i),
            collection_id: fixture.collection_id.clone(),
            vector: vec![i as f32, (i * 2) as f32, (i * 3) as f32, (i * 4) as f32],
            metadata: HashMap::new(),
            timestamp_utc: chrono::Utc::now(),
            ..Default::default()
        })
        .collect();
    
    let avro_payload = fixture.serialize_vectors_to_avro(&large_batch).await?;
    
    // Insert with immediate flush to test WAL-storage coordination
    let response = fixture.service.handle_vector_insert(
        &fixture.collection_id,
        avro_payload,
        true, // immediate flush
    ).await?;
    
    let response_json: serde_json::Value = serde_json::from_slice(&response)?;
    assert_eq!(response_json["vectors_processed"], 100);
    assert_eq!(response_json["success"], true);
    
    // Verify vectors are searchable (indicating successful coordination)
    let search_payload = fixture.create_search_payload(vec![50.0, 100.0, 150.0, 200.0], 10).await?;
    let search_response = fixture.service.search_vectors_polymorphic(&search_payload).await?;
    
    let search_json: serde_json::Value = serde_json::from_slice(&search_response)?;
    let results = search_json["results"].as_array().unwrap();
    
    assert!(results.len() > 0, "Should find results after coordinated flush");
    
    println!("✅ Batch coordination verified: {} vectors processed, {} searchable", 100, results.len());
    Ok(())
}

#[tokio::test]
async fn test_zero_copy_operations() -> Result<()> {
    let fixture = VectorServiceTestFixture::new().await?;
    
    // Test zero-copy Avro operations
    let vectors = &fixture.test_vectors[0..5];
    let avro_payload = fixture.serialize_vectors_to_avro(vectors).await?;
    
    let start_time = std::time::Instant::now();
    
    // Perform zero-copy upsert
    let response = fixture.service.handle_vector_insert(
        &fixture.collection_id,
        avro_payload,
        false,
    ).await?;
    
    let duration = start_time.elapsed();
    
    // Verify response
    let response_json: serde_json::Value = serde_json::from_slice(&response)?;
    assert_eq!(response_json["success"], true);
    assert_eq!(response_json["vectors_processed"], 5);
    
    // Performance assertion (should be fast due to zero-copy)
    assert!(duration.as_millis() < 100, "Zero-copy operation should complete quickly, took {}ms", duration.as_millis());
    
    println!("✅ Zero-copy operation verified: {}ms for {} vectors", duration.as_millis(), 5);
    Ok(())
}

#[tokio::test]
async fn test_error_handling() -> Result<()> {
    let fixture = VectorServiceTestFixture::new().await?;
    
    // Test invalid Avro payload
    let invalid_payload = b"invalid_avro_data";
    let result = fixture.service.handle_vector_insert(
        &fixture.collection_id,
        invalid_payload.to_vec(),
        false,
    ).await;
    
    assert!(result.is_err(), "Should return error for invalid Avro payload");
    
    // Test empty collection ID
    let avro_payload = fixture.serialize_vectors_to_avro(&fixture.test_vectors[0..1]).await?;
    let result = fixture.service.handle_vector_insert(
        "",
        avro_payload,
        false,
    ).await;
    
    assert!(result.is_err(), "Should return error for empty collection ID");
    
    // Test invalid search payload
    let invalid_search = b"invalid_search_data";
    let result = fixture.service.search_vectors_polymorphic(invalid_search).await;
    
    assert!(result.is_err(), "Should return error for invalid search payload");
    
    println!("✅ Error handling verified: All invalid inputs properly rejected");
    Ok(())
}

#[tokio::test]
async fn test_concurrent_operations() -> Result<()> {
    let fixture = VectorServiceTestFixture::new().await?;
    
    // Test concurrent inserts
    let mut handles = vec![];
    
    for i in 0..5 {
        let service = fixture.service.clone();
        let collection_id = fixture.collection_id.clone();
        let vectors = vec![VectorRecord {
            id: format!("concurrent_vector_{}", i),
            collection_id: collection_id.clone(),
            vector: vec![i as f32; 4],
            metadata: HashMap::new(),
            timestamp_utc: chrono::Utc::now(),
            ..Default::default()
        }];
        
        let avro_payload = fixture.serialize_vectors_to_avro(&vectors).await?;
        
        let handle = tokio::spawn(async move {
            service.handle_vector_insert(&collection_id, avro_payload, false).await
        });
        
        handles.push(handle);
    }
    
    // Wait for all concurrent operations
    let mut success_count = 0;
    for handle in handles {
        let result = handle.await.unwrap();
        if result.is_ok() {
            success_count += 1;
        }
    }
    
    assert_eq!(success_count, 5, "All concurrent operations should succeed");
    
    // Verify all vectors are searchable
    let search_payload = fixture.create_search_payload(vec![2.0, 2.0, 2.0, 2.0], 10).await?;
    let search_response = fixture.service.search_vectors_polymorphic(&search_payload).await?;
    
    let response_json: serde_json::Value = serde_json::from_slice(&search_response)?;
    let results = response_json["results"].as_array().unwrap();
    
    assert!(results.len() >= 5, "Should find all concurrently inserted vectors");
    
    println!("✅ Concurrent operations verified: {} successful inserts, {} searchable results", success_count, results.len());
    Ok(())
}

/// Integration-style test for complete workflow
#[tokio::test]
async fn test_complete_vector_workflow() -> Result<()> {
    let fixture = VectorServiceTestFixture::new().await?;
    
    println!("🚀 Testing complete vector workflow...");
    
    // Step 1: Insert initial batch
    let initial_batch = &fixture.test_vectors[0..5];
    let avro_payload = fixture.serialize_vectors_to_avro(initial_batch).await?;
    let insert_response = fixture.service.handle_vector_insert(
        &fixture.collection_id,
        avro_payload,
        false,
    ).await?;
    
    let insert_json: serde_json::Value = serde_json::from_slice(&insert_response)?;
    assert_eq!(insert_json["vectors_processed"], 5);
    println!("   ✅ Initial insert: {} vectors", insert_json["vectors_processed"]);
    
    // Step 2: Search inserted vectors
    let search_payload = fixture.create_search_payload(vec![2.0, 3.0, 4.0, 5.0], 10).await?;
    let search_response = fixture.service.search_vectors_polymorphic(&search_payload).await?;
    
    let search_json: serde_json::Value = serde_json::from_slice(&search_response)?;
    let initial_results = search_json["results"].as_array().unwrap().len();
    println!("   ✅ Initial search: {} results", initial_results);
    
    // Step 3: Upsert with modifications (should replace existing)
    let mut modified_batch = initial_batch.to_vec();
    for vector in &mut modified_batch {
        vector.metadata.insert("modified".to_string(), serde_json::Value::Bool(true));
    }
    
    let modified_payload = fixture.serialize_vectors_to_avro(&modified_batch).await?;
    let upsert_response = fixture.service.handle_vector_insert(
        &fixture.collection_id,
        modified_payload,
        true, // flush immediately
    ).await?;
    
    let upsert_json: serde_json::Value = serde_json::from_slice(&upsert_response)?;
    assert_eq!(upsert_json["vectors_processed"], 5);
    println!("   ✅ Upsert with flush: {} vectors", upsert_json["vectors_processed"]);
    
    // Step 4: Verify upserted data
    let final_search_response = fixture.service.search_vectors_polymorphic(&search_payload).await?;
    let final_search_json: serde_json::Value = serde_json::from_slice(&final_search_response)?;
    let final_results = final_search_json["results"].as_array().unwrap();
    
    // Check that results contain the modified metadata
    let modified_count = final_results.iter()
        .filter(|r| r["metadata"]["modified"] == true)
        .count();
    
    assert!(modified_count > 0, "Should find vectors with modified metadata");
    println!("   ✅ Final verification: {} vectors with modified metadata", modified_count);
    
    println!("✅ Complete workflow verified successfully!");
    Ok(())
}