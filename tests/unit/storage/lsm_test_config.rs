//! Consistent configuration for LSM tests

use proximadb::core::{LsmConfig, BloomFilterConfig};
use proximadb::storage::persistence::filesystem::FilesystemConfig;
use std::path::Path;

/// Create a consistent test configuration for LSM
pub fn create_test_lsm_config(base_path: &str) -> LsmConfig {
    LsmConfig {
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
        enable_wal: true,
        
        // Directories - these will be used by LSM directly, not through assignment service
        // LSM tree will use collection-specific subdirectories
        wal_directory: format!("{}/lsm/wal", base_path),
        data_directory: format!("{}/lsm/data", base_path),
        
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
    fs::create_dir_all(base_path.join("wal")).await?;
    
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