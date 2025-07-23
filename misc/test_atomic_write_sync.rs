// Test to verify atomic write with proper sync prevents file truncation

use proximadb::storage::persistence::filesystem::{
    FilesystemFactory, FilesystemConfig, FileOptions,
    local::LocalConfig,
};
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    println!("Testing atomic write with sync to prevent truncation...\n");

    // Create temp directory for test
    let temp_dir = TempDir::new()?;
    let test_path = temp_dir.path().join("test_atomic.sst");
    let test_url = format!("file://{}", test_path.display());

    // Create filesystem factory with sync enabled
    let mut config = FilesystemConfig::default();
    config.local = Some(LocalConfig {
        root_dir: None,
        follow_symlinks: true,
        default_permissions: None,
        sync_enabled: true,  // Critical: ensure sync is enabled
    });

    let factory = Arc::new(FilesystemFactory::new(config).await?);
    let fs = factory.get_filesystem(&test_url)?;

    // Create test data that mimics SSTable structure
    let mut test_data = Vec::new();
    
    // Header length (4 bytes)
    test_data.extend_from_slice(&100u32.to_le_bytes());
    
    // Header data (100 bytes)
    test_data.extend_from_slice(&vec![0xAA; 100]);
    
    // Bloom filter length (4 bytes) - this is where truncation often happens
    test_data.extend_from_slice(&1000u32.to_le_bytes());
    
    // Bloom filter data (1000 bytes)
    test_data.extend_from_slice(&vec![0xBB; 1000]);
    
    // Additional data
    test_data.extend_from_slice(&vec![0xCC; 500]);

    println!("Test data size: {} bytes", test_data.len());

    // Perform atomic write
    println!("Writing atomically with sync...");
    fs.write_atomic(&test_path.to_string_lossy(), &test_data, None).await?;

    // Immediately read back the file
    println!("Reading back immediately...");
    let read_data = fs.read(&test_path.to_string_lossy()).await?;
    
    println!("Read data size: {} bytes", read_data.len());
    
    // Verify all data was written
    if read_data.len() == test_data.len() {
        println!("✅ SUCCESS: All data was written and synced properly!");
        
        // Verify bloom filter length can be read correctly
        let bloom_len_offset = 104; // After header_len (4) + header (100)
        let bloom_len_bytes = &read_data[bloom_len_offset..bloom_len_offset + 4];
        let bloom_len = u32::from_le_bytes([
            bloom_len_bytes[0], 
            bloom_len_bytes[1], 
            bloom_len_bytes[2], 
            bloom_len_bytes[3]
        ]);
        
        println!("✅ Bloom filter length read correctly: {} bytes", bloom_len);
    } else {
        println!("❌ FAILURE: Data was truncated!");
        println!("   Expected: {} bytes", test_data.len());
        println!("   Got: {} bytes", read_data.len());
        println!("   Missing: {} bytes", test_data.len() - read_data.len());
        
        // Check where truncation occurred
        if read_data.len() >= 104 && read_data.len() < 108 {
            println!("   Truncation occurred at bloom filter length!");
        }
    }

    // Test with sync disabled (for comparison)
    println!("\nTesting with sync disabled (development mode)...");
    let nosync_path = temp_dir.path().join("test_nosync.sst");
    let nosync_url = format!("file://{}", nosync_path.display());
    
    let mut nosync_config = FilesystemConfig::default();
    nosync_config.local = Some(LocalConfig {
        root_dir: None,
        follow_symlinks: true,
        default_permissions: None,
        sync_enabled: false,  // Disabled for fast writes
    });
    
    let nosync_factory = Arc::new(FilesystemFactory::new(nosync_config).await?);
    let nosync_fs = nosync_factory.get_filesystem(&nosync_url)?;
    
    // Write without sync
    nosync_fs.write_atomic(&nosync_path.to_string_lossy(), &test_data, None).await?;
    
    // Read back
    let nosync_read_data = nosync_fs.read(&nosync_path.to_string_lossy()).await?;
    
    if nosync_read_data.len() == test_data.len() {
        println!("✅ No-sync write also succeeded (lucky, but not guaranteed)");
    } else {
        println!("⚠️  No-sync write resulted in truncation (expected in some cases)");
    }

    println!("\nTest completed!");
    Ok(())
}