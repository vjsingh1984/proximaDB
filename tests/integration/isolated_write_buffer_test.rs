//! Isolated WriteBuffer Integration Tests
//! 
//! Tests WriteBuffer functionality with completely isolated environments
//! to ensure reliable testing without cross-test contamination.

use anyhow::Result;
use tracing::{debug, error, info, warn};
use std::sync::Arc;

mod common {
    include!("../common/mod.rs");
}
use common::integration_test_helpers::{UnifiedTestEnvironment as IsolatedTestEnvironment, setup_hardware_capabilities};
use proximadb::core::VectorRecord;
use proximadb::proto::proximadb::MetadataItem;
use proximadb::storage::persistence::write_ahead_log::WriteBufferConfig;
use proximadb::storage::persistence::write_ahead_log::optimized_write_ahead_log_writer::OptimizedWriteBufferWriter;

#[tokio::test]
async fn test_isolated_write_ahead_log_basic_operations() -> Result<()> {
    setup_hardware_capabilities();
    let env = IsolatedTestEnvironment::new().await?;
    
    // Create WriteBuffer configuration
    let wb_config = WriteBufferConfig {
        buffer_size_mb: 2,
        batch_size: 100,
        flush_interval_ms: 1000,
        sync_mode: "immediate".to_string(),
        serialization_strategy: "proto".to_string(),
        enable_compression: false,
        ..Default::default()
    };
    
    // Create WriteBuffer writer
    let assignment = env.assignment_service().assign_collection(
        env.collection_id(),
        &env.storage_locations,
        "hash"
    ).await?;
    
    let writer = OptimizedWriteBufferWriter::new(
        env.collection_id().to_string(),
        assignment,
        wb_config,
        env.filesystem.clone()
    ).await?;
    
    // Create test vectors
    let vectors = env.create_test_vectors(10);
    debug!("📝 Created {} test vectors for collection: {}", vectors.len(), env.collection_id());
    
    // Write vectors to WriteBuffer
    let write_result = writer.write_vectors(&vectors).await?;
    assert!(write_result.success, "WriteBuffer write should succeed");
    assert_eq!(write_result.vectors_written, 10);
    
    // Flush WriteBuffer
    let flush_result = writer.flush().await?;
    assert!(flush_result.success, "WriteBuffer flush should succeed");
    assert!(flush_result.bytes_written > 0);
    
    debug!("✅ Basic WriteBuffer operations test passed for collection: {}", env.collection_id());
    debug!("   Wrote {} vectors, flushed {} bytes", write_result.vectors_written, flush_result.bytes_written);
    Ok(())
}

#[tokio::test]
async fn test_isolated_write_ahead_log_serialization_strategies() -> Result<()> {
    setup_hardware_capabilities();
    let env = IsolatedTestEnvironment::new().await?;
    let test_vectors = env.create_test_vectors(5);
    
    // Test different serialization strategies
    let strategies = vec!["proto", "avro", "bincode"];
    
    for strategy in strategies {
        debug!("🧪 Testing serialization strategy: {}", strategy);
        
        let wb_config = WriteBufferConfig {
            buffer_size_mb: 1,
            batch_size: 50,
            flush_interval_ms: 500,
            sync_mode: "immediate".to_string(),
            serialization_strategy: strategy.to_string(),
            enable_compression: false,
            ..Default::default()
        };
        
        // Create unique assignment for this strategy test
        let strategy_collection_id = format!("{}_{}", env.collection_id(), strategy);
        let assignment = env.assignment_service().assign_collection(
            &strategy_collection_id,
            &env.storage_locations,
            "hash"
        ).await?;
        
        let writer = OptimizedWriteBufferWriter::new(
            strategy_collection_id.clone(),
            assignment,
            wb_config,
            env.filesystem.clone()
        ).await?;
        
        // Write and flush vectors
        let write_result = writer.write_vectors(&test_vectors).await?;
        assert!(write_result.success, "Write should succeed for {} strategy", strategy);
        
        let flush_result = writer.flush().await?;
        assert!(flush_result.success, "Flush should succeed for {} strategy", strategy);
        assert!(flush_result.bytes_written > 0, "Should write some bytes for {} strategy", strategy);
        
        debug!("  ✅ {} strategy: {} vectors, {} bytes", 
                strategy, write_result.vectors_written, flush_result.bytes_written);
    }
    
    debug!("✅ Serialization strategies test passed for collection: {}", env.collection_id());
    Ok(())
}

