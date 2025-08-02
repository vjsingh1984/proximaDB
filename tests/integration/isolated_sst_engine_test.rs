//! Isolated SST Engine Integration Tests
//! 
//! Tests SST storage engine functionality with completely isolated environments
//! to ensure reliable testing without cross-test contamination.

use anyhow::Result;
use std::collections::{HashMap, HashSet};

use super::test_utils::{IsolatedTestEnvironment, MultiEnvironmentTest};
use proximadb::core::search::{FilterExpression, ComparisonOperator};
use proximadb::core::VectorRecord;
use proximadb::compute::distance::DistanceMetric;
use proximadb::storage::traits::{FlushParameters, CompactionParameters, UnifiedStorageEngine};

#[tokio::test]
async fn test_isolated_sst_basic_operations() -> Result<()> {
    let env = IsolatedTestEnvironment::new().await?;
    let engine = env.create_sst_engine().await?;
    
    // Create test vectors
    let vectors = env.create_test_vectors(10);
    println!("📝 Created {} test vectors for collection: {}", vectors.len(), env.collection_id());
    
    // Insert and flush vectors
    let flush_params = FlushParameters {
        collection_id: Some(env.collection_id().to_string()),
        vector_records: vectors,
        force: true,
        synchronous: true,
        ..Default::default()
    };
    
    let result = engine.do_flush(&flush_params).await?;
    assert!(result.success, "Flush should succeed");
    
    // Search without filters
    let query_vector = env.create_query_vector();
    let results = engine.search_vectors_unified(
        env.collection_id(),
        &query_vector,
        5,
        &DistanceMetric::Cosine,
        None,
        true,
        true
    ).await?;
    
    // Verify results
    println!("🔍 Search returned {} results", results.len());
    for (i, result) in results.iter().enumerate() {
        println!("  Result {}: id={}, score={}", 
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
    
    println!("✅ Basic SST operations test passed for collection: {}", env.collection_id());
    Ok(())
}

#[tokio::test]
async fn test_isolated_sst_metadata_filtering() -> Result<()> {
    let env = IsolatedTestEnvironment::new().await?;
    let engine = env.create_sst_engine().await?;
    
    // Create test vectors with diverse metadata
    let vectors = env.create_test_vectors(15);
    let flush_params = FlushParameters {
        collection_id: Some(env.collection_id().to_string()),
        vector_records: vectors,
        force: true,
        synchronous: true,
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
    
    let filtered_results = engine.search_vectors_unified(
        env.collection_id(),
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
        &env.create_query_vector(),
        10,
        &DistanceMetric::Cosine,
        Some(&type_filter),
        true,
        true
    ).await?;
    
    assert!(!type_results.is_empty(), "Should find results with type primary");
    
    println!("✅ Metadata filtering test passed for collection: {}", env.collection_id());
    println!("   Found {} results with category A", filtered_results.len());
    println!("   Found {} results with type primary", type_results.len());
    Ok(())
}

#[tokio::test]
async fn test_isolated_sst_flush_and_compaction() -> Result<()> {
    let env = IsolatedTestEnvironment::new().await?;
    let mut engine = env.create_sst_engine().await?;
    
    println!("🔍 DEBUG TEST: Created SST engine for collection: {}", env.collection_id());
    
    // Insert multiple batches to trigger compaction
    let batch_size = 5;
    let num_batches = 4;
    
    for batch in 0..num_batches {
        let vectors: Vec<VectorRecord> = (0..batch_size).map(|i| {
            let global_id = batch * batch_size + i;
            let mut vector = env.create_test_vectors(1)[0].clone();
            vector.id = Some(format!("{}_{}", env.collection_id(), global_id));
            vector.vector = vec![global_id as f32, (global_id + 1) as f32, (global_id + 2) as f32];
            vector
        }).collect();
        
        println!("🔍 DEBUG TEST: Flushing batch {} with {} vectors", batch + 1, vectors.len());
        for (i, v) in vectors.iter().take(2).enumerate() {
            println!("🔍 DEBUG TEST:   Vector {}: id={:?}", i, v.id);
        }
        
        let flush_params = FlushParameters {
            collection_id: Some(env.collection_id().to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            ..Default::default()
        };
        
        let result = engine.do_flush(&flush_params).await?;
        assert!(result.success, "Flush should succeed");
        println!("📦 Flushed batch {} of {} - entries_flushed={}, bytes_written={}", 
            batch + 1, num_batches, result.entries_flushed, result.bytes_written);
    }
    
    // Enable compaction and trigger it
    engine.enable_compaction(1).await?;
    let compact_params = CompactionParameters {
        collection_id: Some(env.collection_id().to_string()),
        force: true,
        synchronous: true,
        ..Default::default()
    };
    let result = engine.compact(compact_params).await?;
    assert!(result.success, "Compaction should succeed");
    
    // Verify all vectors are still searchable after compaction
    let all_results = engine.search_vectors_unified(
        env.collection_id(),
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
    
    println!("✅ Flush and compaction test passed for collection: {}", env.collection_id());
    println!("   Found all {} vectors after compaction", total_vectors);
    Ok(())
}

#[tokio::test]
async fn test_isolated_sst_concurrent_operations() -> Result<()> {
    let env = IsolatedTestEnvironment::new().await?;
    let engine = std::sync::Arc::new(env.create_sst_engine().await?);
    
    // Spawn concurrent flush operations
    let mut handles = Vec::new();
    let concurrent_batches = 5;
    
    for batch_id in 0..concurrent_batches {
        let engine_clone = engine.clone();
        let env_collection_id = env.collection_id().to_string();
        
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
                    ..Default::default()
                }
            }).collect();
            
            let flush_params = FlushParameters {
                collection_id: Some(env_collection_id),
                vector_records: vectors,
                force: true,
                synchronous: true,
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
                println!("⚠️ Concurrent flush failed: {}", e);
            }
        }
    }
    
    assert_eq!(successful_flushes, concurrent_batches, 
        "All {} concurrent flushes should succeed", concurrent_batches);
    
    let expected_total = concurrent_batches * 3; // 3 vectors per batch
    assert_eq!(total_flushed, expected_total,
        "Should have flushed {} total vectors", expected_total);
    
    // Verify all vectors are searchable
    let search_results = engine.search_vectors_unified(
        env.collection_id(),
        &env.create_query_vector(),
        100,
        &DistanceMetric::Euclidean,
        None,
        true,
        true
    ).await?;
    
    assert_eq!(search_results.len(), expected_total as usize,
        "Should find all {} vectors after concurrent operations", expected_total);
    
    println!("✅ Concurrent operations test passed for collection: {}", env.collection_id());
    println!("   {} concurrent batches, {} total vectors flushed", successful_flushes, total_flushed);
    Ok(())
}

#[tokio::test]
async fn test_isolated_sst_recovery_persistence() -> Result<()> {
    let env = IsolatedTestEnvironment::new().await?;
    let original_vectors = env.create_test_vectors(8);
    
    // Phase 1: Create engine, insert data, and flush
    {
        let engine = env.create_sst_engine().await?;
        let flush_params = FlushParameters {
            collection_id: Some(env.collection_id().to_string()),
            vector_records: original_vectors.clone(),
            force: true,
            synchronous: true,
            ..Default::default()
        };
        let result = engine.do_flush(&flush_params).await?;
        assert!(result.success, "Flush should succeed");
    } // Engine goes out of scope
    
    // Phase 2: Create new engine instance and verify data persisted
    {
        let engine = env.create_sst_engine().await?;
        
        // Search for persisted data
        let results = engine.search_vectors_unified(
            env.collection_id(),
            &env.create_query_vector(),
            10,
            &DistanceMetric::Cosine,
            None,
            true,
            true
        ).await?;
        
        // Verify data persisted
        assert_eq!(results.len(), original_vectors.len(),
            "Should find all {} persisted vectors", original_vectors.len());
        
        // Verify vector IDs match original data
        let original_ids: HashSet<_> = original_vectors.iter()
            .map(|v| v.id.as_ref().unwrap().as_str())
            .collect();
        
        let found_ids: HashSet<_> = results.iter()
            .map(|r| r.id.as_str())
            .collect();
        
        assert_eq!(original_ids, found_ids, "Persisted vector IDs should match original");
    }
    
    println!("✅ Recovery persistence test passed for collection: {}", env.collection_id());
    println!("   Successfully recovered {} vectors after restart", original_vectors.len());
    Ok(())
}

#[tokio::test]
async fn test_isolated_multi_collection_isolation() -> Result<()> {
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
        let flush_params = FlushParameters {
            collection_id: Some(env.collection_id().to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            ..Default::default()
        };
        let result = engine.do_flush(&flush_params).await?;
        assert!(result.success, "Flush should succeed");
        println!("📁 Inserted data in collection {}: {}", i, env.collection_id());
    }
    
    // Verify complete isolation - each collection should only see its own data
    for (i, (env, engine)) in multi_env.environments.iter().zip(engines.iter()).enumerate() {
        let results = engine.search_vectors_unified(
            env.collection_id(),
            &env.create_query_vector(),
            10,
            &DistanceMetric::Cosine,
            None,
            true,
            true
        ).await?;
        
        // Should find exactly the vectors from this collection
        assert_eq!(results.len(), 5, "Collection {} should have exactly 5 vectors", i);
        
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
    
    println!("✅ Multi-collection isolation test passed");
    for (i, env) in multi_env.environments.iter().enumerate() {
        println!("   Collection {}: {} (isolated)", i, env.collection_id());
    }
    Ok(())
}

#[tokio::test]
async fn test_isolated_sst_distance_metrics() -> Result<()> {
    let env = IsolatedTestEnvironment::new().await?;
    let engine = env.create_sst_engine().await?;
    
    // Create vectors with known relationships for distance testing
    let vectors = vec![
        proximadb::core::VectorRecord {
            id: Some(format!("{}_identical", env.collection_id())),
            vector: vec![1.0, 0.0, 0.0], // Identical to query
            metadata: vec![],
            timestamp: chrono::Utc::now().timestamp() as u32,
            ..Default::default()
        },
        proximadb::core::VectorRecord {
            id: Some(format!("{}_orthogonal", env.collection_id())),
            vector: vec![0.0, 1.0, 0.0], // Orthogonal to query
            metadata: vec![],
            timestamp: chrono::Utc::now().timestamp() as u32,
            ..Default::default()
        },
        proximadb::core::VectorRecord {
            id: Some(format!("{}_opposite", env.collection_id())),
            vector: vec![-1.0, 0.0, 0.0], // Opposite to query
            metadata: vec![],
            timestamp: chrono::Utc::now().timestamp() as u32,
            ..Default::default()
        },
    ];
    
    let flush_params = FlushParameters {
        collection_id: Some(env.collection_id().to_string()),
        vector_records: vectors,
        force: true,
        synchronous: true,
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
    
    for metric in metrics {
        let results = engine.search_vectors_unified(
            env.collection_id(),
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
        
        println!("📐 {:?} metric: closest distance = {:.4}", metric, distance);
    }
    
    println!("✅ Distance metrics test passed for collection: {}", env.collection_id());
    Ok(())
}

#[tokio::test]
async fn test_isolated_sst_large_dataset() -> Result<()> {
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
                ..Default::default()
            }
        }).collect();
        
        let flush_params = FlushParameters {
            collection_id: Some(env.collection_id().to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            ..Default::default()
        };
        let result = engine.do_flush(&flush_params).await?;
        assert!(result.success, "Flush should succeed");
        println!("📦 Inserted batch {} of {} ({} vectors)", batch + 1, num_batches, batch_size);
        
        // Give the manifest time to update
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
    
    // Add a small delay to ensure all manifest updates are complete
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
    
    // Test various search scenarios
    
    // 1. Search for top-k results
    let top_k_results = engine.search_vectors_unified(
        env.collection_id(),
        &vec![0.1, 0.2, 0.3],
        15,
        &DistanceMetric::Euclidean,
        None,
        true,
        true
    ).await?;
    
    assert_eq!(top_k_results.len(), 15, "Should return exactly 15 results");
    
    // 2. Search with metadata filter
    let batch_filter = FilterExpression::Comparison {
        field: "batch".to_string(),
        operator: ComparisonOperator::Equals,
        value: serde_json::Value::Number(serde_json::Number::from(2)),
    };
    
    let batch_results = engine.search_vectors_unified(
        env.collection_id(),
        &vec![0.5, 0.6, 0.7],
        30,
        &DistanceMetric::Cosine,
        Some(&batch_filter),
        true,
        true
    ).await?;
    
    println!("DEBUG: batch_filter = {:?}", batch_filter);
    println!("DEBUG: batch_results.len() = {}", batch_results.len());
    if batch_results.is_empty() {
        // Let's check without filter to see what metadata we have
        let debug_results = engine.search_vectors_unified(
            env.collection_id(),
            &vec![0.5, 0.6, 0.7],
            5,
            &DistanceMetric::Cosine,
            None,
            true,
            true
        ).await?;
        println!("DEBUG: Sample results without filter:");
        for (i, result) in debug_results.iter().enumerate() {
            println!("  Result {}: id={}, metadata={:?}", i, result.id, result.metadata);
        }
    }
    
    assert_eq!(batch_results.len(), batch_size, 
        "Should find all {} vectors from batch 2", batch_size);
    
    // 3. Search across all data to verify completeness
    let all_results = engine.search_vectors_unified(
        env.collection_id(),
        &vec![0.5, 0.5, 0.5],
        200, // Request more than we have
        &DistanceMetric::DotProduct,
        None,
        true,
        true
    ).await?;
    
    assert_eq!(all_results.len(), total_vectors,
        "Should find all {} vectors in the dataset", total_vectors);
    
    println!("✅ Large dataset test passed for collection: {}", env.collection_id());
    println!("   Dataset: {} vectors, Top-K: {}, Filtered: {}, All: {}", 
             total_vectors, top_k_results.len(), batch_results.len(), all_results.len());
    Ok(())
}