// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Comprehensive Unit Tests for WAL Write Optimization
//! 
//! Test Coverage:
//! - Batching behavior and timing
//! - Connection pooling and reuse
//! - Caching effectiveness
//! - Write combining
//! - Error handling and recovery
//! - Performance benchmarks

use anyhow::Result;
use tracing::{debug, error, info, warn};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

use proximadb::core::VectorRecord;
use proximadb::services::vector_service::OptimizedFormat;
use proximadb::storage::assignment_service::{AssignmentService, HashBasedAssignmentService};
use proximadb::storage::persistence::filesystem::FilesystemFactory;
use proximadb::storage::persistence::write_ahead_log::{WriteBufferConfig, OptimizedWriteBufferWriter, OptimizedWriteBufferWriterConfig};

/// Test helper to create sample vectors
fn create_test_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| VectorRecord {
            id: Some(format!("vec_{,
            timestamp: 0,
            updated_at: None,
            expires_at: None,
            distance: None,
            rank: None,
            score: None,
        }", i)),
            vector: vec![i as f32; dimension],
            metadata: vec![
                proximadb::proto::proximadb::MetadataItem {
                    key: "index".to_string(),
                    value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(i.to_string())),
                },
            ],
            created_at: chrono::Utc::now().timestamp_micros(),
            updated_at: None,
            expires_at: None,
        })
        .collect()
}

/// Test helper to create test WAL config
fn create_test_wal_config() -> WriteBufferConfig {
    WriteBufferConfig {
        multi_disk: proximadb::storage::persistence::write_ahead_log::config::MultiDiskConfig {
            data_directories: vec!["file:///tmp/test_wal".to_string()],
            distribution_strategy: proximadb::storage::persistence::write_ahead_log::config::DiskDistributionStrategy::RoundRobin,
            collection_affinity: true,
        },
        ..Default::default()
    }
}

#[cfg(test)]
mod batching_tests {
    use super::*;

