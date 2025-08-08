//! Comprehensive filesystem API tests

use super::*;
use anyhow::Result;
use std::sync::Arc;
use crate::storage::transaction_coordinator::{TransactionCoordinator, StagingConfig, TransactionStageType};

/// Test basic filesystem operations across all backends
#[cfg(test)]
mod filesystem_api_tests {
    use super::*;

    /// Helper to create test filesystem factory with atomic coordinator
    async fn create_test_factory() -> Result<(Arc<FilesystemFactory>, Arc<TransactionCoordinator>)> {
        let mut config = FilesystemConfig::default();
        config.default_fs = Some("file:///tmp/proximadb-fs-tests".to_string());
        
        // Ensure test directory exists
        std::fs::create_dir_all("/tmp/proximadb-fs-tests")?;
        
        // Configure local filesystem with default settings
        config.local = Some(super::local::LocalConfig {
            root_dir: None,  // Allow access to all paths
            follow_symlinks: true,
            default_permissions: None,
            sync_enabled: true,
        });
        
        let factory = Arc::new(FilesystemFactory::new(config).await?);
        let coordinator = Arc::new(TransactionCoordinator::new(factory.clone(), None).await?);
        
        Ok((factory, coordinator))
    }

    #[tokio::test]
    async fn test_local_filesystem_basic_operations() -> Result<()> {
        let (factory, coordinator) = create_test_factory().await?;
        
        let test_url = "file:///tmp/proximadb-fs-tests/test_basic.txt";
        let test_data = b"Hello, Filesystem API!";

        // Use atomic write through coordinator
        let staging_config = StagingConfig {
            base_url: "file:///tmp/proximadb-fs-tests".to_string(),
            collection_id: None,
            operation_type: TransactionStageType::Custom("test".to_string()),
            auto_cleanup: true,
            ..Default::default()
        };
        
        let operation = coordinator.begin_atomic_operation(&staging_config).await?;
        
        // Write to staging
        coordinator.write_to_staging(&operation.operation_id, "test_basic.txt", test_data).await?;
        
        // Finalize the operation
        coordinator.finalize_atomic_operation(&operation.operation_id).await?;

        // Test exists
        assert!(factory.exists(test_url).await?);

        // Test read
        let read_data = factory.read(test_url).await?;
        assert_eq!(test_data, &read_data[..]);

        // Test metadata
        let metadata = factory.metadata(test_url).await?;
        assert_eq!(metadata.size, test_data.len() as u64);
        assert!(!metadata.is_directory);

        // Test delete
        factory.delete(test_url).await?;
        assert!(!factory.exists(test_url).await?);

        Ok(())
    }

