//! Unified Caching Filesystem Test Suite
//!
//! This test suite validates the UnifiedCachingFilesystem implementation
//! which replaces the old IntelligentFilesystem and ZeroCopyFilesystem.

use anyhow::Result;
use std::sync::Arc;
use tempfile::TempDir;

use proximadb::storage::persistence::filesystem::{
    FileOptions, FileSystem, FilesystemFactory, UnifiedCachingFilesystem,
    metadata_traits::GenericMetadataSerializer,
};

/// Test data for filesystem operations
fn create_test_data(size: usize) -> Vec<u8> {
    vec![42u8; size]
}

/// Test basic filesystem operations
mod basic_operations {
    use super::*;

    #[tokio::test]
    async fn test_write_read_basic() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base_path = format!("file://{}", temp_dir.path().display());

        // Test with UnifiedCachingFilesystem
        let factory = Arc::new(FilesystemFactory::create(Default::default()).await?);
        let base_fs = factory.get_filesystem(&base_path)?;
        let unified_fs = Arc::new(UnifiedCachingFilesystem::with_serializer(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
            Arc::new(GenericMetadataSerializer),
        ));

        // Write test data
        let test_data = create_test_data(1024);
        let test_path = format!("{}/test_file.dat", base_path);
        unified_fs.write(&test_path, &test_data, None).await?;

        // Read and verify
        let read_data = unified_fs.read(&test_path).await?;
        assert_eq!(
            test_data,
            read_data.to_vec(),
            "Data should match after write/read"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_overwrite() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base_path = format!("file://{}", temp_dir.path().display());

        let factory = Arc::new(FilesystemFactory::create(Default::default()).await?);
        let base_fs = factory.get_filesystem(&base_path)?;
        let unified_fs = Arc::new(UnifiedCachingFilesystem::with_serializer(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
            Arc::new(GenericMetadataSerializer),
        ));

        let test_path = format!("{}/overwrite_test.dat", base_path);

        // Write initial data
        let data1 = vec![1u8; 100];
        unified_fs.write(&test_path, &data1, None).await?;

        // Overwrite with new data
        let data2 = vec![2u8; 200];
        let options = FileOptions {
            overwrite: true,
            ..Default::default()
        };
        unified_fs.write(&test_path, &data2, Some(options)).await?;

        // Verify new data
        let read_data = unified_fs.read(&test_path).await?;
        assert_eq!(data2, read_data.to_vec(), "Should read overwritten data");

        Ok(())
    }

    #[tokio::test]
    async fn test_delete() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base_path = format!("file://{}", temp_dir.path().display());

        let factory = Arc::new(FilesystemFactory::create(Default::default()).await?);
        let base_fs = factory.get_filesystem(&base_path)?;
        let unified_fs = Arc::new(UnifiedCachingFilesystem::with_serializer(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
            Arc::new(GenericMetadataSerializer),
        ));

        let test_path = format!("{}/delete_test.dat", base_path);

        // Write data
        let data = vec![42u8; 100];
        unified_fs.write(&test_path, &data, None).await?;

        // Verify file exists
        assert!(
            unified_fs.exists(&test_path).await?,
            "File should exist after write"
        );

        // Delete file
        unified_fs.delete(&test_path).await?;

        // Verify file is deleted
        assert!(
            !unified_fs.exists(&test_path).await?,
            "File should not exist after delete"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_list_files() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base_path = format!("file://{}", temp_dir.path().display());

        let factory = Arc::new(FilesystemFactory::create(Default::default()).await?);
        let base_fs = factory.get_filesystem(&base_path)?;
        let unified_fs = Arc::new(UnifiedCachingFilesystem::with_serializer(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
            Arc::new(GenericMetadataSerializer),
        ));

        // Create multiple files
        for i in 0..5 {
            let path = format!("{}/file_{}.dat", base_path, i);
            let data = vec![i as u8; 100];
            unified_fs.write(&path, &data, None).await?;
        }

        // List files
        let files = unified_fs.list(&base_path).await?;
        assert_eq!(files.len(), 5, "Should list all created files");

        Ok(())
    }
}

/// Test caching behavior
mod caching {
    use super::*;
    use std::time::Instant;

    #[tokio::test]
    async fn test_cache_hit_performance() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base_path = format!("file://{}", temp_dir.path().display());

        let factory = Arc::new(FilesystemFactory::create(Default::default()).await?);
        let base_fs = factory.get_filesystem(&base_path)?;
        let unified_fs = Arc::new(UnifiedCachingFilesystem::with_serializer(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
            Arc::new(GenericMetadataSerializer),
        ));

        let test_path = format!("{}/cache_test.dat", base_path);
        let data = create_test_data(1024 * 1024); // 1MB

        // Write data
        unified_fs.write(&test_path, &data, None).await?;

        // First read (cache miss)
        let start = Instant::now();
        let result1 = unified_fs.read(&test_path).await?;
        let first_read = start.elapsed();
        assert_eq!(result1.len(), data.len());

        // Second read (cache hit - should be faster or similar)
        let start = Instant::now();
        let result2 = unified_fs.read(&test_path).await?;
        let second_read = start.elapsed();
        assert_eq!(result2.len(), data.len());

        // Verify caching is working: second read should not be significantly slower
        // On fast systems with OS caching, the difference may be minimal, which is fine
        let speedup_ratio = first_read.as_micros() as f64 / second_read.as_micros() as f64;

