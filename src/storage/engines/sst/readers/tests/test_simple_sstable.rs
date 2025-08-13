//! Simple test for SSTable format - direct reading without strategies

use crate::storage::engines::sst::{SstRecord, SstableWriter, SstableHeader, DataBlock, IndexEntry};
use crate::storage::persistence::filesystem::{FilesystemFactory, FilesystemConfig};
use crate::core::config::SstConfig;
use std::sync::Arc;
use std::collections::BTreeMap;
use tempfile::TempDir;
use tracing::{debug, error, info};

fn create_test_config() -> SstConfig {
    SstConfig {
        block_size_mb: 4, // Use small 4KB blocks for tests
        decompression_cache_config: None,
        ..SstConfig::default()
    }
}

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
    let test_config = create_test_config();
    let block_size = (test_config.block_size_mb * 1024) as usize;
    let writer = SstableWriter::new(&sstable_path, block_size, filesystem.clone());
    
    // Create test records
    let mut records = BTreeMap::new();
    let test_record = SstRecord {
        id: "test_id".to_string(),
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
    records.insert(test_record.id.clone(), test_record);
    
    // Write records using streaming approach for production consistency
    let record_count = records.len();
    let sorted_records_iter = records.into_iter(); // BTreeMap already sorted by key
    writer.write_sorted_records(sorted_records_iter, record_count).await.unwrap();
    debug!("✓ SSTable written successfully");
    
    // Read the file directly
    let file_url = format!("file://{}", sstable_path.display());
    let fs = filesystem.get_filesystem(&file_url).unwrap();
    let data = fs.read(&file_url).await.unwrap();
    debug!("✓ Read {} bytes from SSTable", data.len());
    
    // Parse the SSTable manually
    let mut offset = 0;
    
    // Check SST1 magic bytes
    assert_eq!(&data[0..4], b"SST1", "Missing SST1 magic bytes");
    offset += 4;
    debug!("  ✓ SST1 magic bytes verified");
    
    // Read header length
    let header_len = u32::from_le_bytes([data[offset], data[offset+1], data[offset+2], data[offset+3]]) as usize;
    offset += 4;
    debug!("  Header length: {} bytes", header_len);
    
    // Read header
    let header_data = &data[offset..offset + header_len];
    let header: SstableHeader = bincode::deserialize(header_data).unwrap();
    offset += header_len;
    debug!("  Header: {} entries, min={}, max={}", header.entry_count, header.min_key, header.max_key);
    
    // Read bloom filter length
    let bloom_len = u32::from_le_bytes([
        data[offset], data[offset + 1], data[offset + 2], data[offset + 3]
    ]) as usize;
    offset += 4;
    debug!("  Bloom filter length: {} bytes", bloom_len);
    offset += bloom_len; // Skip bloom data
    
    // Read index length
    let index_len = u32::from_le_bytes([
        data[offset], data[offset + 1], data[offset + 2], data[offset + 3]
    ]) as usize;
    offset += 4;
    debug!("  Index length: {} bytes", index_len);
    
    // Read index
    let index_data = &data[offset..offset + index_len];
    // Deserialize index entries using custom deserialization
    let mut index_entries = Vec::new();
    let mut cursor = std::io::Cursor::new(index_data);
    
    while (cursor.position() as usize) < index_data.len() {
        use std::io::Read;
use tracing::{debug, error, info};
        
        // Read entry length
        let mut len_bytes = [0u8; 4];
        if cursor.read_exact(&mut len_bytes).is_err() {
            break;
        }
        let entry_len = u32::from_le_bytes(len_bytes) as usize;
        
        if cursor.position() as usize + entry_len > index_data.len() {
            break;
        }
        
        // Read entry data
        let mut entry_data = vec![0u8; entry_len];
        if cursor.read_exact(&mut entry_data).is_err() {
            break;
        }
        
        // Deserialize the entry
        if let Ok(entry) = IndexEntry::deserialize(&entry_data) {
            index_entries.push(entry);
        } else {
            break;
        }
    }
    offset += index_len;
    debug!("  Index: {} entries", index_entries.len());
    
    // Read first data block
    let block_len = u32::from_le_bytes([
        data[offset], data[offset + 1], data[offset + 2], data[offset + 3]
    ]) as usize;
    offset += 4;
    debug!("  First block length: {} bytes", block_len);
    
    let block_data = &data[offset..offset + block_len];
    let block: DataBlock = DataBlock::deserialize(block_data).unwrap();
    debug!("  Block {} has {} records", block.block_id, block.records.len());
    
    // Verify the record
    assert_eq!(block.records.len(), 1);
    let record = &block.records[0];
    assert_eq!(record.id, "test_id");
    assert_eq!(record.vector, vec![1.0, 2.0, 3.0]);
    
    debug!("\n✓ Successfully read and verified SSTable format!");
}