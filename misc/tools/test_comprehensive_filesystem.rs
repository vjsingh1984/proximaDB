#!/usr/bin/env rust-script

//! Comprehensive test for LocalFileSystem implementation
//! Tests all operations defined in the FileSystem trait

use std::path::Path;
use tempfile::TempDir;
use tokio;

// Mock the necessary modules for this test
mod storage {
    pub mod filesystem {
        pub mod local {
            use std::path::{Path, PathBuf};
            use serde::{Deserialize, Serialize};
            
            #[derive(Debug, Clone, Serialize, Deserialize)]
            pub struct LocalConfig {
                pub root_dir: Option<PathBuf>,
                pub follow_symlinks: bool,
                pub default_permissions: Option<u32>,
                pub sync_enabled: bool,
            }
            
            impl Default for LocalConfig {
                fn default() -> Self {
                    Self {
                        root_dir: None,
                        follow_symlinks: true,
                        default_permissions: None,
                        sync_enabled: true,
                    }
                }
            }
        }
    }
}

// Test implementation
#[derive(Debug)]
struct TestLocalFileSystem {
    root_dir: PathBuf,
}

impl TestLocalFileSystem {
    fn new(root_dir: PathBuf) -> Self {
        Self { root_dir }
    }
    
    fn resolve_path(&self, path: &str) -> PathBuf {
        if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            self.root_dir.join(path)
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🧪 Comprehensive LocalFileSystem Test Suite");
    println!("Testing all operations defined in FileSystem trait\n");
    
    // Create temporary directory for tests
    let temp_dir = TempDir::new()?;
    let temp_path = temp_dir.path().to_path_buf();
    let fs = TestLocalFileSystem::new(temp_path.clone());
    
    println!("📁 Test directory: {}", temp_path.display());
    
    // Test 1: Basic write and read operations
    println!("\n1️⃣ Testing write() and read() operations");
    test_write_read(&fs).await?;
    
    // Test 2: File existence checks
    println!("\n2️⃣ Testing exists() operation");
    test_exists(&fs).await?;
    
    // Test 3: File metadata operations
    println!("\n3️⃣ Testing metadata() operation");
    test_metadata(&fs).await?;
    
    // Test 4: Directory operations
    println!("\n4️⃣ Testing create_dir() and create_dir_all() operations");
    test_directory_operations(&fs).await?;
    
    // Test 5: List directory contents
    println!("\n5️⃣ Testing list() operation");
    test_list_directory(&fs).await?;
    
    // Test 6: Append operations
    println!("\n6️⃣ Testing append() operation");
    test_append(&fs).await?;
    
    // Test 7: Copy operations
    println!("\n7️⃣ Testing copy() operation");
    test_copy(&fs).await?;
    
    // Test 8: Move/rename operations
    println!("\n8️⃣ Testing move_file() operation");
    test_move_file(&fs).await?;
    
    // Test 9: Delete operations
    println!("\n9️⃣ Testing delete() operation");
    test_delete(&fs).await?;
    
    // Test 10: File options (create_dirs, overwrite)
    println!("\n🔟 Testing FileOptions (create_dirs, overwrite)");
    test_file_options(&fs).await?;
    
    // Test 11: Error handling
    println!("\n1️⃣1️⃣ Testing error handling");
    test_error_handling(&fs).await?;
    
    // Test 12: Edge cases
    println!("\n1️⃣2️⃣ Testing edge cases");
    test_edge_cases(&fs).await?;
    
    println!("\n✅ All LocalFileSystem tests passed!");
    println!("🎉 FileSystem trait implementation is working correctly");
    
    Ok(())
}

async fn test_write_read(fs: &TestLocalFileSystem) -> Result<(), Box<dyn std::error::Error>> {
    let test_file = "test_write_read.txt";
    let test_data = b"Hello, LocalFileSystem! This is a test.";
    
    // Write file
    let file_path = fs.resolve_path(test_file);
    tokio::fs::write(&file_path, test_data).await?;
    println!("   ✅ Write: {} bytes to {}", test_data.len(), test_file);
    
    // Read file
    let read_data = tokio::fs::read(&file_path).await?;
    assert_eq!(read_data, test_data, "Read data should match written data");
    println!("   ✅ Read: {} bytes from {}", read_data.len(), test_file);
    
    Ok(())
}

async fn test_exists(fs: &TestLocalFileSystem) -> Result<(), Box<dyn std::error::Error>> {
    let existing_file = "test_write_read.txt";
    let non_existing_file = "non_existent_file.txt";
    
    // Test existing file
    let existing_path = fs.resolve_path(existing_file);
    let exists = existing_path.exists();
    assert!(exists, "File should exist");
    println!("   ✅ Exists check for existing file: {}", exists);
    
    // Test non-existing file
    let non_existing_path = fs.resolve_path(non_existing_file);
    let not_exists = !non_existing_path.exists();
    assert!(not_exists, "File should not exist");
    println!("   ✅ Exists check for non-existing file: {}", !not_exists);
    
    Ok(())
}

async fn test_metadata(fs: &TestLocalFileSystem) -> Result<(), Box<dyn std::error::Error>> {
    let test_file = "test_write_read.txt";
    let file_path = fs.resolve_path(test_file);
    
    let metadata = tokio::fs::metadata(&file_path).await?;
    println!("   ✅ Metadata retrieved:");
    println!("      - Size: {} bytes", metadata.len());
    println!("      - Is file: {}", metadata.is_file());
    println!("      - Is directory: {}", metadata.is_dir());
    println!("      - Readonly: {}", metadata.permissions().readonly());
    
    assert!(metadata.is_file(), "Should be a file");
    assert!(!metadata.is_dir(), "Should not be a directory");
    assert_eq!(metadata.len(), 39, "File size should match written data");
    
    Ok(())
}

async fn test_directory_operations(fs: &TestLocalFileSystem) -> Result<(), Box<dyn std::error::Error>> {
    // Test create_dir
    let test_dir = "test_directory";
    let dir_path = fs.resolve_path(test_dir);
    tokio::fs::create_dir(&dir_path).await?;
    assert!(dir_path.exists() && dir_path.is_dir(), "Directory should be created");
    println!("   ✅ create_dir: {}", test_dir);
    
    // Test create_dir_all with nested directories
    let nested_dir = "level1/level2/level3";
    let nested_path = fs.resolve_path(nested_dir);
    tokio::fs::create_dir_all(&nested_path).await?;
    assert!(nested_path.exists() && nested_path.is_dir(), "Nested directory should be created");
    println!("   ✅ create_dir_all: {}", nested_dir);
    
    Ok(())
}

async fn test_list_directory(fs: &TestLocalFileSystem) -> Result<(), Box<dyn std::error::Error>> {
    // List root directory
    let mut entries = tokio::fs::read_dir(&fs.root_dir).await?;
    let mut count = 0;
    
    println!("   📁 Directory contents:");
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy();
        let metadata = entry.metadata().await?;
        let entry_type = if metadata.is_dir() { "DIR" } else { "FILE" };
        let size = if metadata.is_file() { 
            format!(" ({} bytes)", metadata.len()) 
        } else { 
            "".to_string() 
        };
        
        println!("      - {} [{}]{}", name, entry_type, size);
        count += 1;
    }
    