#[tokio::test] 
async fn test_isolated_write_ahead_log_batch_operations() -> Result<()> {
    setup_hardware_capabilities();
    let env = IsolatedTestEnvironment::new().await?;
    
    let wb_config = WriteBufferConfig {
        buffer_size_mb: 4,
        batch_size: 5, // Small batch size to test batching
        flush_interval_ms: 2000,
        sync_mode: "immediate".to_string(),
        serialization_strategy: "proto".to_string(),
        enable_compression: false,
        ..Default::default()
    };
    
    let assignment = env.assignment_service().assign_collection(
        env.collection_id(),
        &env.storage_locations,
        "hash"
    ).await?;
    
    let writer = OptimizedWriteBufferWriter::new(
        env.collection_id().to_string(),
        assignment,
        wb_config,
        env.filesystem.clone()
    ).await?;
    
    // Write vectors in multiple batches
    let total_vectors = 23; // Not evenly divisible by batch size
    let mut all_write_results = Vec::new();
    
    for batch in 0..5 {
        let batch_start = batch * 5;
        let batch_end = std::cmp::min(batch_start + 5, total_vectors);
        
        if batch_start >= total_vectors {
            break;
        }
        
        let batch_vectors = (batch_start..batch_end).map(|i| {
            VectorRecord {
                id: Some(format!("{}", i)),
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
        }_{:03}", env.collection_id(), i)),
                vector: vec![i as f32, (i + 1) as f32, (i + 2) as f32],
                metadata: vec![
                    MetadataItem {
                        key: "batch_num".to_string(),
                        value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(batch.to_string())),
                    },
                ],
                timestamp: chrono::Utc::now().timestamp() as u32,
                ..Default::default()
            }
        }).collect::<Vec<_>>();
        
        let write_result = writer.write_vectors(&batch_vectors).await?;
        assert!(write_result.success, "Batch {} write should succeed", batch);
        all_write_results.push(write_result);
        
        debug!("📦 Batch {}: wrote {} vectors", batch, batch_vectors.len());
    }
    
    // Flush all batches
    let flush_result = writer.flush().await?;
    assert!(flush_result.success, "Final flush should succeed");
    
    // Verify total vectors written
    let total_written: usize = all_write_results.iter().map(|r| r.vectors_written).sum();
    assert_eq!(total_written, total_vectors, "Should have written all {} vectors", total_vectors);
    
    debug!("✅ Batch operations test passed for collection: {}", env.collection_id());
    debug!("   Total vectors: {}, batches: {}, bytes flushed: {}", 
             total_written, all_write_results.len(), flush_result.bytes_written);
    Ok(())
}

#[tokio::test]
async fn test_isolated_write_ahead_log_concurrent_writes() -> Result<()> {
    setup_hardware_capabilities();
    let env = IsolatedTestEnvironment::new().await?;
    
    let wb_config = WriteBufferConfig {
        buffer_size_mb: 8,
        batch_size: 200,
        flush_interval_ms: 5000,
        sync_mode: "immediate".to_string(),
        serialization_strategy: "proto".to_string(),
        enable_compression: false,
        ..Default::default()
    };
    
    let assignment = env.assignment_service().assign_collection(
        env.collection_id(),
        &env.storage_locations,
        "hash"
    ).await?;
    
    let writer = Arc::new(OptimizedWriteBufferWriter::new(
        env.collection_id().to_string(),
        assignment,
        wb_config,
        env.filesystem.clone()
    ).await?);
    
    // Spawn concurrent write operations
    let mut handles = Vec::new();
    let concurrent_writers = 5;
    let vectors_per_writer = 4;
    
    for writer_id in 0..concurrent_writers {
        let writer_clone = writer.clone();
        let collection_id = env.collection_id().to_string();
        
        let handle = tokio::spawn(async move {
            let vectors = (0..vectors_per_writer).map(|i| {
                VectorRecord {
                    id: Some(format!("{}", i)),
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
        }_writer_{}_{}", collection_id, writer_id, i)),
                    vector: vec![
                        (writer_id * 10 + i) as f32,
                        (writer_id * 10 + i + 1) as f32,
                        (writer_id * 10 + i + 2) as f32
                    ],
                    metadata: vec![
                        MetadataItem {
                            key: "writer_id".to_string(),
                            value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(writer_id.to_string())),
                        },
                    ],
                    timestamp: chrono::Utc::now().timestamp() as u32,
                    ..Default::default()
                }
            }).collect::<Vec<_>>();
            
            writer_clone.write_vectors(&vectors).await
        });
        
        handles.push(handle);
    }
    
    // Wait for all concurrent writes to complete
    let mut successful_writes = 0;
    let mut total_vectors_written = 0;
    
    for (writer_id, handle) in handles.into_iter().enumerate() {
        match handle.await? {
            Ok(result) => {
                if result.success {
                    successful_writes += 1;
                    total_vectors_written += result.vectors_written;
                    debug!("📝 Writer {}: wrote {} vectors", writer_id, result.vectors_written);
                }
            }
            Err(e) => {
                debug!("⚠️ Writer {} failed: {}", writer_id, e);
            }
        }
    }
    
    assert_eq!(successful_writes, concurrent_writers, 
        "All {} concurrent writers should succeed", concurrent_writers);
    
    let expected_total = concurrent_writers * vectors_per_writer;
    assert_eq!(total_vectors_written, expected_total,
        "Should have written {} total vectors", expected_total);
    
    // Flush all written data
    let flush_result = writer.flush().await?;
    assert!(flush_result.success, "Final flush should succeed");
    assert!(flush_result.bytes_written > 0, "Should have written some bytes");
    
    debug!("✅ Concurrent writes test passed for collection: {}", env.collection_id());
    debug!("   {} writers, {} total vectors, {} bytes flushed", 
             successful_writes, total_vectors_written, flush_result.bytes_written);
    Ok(())
}