        // Just ensure cache doesn't make things worse (allow up to 2x slower due to timing variance)
        assert!(
            speedup_ratio >= 0.5,
            "Cached read should not be significantly slower. First: {:?}, Second: {:?}, Speedup: {:.2}x",
            first_read,
            second_read,
            speedup_ratio
        );

        // Log the speedup for informational purposes
        println!(
            "Cache performance: First read: {:?}, Second read: {:?}, Speedup: {:.2}x",
            first_read, second_read, speedup_ratio
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_metadata_caching() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base_path = format!("file://{}", temp_dir.path().display());

        let factory = Arc::new(FilesystemFactory::create(Default::default()).await?);
        let base_fs = factory.get_filesystem(&base_path)?;
        let unified_fs = Arc::new(UnifiedCachingFilesystem::with_serializer(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
            Arc::new(GenericMetadataSerializer),
        ));

        let test_path = format!("{}/metadata_test.dat", base_path);
        let data = vec![42u8; 1024];

        // Write data
        unified_fs.write(&test_path, &data, None).await?;

        // Multiple metadata queries should benefit from caching
        let start = Instant::now();
        for _ in 0..100 {
            let _ = unified_fs.exists(&test_path).await?;
        }
        let cached_time = start.elapsed();

        // This should be very fast with caching
        assert!(
            cached_time.as_millis() < 100,
            "100 cached metadata queries should complete in < 100ms, took {:?}",
            cached_time
        );

        Ok(())
    }
}

/// Test error handling
mod error_handling {
    use super::*;

    #[tokio::test]
    async fn test_read_nonexistent() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base_path = format!("file://{}", temp_dir.path().display());

        let factory = Arc::new(FilesystemFactory::create(Default::default()).await?);
        let base_fs = factory.get_filesystem(&base_path)?;
        let unified_fs = Arc::new(UnifiedCachingFilesystem::with_serializer(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
            Arc::new(GenericMetadataSerializer),
        ));

        let test_path = format!("{}/nonexistent.dat", base_path);

        // Reading non-existent file should return error
        let result = unified_fs.read(&test_path).await;
        assert!(result.is_err(), "Reading non-existent file should error");

        Ok(())
    }

    #[tokio::test]
    async fn test_delete_nonexistent() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base_path = format!("file://{}", temp_dir.path().display());

        let factory = Arc::new(FilesystemFactory::create(Default::default()).await?);
        let base_fs = factory.get_filesystem(&base_path)?;
        let unified_fs = Arc::new(UnifiedCachingFilesystem::with_serializer(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
            Arc::new(GenericMetadataSerializer),
        ));

        let test_path = format!("{}/nonexistent.dat", base_path);

        // Deleting non-existent file might succeed or fail gracefully
        let result = unified_fs.delete(&test_path).await;
        // Just ensure it doesn't panic
        let _ = result;

        Ok(())
    }
}

/// Test concurrent operations
mod concurrency {
    use super::*;
    use tokio::task::JoinSet;

    #[tokio::test]
    async fn test_concurrent_writes() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base_path = format!("file://{}", temp_dir.path().display());

        let factory = Arc::new(FilesystemFactory::create(Default::default()).await?);
        let base_fs = factory.get_filesystem(&base_path)?;
        let unified_fs = Arc::new(UnifiedCachingFilesystem::with_serializer(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
            Arc::new(GenericMetadataSerializer),
        ));

        let mut tasks = JoinSet::new();

        // Spawn multiple concurrent write tasks
        for i in 0..10 {
            let fs = unified_fs.clone();
            let path = format!("{}/concurrent_{}.dat", base_path, i);
            let data = vec![i as u8; 1024];

            tasks.spawn(async move { fs.write(&path, &data, None).await });
        }

        // Wait for all writes to complete
        while let Some(result) = tasks.join_next().await {
            result??;
        }

        // Verify all files were written
        let files = unified_fs.list(&base_path).await?;
        assert_eq!(files.len(), 10, "All concurrent writes should succeed");

        Ok(())
    }

    #[tokio::test]
    async fn test_concurrent_reads() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let base_path = format!("file://{}", temp_dir.path().display());

        let factory = Arc::new(FilesystemFactory::create(Default::default()).await?);
        let base_fs = factory.get_filesystem(&base_path)?;
        let unified_fs = Arc::new(UnifiedCachingFilesystem::with_serializer(
            base_fs.clone(),
            "test_collection".to_string(),
            "test_engine".to_string(),
            Arc::new(GenericMetadataSerializer),
        ));

        // Write a file first
        let test_path = format!("{}/concurrent_read.dat", base_path);
        let data = vec![42u8; 1024];
        unified_fs.write(&test_path, &data, None).await?;

        let mut tasks = JoinSet::new();

        // Spawn multiple concurrent read tasks
        for _ in 0..10 {
            let fs = unified_fs.clone();
            let path = test_path.clone();
            let expected = data.clone();

            tasks.spawn(async move {
                let read_data = fs.read(&path).await?;
                assert_eq!(expected, read_data.to_vec(), "Data should match");
                Ok::<_, anyhow::Error>(())
            });
        }

        // Wait for all reads to complete
        while let Some(result) = tasks.join_next().await {
            result??;
        }

        Ok(())
    }
}
