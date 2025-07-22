//! Test for SSTable format fix - verifies bloom filter read/write

use crate::storage::engines::lsm::{LsmRecord, SstableWriter};
use crate::storage::engines::lsm::readers::UnifiedSstableReader;
use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use std::sync::Arc;
use std::collections::BTreeMap;
use tempfile::TempDir;

#[tokio::test]
async fn test_sstable_format_with_bloom_filter() {
    // Initialize tracing for debugging
    let _ = tracing_subscriber::fmt::try_init();
    // Create temp directory
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();
    
    // Create filesystem factory
    let config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(config).await.unwrap());
    
    // Write SSTable with bloom filter
    let sstable_path = temp_path.join("test_bloom.sst");
    let writer = SstableWriter::new(&sstable_path, 4096, filesystem.clone());
    
    // Create test records
    let mut records = BTreeMap::new();
    for i in 0..10 {
        let record = LsmRecord {
            id: format!("vec_{:03}", i),
            collection_id: "test_collection".to_string(),
            vector: vec![i as f32; 3],
            metadata: std::collections::HashMap::new(),
            timestamp: chrono::Utc::now().timestamp(),
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
            expires_at: None,
            version: 1,
            is_tombstone: false,
            sequence_number: i as u64,
            level: 0,
        };
        records.insert(record.id.clone(), record);
    }
    
    // Write records
    writer.write_records(records).await.unwrap();
    
    // Read SSTable metadata (this will test bloom filter reading)
    let reader = UnifiedSstableReader::new(filesystem.clone());
    let file_url = format!("file://{}", sstable_path.display());
    
    // This should not panic with "unexpected end of file"
    reader.load_metadata(&file_url).await.unwrap();
    
    // Test bloom filter functionality
    println!("Testing bloom filter...");
    let contains_005 = reader.might_contain_key(&file_url, "vec_005").await;
    let contains_009 = reader.might_contain_key(&file_url, "vec_009").await;
    let contains_fake = reader.might_contain_key(&file_url, "fake_key").await;
    
    println!("Bloom filter results:");
    println!("  vec_005: {}", contains_005);
    println!("  vec_009: {}", contains_009);
    println!("  fake_key: {}", contains_fake);
    
    assert!(contains_005, "Bloom filter should report vec_005 might exist");
    assert!(contains_009, "Bloom filter should report vec_009 might exist");
    
    // Test retrieving a vector
    println!("\nTesting vector retrieval...");
    match reader.get_vector(&file_url, "vec_005").await {
        Ok(Some(vector)) => {
            println!("✓ Found vector: {:?}", vector.id);
            assert_eq!(vector.id, Some("vec_005".to_string()));
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
    let filesystem = Arc::new(FilesystemFactory::new(config).await.unwrap());
    
    // Create an empty file
    let empty_file = temp_path.join("empty.sst");
    tokio::fs::write(&empty_file, b"").await.unwrap();
    
    let reader = UnifiedSstableReader::new(filesystem.clone());
    let file_url = format!("file://{}", empty_file.display());
    
    // Should handle empty file gracefully
    let result = reader.load_metadata(&file_url).await;
    assert!(result.is_err(), "Expected error for empty file");
    let error_msg = result.unwrap_err().to_string();
    println!("Actual error: {}", error_msg);
    assert!(error_msg.contains("Failed to read header length") || 
            error_msg.contains("expected at least 4 bytes") ||
            error_msg.contains("unexpected end of file") ||
            error_msg.contains("SSTable file too small"),
            "Expected error about file size/header, got: {}", error_msg);
}

#[tokio::test]
async fn test_sstable_truncated_file_handling() {
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();
    let config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(config).await.unwrap());
    
    // Create a file with only header length but no header data
    let truncated_file = temp_path.join("truncated.sst");
    let header_len: u32 = 100;
    tokio::fs::write(&truncated_file, header_len.to_le_bytes()).await.unwrap();
    
    let reader = UnifiedSstableReader::new(filesystem.clone());
    let file_url = format!("file://{}", truncated_file.display());
    
    // Should handle truncated file gracefully
    let result = reader.load_metadata(&file_url).await;
    assert!(result.is_err(), "Expected error for truncated file");
    let error_msg = result.unwrap_err().to_string();
    println!("Actual error for truncated file: {}", error_msg);
    assert!(error_msg.contains("Failed to read complete header") || 
            error_msg.contains("Failed to read header") ||
            error_msg.contains("unexpected end of file") ||
            error_msg.contains("failed to fill whole buffer"),
            "Expected error about incomplete header, got: {}", error_msg);
}