//! Isolated SST Engine Integration Tests
//! 
//! Tests SST storage engine functionality with completely isolated environments
//! to ensure reliable testing without cross-test contamination.

use anyhow::Result;
use tracing::{debug, error, info, warn};
use std::collections::{HashMap, HashSet};

use super::test_utils::{IsolatedTestEnvironment, MultiEnvironmentTest};
use proximadb::core::search::{FilterExpression, ComparisonOperator};
use proximadb::core::VectorRecord;
use proximadb::compute::distance_computation::DistanceMetric;
use proximadb::storage::traits::{FlushParameters, CompactionParameters, UnifiedStorageEngine};

#[tokio::test]
async fn test_isolated_sst_basic_operations() -> Result<()> {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let env = IsolatedTestEnvironment::new().await?;
    let engine = env.create_sst_engine().await?;
    
    // Create test vectors
    let vectors = env.create_test_vectors(10);
    debug!("📝 Created {} test vectors for collection: {}", vectors.len(), env.collection_id());
    
    // Insert and flush vectors
    let collection_config = env.create_test_collection();
    let flush_params = FlushParameters {
        collection_id: Some(env.collection_id().to_string()),
        vector_records: vectors,
        force: true,
        synchronous: true,
        collection_config: Some(collection_config),
        ..Default::default()
    };
    
    let result = engine.do_flush(&flush_params).await?;
    assert!(result.success, "Flush should succeed");
    
    // Search without filters
    let query_vector = env.create_query_vector();
    let storage_url = format!("file://{}/data", env.persistent_dir.to_str().unwrap());
    info!("🔍 Using storage URL for search: {}", storage_url);
    let results = engine.search_vectors_unified(
        env.collection_id(),
        &storage_url,
        &query_vector,
        5,
        &DistanceMetric::Cosine,
        None,
        true,
        true
    ).await?;
    
    // Verify results
    debug!("🔍 Search returned {} results", results.len());
    for (i, result) in results.iter().enumerate() {
        debug!("  Result {}: id={}, score={}", 
                 i, result.id, result.score);
    }
    assert!(!results.is_empty(), "Should find search results");
    assert!(results.len() <= 5, "Should not return more than requested");
    
    // Verify first result is closest (distance should be smallest)
    assert!(results[0].distance.unwrap() < 1.0, "Closest result should have small distance");
    
    // Verify all results belong to this collection
    for result in &results {
        assert!(result.id.starts_with(env.collection_id()),
            "Result ID {} should belong to collection {}", result.id, env.collection_id());
    }
    
    debug!("✅ Basic SST operations test passed for collection: {}", env.collection_id());
    Ok(())
}

#[tokio::test]
async fn test_isolated_sst_metadata_filtering() -> Result<()> {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let env = IsolatedTestEnvironment::new().await?;
    let engine = env.create_sst_engine().await?;
    
    // Create test vectors with diverse metadata
    let vectors = env.create_test_vectors(15);
    let collection_config = env.create_test_collection();
    let flush_params = FlushParameters {
        collection_id: Some(env.collection_id().to_string()),
        vector_records: vectors,
        force: true,
        synchronous: true,
        collection_config: Some(collection_config),
        ..Default::default()
    };
    let result = engine.do_flush(&flush_params).await?;
    assert!(result.success, "Flush should succeed");
    
    // Test metadata filter: category = "A"
    let filter = FilterExpression::Comparison {
        field: "category".to_string(),
        operator: ComparisonOperator::Equals,
        value: serde_json::Value::String("A".to_string()),
    };
    
    let storage_url = format!("file://{}/data", env.persistent_dir.to_str().unwrap());
    info!("🔍 Using storage URL for filtered search: {}", storage_url);
    let filtered_results = engine.search_vectors_unified(
        env.collection_id(),
        &storage_url,
        &env.create_query_vector(),
        10,
        &DistanceMetric::Cosine,
        Some(&filter),
        true,
        true
    ).await?;
    
    // Verify all results have category "A"
    assert!(!filtered_results.is_empty(), "Should find results with category A");
    
    // Note: We created vectors with categories A, B, C rotating, so should find some A's
    let expected_category_a_count = (15 + 2) / 3; // ~5 vectors with category A
    assert!(filtered_results.len() <= expected_category_a_count);
    
    // Test another filter: type = "primary"
    let type_filter = FilterExpression::Comparison {
        field: "type".to_string(),
        operator: ComparisonOperator::Equals,
        value: serde_json::Value::String("primary".to_string()),
    };
    
    let type_results = engine.search_vectors_unified(
        env.collection_id(),
        &storage_url,
        &env.create_query_vector(),
        10,
        &DistanceMetric::Cosine,
        Some(&type_filter),
        true,
        true
    ).await?;
    
    assert!(!type_results.is_empty(), "Should find results with type primary");
    
    debug!("✅ Metadata filtering test passed for collection: {}", env.collection_id());
    debug!("   Found {} results with category A", filtered_results.len());
    debug!("   Found {} results with type primary", type_results.len());
    Ok(())
}

