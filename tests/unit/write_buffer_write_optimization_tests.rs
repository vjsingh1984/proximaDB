//! Unit Tests for WAL Write Optimization
//!
//! Tests the actual OptimizedWriteBufferWriter implementation
//! that exists in the production code.

use anyhow::Result;
use std::sync::Arc;
use tempfile::TempDir;

use proximadb::proto::proximadb_v1::{VectorRecord, SqlValue, sql_value};
use proximadb::storage::persistence::filesystem::FilesystemFactory;
use proximadb::storage::persistence::write_ahead_log::config::{
    WALConfig, WriteBufferStrategyType, MemTableConfig, MultiDiskConfig,
    CompressionConfig as WALCompressionConfig, PerformanceConfig, DiskDistributionStrategy
};
use proximadb::storage::persistence::write_ahead_log::optimized_write_buffer_writer::OptimizedWriteBufferWriter;

/// Test helper to create sample vectors
fn create_test_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
    (0..count)
        .map(|i| {
            let mut metadata = std::collections::HashMap::new();
            metadata.insert("index".to_string(), SqlValue {
                value: Some(sql_value::Value::StringValue(i.to_string())),
            });

            VectorRecord {
                id: format!("vec_{}", i),
                vector: vec![i as f32; dimension],
                metadata,
                timestamp: chrono::Utc::now().timestamp(),
                updated_at: Some(chrono::Utc::now().timestamp()),
                expires_at: None,
                version: Some(1),
                quantized_vector: vec![],
                source: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod wal_writer_tests {
    use super::*;

    fn create_test_wal_config(temp_dir: &TempDir) -> WALConfig {
        WALConfig {
            strategy_type: WriteBufferStrategyType::default(),
            memtable: MemTableConfig::default(),
            multi_disk: MultiDiskConfig {
                data_directories: vec![format!("file://{}/wal", temp_dir.path().display())],
                distribution_strategy: DiskDistributionStrategy::RoundRobin,
                collection_affinity: false,
            },
            compression: WALCompressionConfig::default(),
            performance: PerformanceConfig::default(),
            enable_mvcc: false,
            enable_ttl: false,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_optimized_wal_writer_creation() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let config = Arc::new(create_test_wal_config(&temp_dir));
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await?);

        // Create the actual writer from production code
        let writer = OptimizedWriteBufferWriter::new(config, filesystem_factory).await?;

        // Test basic write operation
        let vectors = create_test_vectors(10, 128);
        let result = writer.write_vectors(
            "test_collection",
            vectors,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
            format!("file://{}", temp_dir.path().display()),
        ).await;

        assert!(result.is_ok(), "Write operation should succeed");
        Ok(())
    }

    #[tokio::test]
    async fn test_wal_batch_writes() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let config = Arc::new(create_test_wal_config(&temp_dir));
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let writer = Arc::new(OptimizedWriteBufferWriter::new(config, filesystem_factory).await?);

        // Write multiple batches concurrently
        let mut handles = Vec::new();
        for batch in 0..5 {
            let vectors = create_test_vectors(20, 128);
            let sequences: Vec<u64> = (batch * 20..(batch + 1) * 20).collect();
            let base_location = format!("file://{}", temp_dir.path().display());
            let writer_ref = writer.clone();

            let handle = tokio::spawn(async move {
                writer_ref.write_vectors(
                    "test_collection",
                    vectors,
                    sequences,
                    base_location,
                ).await
            });
            handles.push(handle);
        }

        // Wait for all writes to complete
        for handle in handles {
            let result = handle.await??;
            assert!(!result.is_empty(), "Should return WAL file path");
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_wal_concurrent_writes() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let config = Arc::new(create_test_wal_config(&temp_dir));
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let writer = Arc::new(OptimizedWriteBufferWriter::new(config, filesystem_factory).await?);

        // Concurrent writes to multiple collections
        let mut handles = Vec::new();
        for collection_id in 0..10 {
            let writer_clone = writer.clone();
            let base_location = format!("file://{}", temp_dir.path().display());

            let handle = tokio::spawn(async move {
                let vectors = create_test_vectors(50, 128);
                let sequences: Vec<u64> = (0..50).collect();

                writer_clone.write_vectors(
                    &format!("collection_{}", collection_id),
                    vectors,
                    sequences,
                    base_location,
                ).await
            });
            handles.push(handle);
        }

        // All writes should succeed
        for handle in handles {
            let result = handle.await??;
            assert!(!result.is_empty(), "Each write should return a WAL file path");
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_wal_large_batch() -> Result<()> {
        let temp_dir = TempDir::new()?;

        // Create config with adjusted performance settings for large batch
        let mut config = create_test_wal_config(&temp_dir);
        config.performance.memory_flush_size_bytes = 500 * 1024 * 1024; // 500 MB
        config.performance.batch_threshold = 10000; // Use batch_threshold field

        let config = Arc::new(config);
        let filesystem_factory = Arc::new(FilesystemFactory::new(Default::default()).await?);
        let writer = OptimizedWriteBufferWriter::new(config, filesystem_factory).await?;

        // Write a large batch
        let vectors = create_test_vectors(5000, 256);
        let sequences: Vec<u64> = (0..5000).collect();

        let result = writer.write_vectors(
            "large_collection",
            vectors,
            sequences,
            format!("file://{}", temp_dir.path().display()),
        ).await?;

        assert!(!result.is_empty(), "Large batch should be written successfully");
        Ok(())
    }
}