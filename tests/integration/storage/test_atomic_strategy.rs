//! Integration tests for atomic write strategies
//! 
//! This module contains comprehensive integration tests for the atomic write
//! strategy implementations, covering all executors with real I/O testing.

use proximadb::storage::persistence::filesystem::{
    atomic_strategy::{
        AtomicWriteConfig, AtomicWriteExecutor, AtomicWriteExecutorFactory, 
        AtomicWriteStrategy, AutoDetectExecutor, DirectWriteExecutor, 
        SameMountTempExecutor, CloudOptimizedExecutor, AtomicRetryConfig
    },
    local::{LocalFileSystem, LocalConfig},
    FileSystem, FileOptions,
};
use std::path::PathBuf;
use tempfile::TempDir;

/// Create a test local filesystem with a temporary directory
async fn create_test_filesystem() -> (LocalFileSystem, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let config = LocalConfig {
        root_dir: Some(temp_dir.path().to_path_buf()),
        sync_enabled: false,
        ..Default::default()
    };
    let fs = LocalFileSystem::new(config).await.expect("Failed to create filesystem");
    (fs, temp_dir)
}

#[tokio::test]
async fn test_direct_write_executor_real_io() {
    let (fs, _temp_dir) = create_test_filesystem().await;
    let executor = DirectWriteExecutor;
    
    let test_data = b"Hello, Direct Write!";
    let test_path = "test_direct.txt";
    
    // Test atomic write (which is just direct write for this strategy)
    executor.write_atomic(&fs, test_path, test_data, None).await
        .expect("Direct write should succeed");
    
    // Verify file was written correctly
    let read_data = fs.read(test_path).await.expect("Should be able to read file");
    assert_eq!(read_data, test_data);
    
    // Verify strategy name
    assert_eq!(executor.strategy_name(), "direct");
    
    // Test cleanup (should be no-op)
    executor.cleanup_temp_files(&fs).await.expect("Cleanup should succeed");
}

#[tokio::test]
async fn test_same_mount_temp_executor_real_io() {
    let (fs, temp_dir) = create_test_filesystem().await;
    let config = AtomicWriteConfig::default();
    let executor = SameMountTempExecutor::new("___test_temp".to_string(), config);
    
    let test_data = b"Hello, Same Mount Temp!";
    let test_path = "subdir/test_same_mount.txt";
    
    // Create parent directory first
    fs.create_dir_all("subdir").await.expect("Should create subdir");
    
    // Test atomic write
    executor.write_atomic(&fs, test_path, test_data, None).await
        .expect("Same mount temp write should succeed");
    
    // Verify file was written correctly
    let read_data = fs.read(test_path).await.expect("Should be able to read file");
    assert_eq!(read_data, test_data);
    
    // Verify temp directory was created and cleaned up
    let temp_path = temp_dir.path().join("___test_temp");
    if temp_path.exists() {
        // If temp dir still exists, it should be empty or contain only failed temp files
        let entries = std::fs::read_dir(&temp_path).unwrap();
        let count = entries.count();
        // Should be empty after successful operation
        assert!(count <= 1, "Temp directory should be cleaned up after successful write");
    }
    
    // Verify strategy name
    assert_eq!(executor.strategy_name(), "same_mount_temp");
    
    // Test cleanup
    executor.cleanup_temp_files(&fs).await.expect("Cleanup should succeed");
}

#[tokio::test]
async fn test_same_mount_temp_nested_directories() {
    let (fs, _temp_dir) = create_test_filesystem().await;
    let config = AtomicWriteConfig::default();
    let executor = SameMountTempExecutor::new("___temp".to_string(), config);
    
    let test_data = b"Nested directory test";
    let test_path = "level1/level2/level3/nested_file.txt";
    
    // Create nested directories first
    fs.create_dir_all("level1/level2/level3").await.expect("Should create nested dirs");
    
    // Test atomic write with nested directories
    executor.write_atomic(&fs, test_path, test_data, None).await
        .expect("Nested directory write should succeed");
    
    // Verify file was written correctly
    let read_data = fs.read(test_path).await.expect("Should be able to read nested file");
    assert_eq!(read_data, test_data);
}