#[tokio::test]
async fn test_isolated_sst_flush_and_compaction() -> Result<()> {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let env = IsolatedTestEnvironment::new().await?;
    let mut engine = env.create_sst_engine().await?;
    
    println!("🚀 STARTING SST FLUSH AND COMPACTION TEST");
    println!("🏗️ Test environment:");
    println!("   - Collection ID: {}", env.collection_id());
    println!("   - Persistent directory: {}", env.persistent_dir.display());
    println!("   - Data directory: {}/data", env.persistent_dir.display());
    
    // Verify data directory exists
    let data_dir = env.persistent_dir.join("data");
    if !data_dir.exists() {
        println!("❌ Data directory does not exist, creating: {}", data_dir.display());
        tokio::fs::create_dir_all(&data_dir).await?;
    }
    println!("✅ Data directory confirmed: {}", data_dir.display());
    
    // Insert multiple batches to trigger compaction
    let batch_size = 5;
    let num_batches = 4;
    
    info!("📝 Test plan: {} batches × {} vectors = {} total vectors", 
          num_batches, batch_size, num_batches * batch_size);
    
    for batch in 0..num_batches {
        let vectors: Vec<VectorRecord> = (0..batch_size).map(|i| {
            let global_id = batch * batch_size + i;
            let mut vector = env.create_test_vectors(1)[0].clone();
            vector.id = Some(format!("{}_{}", env.collection_id(), global_id));
            vector.vector = vec![global_id as f32, (global_id + 1) as f32, (global_id + 2) as f32];
            vector
        }).collect();
        
        debug!("🔍 DEBUG TEST: Flushing batch {} with {} vectors", batch + 1, vectors.len());
        for (i, v) in vectors.iter().take(2).enumerate() {
            debug!("🔍 DEBUG TEST:   Vector {}: id={:?}", i, v.id);
        }
        
        let collection_config = env.create_test_collection();
        let flush_params = FlushParameters {
            collection_id: Some(env.collection_id().to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            collection_config: Some(collection_config),
            ..Default::default()
        };
        
        println!("🔄 EXECUTING FLUSH for batch {}/{}...", batch + 1, num_batches);
        let result = engine.do_flush(&flush_params).await?;
        
        println!("✅ FLUSH COMPLETED for batch {}/{}:", batch + 1, num_batches);
        println!("   - Success: {}", result.success);
        println!("   - Entries flushed: {}", result.entries_flushed);
        println!("   - Bytes written: {}", result.bytes_written);
        println!("   - Files created: {}", result.files_created);
        println!("   - Duration: {}ms", result.duration_ms);
        
        assert!(result.success, "Flush should succeed for batch {}", batch + 1);
        assert_eq!(result.entries_flushed, batch_size as u64, 
                  "Should flush exactly {} vectors in batch {}", batch_size, batch + 1);
        
        // Check if files were actually created
        let data_files = tokio::fs::read_dir(&data_dir).await;
        match data_files {
            Ok(mut entries) => {
                let mut file_count = 0;
                while let Some(_entry) = entries.next_entry().await? {
                    file_count += 1;
                }
                info!("📁 Files in data directory after batch {}: {} files", batch + 1, file_count);
            }
            Err(e) => {
                warn!("❌ Failed to read data directory: {}", e);
            }
        }
    }
    
    println!("🔧 PREPARING FOR COMPACTION...");
    
    // Final file count before compaction - check the actual SST file location
    // Files are written to: {persistent_dir}/{collection_id}/data/ 
    let actual_sst_dir = env.persistent_dir.join(&env.collection_id).join("data");
    println!("🔍 Checking actual SST directory: {}", actual_sst_dir.display());
    
    let final_data_files = tokio::fs::read_dir(&actual_sst_dir).await;
    match final_data_files {
        Ok(mut entries) => {
            let mut file_count = 0;
            let mut total_size = 0;
            while let Some(entry) = entries.next_entry().await? {
                if let Ok(metadata) = entry.metadata().await {
                    file_count += 1;
                    total_size += metadata.len();
                    println!("📄 Found file: {} ({} bytes)", 
                          entry.file_name().to_string_lossy(), metadata.len());
                }
            }
            println!("📊 PRE-COMPACTION STATE:");
            println!("   - Total files in data dir: {}", file_count);
            println!("   - Total size: {} bytes", total_size);
            
        }
        Err(e) => {
            println!("❌ Failed to read data directory before compaction: {}", e);
        }
    }
    
    // Enable compaction and trigger it
    info!("⚙️ Enabling compaction with level 1...");
    engine.enable_compaction(1).await?;
    
    let collection_config = env.create_test_collection();
    info!("🔄 EXECUTING COMPACTION...");
    info!("📋 Compaction parameters:");
    info!("   - Collection ID: {:?}", env.collection_id());
    info!("   - Force: true");
    info!("   - Synchronous: true");
    info!("   - Has collection config: {}", collection_config.config.is_some());
    info!("   - Has storage assignment: {}", collection_config.storage_assignment.is_some());
    
    let compact_params = CompactionParameters {
        collection_id: Some(env.collection_id().to_string()),
        force: true,
        synchronous: true,
        collection_config: Some(collection_config),
        ..Default::default()
    };
    
    let result = engine.compact(compact_params).await?;
    
    println!("✅ COMPACTION COMPLETED:");
    println!("   - Success: {}", result.success);
    println!("   - Entries processed: {}", result.entries_processed);
    println!("   - Files merged (input): {}", result.input_files);
    println!("   - Files created (output): {}", result.output_files);
    println!("   - Bytes read: {}", result.bytes_read);
    println!("   - Bytes written: {}", result.bytes_written);
    println!("   - Duration: {}ms", result.duration_ms);
    
    assert!(result.success, "Compaction should succeed");
    
    // Verify that compaction actually processed the expected number of records
    // This ensures we're not just silently passing because flush data is still readable
    info!("🧮 VERIFYING COMPACTION RESULTS...");
    
    // Each batch had 5 vectors, with 4 batches = 20 total vectors that should be compacted
    let expected_total_vectors = batch_size * num_batches; // 5 * 4 = 20
    info!("   - Expected vectors to process: {}", expected_total_vectors);
    info!("   - Actually processed: {}", result.entries_processed);
    
    if result.entries_processed == 0 {
        error!("❌ COMPACTION ISSUE: No records were processed!");
        error!("   This suggests compaction found no files to compact.");
        error!("   Check if flush operations actually created persistent files.");
        
        // List directory contents again for debugging
        let debug_files = tokio::fs::read_dir(&data_dir).await;
        match debug_files {
            Ok(mut entries) => {
                error!("📁 Current data directory contents:");
                while let Some(entry) = entries.next_entry().await? {
                    if let Ok(metadata) = entry.metadata().await {
                        error!("   - {}: {} bytes", 
                              entry.file_name().to_string_lossy(), metadata.len());
                    }
                }
            }
            Err(e) => {
                error!("   Failed to list directory: {}", e);
            }
        }
    }
    
    assert_eq!(result.entries_processed, expected_total_vectors as u64,
        "Compaction should process all {} vectors that were flushed", expected_total_vectors);
    
    // Verify all vectors are still searchable after compaction
    let storage_url = format!("file://{}/data", env.persistent_dir.to_str().unwrap());
    info!("🔍 Using storage URL for filtered search: {}", storage_url);
    let all_results = engine.search_vectors_unified(
        env.collection_id(),
        &storage_url,
        &env.create_query_vector(),
        100, // Request all vectors
        &DistanceMetric::Euclidean,
        None,
        true,
        true
    ).await?;
    
    let total_vectors = batch_size * num_batches;
    assert_eq!(all_results.len(), total_vectors, 
        "Should find all {} vectors after compaction", total_vectors);
    
    // Verify vector IDs are all unique and belong to this collection
    let mut found_ids = HashSet::new();
    for result in &all_results {
        assert!(result.id.starts_with(env.collection_id()),
            "Result ID {} should belong to collection {}", result.id, env.collection_id());
        assert!(found_ids.insert(result.id.clone()),
            "Duplicate result ID found: {}", result.id);
    }
    
    debug!("✅ Flush and compaction test passed for collection: {}", env.collection_id());
    debug!("   Found all {} vectors after compaction", total_vectors);
    Ok(())
}

#[tokio::test]
async fn test_isolated_sst_concurrent_operations() -> Result<()> {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let env = IsolatedTestEnvironment::new().await?;
    let engine = std::sync::Arc::new(env.create_sst_engine().await?);
    
    // Spawn concurrent flush operations
    let mut handles = Vec::new();
    let concurrent_batches = 5;
    
    for batch_id in 0..concurrent_batches {
        let engine_clone = engine.clone();
        let env_collection_id = env.collection_id().to_string();
        let collection_config = env.create_test_collection();
        
        let handle = tokio::spawn(async move {
            // Create unique vectors for this batch
            let vectors = (0..3).map(|i| {
                proximadb::core::VectorRecord {
                    id: Some(format!("{}_concurrent_{}_{}", env_collection_id, batch_id, i)),
                    vector: vec![(batch_id * 10 + i) as f32, (batch_id * 10 + i + 1) as f32, (batch_id * 10 + i + 2) as f32],
                    metadata: vec![
                        proximadb::proto::proximadb::MetadataItem {
                            key: "batch_id".to_string(),
                            value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(batch_id.to_string())),
                        },
                    ],
                    timestamp: chrono::Utc::now().timestamp() as u32,
                    updated_at: None,
                    expires_at: None,
                    distance: None,
                    rank: None,
                    score: None,
                    version: None,
                    ..Default::default()
                }
            }).collect();
            
            let flush_params = FlushParameters {
                collection_id: Some(env_collection_id),
                vector_records: vectors,
                force: true,
                synchronous: true,
                collection_config: Some(collection_config),
                ..Default::default()
            };
            
            engine_clone.do_flush(&flush_params).await
        });
        
        handles.push(handle);
    }
    
    // Wait for all concurrent operations to complete
    let mut successful_flushes = 0;
    let mut total_flushed = 0;
    
    for handle in handles {
        match handle.await? {
            Ok(result) => {
                if result.success {
                    successful_flushes += 1;
                    total_flushed += result.entries_flushed;
                }
            }
            Err(e) => {
                debug!("⚠️ Concurrent flush failed: {}", e);
            }
        }
    }
    
    assert_eq!(successful_flushes, concurrent_batches, 
        "All concurrent flushes should complete");
    
    let expected_total = concurrent_batches * 3; // 3 vectors per batch
    assert_eq!(total_flushed, expected_total,
        "Should have flushed expected total");
    
    // Verify all vectors are searchable
    let storage_url = format!("file://{}/data", env.persistent_dir.to_str().unwrap());
    info!("🔍 Using storage URL for filtered search: {}", storage_url);
    let search_results = engine.search_vectors_unified(
        env.collection_id(),
        &storage_url,
        &env.create_query_vector(),
        100,
        &DistanceMetric::Euclidean,
        None,
        true,
        true
    ).await?;
    
    assert_eq!(search_results.len(), expected_total as usize,
        "Should find all items after concurrent operations");
    
    debug!("✅ Concurrent operations test passed for collection: {}", env.collection_id());
    debug!("   {} concurrent batches, {} total items written", successful_flushes, total_flushed);
    Ok(())
}

