//! Test for SSTable format fix - verifies bloom filter read/write

use crate::core::config::SstConfig;
use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::engines::impls::sst::SstableWriter;
use crate::storage::engines::impls::sst::readers::UnifiedSstableReader;
use crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem;
use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use std::collections::BTreeMap;
use std::sync::Arc;
use tempfile::TempDir;
use tracing::{debug, info};

fn create_test_config() -> SstConfig {
    SstConfig {
        block_size_kb: 4, // Use small 4KB blocks for tests
        decompression_cache_config: None,
        ..SstConfig::default()
    }
}

#[tokio::test]
async fn test_sstable_format_with_bloom_filter() {
    // Initialize hardware capabilities for testing
    let _ = crate::core::hardware_capabilities::initialize_hardware_capabilities_default();

    // Initialize tracing for debugging
    let _ = tracing_subscriber::fmt::try_init();
    // Create temp directory
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();

    // Create filesystem factory
    let config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::create(config).await.unwrap());

    // Write SSTable with bloom filter
    let sstable_path = temp_path.join("test_bloom.sstable");
    let test_config = create_test_config();
    let block_size = (test_config.block_size_kb * 1024) as usize;
    let writer = SstableWriter::new(&sstable_path, block_size, filesystem.clone());

    // Create test records
    let mut records = BTreeMap::new();
    for i in 0..10 {
        let record = VectorRecord {
            id: format!("vec_{:03}", i),
            vector: vec![i as f32; 3],
            metadata: std::collections::HashMap::new(),
            timestamp: Some(chrono::Utc::now().timestamp()),
            updated_at: Some(chrono::Utc::now().timestamp()),
            expires_at: None,
            version: Some(1),
            source: None,
        };
        records.insert(record.id.clone(), record);
    }

    // Write records using streaming approach for production consistency
    let record_count = records.len();
    let sorted_records_iter = records.into_iter(); // BTreeMap already sorted by key
    writer
        .write_sorted_vector_records(sorted_records_iter, record_count)
        .await
        .unwrap();

    // Read SSTable metadata (this will test bloom filter reading)
    let filesystem_factory = Arc::new(
        FilesystemFactory::create(FilesystemConfig::default())
            .await
            .unwrap(),
    );
    let base_fs = filesystem_factory.get_filesystem("file://").unwrap();
    let unified_fs = Arc::new(UnifiedCachingFilesystem::new(
        base_fs,
        "test_collection".to_string(),
        "sst".to_string(),
    ));
    let reader = UnifiedSstableReader::new(
        filesystem_factory,
        unified_fs,
        "test_collection".to_string(),
    );
    let file_url = format!("file://{}", sstable_path.display());

    // This should not panic with "unexpected end of file"
    reader.load_metadata(&file_url).await.unwrap();

    // Test bloom filter functionality
    debug!("Testing bloom filter...");
    let contains_005 = reader.might_contain_key(&file_url, "vec_005").await;
    let contains_009 = reader.might_contain_key(&file_url, "vec_009").await;
    let contains_fake = reader.might_contain_key(&file_url, "fake_key").await;

    debug!("Bloom filter results:");
    debug!("  vec_005: {}", contains_005);
    debug!("  vec_009: {}", contains_009);
    debug!("  fake_key: {}", contains_fake);

    assert!(
        contains_005,
        "Bloom filter should report vec_005 might exist"
    );
    assert!(
        contains_009,
        "Bloom filter should report vec_009 might exist"
    );

    // Test retrieving a vector
    debug!("\nTesting vector retrieval...");
    match reader.vector(&file_url, "vec_005").await {
        Ok(Some(vector)) => {
            debug!("✓ Found vector: {:?}", vector.id);
            assert_eq!(vector.id, "vec_005".to_string());
        }
        Ok(None) => {
            panic!("Vector vec_005 not found in SSTable");
        }
        Err(e) => {
            panic!("Error retrieving vector: {}", e);
        }
    }
}

#[tokio::test]
async fn test_sstable_empty_file_handling() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();
    let config = FilesystemConfig::default();
    let filesystem_factory = Arc::new(FilesystemFactory::create(config).await.unwrap());

    // Create an empty file
    let empty_file = temp_path.join("empty.sstable");
    tokio::fs::write(&empty_file, b"").await.unwrap();

    let base_fs = filesystem_factory.get_filesystem("file://").unwrap();
    let unified_fs = Arc::new(UnifiedCachingFilesystem::new(
        base_fs,
        "test_collection".to_string(),
        "sst".to_string(),
    ));
    let reader = UnifiedSstableReader::new(
        filesystem_factory,
        unified_fs,
        "test_collection".to_string(),
    );
    let file_url = format!("file://{}", empty_file.display());

    // Should handle empty file gracefully
    let result = reader.load_metadata(&file_url).await;
    assert!(result.is_err(), "Expected error for empty file");
    let error_msg = result.unwrap_err().to_string();
    debug!("Actual error: {}", error_msg);
    assert!(
        error_msg.contains("Failed to read header length")
            || error_msg.contains("expected at least 4 bytes")
            || error_msg.contains("unexpected end of file")
            || error_msg.contains("SSTable file too small"),
        "Expected error about file size/header, got: {}",
        error_msg
    );
}

#[tokio::test]
async fn test_sstable_truncated_file_handling() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();
    let config = FilesystemConfig::default();
    let filesystem_factory = Arc::new(FilesystemFactory::create(config).await.unwrap());

    // Create a file with only header length but no header data
    let truncated_file = temp_path.join("truncated.sstable");
    let header_len: u32 = 100;
    tokio::fs::write(&truncated_file, header_len.to_le_bytes())
        .await
        .unwrap();

    let base_fs = filesystem_factory.get_filesystem("file://").unwrap();
    let unified_fs = Arc::new(UnifiedCachingFilesystem::new(
        base_fs,
        "test_collection".to_string(),
        "sst".to_string(),
    ));
    let reader = UnifiedSstableReader::new(
        filesystem_factory,
        unified_fs,
        "test_collection".to_string(),
    );
    let file_url = format!("file://{}", truncated_file.display());

    // Should handle truncated file gracefully
    let result = reader.load_metadata(&file_url).await;
    assert!(result.is_err(), "Expected error for truncated file");
    let error_msg = result.unwrap_err().to_string();
    debug!("Actual error for truncated file: {}", error_msg);
    assert!(
        error_msg.contains("Failed to read complete header")
            || error_msg.contains("Failed to read header")
            || error_msg.contains("unexpected end of file")
            || error_msg.contains("failed to fill whole buffer")
            || error_msg.contains("SSTable file too small"),
        "Expected error about incomplete header or file size, got: {}",
        error_msg
    );
}
