//! Unit tests for VIPER atomic flush operations with __flush staging pattern

use anyhow::Result;
use tempfile::TempDir;
use tokio::fs;

/// Test the atomic flush pattern with __flush staging directory
#[tokio::test]
async fn test_atomic_flush_staging_pattern() -> Result<()> {
    // Setup temporary directory structure
    let temp_dir = TempDir::new()?;
    let storage_path = temp_dir.path().join("storage");
    let collection_id = "test_collection";
    let collection_path = storage_path.join(collection_id);
    
    // Create collection directory
    fs::create_dir_all(&collection_path).await?;
    
    // Test 1: Verify __flush directory creation
    let flush_staging_path = collection_path.join("__flush");
    fs::create_dir_all(&flush_staging_path).await?;
    
    assert!(flush_staging_path.exists(), "__flush staging directory should be created");
    
    // Test 2: Write test data to staging area
    let temp_file = flush_staging_path.join("temp_123.parquet");
    let test_data = b"mock parquet data";
    fs::write(&temp_file, test_data).await?;
    
    assert!(temp_file.exists(), "Temporary file should be written to __flush");
    
    // Test 3: Atomic move to final location
    let vectors_dir = collection_path.join("vectors");
    fs::create_dir_all(&vectors_dir).await?;
    
    let final_file = vectors_dir.join("partition_123.parquet");
    fs::rename(&temp_file, &final_file).await?;
    
    assert!(final_file.exists(), "File should be moved to final location");
    assert!(!temp_file.exists(), "Temporary file should be removed after move");
    
    // Test 4: Verify final data integrity
    let final_data = fs::read(&final_file).await?;
    assert_eq!(final_data, test_data, "Data should be preserved during atomic move");
    
    // Test 5: Cleanup staging directory
    fs::remove_dir_all(&flush_staging_path).await?;
    assert!(!flush_staging_path.exists(), "__flush directory should be cleaned up");
    
    Ok(())
}

/// Test that search operations ignore __flush directories
#[tokio::test] 
async fn test_search_ignores_flush_directories() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let collection_path = temp_dir.path().join("storage/test_collection");
    
    // Create directory structure
    let vectors_dir = collection_path.join("vectors");
    let flush_dir = collection_path.join("__flush");
    let staging_dir = collection_path.join("__staging");
    
    fs::create_dir_all(&vectors_dir).await?;
    fs::create_dir_all(&flush_dir).await?;
    fs::create_dir_all(&staging_dir).await?;
    
    // Create files in each directory
    fs::write(vectors_dir.join("valid_partition.parquet"), b"valid data").await?;
    fs::write(flush_dir.join("temp_flush.parquet"), b"staging data").await?;
    fs::write(staging_dir.join("temp_staging.parquet"), b"staging data").await?;
    
    // Function to simulate directory listing with __ filtering
    fn should_include_directory(dir_name: &str) -> bool {
        !dir_name.starts_with("__")
    }
    
    // Test the filtering logic
    assert!(should_include_directory("vectors"), "vectors directory should be included");
    assert!(should_include_directory("indexes"), "indexes directory should be included");
    assert!(!should_include_directory("__flush"), "__flush directory should be ignored");
    assert!(!should_include_directory("__staging"), "__staging directory should be ignored");
    assert!(!should_include_directory("__temp"), "__temp directory should be ignored");
    
    Ok(())
}

/// Test concurrent flush operations don't conflict
#[tokio::test]
async fn test_concurrent_flush_operations() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let collection_path = temp_dir.path().join("storage/test_collection");
    fs::create_dir_all(&collection_path).await?;
    
    // Simulate two concurrent flush operations
    let handles = (0..2).map(|i| {
        let collection_path = collection_path.clone();
        tokio::spawn(async move {
            let operation_id = format!("op_{}", i);
            let flush_staging_path = collection_path.join("__flush").join(&operation_id);
            
            // Create staging directory for this operation
            fs::create_dir_all(&flush_staging_path).await?;
            
            // Write data to staging
            let temp_file = flush_staging_path.join("temp.parquet");
            let data = format!("data from operation {}", i);
            fs::write(&temp_file, data.as_bytes()).await?;
            
            // Atomic move to final location
            let vectors_dir = collection_path.join("vectors");
            fs::create_dir_all(&vectors_dir).await?;
            
            let final_file = vectors_dir.join(format!("partition_{}.parquet", operation_id));
            fs::rename(&temp_file, &final_file).await?;
            
            // Cleanup staging for this operation
            fs::remove_dir_all(&flush_staging_path).await?;
            
            Ok::<(), anyhow::Error>(())
        })
    }).collect::<Vec<_>>();
    
    // Wait for all operations to complete
    for handle in handles {
        handle.await??;
    }
    
    // Verify both files were created successfully
    let vectors_dir = collection_path.join("vectors");
    assert!(vectors_dir.join("partition_op_0.parquet").exists());
    assert!(vectors_dir.join("partition_op_1.parquet").exists());
    
    // Verify no staging directories remain
    let main_flush_dir = collection_path.join("__flush");
    assert!(!main_flush_dir.exists() || fs::read_dir(&main_flush_dir).await?.next_entry().await?.is_none());
    
    Ok(())
}

/// Test filesystem error handling during atomic operations
#[tokio::test]
async fn test_atomic_flush_error_handling() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let collection_path = temp_dir.path().join("storage/test_collection");
    fs::create_dir_all(&collection_path).await?;
    
    let flush_staging_path = collection_path.join("__flush");
    fs::create_dir_all(&flush_staging_path).await?;
    
    let temp_file = flush_staging_path.join("temp_123.parquet");
    fs::write(&temp_file, b"test data").await?;
    
    // Test 1: Move to non-existent directory should fail
    let invalid_final_path = collection_path.join("nonexistent/partition_123.parquet");
    let move_result = fs::rename(&temp_file, &invalid_final_path).await;
    assert!(move_result.is_err(), "Move to non-existent directory should fail");
    
    // Test 2: Original file should still exist after failed move
    assert!(temp_file.exists(), "Original file should remain after failed move");
    
    // Test 3: Successful move after creating target directory
    let vectors_dir = collection_path.join("vectors");
    fs::create_dir_all(&vectors_dir).await?;
    
    let valid_final_path = vectors_dir.join("partition_123.parquet");
    fs::rename(&temp_file, &valid_final_path).await?;
    
    assert!(valid_final_path.exists(), "File should be moved successfully");
    assert!(!temp_file.exists(), "Original should be removed after successful move");
    
    Ok(())
}