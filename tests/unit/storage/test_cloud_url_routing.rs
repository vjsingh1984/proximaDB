//! Unit tests for cloud URL routing in WriteBufferBatchStrategy
//!
//! Tests comprehensive URL validation, parsing, and routing for different cloud providers.
//!
//! NOTE: These tests are disabled as they use obsolete WriteBufferBatchStrategy APIs.
//! Cloud URL routing is now handled internally by the unified batch strategy.

#![cfg(disabled_due_to_obsolete_apis)]

use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use proximadb::core::VectorRecord;
use proximadb::storage::memtable::specialized::write_buffer_behavior::WriteBufferVectorBatch;
use proximadb::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use proximadb::storage::persistence::write_buffer::batch_strategy::WriteBufferBatchStrategy;
use proximadb::storage::persistence::write_buffer::bincode_batch::BincodeWalBatchStrategy;
use proximadb::storage::persistence::write_buffer::config::WriteBufferConfig;
use proximadb::storage::BatchId;

/// Helper function to create test vector records
fn create_test_vector_records(collection_id: &str, count: usize) -> Vec<VectorRecord> {
    let now = chrono::Utc::now().timestamp_millis();
    
    (0..count)
        .map(|i| VectorRecord {
            id: format!("vector_{}", i),
            collection_id: collection_id.to_string(),
            vector: vec![1.0f32; 100], // 100-dimensional vector
            metadata: HashMap::new(),
            timestamp: now as u32,
            created_at: now,
            updated_at: Some(now as u32),
            expires_at: None,
            version: Some(1),
            rank: None,
            score: None,
            distance: None,
        })
        .collect()
}

/// Helper function to create test WAL batch
fn create_test_wal_batch(collection_id: &str, vectors: Vec<VectorRecord>) -> WriteBufferVectorBatch {
    let total_size_bytes = vectors.iter().map(|v| v.actual_size_bytes()).sum();
    let batch_id = BatchId::new(collection_id.to_string(), 1, vectors.len() as u64);
    
    WriteBufferVectorBatch {
        batch_id,
        vector_records: vectors,
        created_at: SystemTime::now(),
        total_size_bytes,
        is_flushed: false,
    }
}

/// Create test filesystem factory with URL validation
async fn create_test_filesystem_factory() -> Result<Arc<FilesystemFactory>> {
    let config = FilesystemConfig::default();
    let mut factory = FilesystemFactory::new(config);
    factory.initialize().await?;
    Ok(Arc::new(factory))
}

#[tokio::test]
async fn test_url_validation_for_different_providers() -> Result<()> {
    let filesystem_factory = create_test_filesystem_factory().await?;
    
    // Test valid URLs
    let valid_urls = vec![
        "file:///tmp/test.bin",
        "file:///workspace/data/wal/test.bin",
        "s3://my-bucket/path/to/file.bin",
        "s3://test-bucket/wal/collection_a/test.bin",
        "gcs://my-bucket/vectors/embeddings.bin",
        "adls://account/container/path/to/file.bin",
        "abfs://container@account.dfs.core.windows.net/path/to/file.bin",
        "hdfs://namenode:9000/path/to/file.bin",
    ];
    
    for url in valid_urls {
        let result = filesystem_factory.validate_url(url);
        assert!(result.is_ok(), "URL validation failed for valid URL: {}", url);
        println!("✅ Valid URL: {}", url);
    }
    
    // Test invalid URLs
    let invalid_urls = vec![
        "file://relative/path.bin",       // Missing leading slash
        "s3:///missing-bucket/file.bin",  // Missing bucket
        "gcs://missing-bucket//file.bin", // Missing bucket
        "adls://account/missing-container", // Missing container
        "abfs://invalid-format/file.bin", // Missing @ format
        "hdfs:///missing-namenode/file.bin", // Missing namenode
        "unknown://unsupported/scheme.bin", // Unsupported scheme
    ];
    
    for url in invalid_urls {
        let result = filesystem_factory.validate_url(url);
        assert!(result.is_err(), "URL validation should have failed for invalid URL: {}", url);
        println!("❌ Invalid URL: {} - {}", url, result.unwrap_err());
    }
    
    println!("✅ URL validation test completed successfully");
    Ok(())
}