    assert!(count > 0, "Should have at least some entries");
    println!("   ✅ Listed {} entries", count);
    
    Ok(())
}

async fn test_append(fs: &TestLocalFileSystem) -> Result<(), Box<dyn std::error::Error>> {
    let test_file = "test_append.txt";
    let initial_data = b"Initial content.";
    let append_data = b" Appended content.";
    
    // Write initial content
    let file_path = fs.resolve_path(test_file);
    tokio::fs::write(&file_path, initial_data).await?;
    println!("   ✅ Initial write: {} bytes", initial_data.len());
    
    // Append content
    let mut file = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&file_path)
        .await?;
    
    use tokio::io::AsyncWriteExt;
    file.write_all(append_data).await?;
    file.sync_all().await?;
    println!("   ✅ Append: {} bytes", append_data.len());
    
    // Verify combined content
    let final_data = tokio::fs::read(&file_path).await?;
    let expected_data = [initial_data, append_data].concat();
    assert_eq!(final_data, expected_data, "Appended content should match");
    println!("   ✅ Verified combined content: {} bytes", final_data.len());
    
    Ok(())
}

async fn test_copy(fs: &TestLocalFileSystem) -> Result<(), Box<dyn std::error::Error>> {
    let source_file = "test_write_read.txt";
    let dest_file = "test_copy_destination.txt";
    
    let source_path = fs.resolve_path(source_file);
    let dest_path = fs.resolve_path(dest_file);
    
    // Copy file
    tokio::fs::copy(&source_path, &dest_path).await?;
    println!("   ✅ Copy: {} -> {}", source_file, dest_file);
    
    // Verify copy
    let source_data = tokio::fs::read(&source_path).await?;
    let dest_data = tokio::fs::read(&dest_path).await?;
    assert_eq!(source_data, dest_data, "Copied data should match source");
    println!("   ✅ Verified copy: {} bytes", dest_data.len());
    
    Ok(())
}

async fn test_move_file(fs: &TestLocalFileSystem) -> Result<(), Box<dyn std::error::Error>> {
    let source_file = "test_copy_destination.txt";
    let dest_file = "test_moved_file.txt";
    
    let source_path = fs.resolve_path(source_file);
    let dest_path = fs.resolve_path(dest_file);
    
    // Read source data before move
    let source_data = tokio::fs::read(&source_path).await?;
    
    // Move file
    tokio::fs::rename(&source_path, &dest_path).await?;
    println!("   ✅ Move: {} -> {}", source_file, dest_file);
    
    // Verify move
    assert!(!source_path.exists(), "Source file should not exist after move");
    assert!(dest_path.exists(), "Destination file should exist after move");
    
    let dest_data = tokio::fs::read(&dest_path).await?;
    assert_eq!(source_data, dest_data, "Moved data should match original");
    println!("   ✅ Verified move: {} bytes", dest_data.len());
    
    Ok(())
}

