//! Integration tests for LocalFileSystem
//! 
//! This module contains comprehensive integration tests for the LocalFileSystem
//! implementation, covering all filesystem operations with real I/O testing.

use proximadb::storage::persistence::filesystem::{
    local::{LocalFileSystem, LocalConfig},
    DirEntry, FileMetadata, FileOptions, FileSystem, FilesystemError,
};
use std::path::PathBuf;
use tempfile::TempDir;

/// Create a test local filesystem with a temporary directory
async fn create_test_filesystem() -> (LocalFileSystem, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let config = LocalConfig {
        root_dir: Some(temp_dir.path().to_path_buf()),
        follow_symlinks: true,
        default_permissions: None,
        sync_enabled: true,
    };
    let fs = LocalFileSystem::new(config).await.expect("Failed to create filesystem");
    (fs, temp_dir)
}

#[tokio::test]
async fn test_local_filesystem_basic_operations() {
    let (fs, _temp_dir) = create_test_filesystem().await;

    // Test write and read
    let test_data = b"Hello, ProximaDB!";
    let test_path = "test_file.txt";

    fs.write(test_path, test_data, None).await.unwrap();
    assert!(fs.exists(test_path).await.unwrap());

    let read_data = fs.read(test_path).await.unwrap();
    assert_eq!(read_data, test_data);

    // Test metadata
    let metadata = fs.metadata(test_path).await.unwrap();
    assert_eq!(metadata.size, test_data.len() as u64);
    assert!(!metadata.is_directory);

    // Test append
    let append_data = b" Filesystem test";
    fs.append(test_path, append_data).await.unwrap();

    let full_data = fs.read(test_path).await.unwrap();
    assert_eq!(full_data.len(), test_data.len() + append_data.len());

    // Test directory operations
    let dir_path = "test_dir";
    fs.create_dir(dir_path).await.unwrap();
    assert!(fs.exists(dir_path).await.unwrap());

    let dir_metadata = fs.metadata(dir_path).await.unwrap();
    assert!(dir_metadata.is_directory);

    // Test list directory
    let entries = fs.list(".").await.unwrap();
    assert!(entries.len() >= 2); // At least test_file.txt and test_dir

    // Test copy
    let copy_path = "test_file_copy.txt";
    fs.copy(test_path, copy_path).await.unwrap();
    assert!(fs.exists(copy_path).await.unwrap());

    // Test delete
    fs.delete(test_path).await.unwrap();
    assert!(!fs.exists(test_path).await.unwrap());

    fs.delete(copy_path).await.unwrap();
    fs.delete(dir_path).await.unwrap();
}

#[tokio::test]
async fn test_local_filesystem_configuration_variants() {
    let temp_dir = TempDir::new().unwrap();
    
    // Test with custom configuration
    let config = LocalConfig {
        root_dir: Some(temp_dir.path().to_path_buf()),
        follow_symlinks: false,
        default_permissions: Some(0o644),
        sync_enabled: false,
    };

    let fs = LocalFileSystem::new(config).await.unwrap();
    let test_data = b"Configuration test data";
    let test_path = "config_test.txt";

    // Test write with custom configuration
    fs.write(test_path, test_data, None).await.unwrap();
    assert!(fs.exists(test_path).await.unwrap());

    let read_data = fs.read(test_path).await.unwrap();
    assert_eq!(read_data, test_data);

    // Test metadata
    let metadata = fs.metadata(test_path).await.unwrap();
    assert_eq!(metadata.size, test_data.len() as u64);
    assert!(!metadata.is_directory);

    // Cleanup
    fs.delete(test_path).await.unwrap();
}

#[tokio::test]
async fn test_local_filesystem_error_handling() {
    let (fs, _temp_dir) = create_test_filesystem().await;

    // Test reading non-existent file
    let result = fs.read("non_existent_file.txt").await;
    assert!(result.is_err());
    if let Err(FilesystemError::NotFound(_)) = result {
        // Expected error type
    } else {
        panic!("Expected NotFound error");
    }

    // Test deleting non-existent file
    let result = fs.delete("non_existent_file.txt").await;
    assert!(result.is_err());
    if let Err(FilesystemError::NotFound(_)) = result {
        // Expected error type
    } else {
        panic!("Expected NotFound error");
    }

    // Test overwrite protection
    let test_data = b"Original data";
    let test_path = "overwrite_test.txt";
    
    fs.write(test_path, test_data, None).await.unwrap();
    
    let options = FileOptions {
        overwrite: false,
        ..Default::default()
    };
    
    let result = fs.write(test_path, b"New data", Some(options)).await;
    assert!(result.is_err());
    if let Err(FilesystemError::AlreadyExists(_)) = result {
        // Expected error type
    } else {
        panic!("Expected AlreadyExists error");
    }

    // Cleanup
    fs.delete(test_path).await.unwrap();
}