#[tokio::test]
async fn test_bucket_extraction_from_urls() -> Result<()> {
    let filesystem_factory = create_test_filesystem_factory().await?;
    
    // Test S3 bucket extraction
    let s3_url = "s3://my-s3-bucket/path/to/file.bin";
    let bucket = filesystem_factory.extract_bucket_from_url(s3_url)?;
    assert_eq!(bucket, Some("my-s3-bucket".to_string()));
    
    // Test GCS bucket extraction
    let gcs_url = "gcs://my-gcs-bucket/vectors/embeddings.bin";
    let bucket = filesystem_factory.extract_bucket_from_url(gcs_url)?;
    assert_eq!(bucket, Some("my-gcs-bucket".to_string()));
    
    // Test ADLS container extraction
    let adls_url = "adls://account/my-container/path/to/file.bin";
    let container = filesystem_factory.extract_bucket_from_url(adls_url)?;
    assert_eq!(container, Some("my-container".to_string()));
    
    // Test ABFS container extraction
    let abfs_url = "abfs://my-container@account.dfs.core.windows.net/path/to/file.bin";
    let container = filesystem_factory.extract_bucket_from_url(abfs_url)?;
    assert_eq!(container, Some("my-container".to_string()));
    
    // Test file URL (should return None)
    let file_url = "file:///tmp/test.bin";
    let bucket = filesystem_factory.extract_bucket_from_url(file_url)?;
    assert_eq!(bucket, None);
    
    println!("✅ Bucket extraction test completed successfully");
    Ok(())
}

#[tokio::test]
async fn test_account_extraction_from_azure_urls() -> Result<()> {
    let filesystem_factory = create_test_filesystem_factory().await?;
    
    // Test ADLS account extraction
    let adls_url = "adls://my-account/container/path/to/file.bin";
    let account = filesystem_factory.extract_account_from_url(adls_url)?;
    assert_eq!(account, Some("my-account".to_string()));
    
    // Test ABFS account extraction
    let abfs_url = "abfs://container@my-account.dfs.core.windows.net/path/to/file.bin";
    let account = filesystem_factory.extract_account_from_url(abfs_url)?;
    assert_eq!(account, Some("my-account.dfs.core.windows.net".to_string()));
    
    // Test S3 URL (should return None)
    let s3_url = "s3://my-bucket/path/to/file.bin";
    let account = filesystem_factory.extract_account_from_url(s3_url)?;
    assert_eq!(account, None);
    
    println!("✅ Account extraction test completed successfully");
    Ok(())
}

#[tokio::test]
async fn test_path_extraction_from_urls() -> Result<()> {
    let filesystem_factory = create_test_filesystem_factory().await?;
    
    // Test file URL path extraction
    let file_url = "file:///tmp/test/file.bin";
    let path = filesystem_factory.extract_path_from_url(file_url)?;
    assert_eq!(path, "/tmp/test/file.bin");
    
    // Test S3 path extraction (should remove bucket)
    let s3_url = "s3://my-bucket/path/to/file.bin";
    let path = filesystem_factory.extract_path_from_url(s3_url)?;
    assert_eq!(path, "path/to/file.bin");
    
    // Test GCS path extraction (should remove bucket)
    let gcs_url = "gcs://my-bucket/vectors/embeddings.bin";
    let path = filesystem_factory.extract_path_from_url(gcs_url)?;
    assert_eq!(path, "vectors/embeddings.bin");
    
    // Test ADLS path extraction (should remove account and container)
    let adls_url = "adls://account/container/path/to/file.bin";
    let path = filesystem_factory.extract_path_from_url(adls_url)?;
    assert_eq!(path, "path/to/file.bin");
    
    // Test ABFS path extraction
    let abfs_url = "abfs://container@account.dfs.core.windows.net/path/to/file.bin";
    let path = filesystem_factory.extract_path_from_url(abfs_url)?;
    assert_eq!(path, "path/to/file.bin");
    
    // Test HDFS path extraction
    let hdfs_url = "hdfs://namenode:9000/user/data/file.bin";
    let path = filesystem_factory.extract_path_from_url(hdfs_url)?;
    assert_eq!(path, "/user/data/file.bin");
    
    println!("✅ Path extraction test completed successfully");
    Ok(())
}