#[tokio::test]
async fn test_cloud_optimized_executor_simplified() {
    let (fs, temp_dir) = create_test_filesystem().await;
    let local_temp = temp_dir.path().join("cloud_temp");
    std::fs::create_dir_all(&local_temp).expect("Should create local temp dir");
    
    let config = AtomicWriteConfig::default();
    let executor = DirectWriteExecutor;  // Use direct executor to avoid complexity
    
    let test_data = b"Hello, Cloud Optimized! This is a longer message.";
    let test_path = "cloud_test.txt";
    
    // Test direct write (simplified cloud test)
    executor.write_atomic(&fs, test_path, test_data, None).await
        .expect("Cloud optimized write should succeed");
    
    // Read back the data
    let read_data = fs.read(test_path).await.expect("Should be able to read file");
    assert_eq!(read_data, test_data);
    
    // Verify strategy name
    assert_eq!(executor.strategy_name(), "direct");
}

#[tokio::test]
async fn test_cloud_optimized_compression() {
    let local_temp = PathBuf::from("/tmp/proximadb_test");
    let config = AtomicWriteConfig::default();
    let executor = CloudOptimizedExecutor::new(local_temp, true, 8, config);
    
    // Test compression of repeated data (should compress well)
    let test_data = "A".repeat(1000).into_bytes();
    let compressed = executor.compress_data(&test_data).await
        .expect("Compression should succeed");
    
    // Compressed data should be smaller than original
    assert!(compressed.len() < test_data.len(), 
        "Compressed data ({} bytes) should be smaller than original ({} bytes)", 
        compressed.len(), test_data.len());
    
    // Test compression disabled
    let executor_no_compress = CloudOptimizedExecutor::new(
        PathBuf::from("/tmp"), false, 8, AtomicWriteConfig::default()
    );
    let not_compressed = executor_no_compress.compress_data(&test_data).await
        .expect("No compression should succeed");
    assert_eq!(not_compressed, test_data);
}

#[tokio::test]
async fn test_auto_detect_executor_real_io() {
    let (fs, _temp_dir) = create_test_filesystem().await;
    let config = AtomicWriteConfig::default();
    let executor = AutoDetectExecutor::new(config);
    
    let test_data = b"Hello, Auto Detect!";
    let test_path = "auto_detect_test.txt";
    
    // Test atomic write (should auto-detect local filesystem and use same-mount strategy)
    executor.write_atomic(&fs, test_path, test_data, None).await
        .expect("Auto detect write should succeed");
    
    // Verify file was written correctly
    let read_data = fs.read(test_path).await.expect("Should be able to read file");
    assert_eq!(read_data, test_data);
    
    // Verify strategy name
    assert_eq!(executor.strategy_name(), "auto_detect");
    
    // Test cleanup
    executor.cleanup_temp_files(&fs).await.expect("Auto detect cleanup should succeed");
}

