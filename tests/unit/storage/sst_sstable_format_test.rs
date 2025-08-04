//! Test SSTable format compatibility between writer and reader

use super::sst_test_config::{create_test_filesystem_config};

use proximadb::storage::engines::sst::{
    sstable_writer::SstableWriter,
    readers::unified_sstable_reader::UnifiedSstableReader,
    SstRecord,
};
use proximadb::storage::persistence::filesystem::FilesystemFactory;
use std::collections::BTreeMap;
use std::sync::Arc;
use tempfile::TempDir;

#[tokio::test]
async fn test_sstable_write_read_format() {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let temp_dir = TempDir::new().unwrap();
    let sstable_path = temp_dir.path().join("test.sst");
    
    // Create filesystem factory with consistent config
    let fs_config = create_test_filesystem_config();
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    // Create a simple record
    let mut records = BTreeMap::new();
    let record = SstRecord {
        id: "test_vec".to_string(),
        vector: vec![1.0, 2.0, 3.0],
        metadata: vec![],
        timestamp: 123456789,
        updated_at: Some(123456789),
        expires_at: None,
        version: Some(1),
        is_tombstone: false,
        sequence_number: 1,
        level: 0,
    };
    records.insert(record.id.clone(), record);
    
    // Write SSTable
    let writer = SstableWriter::new(
        &sstable_path,
        4096, // block size
        filesystem.clone()
    );
    
    writer.write_records(records).await.expect("Failed to write SSTable");
    
    // Verify file exists
    let fs = filesystem.get_filesystem("file:///").unwrap();
    assert!(fs.exists(sstable_path.to_str().unwrap()).await.unwrap());
    
    // Read back using unified reader
    let reader = UnifiedSstableReader::new(filesystem.clone());
    
    // Load metadata (this is where it's failing)
    let file_url = format!("file://{}", sstable_path.display());
    reader.load_metadata(&file_url).await.expect("Failed to load metadata");
    
    // Try to get the vector
    let retrieved = reader.get_vector(&file_url, "test_vec").await
        .expect("Failed to get vector");
    
    assert!(retrieved.is_some(), "Should find the vector");
    let vec = retrieved.unwrap();
    assert_eq!(vec.id.as_ref().unwrap(), "test_vec");
    assert_eq!(vec.vector, vec![1.0, 2.0, 3.0]);
}

#[tokio::test]
async fn test_sstable_format_inspection() {
    // Initialize hardware capabilities
    let _ = proximadb::core::hardware_capabilities::initialize_hardware_capabilities_default();
    let temp_dir = TempDir::new().unwrap();
    let sstable_path = temp_dir.path().join("inspect.sst");
    
    // Create filesystem factory with consistent config
    let fs_config = create_test_filesystem_config();
    let filesystem = Arc::new(FilesystemFactory::new(fs_config).await.unwrap());
    
    // Create records with metadata for bloom filter
    let mut records = BTreeMap::new();
    for i in 0..5 {
        let metadata = vec![
            proximadb::proto::proximadb::MetadataItem {
                key: "category".to_string(),
                value: Some(proximadb::proto::proximadb::metadata_item::Value::StringValue(format!("cat_{}", i % 2))),
            },
        ];
        
        let record = SstRecord {
            id: format!("vec_{}", i),
            vector: vec![i as f32; 3],
            metadata,
            timestamp: 123456789,
            updated_at: Some(123456789),
            expires_at: None,
            version: Some(1),
            is_tombstone: false,
            sequence_number: i as u64,
            level: 0,
        };
        records.insert(record.id.clone(), record);
    }
    
    // Write SSTable
    let writer = SstableWriter::new(
        &sstable_path,
        4096,
        filesystem.clone()
    );
    
    writer.write_records(records).await.expect("Failed to write SSTable");
    
    // Read file directly to inspect format
    let fs = filesystem.get_filesystem("file:///").unwrap();
    let file_data = fs.read(sstable_path.to_str().unwrap()).await.unwrap();
    
    println!("SSTable file size: {} bytes", file_data.len());
    
    // Parse header length
    let header_len = u32::from_le_bytes([
        file_data[0], file_data[1], file_data[2], file_data[3]
    ]);
    println!("Header length: {} bytes", header_len);
    
    // Check bloom filter offset and length
    let bloom_offset = 4 + header_len as usize;
    if file_data.len() >= bloom_offset + 4 {
        let bloom_len = u32::from_le_bytes([
            file_data[bloom_offset],
            file_data[bloom_offset + 1],
            file_data[bloom_offset + 2],
            file_data[bloom_offset + 3]
        ]);
        println!("Bloom filter length: {} bytes at offset {}", bloom_len, bloom_offset);
        
        let bloom_end = bloom_offset + 4 + bloom_len as usize;
        assert!(file_data.len() >= bloom_end, 
            "File size {} is too small for bloom filter ending at {}", 
            file_data.len(), bloom_end);
            
        // Check index offset and length
        if file_data.len() >= bloom_end + 4 {
            let index_len = u32::from_le_bytes([
                file_data[bloom_end],
                file_data[bloom_end + 1],
                file_data[bloom_end + 2],
                file_data[bloom_end + 3]
            ]);
            println!("Index length: {} bytes at offset {}", index_len, bloom_end);
        }
    }
    
    // Now try to read with unified reader
    let reader = UnifiedSstableReader::new(filesystem.clone());
    let file_url = format!("file://{}", sstable_path.display());
    reader.load_metadata(&file_url).await.expect("Failed to load metadata");
    
    // Verify bloom filter works
    assert!(reader.might_contain_key(&file_url, "vec_0").await);
    assert!(!reader.might_contain_key(&file_url, "non_existent_key").await);
}