#[tokio::test]
async fn test_wal_batch_strategy_url_routing() -> Result<()> {
    let filesystem_factory = create_test_filesystem_factory().await?;
    
    // Create WAL batch strategy
    let mut strategy = BincodeWalBatchStrategy::new();
    let config = WriteBufferConfig::default();
    
    // Initialize with filesystem
    strategy.initialize(&config, filesystem_factory.clone()).await?;
    
    // Test filesystem access
    let fs = strategy.get_filesystem();
    assert!(fs.is_some(), "Filesystem should be available");
    
    // Create test batch
    let collection_id = "test_collection";
    let vectors = create_test_vector_records(collection_id, 10);
    let batch = create_test_wal_batch(collection_id, vectors);
    
    // Test URL validation in cloud operations
    let valid_urls = vec![
        "file:///tmp/test_wal/",
        "s3://test-bucket/wal/",
        "gcs://test-bucket/proximadb/",
        "adls://account/container/wal/",
    ];
    
    for cloud_url in valid_urls {
        // Test health check with URL validation
        let health_result = strategy.check_cloud_health(cloud_url).await;
        
        // The health check should succeed in validation but may fail in actual connectivity
        // depending on whether the cloud provider is configured and accessible
        match health_result {
            Ok(is_healthy) => {
                println!("✅ Health check for {}: {}", cloud_url, is_healthy);
            }
            Err(e) => {
                println!("⚠️ Health check error for {}: {}", cloud_url, e);
            }
        }
    }
    
    // Test invalid URL handling
    let invalid_urls = vec![
        "invalid://scheme/path/",
        "s3:///missing-bucket/",
        "file://relative/path/",
    ];
    
    for cloud_url in invalid_urls {
        let health_result = strategy.check_cloud_health(cloud_url).await;
        match health_result {
            Ok(is_healthy) => {
                assert!(!is_healthy, "Invalid URL should result in unhealthy status");
                println!("✅ Invalid URL correctly marked as unhealthy: {}", cloud_url);
            }
            Err(e) => {
                println!("✅ Invalid URL correctly rejected: {} - {}", cloud_url, e);
            }
        }
    }
    
    println!("✅ WAL batch strategy URL routing test completed successfully");
    Ok(())
}

#[tokio::test]
async fn test_cloud_url_construction() -> Result<()> {
    let filesystem_factory = create_test_filesystem_factory().await?;
    
    // Test URL construction patterns
    let base_urls = vec![
        "s3://my-bucket/wal",      // Without trailing slash
        "s3://my-bucket/wal/",     // With trailing slash
        "gcs://my-bucket/proximadb", // Without trailing slash
        "gcs://my-bucket/proximadb/", // With trailing slash
    ];
    
    let filename = "test_batch.bin";
    
    for base_url in base_urls {
        let full_url = if base_url.ends_with('/') {
            format!("{}{}", base_url, filename)
        } else {
            format!("{}/{}", base_url, filename)
        };
        
        // Validate the constructed URL
        let validation_result = filesystem_factory.validate_url(&full_url);
        assert!(validation_result.is_ok(), "Constructed URL should be valid: {}", full_url);
        
        // Extract components
        let bucket = filesystem_factory.extract_bucket_from_url(&full_url)?;
        let path = filesystem_factory.extract_path_from_url(&full_url)?;
        
        println!("✅ Base URL: {} -> Full URL: {}", base_url, full_url);
        println!("   Bucket: {:?}, Path: {}", bucket, path);
    }
    
    println!("✅ Cloud URL construction test completed successfully");
    Ok(())
}

#[tokio::test]
async fn test_edge_cases_in_url_parsing() -> Result<()> {
    let filesystem_factory = create_test_filesystem_factory().await?;
    
    // Test edge cases
    let edge_cases = vec![
        ("s3://bucket/", ""),  // Bucket-only URL
        ("s3://bucket", ""),   // Bucket-only URL without slash
        ("gcs://bucket/single", "single"), // Single path segment
        ("adls://account/container", ""), // Container-only URL
        ("hdfs://namenode:9000/", "/"), // Root path
    ];
    
    for (url, expected_path) in edge_cases {
        if let Ok(_) = filesystem_factory.validate_url(url) {
            let path = filesystem_factory.extract_path_from_url(url)?;
            assert_eq!(path, expected_path, "Path extraction failed for URL: {}", url);
            println!("✅ Edge case URL: {} -> Path: '{}'", url, path);
        } else {
            println!("⚠️ Edge case URL validation failed: {}", url);
        }
    }
    
    println!("✅ Edge cases in URL parsing test completed successfully");
    Ok(())
}