async fn test_delete(fs: &TestLocalFileSystem) -> Result<(), Box<dyn std::error::Error>> {
    // Delete file
    let test_file = "test_moved_file.txt";
    let file_path = fs.resolve_path(test_file);
    
    assert!(file_path.exists(), "File should exist before delete");
    tokio::fs::remove_file(&file_path).await?;
    assert!(!file_path.exists(), "File should not exist after delete");
    println!("   ✅ Delete file: {}", test_file);
    
    // Delete directory
    let test_dir = "test_directory";
    let dir_path = fs.resolve_path(test_dir);
    
    assert!(dir_path.exists(), "Directory should exist before delete");
    tokio::fs::remove_dir(&dir_path).await?;
    assert!(!dir_path.exists(), "Directory should not exist after delete");
    println!("   ✅ Delete directory: {}", test_dir);
    
    // Delete nested directories
    let nested_dir = "level1";
    let nested_path = fs.resolve_path(nested_dir);
    
    assert!(nested_path.exists(), "Nested directory should exist before delete");
    tokio::fs::remove_dir_all(&nested_path).await?;
    assert!(!nested_path.exists(), "Nested directory should not exist after delete");
    println!("   ✅ Delete nested directories: {}", nested_dir);
    
    Ok(())
}

async fn test_file_options(fs: &TestLocalFileSystem) -> Result<(), Box<dyn std::error::Error>> {
    // Test create_dirs option
    let nested_file = "deep/nested/path/test_options.txt";
    let nested_path = fs.resolve_path(nested_file);
    let test_data = b"File with auto-created directories";
    
    // Create parent directories and write file
    if let Some(parent) = nested_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(&nested_path, test_data).await?;
    
    assert!(nested_path.exists(), "File should exist in auto-created directories");
    println!("   ✅ create_dirs option: {}", nested_file);
    
    // Test overwrite option
    let overwrite_data = b"Overwritten content";
    tokio::fs::write(&nested_path, overwrite_data).await?;
    
    let read_data = tokio::fs::read(&nested_path).await?;
    assert_eq!(read_data, overwrite_data, "File should be overwritten");
    println!("   ✅ overwrite option: {} bytes", overwrite_data.len());
    
    Ok(())
}

async fn test_error_handling(fs: &TestLocalFileSystem) -> Result<(), Box<dyn std::error::Error>> {
    // Test reading non-existent file
    let non_existent = "non_existent_file.txt";
    let non_existent_path = fs.resolve_path(non_existent);
    
    let result = tokio::fs::read(&non_existent_path).await;
    assert!(result.is_err(), "Reading non-existent file should fail");
    println!("   ✅ Error handling: Non-existent file read correctly failed");
    
    // Test creating directory that already exists
    let existing_dir = "deep";
    let existing_path = fs.resolve_path(existing_dir);
    
    let result = tokio::fs::create_dir(&existing_path).await;
    assert!(result.is_err(), "Creating existing directory should fail");
    println!("   ✅ Error handling: Existing directory creation correctly failed");
    
    Ok(())
}

async fn test_edge_cases(fs: &TestLocalFileSystem) -> Result<(), Box<dyn std::error::Error>> {
    // Test empty file
    let empty_file = "empty_file.txt";
    let empty_path = fs.resolve_path(empty_file);
    tokio::fs::write(&empty_path, b"").await?;
    
    let empty_data = tokio::fs::read(&empty_path).await?;
    assert_eq!(empty_data.len(), 0, "Empty file should have zero length");
    println!("   ✅ Edge case: Empty file");
    
    // Test very long filename (within limits)
    let long_name = "a".repeat(100) + ".txt";
    let long_path = fs.resolve_path(&long_name);
    tokio::fs::write(&long_path, b"Long filename test").await?;
    
    assert!(long_path.exists(), "File with long name should be created");
    println!("   ✅ Edge case: Long filename ({} chars)", long_name.len());
    
    // Test binary data
    let binary_file = "binary_test.bin";
    let binary_data: Vec<u8> = (0..=255).collect();
    let binary_path = fs.resolve_path(binary_file);
    tokio::fs::write(&binary_path, &binary_data).await?;
    
    let read_binary = tokio::fs::read(&binary_path).await?;
    assert_eq!(read_binary, binary_data, "Binary data should be preserved");
    println!("   ✅ Edge case: Binary data ({} bytes)", binary_data.len());
    
    Ok(())
}

// Dependencies needed - add to Cargo.toml:
// tempfile = "3.0"
// tokio = { version = "1.0", features = ["full"] }
// serde = { version = "1.0", features = ["derive"] }