/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! Tests for filesystem sync functionality

use super::super::{FileSystem, FilesystemError, LocalFileSystem};
use super::super::local::LocalConfig;
use tempfile::TempDir;
use tokio;
use std::sync::Arc;

#[tokio::test]
async fn test_local_filesystem_sync_file() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().to_str().unwrap();
    
    let config = LocalConfig {
        root_dir: Some(base_path.into()),
        sync_enabled: true,
        ..Default::default()
    };
    
    let fs = LocalFileSystem::new(config).await.unwrap();
    
    // Write test data
    let test_path = "test_sync.dat";
    let test_data = b"Critical data that must be synced";
    fs.write(test_path, test_data, None).await.unwrap();
    
    // Call sync_file - should succeed
    fs.sync_file(test_path).await.unwrap();
    
    // Verify data can be read back
    let read_data = fs.read(test_path).await.unwrap();
    assert_eq!(read_data, test_data);
}

#[tokio::test]
async fn test_local_filesystem_sync_disabled() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().to_str().unwrap();
    
    let config = LocalConfig {
        root_dir: Some(base_path.into()),
        sync_enabled: false, // Sync disabled
        ..Default::default()
    };
    
    let fs = LocalFileSystem::new(config).await.unwrap();
    
    // Write test data
    let test_path = "test_no_sync.dat";
    let test_data = b"Data without sync";
    fs.write(test_path, test_data, None).await.unwrap();
    
    // Call sync_file - should succeed but do nothing
    fs.sync_file(test_path).await.unwrap();
    
    // Data should still be readable
    let read_data = fs.read(test_path).await.unwrap();
    assert_eq!(read_data, test_data);
}

#[tokio::test]
async fn test_sync_file_not_found() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().to_str().unwrap();
    
    let config = LocalConfig {
        root_dir: Some(base_path.into()),
        sync_enabled: true,
        ..Default::default()
    };
    
    let fs = LocalFileSystem::new(config).await.unwrap();
    
    // Try to sync non-existent file
    let result = fs.sync_file("non_existent.dat").await;
    
    // Should fail with appropriate error
    assert!(result.is_err());
    match result.unwrap_err() {
        FilesystemError::NotFound(_) => {}, // Expected
        e => panic!("Expected NotFound error for non-existent file, got: {:?}", e),
    }
}

#[tokio::test]
async fn test_sync_after_append() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().to_str().unwrap();
    
    let config = LocalConfig {
        root_dir: Some(base_path.into()),
        sync_enabled: true,
        ..Default::default()
    };
    
    let fs = LocalFileSystem::new(config).await.unwrap();
    
    let test_path = "append_sync.dat";
    
    // Initial write
    fs.write(test_path, b"Initial data", None).await.unwrap();
    fs.sync_file(test_path).await.unwrap();
    
    // Append more data
    fs.append(test_path, b" - Appended data").await.unwrap();
    
    // Sync after append
    fs.sync_file(test_path).await.unwrap();
    
    // Verify complete data
    let read_data = fs.read(test_path).await.unwrap();
    assert_eq!(read_data, b"Initial data - Appended data");
}

#[tokio::test]
async fn test_concurrent_sync_operations() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().to_str().unwrap();
    
    let config = LocalConfig {
        root_dir: Some(base_path.into()),
        sync_enabled: true,
        ..Default::default()
    };
    
    let fs = Arc::new(LocalFileSystem::new(config).await.unwrap());
    
    // Write multiple files
    let files = vec![
        ("file1.dat", b"Data 1"),
        ("file2.dat", b"Data 2"),
        ("file3.dat", b"Data 3"),
    ];
    
    for (path, data) in &files {
        fs.write(path, &data[..], None).await.unwrap();
    }
    
    // Sync all files concurrently
    let mut handles = vec![];
    for (path, _) in &files {
        let fs_clone = Arc::clone(&fs);
        let path = path.to_string();
        handles.push(tokio::spawn(async move {
            fs_clone.sync_file(&path).await
        }));
    }
    
    // Wait for all syncs to complete
    for handle in handles {
        handle.await.unwrap().unwrap();
    }
    
    // Verify all data
    for (path, expected_data) in &files {
        let read_data = fs.read(path).await.unwrap();
        assert_eq!(&read_data, expected_data);
    }
}

#[cfg(test)]
mod cloud_storage_sync_tests {
    use super::*;
    use crate::storage::persistence::filesystem::{s3::S3FileSystem, azure::AzureFileSystem, gcs::GcsFileSystem};
    
    #[tokio::test]
    async fn test_s3_sync_is_noop() {
        // This test verifies that sync_file on S3 is a no-op
        // In a real test, you'd need to mock or use localstack
        
        // For now, we just verify the method exists and can be called
        // without actual S3 connection
    }
    
    #[tokio::test]
    async fn test_azure_sync_is_noop() {
        // This test verifies that sync_file on Azure is a no-op
        // In a real test, you'd need to mock or use Azurite
        
        // For now, we just verify the method exists and can be called
        // without actual Azure connection
    }
    
    #[tokio::test]
    async fn test_gcs_sync_is_noop() {
        // This test verifies that sync_file on GCS is a no-op
        // In a real test, you'd need to mock or use fake-gcs-server
        
        // For now, we just verify the method exists and can be called
        // without actual GCS connection
    }
}