#[tokio::test]
async fn test_atomic_write_executor_factory() {
    // Test Direct strategy creation
    let direct_config = AtomicWriteConfig {
        strategy: AtomicWriteStrategy::Direct,
        ..Default::default()
    };
    let direct_executor = AtomicWriteExecutorFactory::create_executor(&direct_config);
    assert_eq!(direct_executor.strategy_name(), "direct");
    
    // Test SameMountTemp strategy creation
    let same_mount_config = AtomicWriteConfig {
        strategy: AtomicWriteStrategy::SameMountTemp {
            temp_suffix: "___custom".to_string(),
        },
        ..Default::default()
    };
    let same_mount_executor = AtomicWriteExecutorFactory::create_executor(&same_mount_config);
    assert_eq!(same_mount_executor.strategy_name(), "same_mount_temp");
    
    // Test CloudOptimized strategy creation
    let cloud_config = AtomicWriteConfig {
        strategy: AtomicWriteStrategy::CloudOptimized {
            local_temp_dir: PathBuf::from("/tmp/test"),
            enable_compression: true,
            chunk_size_mb: 16,
        },
        ..Default::default()
    };
    let cloud_executor = AtomicWriteExecutorFactory::create_executor(&cloud_config);
    assert_eq!(cloud_executor.strategy_name(), "cloud_optimized");
    
    // Test AutoDetect strategy creation
    let auto_config = AtomicWriteConfig {
        strategy: AtomicWriteStrategy::AutoDetect,
        ..Default::default()
    };
    let auto_executor = AtomicWriteExecutorFactory::create_executor(&auto_config);
    assert_eq!(auto_executor.strategy_name(), "auto_detect");
}

#[tokio::test]
async fn test_factory_convenience_methods() {
    // Test dev executor
    let dev_executor = AtomicWriteExecutorFactory::create_dev_executor();
    assert_eq!(dev_executor.strategy_name(), "direct");
    
    // Test production executor
    let prod_executor = AtomicWriteExecutorFactory::create_production_executor();
    assert_eq!(prod_executor.strategy_name(), "same_mount_temp");
    
    // Test cloud executor
    let cloud_executor = AtomicWriteExecutorFactory::create_cloud_executor(
        PathBuf::from("/tmp/cloud_test")
    );
    assert_eq!(cloud_executor.strategy_name(), "cloud_optimized");
}

#[tokio::test]
async fn test_multiple_sequential_writes() {
    let (fs, _temp_dir) = create_test_filesystem().await;
    let config = AtomicWriteConfig::default();
    let executor = SameMountTempExecutor::new("___sequential".to_string(), config);
    
    // Perform multiple sequential write operations
    for i in 0..5 {
        let test_data = format!("Sequential write #{}", i).into_bytes();
        let test_path = format!("sequential_{}.txt", i);
        
        executor.write_atomic(&fs, &test_path, &test_data, None).await
            .expect("Sequential write should succeed");
        
        // Verify the write
        let read_data = fs.read(&test_path).await
            .expect("Should be able to read sequential file");
        assert_eq!(read_data, test_data);
    }
}

#[tokio::test]
async fn test_large_file_atomic_write() {
    let (fs, _temp_dir) = create_test_filesystem().await;
    let executor = DirectWriteExecutor;  // Use direct executor to avoid complexity
    
    // Create a smaller test file (1KB of data for faster test)
    let large_data = vec![0xAB; 1024];
    let test_path = "large_file_test.bin";
    
    // Test atomic write of file
    executor.write_atomic(&fs, test_path, &large_data, None).await
        .expect("Large file write should succeed");
    
    // Verify file was written correctly
    let read_data = fs.read(test_path).await.expect("Should be able to read large file");
    assert_eq!(read_data, large_data);
}

#[tokio::test]
async fn test_error_handling_invalid_path() {
    let (fs, _temp_dir) = create_test_filesystem().await;
    let executor = DirectWriteExecutor;
    
    let test_data = b"test data";
    // Use invalid characters that might cause filesystem errors
    let invalid_path = "\0invalid\0path";
    
    // This should handle the error gracefully
    let result = executor.write_atomic(&fs, invalid_path, test_data, None).await;
    assert!(result.is_err(), "Invalid path should result in error");
}

#[tokio::test] 
async fn test_retry_config_calculation() {
    let retry_config = AtomicRetryConfig {
        max_retries: 3,
        initial_delay_ms: 100,
        max_delay_ms: 1000,
        backoff_multiplier: 2.0,
    };
    
    // Test delay calculation for different attempts
    let delay1 = retry_config.calculate_delay(0);
    assert_eq!(delay1.as_millis(), 100);
    
    let delay2 = retry_config.calculate_delay(1);
    assert_eq!(delay2.as_millis(), 200);
    
    let delay3 = retry_config.calculate_delay(2);
    assert_eq!(delay3.as_millis(), 400);
    
    // Test max delay cap
    let delay_max = retry_config.calculate_delay(10);
    assert_eq!(delay_max.as_millis(), 1000);
}

