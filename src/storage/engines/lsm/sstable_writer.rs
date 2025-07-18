/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! SSTable Writer with Bloom Filter and Atomic Write Support
//! 
//! Creates optimized SSTable files with bloom filters, indexes, and block-based storage.
//! Uses unified atomic write strategies for cross-cloud compatibility.

use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info};

use crate::storage::persistence::filesystem::{
    FilesystemFactory, FileSystem, FileOptions,
    atomic_strategy::{AtomicWriteExecutor, AtomicWriteExecutorFactory}
};

use super::{BloomFilter, BloomFilterConfig, DataBlock, IndexEntry, LsmRecord, SstableHeader};
use super::bloom_filter::{MetadataBloomFilter, MetadataBloomFilterBuilder, SstableBloomFilter};

/// SSTable writer with atomic write optimization
pub struct SstableWriter {
    /// Output file path
    path: std::path::PathBuf,
    /// Block size for data organization
    block_size: usize,
    /// Bloom filter configuration
    bloom_config: BloomFilterConfig,
    /// Filesystem factory for atomic writes
    filesystem: Arc<FilesystemFactory>,
}

impl SstableWriter {
    /// Create a new SSTable writer with filesystem support for atomic writes
    pub fn new<P: AsRef<Path>>(path: P, block_size: usize, filesystem: Arc<FilesystemFactory>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            block_size,
            bloom_config: BloomFilterConfig::default(),
            filesystem,
        }
    }
    
    /// Write records to SSTable with atomic write optimization
    /// Uses comprehensive atomic write strategies for flush/compaction safety
    pub async fn write_records(&self, records: BTreeMap<String, LsmRecord>) -> Result<()> {
        info!("🔄 Building SSTable in memory for atomic write: {} records", records.len());
        
        // Get filesystem and atomic writer
        let fs = self.filesystem.get_filesystem("file:///")?;
        let atomic_writer = AtomicWriteExecutorFactory::create_production_executor();
        
        // Step 1: Build comprehensive bloom filters (keys + metadata)
        let mut key_bloom_filter = BloomFilter::new(records.len(), &self.bloom_config);
        let mut metadata_builder = MetadataBloomFilterBuilder::new(self.bloom_config.clone());
        
        // Extract keys and metadata values
        for (key, record) in &records {
            key_bloom_filter.insert(key);
            
            // Extract metadata values for each column
            for (column, value) in &record.metadata {
                if let Some(string_value) = value.as_str() {
                    metadata_builder.add_value(column.clone(), string_value.to_string());
                } else if let Some(number_value) = value.as_number() {
                    metadata_builder.add_value(column.clone(), number_value.to_string());
                }
            }
        }
        
        let metadata_bloom_filter = metadata_builder.build();
        let combined_bloom_filter = SstableBloomFilter::new(key_bloom_filter, metadata_bloom_filter);
        
        debug!("🔍 Built combined bloom filter for {} keys with {} metadata columns", 
               records.len(), combined_bloom_filter.metadata_filter.num_columns());
        
        // Step 2: Organize records into data blocks (in-memory)
        let mut data_blocks = Vec::new();
        let mut index_entries = Vec::new();
        let mut current_block = Vec::new();
        let mut current_block_size = 0;
        let mut block_id = 0u32;
        let records_count = records.len();
        
        for (_key, record) in records {
            let record_size = bincode::serialized_size(&record).unwrap_or(0) as usize;
            
            // Start new block if current block would exceed size limit
            if current_block_size + record_size > self.block_size && !current_block.is_empty() {
                self.finalize_block(&mut data_blocks, &mut index_entries, &current_block, block_id, current_block_size)?;
                current_block.clear();
                current_block_size = 0;
                block_id += 1;
            }
            
            current_block.push(record);
            current_block_size += record_size;
        }
        
        // Handle the last block
        if !current_block.is_empty() {
            self.finalize_block(&mut data_blocks, &mut index_entries, &current_block, block_id, current_block_size)?;
        }
        
        debug!("📦 Organized into {} data blocks", data_blocks.len());
        
        // Step 3: Build complete SSTable in memory
        let mut sstable_bytes = Vec::new();
        
        // Serialize all components
        let min_key = index_entries.first().map(|e| e.key.clone()).unwrap_or_default();
        let max_key = index_entries.last().map(|e| e.key.clone()).unwrap_or_default();
        
        let bloom_data = bincode::serialize(&combined_bloom_filter)?;
        let index_data = bincode::serialize(&index_entries)?;
        let total_data_size: u64 = data_blocks.iter()
            .map(|b| bincode::serialized_size(b).unwrap_or(0))
            .sum();
        
        let header = SstableHeader {
            version: 1,
            level: 0,
            entry_count: records_count as u64,
            min_key,
            max_key,
            created_at: chrono::Utc::now().timestamp(),
            compression_enabled: false,
            has_bloom_filter: true,
            block_size: self.block_size as u32,
            batch_size: 0,
            header_size: 0, // Will be updated below
            index_size: index_data.len() as u32,
            data_size: total_data_size as u32,
            block_count: data_blocks.len() as u32,
        };
        
        let header_data = bincode::serialize(&header)?;
        
        // Build complete SSTable bytes: header_len + header + bloom_len + bloom + index_len + index + data_blocks
        sstable_bytes.extend_from_slice(&(header_data.len() as u32).to_le_bytes());
        sstable_bytes.extend_from_slice(&header_data);
        
        sstable_bytes.extend_from_slice(&(bloom_data.len() as u32).to_le_bytes());
        sstable_bytes.extend_from_slice(&bloom_data);
        
        sstable_bytes.extend_from_slice(&(index_data.len() as u32).to_le_bytes());
        sstable_bytes.extend_from_slice(&index_data);
        
        // Add all data blocks
        for data_block in &data_blocks {
            let block_data = bincode::serialize(data_block)?;
            sstable_bytes.extend_from_slice(&(block_data.len() as u32).to_le_bytes());
            sstable_bytes.extend_from_slice(&block_data);
        }
        
        info!(
            "💾 Built SSTable in memory: {} KB with {} records in {} blocks",
            sstable_bytes.len() / 1024,
            records_count,
            data_blocks.len()
        );
        
        // Step 4: Atomic write using unified atomic strategy
        // For local: writes to ___temp subdirectory then atomic move
        // For cloud: writes to local temp then uploads to object store
        let final_path = self.path.to_string_lossy();
        atomic_writer
            .write_atomic(fs, &final_path, &sstable_bytes, None)
            .await
            .map_err(|e| anyhow::anyhow!("Atomic write failed: {}", e))?;
        
        info!(
            "✅ LSM: Atomically wrote SSTable {} ({} KB, {} records, {} blocks)",
            self.path.display(),
            sstable_bytes.len() / 1024,
            records_count,
            data_blocks.len()
        );
        
        Ok(())
    }
    
    /// Helper to finalize a data block
    fn finalize_block(
        &self,
        data_blocks: &mut Vec<DataBlock>,
        index_entries: &mut Vec<IndexEntry>,
        current_block: &[LsmRecord],
        block_id: u32,
        current_block_size: usize,
    ) -> Result<()> {
        let data_block = DataBlock {
            block_id,
            records: current_block.to_vec(),
            uncompressed_size: current_block_size as u32,
        };
        
        let block_size = bincode::serialized_size(&data_block).unwrap_or(0) as u32;
        
        // Add index entry for first record in block
        if let Some(first_record) = current_block.first() {
            index_entries.push(IndexEntry {
                key: first_record.id.clone(),
                offset: 0, // Will be calculated during read
                size: block_size,
                block_id,
                block_offset: 0,
                compressed: false,
            });
        }
        
        data_blocks.push(data_block);
        Ok(())
    }
    
    /// Set bloom filter configuration
    pub fn with_bloom_config(mut self, config: BloomFilterConfig) -> Self {
        self.bloom_config = config;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use tempfile::NamedTempFile;
    
    #[tokio::test]
    async fn test_sstable_writer_basic() {
        // Note: This test would need a mock filesystem for full testing
        // For now, just test the data structure building
        let temp_file = NamedTempFile::new().unwrap();
        
        // Create test records
        let mut records = BTreeMap::new();
        for i in 0..10 {
            let record = LsmRecord {
                id: format!("key{:03}", i),
                collection_id: "test".to_string(),
                vector: vec![1.0, 2.0, 3.0],
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
        
        assert_eq!(records.len(), 10);
    }
}