    #[tokio::test]
    async fn test_local_filesystem_directory_operations() -> Result<()> {
        let (factory, _coordinator) = create_test_factory().await?;
        
        let test_dir = "file:///tmp/proximadb-fs-tests/test_dir";

        // Create directory
        factory.create_dir(test_dir).await?;
        assert!(factory.exists(test_dir).await?);

        // Create nested structure
        factory.create_dir_all(&format!("{}/nested/deep", test_dir)).await?;
        
        // Create files directly
        factory.write(&format!("{}/file1.txt", test_dir), b"content1", None).await?;
        factory.write(&format!("{}/file2.txt", test_dir), b"content2", None).await?;

        // List directory
        let entries = factory.list(test_dir).await?;
        assert!(entries.len() >= 3); // file1, file2, nested

        // Cleanup
        factory.delete(test_dir).await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_local_filesystem_range_operations() -> Result<()> {
        let (factory, _coordinator) = create_test_factory().await?;
        
        let test_path = "file:///tmp/proximadb-fs-tests/test_range.txt";
        let test_data = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

        // Clean up any existing file first
        let _ = factory.delete(test_path).await;

        // Write test data directly
        factory.write(test_path, test_data, None).await?;

        // Read ranges
        let fs = factory.get_filesystem(test_path)?;
        let path = FilesystemFactory::resolve_path(test_path)?;
        
        let range1 = fs.read_range(&path, 0, 10).await?;
        assert_eq!(&range1[..], b"0123456789");

        let range2 = fs.read_range(&path, 10, 10).await?;
        assert_eq!(&range2[..], b"ABCDEFGHIJ");

        // Read multiple ranges
        let ranges = vec![0..5, 10..15, 20..25];
        let results = fs.read_ranges(&path, ranges).await?;
        assert_eq!(results.len(), 3);
        assert_eq!(&results[0][..], b"01234");
        assert_eq!(&results[1][..], b"ABCDE");
        assert_eq!(&results[2][..], b"KLMNO");

        // Cleanup
        factory.delete(test_path).await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_local_filesystem_copy_move() -> Result<()> {
        let (factory, _coordinator) = create_test_factory().await?;
        
        let source = "file:///tmp/proximadb-fs-tests/source.txt";
        let dest = "file:///tmp/proximadb-fs-tests/dest.txt";
        let test_data = b"Copy and move test";

        // Test copy
        factory.write(source, test_data, None).await?;
        factory.copy(source, dest).await?;
        
        assert!(factory.exists(source).await?);
        assert!(factory.exists(dest).await?);
        
        let dest_data = factory.read(dest).await?;
        assert_eq!(test_data, &dest_data[..]);

        // Cleanup from copy test
        factory.delete(dest).await?;

        // Test move
        let move_dest = "file:///tmp/proximadb-fs-tests/move_dest.txt";
        factory.move_file(source, move_dest).await?;
        
        assert!(!factory.exists(source).await?);
        assert!(factory.exists(move_dest).await?);

        // Cleanup
        factory.delete(move_dest).await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_local_filesystem_append() -> Result<()> {
        let (factory, _coordinator) = create_test_factory().await?;
        
        let test_path = "file:///tmp/proximadb-fs-tests/append_test.txt";

        // Initial write
        factory.write(test_path, b"Hello", None).await?;
        
        // Append
        factory.append(test_path, b", World!").await?;
        
        // Verify
        let data = factory.read(test_path).await?;
        assert_eq!(b"Hello, World!", &data[..]);

        // Cleanup
        factory.delete(test_path).await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_filesystem_type_identification() -> Result<()> {
        let (factory, _coordinator) = create_test_factory().await?;
        
        let local_fs = factory.get_filesystem("file:///tmp/test")?;
        assert_eq!(local_fs.filesystem_type(), "local");

        Ok(())
    }

    #[tokio::test]
    async fn test_large_file_operations() -> Result<()> {
        let (factory, _coordinator) = create_test_factory().await?;
        
        let test_path = "file:///tmp/proximadb-fs-tests/large_file.bin";
        
        // Clean up any existing file first
        let _ = factory.delete(test_path).await;
        
        // Create 1MB test data
        let test_data: Vec<u8> = (0..1024*1024).map(|i| (i % 256) as u8).collect();
        
        // Write directly
        factory.write(test_path, &test_data, None).await?;
        
        // Verify size
        let metadata = factory.metadata(test_path).await?;
        assert_eq!(metadata.size, test_data.len() as u64);
        
        // Read portions
        let fs = factory.get_filesystem(test_path)?;
        let path = FilesystemFactory::resolve_path(test_path)?;
        let chunk = fs.read_range(&path, 512*1024, 1024).await?;
        assert_eq!(chunk.len(), 1024);
        
        // Cleanup
        factory.delete(test_path).await?;
        
        Ok(())
    }

    #[tokio::test]
    async fn test_concurrent_operations() -> Result<()> {
        let (factory, _coordinator) = create_test_factory().await?;
        
        let mut handles = vec![];
        
        // Spawn concurrent writes
        for i in 0..5 {
            let factory_clone = factory.clone();
            let handle = tokio::spawn(async move {
                let path = format!("file:///tmp/proximadb-fs-tests/concurrent_{}.txt", i);
                let data = format!("Concurrent test {}", i);
                
                // Clean up any existing file first
                let _ = factory_clone.delete(&path).await;
                
                // Write directly
                factory_clone.write(&path, data.as_bytes(), None).await?;
                
                Ok::<_, anyhow::Error>(path)
            });
            handles.push(handle);
        }
        
        // Wait and collect paths
        let mut paths = vec![];
        for handle in handles {
            let path = handle.await??;
            paths.push(path);
        }
        
        // Verify all files exist
        for path in &paths {
            assert!(factory.exists(path).await?);
        }
        
        // Cleanup
        for path in paths {
            factory.delete(&path).await?;
        }
        
        Ok(())
    }

    #[tokio::test]
    async fn test_error_handling() -> Result<()> {
        let (factory, _coordinator) = create_test_factory().await?;
        let fs = factory.get_filesystem("file:///tmp/proximadb-fs-tests")?;
        
        // Read non-existent file
        let result = fs.read("file:///tmp/proximadb-fs-tests/nonexistent.txt").await;
        assert!(result.is_err());
        
        // Get metadata for non-existent file
        let result = fs.metadata("file:///tmp/proximadb-fs-tests/nonexistent.txt").await;
        assert!(result.is_err());
        
        // Try to create file in non-existent directory (should fail without create_dir_all)
        let result = fs.write("file:///tmp/proximadb-fs-tests/nonexistent/dir/file.txt", b"test", None).await;
        assert!(result.is_err());
        
        Ok(())
    }

    #[tokio::test]
    async fn test_cross_filesystem_factory_operations() -> Result<()> {
        let (factory, _coordinator) = create_test_factory().await?;
        
        let local_src = "file:///tmp/proximadb-fs-tests/factory_src.txt";
        let local_dst = "file:///tmp/proximadb-fs-tests/factory_dst.txt";
        let test_data = b"Factory operation test";
        
        // Write source file
        factory.write(local_src, test_data, None).await?;
        
        // Copy using factory
        factory.copy_atomic(local_src, local_dst).await?;
        
        // Verify
        assert!(factory.exists(local_src).await?);
        assert!(factory.exists(local_dst).await?);
        
        let dst_data = factory.read(local_dst).await?;
        assert_eq!(test_data, &dst_data[..]);
        
        // Move using factory
        let move_dst = "file:///tmp/proximadb-fs-tests/factory_move_dst.txt";
        factory.move_atomic(local_dst, move_dst).await?;
        
        assert!(!factory.exists(local_dst).await?);
        assert!(factory.exists(move_dst).await?);
        
        // Cleanup
        factory.delete(local_src).await?;
        factory.delete(move_dst).await?;
        
        Ok(())
    }
}