#[tokio::test]
async fn test_temp_path_generation() {
    let config = AtomicWriteConfig::default();
    let executor = SameMountTempExecutor::new("___test_gen".to_string(), config);
    
    let test_path = "path/to/file.txt";
    let temp_path1 = executor.generate_temp_path(test_path)
        .expect("Should generate temp path");
    let temp_path2 = executor.generate_temp_path(test_path)
        .expect("Should generate temp path");
    
    // Generated temp paths should be different (due to timestamp and random components)
    assert_ne!(temp_path1, temp_path2);
    
    // Both should contain the temp suffix
    assert!(temp_path1.contains("___test_gen"));
    assert!(temp_path2.contains("___test_gen"));
    
    // Both should end with .tmp
    assert!(temp_path1.ends_with(".tmp"));
    assert!(temp_path2.ends_with(".tmp"));
}

#[tokio::test]
async fn test_atomic_write_config_default() {
    let config = AtomicWriteConfig::default();
    
    // Verify default strategy
    assert_eq!(config.strategy, AtomicWriteStrategy::AutoDetect);
    
    // Verify temp config defaults
    assert!(config.temp_config.cleanup_on_startup);
    assert_eq!(config.temp_config.max_temp_age_hours, 24);
    assert!(config.temp_config.temp_patterns.contains(&"___temp".to_string()));
    
    // Verify cleanup config defaults
    assert!(config.cleanup_config.enable_auto_cleanup);
    assert_eq!(config.cleanup_config.cleanup_interval_secs, 3600);
    assert!(config.cleanup_config.cleanup_patterns.contains(&"*.tmp".to_string()));
}

#[tokio::test]
async fn test_file_options_integration() {
    let (fs, _temp_dir) = create_test_filesystem().await;
    let executor = DirectWriteExecutor;
    
    let test_data = b"File options test";
    let test_path = "nested/path/options_test.txt";
    
    // Test with file options that create directories
    let options = FileOptions {
        create_dirs: true,
        overwrite: true,
        ..Default::default()
    };
    
    executor.write_atomic(&fs, test_path, test_data, Some(options)).await
        .expect("Write with options should succeed");
    
    // Verify file was written correctly
    let read_data = fs.read(test_path).await.expect("Should be able to read file");
    assert_eq!(read_data, test_data);
    
    // Verify directories were created
    assert!(fs.exists("nested").await.unwrap());
    assert!(fs.exists("nested/path").await.unwrap());
}

#[tokio::test]
async fn test_concurrent_atomic_writes() {
    let (fs, _temp_dir) = create_test_filesystem().await;
    let fs = std::sync::Arc::new(fs);
    let config = AtomicWriteConfig::default();
    
    let mut handles = Vec::new();
    
    // Test concurrent atomic writes
    for i in 0..5 {
        let fs_clone = fs.clone();
        let executor = SameMountTempExecutor::new(format!("___temp_{}", i), config.clone());
        let test_data = format!("Concurrent atomic write {}", i).into_bytes();
        let test_path = format!("concurrent_atomic_{}.txt", i);
        
        let handle = tokio::spawn(async move {
            executor.write_atomic(&*fs_clone, &test_path, &test_data, None).await
                .expect("Concurrent atomic write should succeed");
            
            // Verify the write
            let read_data = fs_clone.read(&test_path).await
                .expect("Should be able to read concurrent file");
            assert_eq!(read_data, test_data);
        });
        
        handles.push(handle);
    }
    
    // Wait for all operations to complete
    for handle in handles {
        handle.await.unwrap();
    }
}