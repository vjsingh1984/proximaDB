// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Storage encryption integration tests
//!
//! Tests file encryption at rest for SOC 2 compliance.

use std::sync::Arc;
use tempfile::TempDir;

use proximadb::storage::encryption::{
    EncryptionConfig, FileEncryptionLayer, KeyManager, KeyVersionManager,
};
use proximadb::storage::persistence::filesystem::{FileSystem, FsResult};
use proximadb::storage::persistence::filesystem::local::{LocalConfig, LocalFileSystem};

#[tokio::test]
async fn test_file_encryption_roundtrip() {
    // Create temporary directory
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let config = LocalConfig {
        root_dir: Some(temp_dir.path().to_path_buf()),
        ..Default::default()
    };

    // Create key manager with test key
    let master_key = [0u8; 32]; // Test key
    let key_manager = Arc::new(KeyManager::new(master_key));
    let key_version_manager = Arc::new(KeyVersionManager::new(key_manager));

    // Create encryption layer
    let encryption_config = EncryptionConfig {
        enabled: true,
        ..Default::default()
    };
    let encryption_layer = Arc::new(FileEncryptionLayer::new(
        key_version_manager,
        encryption_config.enabled,
        encryption_config.chunk_size,
    ));

    // Create filesystem with encryption
    let fs = LocalFileSystem::new_with_encryption(config, Some(encryption_layer))
        .await
        .expect("Failed to create filesystem");

    // Test data
    let test_data = b"Hello, encrypted world!";
    let file_path = temp_dir.path().join("encrypted_file.txt");
    let test_path = file_path.to_str().unwrap();

    // Write encrypted data
    fs.write(test_path, test_data, None)
        .await
        .expect("Failed to write encrypted data");

    // Read back and verify
    let read_data = fs.read(test_path)
        .await
        .expect("Failed to read encrypted data");

    assert_eq!(read_data, test_data, "Decrypted data should match original");
}

#[tokio::test]
async fn test_file_encryption_disabled() {
    // Create temporary directory
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let config = LocalConfig {
        root_dir: Some(temp_dir.path().to_path_buf()),
        ..Default::default()
    };

    // Create filesystem WITHOUT encryption
    let fs = LocalFileSystem::new(config)
        .await
        .expect("Failed to create filesystem");

    // Test data
    let test_data = b"Hello, unencrypted world!";
    let file_path = temp_dir.path().join("unencrypted_file.txt");
    let test_path = file_path.to_str().unwrap();

    // Write data
    fs.write(test_path, test_data, None)
        .await
        .expect("Failed to write data");

    // Read back and verify
    let read_data = fs.read(test_path)
        .await
        .expect("Failed to read data");

    assert_eq!(read_data, test_data, "Data should match original");
}

#[tokio::test]
async fn test_file_encryption_with_large_data() {
    // Create temporary directory
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let config = LocalConfig {
        root_dir: Some(temp_dir.path().to_path_buf()),
        ..Default::default()
    };

    // Create key manager with test key
    let master_key = [1u8; 32]; // Different test key
    let key_manager = Arc::new(KeyManager::new(master_key));
    let key_version_manager = Arc::new(KeyVersionManager::new(key_manager));

    // Create encryption layer with 1KB chunks
    let encryption_layer = Arc::new(FileEncryptionLayer::new(
        key_version_manager,
        true,
        1024, // 1KB chunks
    ));

    // Create filesystem with encryption
    let fs = LocalFileSystem::new_with_encryption(config, Some(encryption_layer))
        .await
        .expect("Failed to create filesystem");

    // Test data larger than chunk size (5KB)
    let test_data = vec![42u8; 5 * 1024];
    let file_path = temp_dir.path().join("large_encrypted_file.bin");
    let test_path = file_path.to_str().unwrap();

    // Write encrypted data
    fs.write(test_path, &test_data, None)
        .await
        .expect("Failed to write large encrypted data");

    // Read back and verify
    let read_data = fs.read(test_path)
        .await
        .expect("Failed to read large encrypted data");

    assert_eq!(read_data, test_data, "Decrypted large data should match original");
}

#[tokio::test]
async fn test_encryption_layer_access() {
    // Create temporary directory
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let config = LocalConfig {
        root_dir: Some(temp_dir.path().to_path_buf()),
        ..Default::default()
    };

    // Create key manager with test key
    let master_key = [2u8; 32];
    let key_manager = Arc::new(KeyManager::new(master_key));
    let key_version_manager = Arc::new(KeyVersionManager::new(key_manager));

    // Create encryption layer
    let encryption_layer = Arc::new(FileEncryptionLayer::new(
        key_version_manager,
        true,
        4096,
    ));

    // Create filesystem with encryption
    let fs = LocalFileSystem::new_with_encryption(config, Some(encryption_layer))
        .await
        .expect("Failed to create filesystem");

    // Verify encryption layer is accessible
    assert!(
        fs.encryption_layer().is_some(),
        "Encryption layer should be accessible"
    );
}

#[tokio::test]
async fn test_encryption_without_key_fails_gracefully() {
    // Create temporary directory
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let config = LocalConfig {
        root_dir: Some(temp_dir.path().to_path_buf()),
        ..Default::default()
    };

    // Create key manager with one key
    let master_key = [3u8; 32];
    let key_manager = Arc::new(KeyManager::new(master_key));
    let key_version_manager = Arc::new(KeyVersionManager::new(key_manager));

    // Create filesystem with encryption
    let encryption_layer = Arc::new(FileEncryptionLayer::new(
        key_version_manager,
        true,
        4096,
    ));
    let fs = LocalFileSystem::new_with_encryption(config, Some(encryption_layer))
        .await
        .expect("Failed to create filesystem");

    // Write encrypted data
    let test_data = b"Secret data";
    let file_path = temp_dir.path().join("secret_file.txt");
    let test_path = file_path.to_str().unwrap();

    fs.write(test_path, test_data, None)
        .await
        .expect("Failed to write encrypted data");

    // Create a new filesystem with a different key (simulating key rotation)
    let temp_dir2 = TempDir::new().expect("Failed to create second temp dir");
    let config2 = LocalConfig {
        root_dir: Some(temp_dir2.path().to_path_buf()),
        ..Default::default()
    };

    let different_key = [4u8; 32]; // Different key
    let key_manager2 = Arc::new(KeyManager::new(different_key));
    let key_version_manager2 = Arc::new(KeyVersionManager::new(key_manager2));

    let encryption_layer2 = Arc::new(FileEncryptionLayer::new(
        key_version_manager2,
        true,
        4096,
    ));

    // Copy the encrypted file to the new temp directory
    let old_path = temp_dir.path().join("secret_file.txt");
    let new_path = temp_dir2.path().join("secret_file.txt");
    std::fs::copy(old_path, new_path).expect("Failed to copy file");

    let fs2 = LocalFileSystem::new_with_encryption(config2, Some(encryption_layer2))
        .await
        .expect("Failed to create second filesystem");

    // Attempt to read with wrong key - should fail gracefully
    let file_path2 = temp_dir2.path().join("secret_file.txt");
    let test_path2 = file_path2.to_str().unwrap();
    let result: FsResult<Vec<u8>> = fs2.read(test_path2).await;

    assert!(
        result.is_err(),
        "Reading with wrong key should fail: {:?}",
        result
    );
}