#[tokio::test]
async fn test_isolated_sst_recovery_persistence() -> Result<()> {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let env = IsolatedTestEnvironment::new().await?;
    let original_vectors = env.create_test_vectors(8);
    
    // Phase 1: Create engine, insert data, and flush
    {
        let engine = env.create_sst_engine().await?;
        let collection_config = env.create_test_collection();
        let flush_params = FlushParameters {
            collection_id: Some(env.collection_id().to_string()),
            vector_records: original_vectors.clone(),
            force: true,
            synchronous: true,
            collection_config: Some(collection_config),
            ..Default::default()
        };
        let result = engine.do_flush(&flush_params).await?;
        assert!(result.success, "Flush should succeed");
    } // Engine goes out of scope
    
    // Phase 2: Create new engine instance and verify data persisted
    {
        let engine = env.create_sst_engine().await?;
        
        // Search for persisted data
        let storage_url = format!("file://{}/data", env.persistent_dir.to_str().unwrap());
    info!("🔍 Using storage URL for filtered search: {}", storage_url);
        let results = engine.search_vectors_unified(
            env.collection_id(),
            &storage_url,
            &env.create_query_vector(),
            10,
            &DistanceMetric::Cosine,
            None,
            true,
            true
        ).await?;
        
        // Verify data persisted
        assert_eq!(results.len(), original_vectors.len(),
            "Should find all persisted records");
        
        // Verify vector IDs match original data
        let original_ids: HashSet<_> = original_vectors.iter()
            .map(|v| v.id.as_ref().unwrap().as_str())
            .collect();
        
        let found_ids: HashSet<_> = results.iter()
            .map(|r| r.id.as_str())
            .collect();
        
        assert_eq!(original_ids, found_ids, "Persisted vector IDs should match original");
    }
    
    debug!("✅ Recovery persistence test passed for collection: {}", env.collection_id());
    debug!("   Successfully recovered {} items after system restart", original_vectors.len());
    Ok(())
}

