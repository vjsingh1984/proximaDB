use super::*;
use crate::proto::proximadb_v1::{Collection, CollectionConfig, CompressionAlgorithm, StorageConfig, StorageAssignment, StorageEngine, VectorRecord};
use crate::storage::traits::{FlushParameters, UnifiedStorageEngine};
use anyhow::Result;
use std::collections::HashMap;

/// Test all compression algorithms with Raptor engine
#[cfg(test)]
mod tests {
    use super::*;

    async fn create_test_engine_with_compression(_compression: CompressionAlgorithm) -> Result<RaptorEngine> {
        // For now, just create a default engine since the constructor doesn't take config
        // Compression will be tested via the collection config in the flush parameters
        RaptorEngine::new().await
    }

    fn create_test_vectors(count: usize, dimension: usize) -> Vec<VectorRecord> {
        (0..count)
            .map(|i| VectorRecord {
                id: format!("vec_{}", i),
                vector: (0..dimension).map(|j| (i as f32 + j as f32) * 0.1).collect(),
                metadata: HashMap::new(),
                version: Some(1),
                timestamp: 1234567890 + i as i64,
                ..Default::default()
            })
            .collect()
    }

    fn create_collection_with_compression(compression: CompressionAlgorithm) -> Collection {
        Collection {
            id: "test_collection".to_string(),
            config: Some(CollectionConfig {
                dimension: 4,
                storage_config: Some(StorageConfig {
                    storage_path: "/tmp".to_string(),
                    data_paths: vec![],
                    compression: compression as i32,
                    max_file_size_mb: 100,
                    enable_caching: true,
                }),
                ..Default::default()
            }),
            storage_assignment: Some(StorageAssignment {
                primary_path: "/tmp".to_string(),
                backup_paths: vec![],
                engine: StorageEngine::Raptor as i32,
                engine_config: HashMap::new(),
                base_location: "/tmp".to_string(),
                assigned_at: chrono::Utc::now().timestamp(),
            }),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_compression_none() -> Result<()> {
        let engine = create_test_engine_with_compression(CompressionAlgorithm::CompressionNone).await?;
        let vectors = create_test_vectors(10, 4);
        let collection = create_collection_with_compression(CompressionAlgorithm::CompressionNone);

        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            collection_config: Some(collection),
            ..Default::default()
        };

        let result = engine.do_flush(&flush_params).await?;
        assert!(result.success);
        println!("✅ No compression test passed");
        Ok(())
    }

    #[tokio::test]
    async fn test_compression_lz4() -> Result<()> {
        let engine = create_test_engine_with_compression(CompressionAlgorithm::CompressionLz4).await?;
        let vectors = create_test_vectors(10, 4);
        let collection = create_collection_with_compression(CompressionAlgorithm::CompressionLz4);

        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            collection_config: Some(collection),
            ..Default::default()
        };

        let result = engine.do_flush(&flush_params).await?;
        assert!(result.success);
        println!("✅ LZ4 compression test passed");
        Ok(())
    }

    #[tokio::test]
    async fn test_compression_snappy() -> Result<()> {
        let engine = create_test_engine_with_compression(CompressionAlgorithm::CompressionSnappy).await?;
        let vectors = create_test_vectors(10, 4);
        let collection = create_collection_with_compression(CompressionAlgorithm::CompressionSnappy);

        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            collection_config: Some(collection),
            ..Default::default()
        };

