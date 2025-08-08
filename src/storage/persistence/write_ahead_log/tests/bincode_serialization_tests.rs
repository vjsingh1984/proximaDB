//! Tests for Bincode WAL serialization strategy
//!
//! These tests ensure the Bincode serialization strategy correctly handles:
//! - Writing and reading batches with binary format
//! - High-performance serialization/deserialization
//! - Similarity search operations
//! - Memory management and flush operations

use std::sync::Arc;
use anyhow::Result;
use crate::core::VectorRecord;
use crate::proto::proximadb::MetadataItem;
use crate::storage::persistence::write_ahead_log::{
    BincodeSerializationStrategy, WALBatchStrategy, WALConfig, BatchId,
};
use crate::storage::persistence::filesystem::FilesystemFactory;
use crate::storage::memtable::specialized::wal_behavior::WALVectorBatch;
use crate::compute::distance_computation::DistanceMetric;

/// Create test configuration
fn create_test_config() -> WALConfig {
    WALConfig {
        memtable: crate::storage::persistence::write_ahead_log::config::MemTableConfig {
            memtable_type: crate::storage::persistence::write_ahead_log::config::MemTableType::default(),
            global_memory_limit: 10 * 1024 * 1024, // 10MB
            mvcc_versions_retained: 5,
            enable_concurrency: true,
        },
        multi_disk: crate::storage::persistence::write_ahead_log::config::MultiDiskConfig {
            data_directories: vec!["/tmp/proximadb-bincode-test".to_string()],
            ..Default::default()
        },
        performance: crate::storage::persistence::write_ahead_log::config::PerformanceConfig {
            memory_flush_size_bytes: 5 * 1024 * 1024, // 5MB
            sync_mode: crate::storage::persistence::write_ahead_log::config::SyncMode::Always,
            ..Default::default()
        },
        enable_mvcc: true,
        ..Default::default()
    }
}

/// Create test vector with specific patterns
fn create_test_vector(id: &str, dimension: usize, value: f32) -> VectorRecord {
    VectorRecord {
        id: Some(id.to_string()),
        vector: vec![value; dimension],
        metadata: vec![
            MetadataItem {
                key: "type".to_string(),
                value: Some(crate::proto::proximadb::metadata_item::Value::StringValue("bincode_test".to_string())),
            },
            MetadataItem {
                key: "value".to_string(), 
                value: Some(crate::proto::proximadb::metadata_item::Value::StringValue(value.to_string())),
            },
        ],
        timestamp: 1234567890,
        updated_at: Some(1234567890),
        expires_at: None,
        version: Some(1),
        rank: None,
        score: None,
        distance: None,
    }
}

/// Create test batch with size tracking
fn create_test_batch(vectors: Vec<VectorRecord>) -> WALVectorBatch {
    let vector_count = vectors.len();
    WALVectorBatch {
        batch_id: BatchId::new(),
        vector_records: Arc::new(vectors),
        created_at: std::time::SystemTime::now(),
        total_size_bytes: vector_count * 256, // Approximate
        is_flushed: false,
            metadata_bloom_filter: None,
    }
}

/// Create WriteBuffer directory for collection
async fn create_collection_write_buffer_dir(collection_id: &str) {
    let write_buffer_dir = std::path::Path::new("/tmp/proximadb-bincode-test")
        .join(collection_id)
        .join("write_buffer");
    tokio::fs::create_dir_all(&write_buffer_dir)
        .await
        .expect("Failed to create WriteBuffer directory");
}

#[tokio::test]
async fn test_bincode_strategy_initialization() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    
    let strategy = BincodeSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create Bincode strategy");
    
    assert_eq!(strategy.strategy_name(), "BincodeBatch");
}

#[tokio::test]
async fn test_bincode_binary_serialization() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let strategy = BincodeSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create strategy");
    
    let collection_id = "binary_test";
    
    create_collection_write_buffer_dir(collection_id).await;
    // Test with various vector sizes to verify binary efficiency
    let test_sizes = vec![64, 128, 256, 512, 1024];
    
    for size in test_sizes {
        let vectors = vec![
            create_test_vector(&format!("bin_vec_{}", size), size, 0.1),
        ];
        let batch = create_test_batch(vectors);
        
        let sequences = strategy.write_native_batch(batch, collection_id)
            .await
            .expect("Failed to write batch");
        
        assert_eq!(sequences.len(), 1);
        
        // Verify the vector was stored correctly
        let retrieved = strategy.get_collection_vectors(collection_id)
            .await
            .expect("Failed to get vectors");
        
        let found = retrieved.iter()
            .find(|v| v.id.as_ref().unwrap() == &format!("bin_vec_{}", size));
        assert!(found.is_some());
        assert_eq!(found.unwrap().vector.len(), size);
    }
}