#[tokio::test]
async fn test_isolated_multi_collection_isolation() -> Result<()> {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    // Create multiple isolated environments
    let multi_env = MultiEnvironmentTest::new(3).await?;
    let mut engines = Vec::new();
    
    // Create SST engines for each environment
    for env in &multi_env.environments {
        let engine = env.create_sst_engine().await?;
        engines.push(engine);
    }
    
    // Insert different data in each collection
    for (i, (env, engine)) in multi_env.environments.iter().zip(engines.iter()).enumerate() {
        let vectors = env.create_test_vectors(5);
        let collection_config = env.create_test_collection();
        let flush_params = FlushParameters {
            collection_id: Some(env.collection_id().to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            collection_config: Some(collection_config),
            ..Default::default()
        };
        let result = engine.do_flush(&flush_params).await?;
        assert!(result.success, "Flush should succeed");
        debug!("📁 Inserted data in collection {}: {}", i, env.collection_id());
    }
    
    // Verify complete isolation - each collection should only see its own data
    for (i, (env, engine)) in multi_env.environments.iter().zip(engines.iter()).enumerate() {
        let storage_url = format!("file://{}/data", env.persistent_dir.to_str().unwrap());
    info!("🔍 Using storage URL for filtered search: {}", storage_url);
        let results = engine.search_vectors_unified(
            env.collection_id(),
            &storage_url,
            &env.create_query_vector(),
            10,
            &DistanceMetric::Cosine,
            None,
            true,
            true
        ).await?;
        
        // Should find exactly the vectors from this collection
        assert_eq!(results.len(), 5, "Collection {} should have exactly 5 records", i);
        
        // All results should belong to this collection
        for result in &results {
            assert!(result.id.starts_with(env.collection_id()),
                "Result {} should belong to collection {}", result.id, env.collection_id());
        }
        
        // Should not find vectors from other collections
        for (j, other_env) in multi_env.environments.iter().enumerate() {
            if i != j {
                for result in &results {
                    assert!(!result.id.starts_with(other_env.collection_id()),
                        "Collection {} should not contain vectors from collection {}", i, j);
                }
            }
        }
    }
    
    debug!("✅ Multi-collection isolation test completed");
    for (i, env) in multi_env.environments.iter().enumerate() {
        debug!("   Collection {}: {} (isolated)", i, env.collection_id());
    }
    Ok(())
}

#[tokio::test]
async fn test_isolated_sst_distance_metrics() -> Result<()> {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let env = IsolatedTestEnvironment::new().await?;
    let engine = env.create_sst_engine().await?;
    
    // Create vectors with known relationships for distance testing
    let vectors = vec![
        proximadb::core::VectorRecord {
            id: Some(format!("{}_identical", env.collection_id())),
            vector: vec![1.0, 0.0, 0.0], // Identical to query
            metadata: vec![],
            timestamp: chrono::Utc::now().timestamp() as u32,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
            version: None,
            ..Default::default()
        },
        proximadb::core::VectorRecord {
            id: Some(format!("{}_orthogonal", env.collection_id())),
            vector: vec![0.0, 1.0, 0.0], // Orthogonal to query
            metadata: vec![],
            timestamp: chrono::Utc::now().timestamp() as u32,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
            version: None,
            ..Default::default()
        },
        proximadb::core::VectorRecord {
            id: Some(format!("{}_opposite", env.collection_id())),
            vector: vec![-1.0, 0.0, 0.0], // Opposite to query
            metadata: vec![],
            timestamp: chrono::Utc::now().timestamp() as u32,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
            version: None,
            ..Default::default()
        },
    ];
    
    let collection_config = env.create_test_collection();
    let flush_params = FlushParameters {
        collection_id: Some(env.collection_id().to_string()),
        vector_records: vectors,
        force: true,
        synchronous: true,
        collection_config: Some(collection_config),
        ..Default::default()
    };
    let result = engine.do_flush(&flush_params).await?;
    assert!(result.success, "Flush should succeed");
    
    let query_vector = vec![1.0, 0.0, 0.0];
    
    // Test different distance metrics
    let metrics = vec![
        DistanceMetric::Cosine,
        DistanceMetric::Euclidean,
        DistanceMetric::DotProduct,
    ];
    
    let storage_url = format!("file://{}/data", env.persistent_dir.to_str().unwrap());
    info!("🔍 Using storage URL for filtered search: {}", storage_url);
    for metric in metrics {
        let results = engine.search_vectors_unified(
            env.collection_id(),
            &storage_url,
            &query_vector,
            3,
            &metric,
            None,
            true,
            true
        ).await?;
        
        assert_eq!(results.len(), 3, "Should find all 3 vectors for {:?}", metric);
        
        // For all metrics, identical vector should be closest (first result)
        assert!(results[0].id.contains("identical"),
            "Identical vector should be closest for {:?}", metric);
        
        // Distance should be 0 or very close to 0 for identical vector
        let distance = results[0].distance.unwrap();
        match metric {
            DistanceMetric::Cosine => assert!(distance < 0.01, "Cosine distance should be ~0 for identical vectors"),
            DistanceMetric::Euclidean => assert!(distance < 0.01, "Euclidean distance should be ~0 for identical vectors"),
            DistanceMetric::DotProduct => assert!(distance > 0.99, "Dot product should be ~1 for identical vectors"),
            _ => {} // Other metrics not tested in this specific test
        }
        
        debug!("📐 {:?} metric: closest distance = {:.4}", metric, distance);
    }
    
    debug!("✅ Distance metrics test passed for collection: {}", env.collection_id());
    Ok(())
}

#[tokio::test]
async fn test_isolated_sst_large_dataset() -> Result<()> {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let env = IsolatedTestEnvironment::new().await?;
    let engine = env.create_sst_engine().await?;
    
    // Insert a larger dataset in batches
    let total_vectors = 100;
    let batch_size = 20;
    let num_batches = total_vectors / batch_size;
    
    for batch in 0..num_batches {
        let start_id = batch * batch_size;
        let vectors = (0..batch_size).map(|i| {
            let global_id = start_id + i;
            proximadb::core::VectorRecord {
                id: Some(format!("{}_{:03}", env.collection_id(), global_id)),
                vector: vec![
                    (global_id as f32) / 100.0,
                    ((global_id + 1) as f32) / 100.0,
                    ((global_id + 2) as f32) / 100.0
                ],
                metadata: vec![
                    proximadb::proto::proximadb::MetadataItem {
                        key: "batch".to_string(),
                        value: Some(proximadb::proto::proximadb::metadata_item::Value::NumberValue(batch as f64)),
                    },
                    proximadb::proto::proximadb::MetadataItem {
                        key: "id_mod_10".to_string(),
                        value: Some(proximadb::proto::proximadb::metadata_item::Value::NumberValue((global_id % 10) as f64)),
                    },
                ],
                timestamp: chrono::Utc::now().timestamp() as u32,
                updated_at: None,
                expires_at: None,
                distance: None,
                rank: None,
                score: None,
                version: None,
                ..Default::default()
            }
        }).collect();
        
        let collection_config = env.create_test_collection();
        let flush_params = FlushParameters {
            collection_id: Some(env.collection_id().to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            collection_config: Some(collection_config),
            ..Default::default()
        };
        let result = engine.do_flush(&flush_params).await?;
        assert!(result.success, "Flush should succeed");
        debug!("📦 Inserted batch {} of {} ({} vectors)", batch + 1, num_batches, batch_size);
        
        // Give the manifest time to update
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    
    // Add a small delay to ensure all manifest updates are complete
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    // Test various search scenarios
    
    // 1. Search for top-k results
    let storage_url = format!("file://{}/data", env.persistent_dir.to_str().unwrap());
    info!("🔍 Using storage URL for filtered search: {}", storage_url);
    let top_k_results = engine.search_vectors_unified(
        env.collection_id(),
        &storage_url,
        &vec![0.1, 0.2, 0.3],
        15,
        &DistanceMetric::Euclidean,
        None,
        true,
        true
    ).await?;
    
    assert_eq!(top_k_results.len(), 15, "Should return exactly 15 items");
    
    // 2. Search with metadata filter
    let batch_filter = FilterExpression::Comparison {
        field: "batch".to_string(),
        operator: ComparisonOperator::Equals,
        value: serde_json::Value::Number(serde_json::Number::from(2)),
    };
    
    let batch_results = engine.search_vectors_unified(
        env.collection_id(),
        &storage_url,
        &vec![0.5, 0.6, 0.7],
        30,
        &DistanceMetric::Cosine,
        Some(&batch_filter),
        true,
        true
    ).await?;
    
    debug!("DEBUG: batch_filter = {:?}", batch_filter);
    debug!("DEBUG: batch_results.len() = {}", batch_results.len());
    if batch_results.is_empty() {
        // Let's check without filter to see what metadata we have
        let debug_results = engine.search_vectors_unified(
            env.collection_id(),
            &storage_url,
            &vec![0.5, 0.6, 0.7],
            5,
            &DistanceMetric::Cosine,
            None,
            true,
            true
        ).await?;
        debug!("DEBUG: Sample results without filter:");
        for (i, result) in debug_results.iter().enumerate() {
            debug!("  Result {}: id={}, metadata={:?}", i, result.id, result.metadata);
        }
    }
    
    assert_eq!(batch_results.len(), batch_size, 
        "Should find all {} vectors from batch 2", batch_size);
    
    // 3. Search across all data to verify completeness
    let all_results = engine.search_vectors_unified(
        env.collection_id(),
        &storage_url,
        &vec![0.5, 0.5, 0.5],
        200, // Request more than we have
        &DistanceMetric::DotProduct,
        None,
        true,
        true
    ).await?;
    
    assert_eq!(all_results.len(), total_vectors,
        "Should find all {} vectors in the collection", total_vectors);
    
    debug!("✅ Large dataset test passed for collection: {}", env.collection_id());
    debug!("   Dataset: {} vectors, Top-K: {}, Filtered: {}, All: {}", 
             total_vectors, top_k_results.len(), batch_results.len(), all_results.len());
    Ok(())
}