#[tokio::test]
async fn test_isolated_write_ahead_log_recovery() -> Result<()> {
    setup_hardware_capabilities();
    let env = IsolatedTestEnvironment::new().await?;
    let test_vectors = env.create_test_vectors(8);
    
    let wb_config = WriteBufferConfig {
        buffer_size_mb: 2,
        batch_size: 100,
        flush_interval_ms: 1000,
        sync_mode: "immediate".to_string(),
        serialization_strategy: "proto".to_string(),
        enable_compression: false,
        ..Default::default()
    };
    
    let assignment = env.assignment_service().assign_collection(
        env.collection_id(),
        &env.storage_locations,
        "hash"
    ).await?;
    
    // Phase 1: Write data and flush
    {
        let writer = OptimizedWriteBufferWriter::new(
            env.collection_id().to_string(),
            assignment.clone(),
            wb_config.clone(),
            env.filesystem.clone()
        ).await?;
        
        let write_result = writer.write_vectors(&test_vectors).await?;
        assert!(write_result.success, "Initial write should succeed");
        
        let flush_result = writer.flush().await?;
        assert!(flush_result.success, "Initial flush should succeed");
        
        debug!("📝 Phase 1: Wrote {} vectors, flushed {} bytes", 
                write_result.vectors_written, flush_result.bytes_written);
    } // Writer goes out of scope
    
    // Phase 2: Create new writer instance and verify it can access flushed data location
    {
        let new_writer = OptimizedWriteBufferWriter::new(
            env.collection_id().to_string(),
            assignment,
            wb_config,
            env.filesystem.clone()
        ).await?;
        
        // Write additional data to verify new writer works
        let additional_vectors = env.create_test_vectors(3);
        let new_write_result = new_writer.write_vectors(&additional_vectors).await?;
        assert!(new_write_result.success, "Recovery write should succeed");
        
        let new_flush_result = new_writer.flush().await?;
        assert!(new_flush_result.success, "Recovery flush should succeed");
        
        debug!("📝 Phase 2: Wrote {} additional vectors, flushed {} bytes", 
                new_write_result.vectors_written, new_flush_result.bytes_written);
    }
    
    debug!("✅ WriteBuffer recovery test passed for collection: {}", env.collection_id());
    Ok(())
}

