//! Simple test for SSTable format - direct reading without strategies

use crate::storage::engines::lsm::{LsmRecord, SstableWriter, SstableHeader, DataBlock, IndexEntry};
use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use std::sync::Arc;
use std::collections::BTreeMap;
use tempfile::TempDir;

#[tokio::test]
async fn test_simple_sstable_write_read() {
    // Create temp directory
    let temp_dir = TempDir::new().unwrap();
    let temp_path = temp_dir.path();
    
    // Create filesystem factory
    let config = FilesystemConfig::default();
    let filesystem = Arc::new(FilesystemFactory::new(config).await.unwrap());
    
    // Write SSTable
    let sstable_path = temp_path.join("test_simple.sst");
    let writer = SstableWriter::new(&sstable_path, 4096, filesystem.clone());
    
    // Create test records
    let mut records = BTreeMap::new();
    let test_record = LsmRecord {
        id: "test_id".to_string(),
        collection_id: "test_collection".to_string(),
        vector: vec![1.0, 2.0, 3.0],
        metadata: std::collections::HashMap::new(),
        timestamp: 123456789,
        created_at: 123456789,
        updated_at: 123456789,
        expires_at: None,
        version: 1,
        is_tombstone: false,
        sequence_number: 1,
        level: 0,
    };
    records.insert(test_record.id.clone(), test_record);
    
    // Write records
    writer.write_records(records).await.unwrap();
    println!("✓ SSTable written successfully");
    
    // Read the file directly
    let file_url = format!("file://{}", sstable_path.display());
    let fs = filesystem.get_filesystem(&file_url).unwrap();
    let data = fs.read(&file_url).await.unwrap();
    println!("✓ Read {} bytes from SSTable", data.len());
    
    // Parse the SSTable manually
    let mut offset = 0;
    
    // Read header length
    let header_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    offset += 4;
    println!("  Header length: {} bytes", header_len);
    
    // Read header
    let header_data = &data[offset..offset + header_len];
    let header: SstableHeader = bincode::deserialize(header_data).unwrap();
    offset += header_len;
    println!("  Header: {} entries, min={}, max={}", header.entry_count, header.min_key, header.max_key);
    
    // Read bloom filter length
    let bloom_len = u32::from_le_bytes([
        data[offset], data[offset + 1], data[offset + 2], data[offset + 3]
    ]) as usize;
    offset += 4;
    println!("  Bloom filter length: {} bytes", bloom_len);
    offset += bloom_len; // Skip bloom data
    
    // Read index length
    let index_len = u32::from_le_bytes([
        data[offset], data[offset + 1], data[offset + 2], data[offset + 3]
    ]) as usize;
    offset += 4;
    println!("  Index length: {} bytes", index_len);
    
    // Read index
    let index_data = &data[offset..offset + index_len];
    let index_entries: Vec<IndexEntry> = bincode::deserialize(index_data).unwrap();
    offset += index_len;
    println!("  Index: {} entries", index_entries.len());
    
    // Read first data block
    let block_len = u32::from_le_bytes([
        data[offset], data[offset + 1], data[offset + 2], data[offset + 3]
    ]) as usize;
    offset += 4;
    println!("  First block length: {} bytes", block_len);
    
    let block_data = &data[offset..offset + block_len];
    let block: DataBlock = bincode::deserialize(block_data).unwrap();
    println!("  Block {} has {} records", block.block_id, block.records.len());
    
    // Verify the record
    assert_eq!(block.records.len(), 1);
    let record = &block.records[0];
    assert_eq!(record.id, "test_id");
    assert_eq!(record.vector, vec![1.0, 2.0, 3.0]);
    
    println!("\n✓ Successfully read and verified SSTable format!");
}