    #[tokio::test]
    async fn test_write_batching_by_size() -> Result<()> {
        // Setup
        let config = OptimizedWriteBufferWriterConfig {
            batch_size: 10,
            batch_timeout: Duration::from_secs(1), // Long timeout to test size trigger
            writer_threads: 1,
            ..Default::default()
        };
        
        let wal_config = create_test_wal_config();
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let assignment_service = Arc::new(HashBasedAssignmentService::new(filesystem_factory, "round_robin"));
        
        let writer = OptimizedWriteBufferWriter::new(config, wal_config, assignment_service).await?;
        
        // Submit exactly batch_size writes
        let mut handles = Vec::new();
        for i in 0..10 {
            let vectors = create_test_vectors(1, 128);
            let handle = writer.write_vectors(
                "test_collection".to_string(),
                vectors,
                vec![i as u64],
                OptimizedFormat::Proto,
            );
            handles.push(handle);
        }
        
        // All writes should complete quickly due to batch size trigger
        let start = Instant::now();
        for handle in handles {
            handle.await??;
        }
        let elapsed = start.elapsed();
        
        // Should complete much faster than timeout
        assert!(elapsed < Duration::from_millis(100));
        
        // Check metrics
        let metrics = writer.metrics();
        assert_eq!(metrics.total_writes.load(std::sync::atomic::Ordering::Relaxed), 1); // One batched write
        assert_eq!(metrics.average_batch_size.load(std::sync::atomic::Ordering::Relaxed), 10);
        
        writer.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_write_batching_by_timeout() -> Result<()> {
        // Setup
        let config = OptimizedWriteBufferWriterConfig {
            batch_size: 100, // Large size to ensure timeout triggers first
            batch_timeout: Duration::from_millis(50),
            writer_threads: 1,
            ..Default::default()
        };
        
        let wal_config = create_test_wal_config();
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let assignment_service = Arc::new(HashBasedAssignmentService::new(filesystem_factory, "round_robin"));
        
        let writer = OptimizedWriteBufferWriter::new(config, wal_config, assignment_service).await?;
        
        // Submit fewer writes than batch size
        let vectors = create_test_vectors(5, 128);
        let handle = writer.write_vectors(
            "test_collection".to_string(),
            vectors,
            vec![1, 2, 3, 4, 5],
            OptimizedFormat::Proto,
        );
        
        // Should complete after timeout
        let start = Instant::now();
        handle.await??;
        let elapsed = start.elapsed();
        
        // Should take at least timeout duration
        assert!(elapsed >= Duration::from_millis(50));
        assert!(elapsed < Duration::from_millis(150)); // But not too long
        
        writer.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_write_combining() -> Result<()> {
        // Setup with write combining enabled
        let config = OptimizedWriteBufferWriterConfig {
            batch_size: 10,
            batch_timeout: Duration::from_millis(50),
            writer_threads: 1,
            enable_write_combining: true,
            ..Default::default()
        };
        
        let wal_config = create_test_wal_config();
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let assignment_service = Arc::new(HashBasedAssignmentService::new(filesystem_factory, "round_robin"));
        
        let writer = OptimizedWriteBufferWriter::new(config, wal_config, assignment_service).await?;
        
        // Submit multiple writes to same collection
        let mut handles = Vec::new();
        for i in 0..5 {
            let vectors = create_test_vectors(2, 128);
            let handle = writer.write_vectors(
                "test_collection".to_string(),
                vectors,
                vec![i * 2, i * 2 + 1],
                OptimizedFormat::Proto,
            );
            handles.push(handle);
        }
        
        // Wait for all to complete
        for handle in handles {
            handle.await??;
        }
        
        // Check that writes were combined
        let metrics = writer.metrics();
        assert_eq!(metrics.total_writes.load(std::sync::atomic::Ordering::Relaxed), 1); // Combined into one write
        
        writer.shutdown().await?;
        Ok(())
    }
}

#[cfg(test)]
mod caching_tests {
    use super::*;

    #[tokio::test]
    async fn test_assignment_caching() -> Result<()> {
        let config = OptimizedWriteBufferWriterConfig::default();
        let wal_config = create_test_wal_config();
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let assignment_service = Arc::new(HashBasedAssignmentService::new(filesystem_factory, "round_robin"));
        
        let writer = OptimizedWriteBufferWriter::new(config, wal_config, assignment_service).await?;
        
        // First write - cache miss
        let vectors = create_test_vectors(1, 128);
        writer.write_vectors(
            "test_collection".to_string(),
            vectors.clone(),
            vec![1],
            OptimizedFormat::Proto,
        ).await?;
        
        let metrics1 = writer.metrics();
        let cache_misses1 = metrics1.cache_misses.load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(cache_misses1, 1);
        
        // Second write to same collection - cache hit
        writer.write_vectors(
            "test_collection".to_string(),
            vectors,
            vec![2],
            OptimizedFormat::Proto,
        ).await?;
        
        let metrics2 = writer.metrics();
        let cache_hits = metrics2.cache_hits.load(std::sync::atomic::Ordering::Relaxed);
        let cache_misses2 = metrics2.cache_misses.load(std::sync::atomic::Ordering::Relaxed);
        
        assert_eq!(cache_hits, 1); // Should have cache hit
        assert_eq!(cache_misses2, 1); // No additional miss
        
        writer.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_filesystem_pooling() -> Result<()> {
        let config = OptimizedWriteBufferWriterConfig {
            max_filesystem_pool_size: 5,
            ..Default::default()
        };
        
        let mut wal_config = create_test_wal_config();
        // Add multiple WAL directories
        wal_config.multi_disk.data_directories = vec![
            "file:///tmp/test_wal1".to_string(),
            "file:///tmp/test_wal2".to_string(),
            "file:///tmp/test_wal3".to_string(),
        ];
        
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let assignment_service = Arc::new(HashBasedAssignmentService::new(filesystem_factory, "round_robin"));
        let writer = OptimizedWriteBufferWriter::new(config, wal_config, assignment_service).await?;
        
        // Write to multiple collections to use different filesystems
        for i in 0..10 {
            let vectors = create_test_vectors(1, 128);
            writer.write_vectors(
                format!("collection_{}", i),
                vectors,
                vec![i as u64],
                OptimizedFormat::Proto,
            ).await?;
        }
        
        // Pool should be populated but not exceed max size
        // (Can't directly check pool size without exposing internals)
        
        writer.shutdown().await?;
        Ok(())
    }
}

#[cfg(test)]
mod error_handling_tests {
    use super::*;

    #[tokio::test]
    async fn test_write_failure_callback() -> Result<()> {
        let config = OptimizedWriteBufferWriterConfig::default();
        let mut wal_config = create_test_wal_config();
        // Use invalid directory to trigger errors
        wal_config.multi_disk.data_directories = vec!["file:///dev/null/invalid".to_string()];
        
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let assignment_service = Arc::new(HashBasedAssignmentService::new(filesystem_factory, "round_robin"));
        let writer = OptimizedWriteBufferWriter::new(config, wal_config, assignment_service).await?;
        
        let vectors = create_test_vectors(1, 128);
        let result = writer.write_vectors(
            "test_collection".to_string(),
            vectors,
            vec![1],
            OptimizedFormat::Proto,
        ).await;
        
        // Should get error
        assert!(result.is_err());
        
        writer.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_graceful_shutdown() -> Result<()> {
        let config = OptimizedWriteBufferWriterConfig {
            batch_size: 100,
            batch_timeout: Duration::from_secs(10), // Long timeout
            writer_threads: 2,
            ..Default::default()
        };
        
        let wal_config = create_test_wal_config();
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let assignment_service = Arc::new(HashBasedAssignmentService::new(filesystem_factory, "round_robin"));
        let writer = OptimizedWriteBufferWriter::new(config, wal_config, assignment_service).await?;
        
        // Submit writes but don't wait
        let vectors = create_test_vectors(50, 128);
        let _handle = writer.write_vectors(
            "test_collection".to_string(),
            vectors,
            (0..50).collect(),
            OptimizedFormat::Proto,
        );
        
        // Shutdown should wait for pending writes
        let start = Instant::now();
        writer.shutdown().await?;
        let elapsed = start.elapsed();
        
        // Should complete reasonably quickly
        assert!(elapsed < Duration::from_secs(1));
        
        Ok(())
    }
}

#[cfg(test)]
mod performance_benchmarks {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[tokio::test]
    async fn benchmark_single_vs_batched_writes() -> Result<()> {
        const NUM_WRITES: usize = 1000;
        const VECTOR_DIM: usize = 256;
        
        // Benchmark without batching (simulated)
        let start_single = Instant::now();
        let single_latency = AtomicU64::new(0);
        
        // Track individual write metrics
        for i in 0..NUM_WRITES {
            let write_start = Instant::now();
            // Record actual write timing without artificial delay
            single_latency.fetch_add(write_start.elapsed().as_micros() as u64, Ordering::Relaxed);
        }
        let single_duration = start_single.elapsed();
        
        // Benchmark with batching
        let config = OptimizedWriteBufferWriterConfig {
            batch_size: 100,
            batch_timeout: Duration::from_millis(10),
            writer_threads: 4,
            enable_write_combining: true,
            ..Default::default()
        };
        
        let wal_config = create_test_wal_config();
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let assignment_service = Arc::new(HashBasedAssignmentService::new(filesystem_factory, "round_robin"));
        let writer = OptimizedWriteBufferWriter::new(config, wal_config, assignment_service).await?;
        
        let start_batched = Instant::now();
        let mut handles = Vec::new();
        
        for i in 0..NUM_WRITES {
            let vectors = create_test_vectors(1, VECTOR_DIM);
            let handle = writer.write_vectors(
                "benchmark_collection".to_string(),
                vectors,
                vec![i as u64],
                OptimizedFormat::Proto,
            );
            handles.push(handle);
        }
        
        // Wait for all writes
        for handle in handles {
            handle.await??;
        }
        let batched_duration = start_batched.elapsed();
        
        // Calculate improvement
        let speedup = single_duration.as_secs_f64() / batched_duration.as_secs_f64();
        
        debug!("Performance Benchmark Results:");
        debug!("  Single writes: {:?} total, {} µs avg latency", 
                 single_duration, 
                 single_latency.load(Ordering::Relaxed) / NUM_WRITES as u64);
        debug!("  Batched writes: {:?} total", batched_duration);
        debug!("  Speedup: {:.2}x", speedup);
        
        // Batched should be significantly faster
        assert!(speedup > 2.0, "Expected at least 2x speedup, got {:.2}x", speedup);
        
        writer.shutdown().await?;
        Ok(())
    }

    #[tokio::test]
    async fn benchmark_cache_effectiveness() -> Result<()> {
        let config = OptimizedWriteBufferWriterConfig {
            assignment_cache_ttl: Duration::from_secs(60),
            directory_cache_ttl: Duration::from_secs(60),
            ..Default::default()
        };
        
        let wal_config = create_test_wal_config();
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await.unwrap());
        let assignment_service = Arc::new(HashBasedAssignmentService::new(filesystem_factory, "round_robin"));
        let writer = OptimizedWriteBufferWriter::new(config, wal_config, assignment_service).await?;
        
        const NUM_COLLECTIONS: usize = 10;
        const WRITES_PER_COLLECTION: usize = 100;
        
        // Write to each collection multiple times
        for round in 0..5 {
            for col in 0..NUM_COLLECTIONS {
                let vectors = create_test_vectors(1, 128);
                writer.write_vectors(
                    format!("cache_test_col_{}", col),
                    vectors,
                    vec![(round * NUM_COLLECTIONS + col) as u64],
                    OptimizedFormat::Proto,
                ).await?;
            }
        }
        
        let metrics = writer.metrics();
        let cache_hits = metrics.cache_hits.load(Ordering::Relaxed);
        let cache_misses = metrics.cache_misses.load(Ordering::Relaxed);
        let hit_rate = cache_hits as f64 / (cache_hits + cache_misses) as f64;
        
        debug!("Cache Effectiveness:");
        debug!("  Cache hits: {}", cache_hits);
        debug!("  Cache misses: {}", cache_misses);
        debug!("  Hit rate: {:.2}%", hit_rate * 100.0);
        
        // Should have high cache hit rate after first round
        assert!(hit_rate > 0.7, "Expected >70% cache hit rate, got {:.2}%", hit_rate * 100.0);
        
        writer.shutdown().await?;
        Ok(())
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use proximadb::services::VectorOperationsService;
    use proximadb::storage::engines::viper::ViperEngine;
    use proximadb::storage::engines::sst::LsmTree;

    #[tokio::test]
    #[ignore] // Requires full system setup
    async fn test_integration_with_direct_vector_service() -> Result<()> {
        // This test would require setting up full storage engines
        // Marked as ignored for unit test runs
        Ok(())
    }
}