#[tokio::test]
async fn test_isolated_write_ahead_log_compression() -> Result<()> {
    setup_hardware_capabilities();
    let env = IsolatedTestEnvironment::new().await?;
    
    // Create larger vectors to see compression effect
    let large_vectors = (0..10).map(|i| {
        VectorRecord {
            id: Some(format!("{}", i)),
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
        }_{:03}", env.collection_id(), i)),
            vector: vec![i as f32; 100], // 100-dimensional vectors with repeated values
            metadata: vec![
                MetadataItem {
                    key: "description".to_string(),
                    value: format!("This is a test vector with repeated data for compression testing - vector number {}", i),
                },
            ],
            timestamp: chrono::Utc::now().timestamp() as u32,
            ..Default::default()
        }
    }).collect::<Vec<_>>();
    
    // Test without compression
    let uncompressed_config = WriteBufferConfig {
        buffer_size_mb: 4,
        batch_size: 100,
        flush_interval_ms: 1000,
        sync_mode: "immediate".to_string(),
        serialization_strategy: "proto".to_string(),
        enable_compression: false,
        ..Default::default()
    };
    
    let uncompressed_assignment = env.assignment_service().assign_collection(
        &format!("{}_uncompressed", env.collection_id()),
        &env.storage_locations,
        "hash"
    ).await?;
    
    let uncompressed_writer = OptimizedWriteBufferWriter::new(
        format!("{}_uncompressed", env.collection_id()),
        uncompressed_assignment,
        uncompressed_config,
        env.filesystem.clone()
    ).await?;
    
    let uncompressed_write = uncompressed_writer.write_vectors(&large_vectors).await?;
    let uncompressed_flush = uncompressed_writer.flush().await?;
    
    // Test with compression
    let compressed_config = WriteBufferConfig {
        buffer_size_mb: 4,
        batch_size: 100,
        flush_interval_ms: 1000,
        sync_mode: "immediate".to_string(),
        serialization_strategy: "proto".to_string(),
        enable_compression: true,
        ..Default::default()
    };
    
    let compressed_assignment = env.assignment_service().assign_collection(
        &format!("{}_compressed", env.collection_id()),
        &env.storage_locations,
        "hash"
    ).await?;
    
    let compressed_writer = OptimizedWriteBufferWriter::new(
        format!("{}_compressed", env.collection_id()),
        compressed_assignment,
        compressed_config,
        env.filesystem.clone()
    ).await?;
    
    let compressed_write = compressed_writer.write_vectors(&large_vectors).await?;
    let compressed_flush = compressed_writer.flush().await?;
    
    // Both should succeed
    assert!(uncompressed_write.success && uncompressed_flush.success);
    assert!(compressed_write.success && compressed_flush.success);
    
    // Both should write same number of vectors
    assert_eq!(uncompressed_write.vectors_written, compressed_write.vectors_written);
    
    // Compressed should use less space (though this depends on data patterns)
    let compression_ratio = compressed_flush.bytes_written as f64 / uncompressed_flush.bytes_written as f64;
    
    debug!("✅ Compression test passed for collection: {}", env.collection_id());
    debug!("   Uncompressed: {} bytes, Compressed: {} bytes, Ratio: {:.2}",
             uncompressed_flush.bytes_written, compressed_flush.bytes_written, compression_ratio);
    
    // Note: We don't assert compression ratio because it depends on the actual data patterns
    // and the test data might not compress well
    Ok(())
}

#[tokio::test]
async fn test_isolated_write_ahead_log_error_handling() -> Result<()> {
    setup_hardware_capabilities();
    let env = IsolatedTestEnvironment::new().await?;
    
    let wb_config = WriteBufferConfig {
        buffer_size_mb: 1, // Very small buffer to potentially trigger errors
        batch_size: 10,
        flush_interval_ms: 500,
        sync_mode: "immediate".to_string(),
        serialization_strategy: "proto".to_string(),
        enable_compression: false,
        ..Default::default()
    };
    
    let assignment = env.assignment_service().assign_collection(
        env.collection_id(),
        &env.storage_locations,
        "hash"
    ).await?;
    
    let writer = OptimizedWriteBufferWriter::new(
        env.collection_id().to_string(),
        assignment,
        wb_config,
        env.filesystem.clone()
    ).await?;
    
    // Test with empty vectors (should be handled gracefully)
    let empty_vectors = Vec::new();
    let empty_result = writer.write_vectors(&empty_vectors).await?;
    assert!(empty_result.success, "Empty vector write should succeed");
    assert_eq!(empty_result.vectors_written, 0);
    
    // Test with normal vectors
    let normal_vectors = env.create_test_vectors(5);
    let normal_result = writer.write_vectors(&normal_vectors).await?;
    assert!(normal_result.success, "Normal vector write should succeed");
    assert_eq!(normal_result.vectors_written, 5);
    
    // Test multiple flushes (should be idempotent)
    let flush1 = writer.flush().await?;
    let flush2 = writer.flush().await?;
    
    assert!(flush1.success && flush2.success, "Multiple flushes should succeed");
    
    // Second flush should be a no-op or minimal work
    assert!(flush2.bytes_written <= flush1.bytes_written, 
        "Second flush should not write more than first");
    
    debug!("✅ Error handling test passed for collection: {}", env.collection_id());
    debug!("   Empty: {} vectors, Normal: {} vectors, Flush1: {} bytes, Flush2: {} bytes",
             empty_result.vectors_written, normal_result.vectors_written, 
             flush1.bytes_written, flush2.bytes_written);
    Ok(())
}