#[tokio::test]
async fn test_local_filesystem_directory_operations() {
    let (fs, _temp_dir) = create_test_filesystem().await;

    // Test create_dir_all with nested directories
    let nested_path = "level1/level2/level3";
    fs.create_dir_all(nested_path).await.unwrap();
    assert!(fs.exists(nested_path).await.unwrap());

    let metadata = fs.metadata(nested_path).await.unwrap();
    assert!(metadata.is_directory);

    // Test listing directory contents
    let test_file_path = "level1/test_file.txt";
    fs.write(test_file_path, b"test content", None).await.unwrap();

    let entries = fs.list("level1").await.unwrap();
    assert!(entries.len() >= 2); // At least level2 dir and test_file.txt

    // Verify entries contain expected items
    let names: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
    assert!(names.contains(&"level2".to_string()));
    assert!(names.contains(&"test_file.txt".to_string()));

    // Test file vs directory identification in entries
    for entry in &entries {
        if entry.name == "level2" {
            assert!(entry.is_directory);
        } else if entry.name == "test_file.txt" {
            assert!(!entry.is_directory);
        }
    }

    // Cleanup
    fs.delete("level1").await.unwrap();
}

#[tokio::test]
async fn test_local_filesystem_file_operations_with_options() {
    let (fs, _temp_dir) = create_test_filesystem().await;

    // Test file creation with directory creation
    let nested_file = "auto_create/subdirs/test.txt";
    let test_data = b"Auto-created directories test";
    
    let options = FileOptions {
        create_dirs: true,
        overwrite: true,
        ..Default::default()
    };
    
    fs.write(nested_file, test_data, Some(options)).await.unwrap();
    assert!(fs.exists(nested_file).await.unwrap());
    assert!(fs.exists("auto_create").await.unwrap());
    assert!(fs.exists("auto_create/subdirs").await.unwrap());

    // Test append operation
    let append_data = b" - appended content";
    fs.append(nested_file, append_data).await.unwrap();

    let full_content = fs.read(nested_file).await.unwrap();
    let expected_content = [test_data, append_data].concat();
    assert_eq!(full_content, expected_content);

    // Test move operation
    let source_path = "source_file.txt";
    let dest_path = "destination_file.txt";
    
    fs.write(source_path, b"Move test data", None).await.unwrap();
    fs.move_file(source_path, dest_path).await.unwrap();
    
    assert!(!fs.exists(source_path).await.unwrap());
    assert!(fs.exists(dest_path).await.unwrap());
    
    let moved_data = fs.read(dest_path).await.unwrap();
    assert_eq!(moved_data, b"Move test data");

    // Cleanup
    fs.delete("auto_create").await.unwrap();
    fs.delete(dest_path).await.unwrap();
}

#[tokio::test]
async fn test_local_filesystem_atomic_operations() {
    let temp_dir = TempDir::new().unwrap();
    let config = LocalConfig {
        root_dir: Some(temp_dir.path().to_path_buf()),
        sync_enabled: true,
        ..Default::default()
    };

    let fs = LocalFileSystem::new(config).await.unwrap();

    // Test atomic write capability
    assert!(fs.supports_atomic_writes());

    // Test write_atomic operation
    let test_data = b"Atomic write test data";
    let test_path = "atomic_test.txt";
    
    let options = FileOptions {
        create_dirs: true,
        overwrite: true,
        ..Default::default()
    };
    
    fs.write_atomic(test_path, test_data, Some(options)).await.unwrap();
    assert!(fs.exists(test_path).await.unwrap());

    let read_data = fs.read(test_path).await.unwrap();
    assert_eq!(read_data, test_data);

    // Test sync operation
    fs.sync().await.unwrap();

    // Cleanup
    fs.delete(test_path).await.unwrap();
}

#[tokio::test]
async fn test_local_filesystem_path_resolution() {
    let (fs, temp_dir) = create_test_filesystem().await;

    // Test relative path resolution
    let test_data = b"Path resolution test";
    let relative_path = "subdir/relative_test.txt";
    
    let options = FileOptions {
        create_dirs: true,
        ..Default::default()
    };
    
    fs.write(relative_path, test_data, Some(options)).await.unwrap();
    assert!(fs.exists(relative_path).await.unwrap());

    // Test absolute path handling (should work within root)
    let absolute_path = temp_dir.path().join("absolute_test.txt");
    fs.write(&absolute_path.to_string_lossy(), test_data, None).await.unwrap();
    assert!(fs.exists(&absolute_path.to_string_lossy()).await.unwrap());

    // Cleanup
    fs.delete(relative_path).await.unwrap();
    fs.delete(&absolute_path.to_string_lossy()).await.unwrap();
    fs.delete("subdir").await.unwrap();
}