#[tokio::test]
async fn test_bincode_large_batch_performance() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let strategy = BincodeSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create strategy");
    
    let collection_id = "perf_test";
    create_collection_write_buffer_dir(collection_id).await;
    
    // Create a large batch to test performance
    let mut vectors = Vec::new();
    for i in 0..1000 {
        vectors.push(create_test_vector(&format!("perf_{}", i), 128, i as f32 * 0.001));
    }
    
    let batch = create_test_batch(vectors);
    let start = std::time::Instant::now();
    
    let sequences = strategy.write_native_batch(batch, collection_id)
        .await
        .expect("Failed to write large batch");
    
    let write_duration = start.elapsed();
    
    assert_eq!(sequences.len(), 1000);
    println!("Bincode wrote 1000 vectors in {:?}", write_duration);
    
    // Verify retrieval performance
    let start = std::time::Instant::now();
    let retrieved = strategy.get_collection_vectors(collection_id)
        .await
        .expect("Failed to get vectors");
    
    let read_duration = start.elapsed();
    
    assert_eq!(retrieved.len(), 1000);
    println!("Bincode read 1000 vectors in {:?}", read_duration);
}

#[tokio::test]
async fn test_bincode_similarity_search_accuracy() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let strategy = BincodeSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create strategy");
    
    let collection_id = "similarity_accuracy_test";
    create_collection_write_buffer_dir(collection_id).await;
    
    // Create vectors with known distances
    // For cosine similarity, we need vectors with different directions, not just magnitudes
    let mut exact_vec = create_test_vector("exact_match", 128, 1.0);
    
    let mut close_vec = create_test_vector("close_match", 128, 0.95);
    // Perturb slightly to create different direction
    for i in 0..10 {
        close_vec.vector[i] = 0.85;
    }
    
    let mut medium_vec = create_test_vector("medium_match", 128, 0.5);
    // More perturbation
    for i in 0..64 {
        medium_vec.vector[i] = 0.2;
    }
    
    let mut far_vec = create_test_vector("far_match", 128, 0.1);
    // Even more different
    for i in 0..96 {
        far_vec.vector[i] = -0.3;
    }
    
    let opposite_vec = create_test_vector("opposite", 128, -1.0);
    
    let vectors = vec![exact_vec, close_vec, medium_vec, far_vec, opposite_vec];
    
    let batch = create_test_batch(vectors);
    strategy.write_native_batch(batch, collection_id)
        .await
        .expect("Failed to write batch");
    
    // Search with exact match query
    let query = vec![1.0; 128];
    let results = strategy.search_vectors_similarity(
        collection_id,
        &query,
        5,
        Some(DistanceMetric::Cosine),
    )
    .await
    .expect("Failed to search");
    
    assert_eq!(results.len(), 5);
    // First result should be exact match
    assert_eq!(results[0].0, "exact_match");
    assert!(results[0].1 < 0.001); // Very small distance
    
    // Second should be close match
    assert_eq!(results[1].0, "close_match");
}

#[tokio::test]
async fn test_bincode_memory_management() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let strategy = BincodeSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create strategy");
    
    let collection_id = "memory_test";
    create_collection_write_buffer_dir(collection_id).await;
    
    // Get initial memory stats
    let initial_stats = strategy.get_stats()
        .await
        .expect("Failed to get stats");
    
    let initial_memory = initial_stats.memory_size_bytes;
    
    // Add vectors and track memory growth
    for batch_num in 0..5 {
        let mut vectors = Vec::new();
        for i in 0..100 {
            vectors.push(create_test_vector(
                &format!("mem_{}_{}", batch_num, i), 
                256, 
                0.1
            ));
        }
        
        let batch = create_test_batch(vectors);
        strategy.write_native_batch(batch, collection_id)
            .await
            .expect("Failed to write batch");
        
        let stats = strategy.get_stats()
            .await
            .expect("Failed to get stats");
        
        // Memory should increase with each batch
        assert!(stats.memory_size_bytes > initial_memory);
        assert_eq!(stats.total_entries, (batch_num + 1) * 100);
    }
}

#[tokio::test]
async fn test_bincode_concurrent_writes() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let strategy = Arc::new(BincodeSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create strategy"));
    
    // Spawn multiple concurrent write tasks
    let mut handles = vec![];
    
    for task_id in 0..5 {
        let strategy_clone = strategy.clone();
        let handle = tokio::spawn(async move {
            let collection_id = format!("concurrent_{}", task_id);
            create_collection_write_buffer_dir(&collection_id).await;
            
            for batch_id in 0..10 {
                let vectors = vec![
                    create_test_vector(
                        &format!("task_{}_batch_{}", task_id, batch_id),
                        128,
                        task_id as f32 * 0.1
                    ),
                ];
                let batch = create_test_batch(vectors);
                
                strategy_clone.write_native_batch(batch, &collection_id)
                    .await
                    .expect("Failed to write batch");
            }
        });
        handles.push(handle);
    }
    
    // Wait for all tasks
    for handle in handles {
        handle.await.expect("Task failed");
    }
    
    // Verify all data was written
    for task_id in 0..5 {
        let collection_id = format!("concurrent_{}", task_id);
        let vectors = strategy.get_collection_vectors(&collection_id)
            .await
            .expect("Failed to get vectors");
        
        assert_eq!(vectors.len(), 10);
    }
}

