//! Consistent configuration for SST tests

use proximadb::core::{SstConfig, BloomFilterConfig};
use proximadb::storage::persistence::filesystem::FilesystemConfig;
use std::path::Path;

/// Create a consistent test configuration for SST
pub fn create_test_sst_config(base_path: &str) -> SstConfig {
    SstConfig {
        // Memory settings
        memtable_size_mb: 16,  // Smaller for tests
        memory_flush_size_bytes: 1024 * 1024, // 1MB flush threshold
        write_buffer_size_mb: 4,
        cache_size_mb: 32,
        
        // Level configuration
        level_count: 4,  // Fewer levels for tests
        max_levels: 4,
        compaction_threshold: 2,  // Low threshold for testing
        max_files_per_level: 4,
        level_size_multiplier: 4.0,
        
        // Block and file settings
        block_size_kb: 16,  // Smaller blocks for tests
        
        // Storage type
        memtable_type: "skiplist".to_string(),
        compaction_strategy: "leveled".to_string(),
        compression: "none".to_string(),  // No compression for tests
        
        // Bloom filter - use consistent settings
        bloom_filter_config: Some(BloomFilterConfig {
            bits_per_key: 10,
            enabled: true,
            ..Default::default()
        }),
        
        // Background operations
        background_thread_count: 2,
        
        // Sync and persistence
        sync_mode: "immediate".to_string(),  // Immediate sync for tests
        enable_write_buffer: true,  // Enable write buffer for tests
        
        // Directories - use assignment service compatible paths
        // The assignment service will create {base_path}/{collection_id}/data
        // So we set the base path here and let assignment service handle collection paths
        write_buffer_directory: format!("{}/write_buffer", base_path),
        data_directory: format!("{}/data", base_path),
        
        // Memory mapping
        mmap_enabled: false,
        prefetch_enabled: false,
        prefetch_size_kb: 0,
    }
}

/// Create consistent filesystem configuration for tests
pub fn create_test_filesystem_config() -> FilesystemConfig {
    FilesystemConfig::default()
}

/// Setup test directories
pub async fn setup_test_directories(base_path: &Path) -> anyhow::Result<()> {
    use tokio::fs;
    
    // Create base directory
    fs::create_dir_all(base_path).await?;
    
    // Create subdirectories
    fs::create_dir_all(base_path.join("data")).await?;
    fs::create_dir_all(base_path.join("write_buffer")).await?;
    
    Ok(())
}

/// Setup storage assignment for a collection with proper directory creation
pub async fn setup_storage_assignment(collection_id: &str, base_path: &str) -> anyhow::Result<()> {
    use proximadb::core::config::StorageLocation;
    
    let assignment_service = proximadb::storage::assignment_service::get_assignment_service();
    
    // UnifiedAssignment will add /{collection_id}/data to the base URL
    let storage_location = StorageLocation {
        url: format!("file://{}", base_path),
        weight: 1,
        tags: Default::default(),
    };
    
    assignment_service
        .assign_collection(collection_id, &[storage_location], "hash")
        .await?;
    
    // Verify assignment was created and create the expected directories
    let assignment = assignment_service.get_assignment(collection_id).await;
    if let Some(assignment) = assignment {
        let data_path = assignment.data_url.strip_prefix("file://").unwrap_or(&assignment.data_url);
        tokio::fs::create_dir_all(data_path).await?;
        println!("Created data directory: {}", data_path);
    } else {
        return Err(anyhow::anyhow!("Assignment was not created for collection {}", collection_id));
    }
    
    Ok(())
}

/// Cleanup test directories  
pub async fn cleanup_test_directories(base_path: &Path) -> anyhow::Result<()> {
    use tokio::fs;
    
    if base_path.exists() {
        fs::remove_dir_all(base_path).await?;
    }
    
    Ok(())
}

/// Cleanup assignment for a collection
pub async fn cleanup_assignment(collection_id: &str) -> anyhow::Result<()> {
    let assignment_service = proximadb::storage::assignment_service::get_assignment_service();
    
    // Try to remove assignment, ignore if it doesn't exist
    let _ = assignment_service.remove_assignment(collection_id).await;
    
    Ok(())
}