#[tokio::test]
async fn test_local_filesystem_metadata_operations() {
    let (fs, _temp_dir) = create_test_filesystem().await;

    // Test file metadata
    let test_data = b"Metadata test content with some length";
    let test_path = "metadata_test.txt";
    
    fs.write(test_path, test_data, None).await.unwrap();
    
    let metadata = fs.metadata(test_path).await.unwrap();
    assert_eq!(metadata.size, test_data.len() as u64);
    assert!(!metadata.is_directory);
    assert!(metadata.path.ends_with("metadata_test.txt"));
    assert!(metadata.created.is_some() || metadata.modified.is_some());

    #[cfg(unix)]
    {
        // On Unix systems, permissions should be available
        assert!(metadata.permissions.is_some());
    }

    // Test directory metadata
    let dir_path = "metadata_dir";
    fs.create_dir(dir_path).await.unwrap();
    
    let dir_metadata = fs.metadata(dir_path).await.unwrap();
    assert!(dir_metadata.is_directory);
    assert!(dir_metadata.path.ends_with("metadata_dir"));

    // Cleanup
    fs.delete(test_path).await.unwrap();
    fs.delete(dir_path).await.unwrap();
}

#[tokio::test]
async fn test_local_filesystem_large_file_operations() {
    let (fs, _temp_dir) = create_test_filesystem().await;

    // Test with larger file (10KB)
    let large_data = vec![0xAB; 10240];
    let large_file_path = "large_file_test.bin";
    
    fs.write(large_file_path, &large_data, None).await.unwrap();
    assert!(fs.exists(large_file_path).await.unwrap());

    let read_data = fs.read(large_file_path).await.unwrap();
    assert_eq!(read_data.len(), large_data.len());
    assert_eq!(read_data, large_data);

    let metadata = fs.metadata(large_file_path).await.unwrap();
    assert_eq!(metadata.size, large_data.len() as u64);

    // Cleanup
    fs.delete(large_file_path).await.unwrap();
}

#[tokio::test]
async fn test_local_filesystem_concurrent_operations() {
    let temp_dir = TempDir::new().unwrap();
    let config = LocalConfig {
        root_dir: Some(temp_dir.path().to_path_buf()),
        ..Default::default()
    };

    let fs = std::sync::Arc::new(LocalFileSystem::new(config).await.unwrap());
    
    // Test concurrent file operations
    let mut handles = Vec::new();
    
    for i in 0..10 {
        let fs_clone = fs.clone();
        let test_data = format!("Concurrent test data {}", i).into_bytes();
        let test_path = format!("concurrent_test_{}.txt", i);
        
        let handle = tokio::spawn(async move {
            fs_clone.write(&test_path, &test_data, None).await.unwrap();
            let read_data = fs_clone.read(&test_path).await.unwrap();
            assert_eq!(read_data, test_data);
            fs_clone.delete(&test_path).await.unwrap();
        });
        
        handles.push(handle);
    }
    
    // Wait for all operations to complete
    for handle in handles {
        handle.await.unwrap();
    }
}

#[tokio::test]
async fn test_local_filesystem_invalid_root_directory() {
    // Test with non-existent root directory
    let non_existent_path = PathBuf::from("/definitely/does/not/exist/path");
    let config = LocalConfig {
        root_dir: Some(non_existent_path),
        ..Default::default()
    };

    let result = LocalFileSystem::new(config).await;
    assert!(result.is_err());
    if let Err(FilesystemError::NotFound(_)) = result {
        // Expected error type
    } else {
        panic!("Expected NotFound error for non-existent root directory");
    }
}

#[tokio::test]
async fn test_local_filesystem_without_root_directory() {
    // Test with no root directory configured
    let config = LocalConfig {
        root_dir: None,
        ..Default::default()
    };

    let fs = LocalFileSystem::new(config).await.unwrap();

    // This should work in the current directory
    let test_data = b"No root dir test";
    let test_path = "no_root_test.txt";
    
    fs.write(test_path, test_data, None).await.unwrap();
    assert!(fs.exists(test_path).await.unwrap());

    let read_data = fs.read(test_path).await.unwrap();
    assert_eq!(read_data, test_data);

    // Cleanup
    fs.delete(test_path).await.unwrap();
}

#[tokio::test]
async fn test_local_filesystem_type_identification() {
    let (fs, _temp_dir) = create_test_filesystem().await;
    
    // Test filesystem type
    assert_eq!(fs.filesystem_type(), "local");
}

#[tokio::test]
async fn test_local_filesystem_sync_operations() {
    let temp_dir = TempDir::new().unwrap();
    let config = LocalConfig {
        root_dir: Some(temp_dir.path().to_path_buf()),
        sync_enabled: true,
        ..Default::default()
    };

    let fs = LocalFileSystem::new(config).await.unwrap();
    
    // Test sync operation
    fs.sync().await.unwrap();
    
    // Test with sync disabled
    let config_no_sync = LocalConfig {
        root_dir: Some(temp_dir.path().to_path_buf()),
        sync_enabled: false,
        ..Default::default()
    };

    let fs_no_sync = LocalFileSystem::new(config_no_sync).await.unwrap();
    let test_data = b"No sync test data";
    let test_path = "no_sync_test.txt";
    
    fs_no_sync.write(test_path, test_data, None).await.unwrap();
    fs_no_sync.append(test_path, b" appended").await.unwrap();
    
    let read_data = fs_no_sync.read(test_path).await.unwrap();
    assert!(read_data.starts_with(test_data));
    
    fs_no_sync.delete(test_path).await.unwrap();
}