#[tokio::test]
async fn test_bincode_edge_cases() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let strategy = BincodeSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create strategy");
    
    let collection_id = "edge_cases";
    
    create_collection_write_buffer_dir(collection_id).await;
    // Test empty batch
    let empty_batch = create_test_batch(vec![]);
    let empty_sequences = strategy.write_native_batch(empty_batch, collection_id)
        .await
        .expect("Failed to write empty batch");
    assert_eq!(empty_sequences.len(), 0);
    
    // Test vectors with no ID
    let mut no_id_vector = create_test_vector("", 64, 0.5);
    no_id_vector.id = None;
    let no_id_batch = create_test_batch(vec![no_id_vector]);
    
    let no_id_sequences = strategy.write_native_batch(no_id_batch, collection_id)
        .await
        .expect("Failed to write no-id batch");
    assert_eq!(no_id_sequences.len(), 1);
    
    // Test very large vector
    let large_vector = create_test_vector("large", 10000, 0.1);
    let large_batch = create_test_batch(vec![large_vector]);
    
    let large_sequences = strategy.write_native_batch(large_batch, collection_id)
        .await
        .expect("Failed to write large vector");
    assert_eq!(large_sequences.len(), 1);
    
    // Verify all vectors can be retrieved
    let all_vectors = strategy.get_collection_vectors(collection_id)
        .await
        .expect("Failed to get vectors");
    
    assert_eq!(all_vectors.len(), 2); // no-id and large vector
}

#[tokio::test]
async fn test_bincode_collection_isolation() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let strategy = BincodeSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create strategy");
    
    // Write distinct data to multiple collections
    let collections = vec!["col_a", "col_b", "col_c"];
    
    for (idx, collection_id) in collections.iter().enumerate() {
        create_collection_write_buffer_dir(collection_id).await;
        let vectors = vec![
            create_test_vector(&format!("{}_vec1", collection_id), 64, idx as f32),
            create_test_vector(&format!("{}_vec2", collection_id), 64, idx as f32 + 0.5),
        ];
        let batch = create_test_batch(vectors);
        
        strategy.write_native_batch(batch, collection_id)
            .await
            .expect("Failed to write batch");
    }
    
    // Verify each collection has only its own data
    for collection_id in collections.iter() {
        let vectors = strategy.get_collection_vectors(collection_id)
            .await
            .expect("Failed to get vectors");
        
        assert_eq!(vectors.len(), 2);
        for vector in vectors {
            assert!(vector.id.as_ref().unwrap().starts_with(collection_id));
        }
        
        // Verify collection stats
        let stats = strategy.get_collection_stats(collection_id)
            .await
            .expect("Failed to get collection stats");
        
        assert_eq!(stats.total_entries, 2);
        assert!(stats.memory_size_bytes > 0);
    }
}

#[tokio::test]
async fn test_bincode_batch_metadata() {
    let config = create_test_config();
    let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
    let strategy = BincodeSerializationStrategy::new(&config, filesystem_factory.clone())
        .await
        .expect("Failed to create strategy");
    
    let collection_id = "metadata_test";
    create_collection_write_buffer_dir(collection_id).await;
    
    // Create vectors with complex metadata
    let mut vectors = Vec::new();
    for i in 0..5 {
        let mut vector = create_test_vector(&format!("meta_{}", i), 128, 0.1);
        vector.metadata = vec![
            MetadataItem {
                key: "index".to_string(),
                value: Some(crate::proto::proximadb::metadata_item::Value::StringValue(i.to_string())),
            },
            MetadataItem {
                key: "binary_data".to_string(),
                value: Some(crate::proto::proximadb::metadata_item::Value::StringValue(format!("{:08b}", i))),
            },
            MetadataItem {
                key: "timestamp".to_string(),
                value: Some(crate::proto::proximadb::metadata_item::Value::StringValue((1234567890 + i * 1000).to_string())),
            },
        ];
        vectors.push(vector);
    }
    
    let batch = create_test_batch(vectors);
    strategy.write_native_batch(batch, collection_id)
        .await
        .expect("Failed to write batch");
    
    // Retrieve and verify metadata preservation
    let retrieved = strategy.get_collection_vectors(collection_id)
        .await
        .expect("Failed to get vectors");
    
    assert_eq!(retrieved.len(), 5);
    
    for vector in retrieved {
        assert_eq!(vector.metadata.len(), 3);
        
        // Verify metadata keys exist
        let keys: Vec<_> = vector.metadata.iter().map(|m| &m.key).collect();
        assert!(keys.contains(&&"index".to_string()));
        assert!(keys.contains(&&"binary_data".to_string()));
        assert!(keys.contains(&&"timestamp".to_string()));
    }
}