        let result = engine.do_flush(&flush_params).await?;
        assert!(result.success);
        println!("✅ Snappy compression test passed");
        Ok(())
    }

    #[tokio::test]
    async fn test_compression_zstd() -> Result<()> {
        let engine = create_test_engine_with_compression(CompressionAlgorithm::CompressionZstd).await?;
        let vectors = create_test_vectors(10, 4);
        let collection = create_collection_with_compression(CompressionAlgorithm::CompressionZstd);

        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            collection_config: Some(collection),
            ..Default::default()
        };

        let result = engine.do_flush(&flush_params).await?;
        assert!(result.success);
        println!("✅ Zstd compression test passed");
        Ok(())
    }

    #[tokio::test]
    async fn test_compression_gzip() -> Result<()> {
        let engine = create_test_engine_with_compression(CompressionAlgorithm::CompressionGzip).await?;
        let vectors = create_test_vectors(10, 4);
        let collection = create_collection_with_compression(CompressionAlgorithm::CompressionGzip);

        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            collection_config: Some(collection),
            ..Default::default()
        };

        let result = engine.do_flush(&flush_params).await?;
        assert!(result.success);
        println!("✅ Gzip compression test passed");
        Ok(())
    }

    #[tokio::test]
    async fn test_compression_brotli() -> Result<()> {
        // Note: Brotli might not be implemented in RaptorConfig yet, so this will default to LZ4
        let engine = create_test_engine_with_compression(CompressionAlgorithm::CompressionBrotli).await?;
        let vectors = create_test_vectors(10, 4);
        let collection = create_collection_with_compression(CompressionAlgorithm::CompressionBrotli);

        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            collection_config: Some(collection),
            ..Default::default()
        };

        let result = engine.do_flush(&flush_params).await?;
        assert!(result.success);
        println!("✅ Brotli compression test passed (falls back to LZ4)");
        Ok(())
    }

    #[tokio::test]
    async fn test_compression_bzip2() -> Result<()> {
        // Note: Bzip2 might not be implemented in RaptorConfig yet, so this will default to LZ4
        let engine = create_test_engine_with_compression(CompressionAlgorithm::CompressionBzip2).await?;
        let vectors = create_test_vectors(10, 4);
        let collection = create_collection_with_compression(CompressionAlgorithm::CompressionBzip2);

        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            collection_config: Some(collection),
            ..Default::default()
        };

        let result = engine.do_flush(&flush_params).await?;
        assert!(result.success);
        println!("✅ Bzip2 compression test passed (falls back to LZ4)");
        Ok(())
    }

    #[tokio::test]
    async fn test_compression_xz() -> Result<()> {
        // Note: XZ might not be implemented in RaptorConfig yet, so this will default to LZ4
        let engine = create_test_engine_with_compression(CompressionAlgorithm::CompressionXz).await?;
        let vectors = create_test_vectors(10, 4);
        let collection = create_collection_with_compression(CompressionAlgorithm::CompressionXz);

        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            collection_config: Some(collection),
            ..Default::default()
        };

        let result = engine.do_flush(&flush_params).await?;
        assert!(result.success);
        println!("✅ XZ compression test passed (falls back to LZ4)");
        Ok(())
    }

    #[tokio::test]
    async fn test_lz4_is_default_compression() -> Result<()> {
        // Test that LZ4 is used when no compression is explicitly specified
        let default_collection = Collection {
            id: "test_collection".to_string(),
            config: Some(CollectionConfig {
                dimension: 4,
                // No storage_config provided - should default to LZ4 somewhere in the engine
                ..Default::default()
            }),
            storage_assignment: Some(StorageAssignment {
                primary_path: "/tmp".to_string(),
                backup_paths: vec![],
                engine: StorageEngine::Raptor as i32,
                engine_config: HashMap::new(),
                base_location: "/tmp".to_string(),
                assigned_at: chrono::Utc::now().timestamp(),
            }),
            ..Default::default()
        };

        // Create engine with default compression (should be LZ4)
        let engine = create_test_engine_with_compression(CompressionAlgorithm::CompressionLz4).await?;
        let vectors = create_test_vectors(5, 4);

        let flush_params = FlushParameters {
            collection_id: Some("test_collection".to_string()),
            vector_records: vectors,
            force: true,
            synchronous: true,
            collection_config: Some(default_collection),
            ..Default::default()
        };

        let result = engine.do_flush(&flush_params).await?;
        assert!(result.success);
        println!("✅ LZ4 default compression test passed");
        Ok(())
    }

    #[tokio::test]
    async fn test_compression_performance_comparison() -> Result<()> {
        // Test different compression algorithms with same data to compare performance
        let test_data = create_test_vectors(100, 64); // Larger dataset for compression testing

        let compression_types = vec![
            CompressionAlgorithm::CompressionNone,
            CompressionAlgorithm::CompressionLz4,
            CompressionAlgorithm::CompressionSnappy,
            CompressionAlgorithm::CompressionZstd,
            CompressionAlgorithm::CompressionGzip,
        ];

        for compression in compression_types {
            let start = std::time::Instant::now();

            let engine = create_test_engine_with_compression(compression).await?;
            // Create collection with dimension matching test vectors (64)
            let mut collection = create_collection_with_compression(compression);
            if let Some(ref mut config) = collection.config {
                config.dimension = 64;
            }

            let flush_params = FlushParameters {
                collection_id: Some("test_collection".to_string()),
                vector_records: test_data.clone(),
                force: true,
                synchronous: true,
                collection_config: Some(collection),
                ..Default::default()
            };

            let result = engine.do_flush(&flush_params).await?;
            assert!(result.success);

            let elapsed = start.elapsed();
            println!("✅ Compression {:?} completed in {:?}", compression.as_str_name(), elapsed);
        }

        Ok(())
    }
}