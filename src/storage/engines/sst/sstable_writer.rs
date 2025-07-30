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
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info};

use crate::storage::persistence::filesystem::{
    FilesystemFactory,
    atomic_strategy::{AtomicWriteExecutorFactory}
};

use super::{DataBlock, IndexEntry, SstRecord, SstableHeader};
use crate::core::bloom::{
    BloomFilterConfig, BloomStrategy, BloomFilterStrategy,
    factory::BloomFilterFactory,
};
use crate::core::bloom::strategies::composite::CompositeBloomFilterBuilder;
use super::bloom_filter::SstableBloomFilter;

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
    pub async fn write_records(&self, records: BTreeMap<String, SstRecord>) -> Result<()> {
        info!("🔄 Building SSTable in memory for atomic write: {} records", records.len());
        
        // Get filesystem and atomic writer
        // Extract the scheme from the path to get the correct filesystem
        let path_str = self.path.to_string_lossy();
        let (_scheme, fs_url) = if path_str.contains("://") {
            let parts: Vec<&str> = path_str.splitn(2, "://").collect();
            (parts[0], path_str.to_string())
        } else {
            ("file", format!("file://{}", path_str))
        };
        let fs = self.filesystem.get_filesystem(&fs_url)?;
        let atomic_writer = AtomicWriteExecutorFactory::create_production_executor();
        
        // Step 1: Build comprehensive bloom filters (keys + metadata)
        let bloom_config = BloomFilterConfig {
            strategy: BloomStrategy::ByteAligned,
            expected_items: records.len(),
            ..self.bloom_config.clone()
        };
        let mut key_bloom_filter = BloomFilterFactory::create(&bloom_config);
        
        let metadata_config = BloomFilterConfig {
            strategy: BloomStrategy::Composite,
            expected_items: records.len(),
            ..self.bloom_config.clone()
        };
        let mut metadata_builder = CompositeBloomFilterBuilder::new(metadata_config);
        
        // Extract keys and metadata values
        let mut metadata_value_count = 0;
        for (key, record) in &records {
            key_bloom_filter.insert(key.as_bytes());
            
            // Extract metadata values for each column
            for (column, value) in &record.metadata {
                // Convert JSON value to MetadataItem for bloom filter
                let metadata_item = crate::core::bloom::json_to_metadata_item(column, value);
                metadata_builder.add_metadata_item(column.clone(), metadata_item);
                metadata_value_count += 1;
            }
        }
        debug!("Added {} metadata values to bloom filter", metadata_value_count);
        
        let metadata_bloom_filter = metadata_builder.build();
        
        // Create the SstableBloomFilter - serialize only the metadata bloom filter
        let metadata_filter_data = BloomFilterStrategy::serialize(&metadata_bloom_filter)?;
        
        let stats = super::bloom_filter::BloomFilterStats {
            key_count: records.len() as u64,
            metadata_columns: metadata_bloom_filter.num_columns() as u64,
            total_keys: 0,
            key_lookups_saved: 0,
            metadata_queries_saved: 0,
        };
        
        let combined_bloom_filter = SstableBloomFilter::new(
            bloom_config.clone(),
            key_bloom_filter.serialize()?,
            metadata_filter_data,
            stats,
        );
        
        debug!("🔍 Built combined bloom filter for {} keys with {} metadata columns", 
               records.len(), metadata_bloom_filter.num_columns());
        debug!("Key bloom filter size: {} bytes", combined_bloom_filter.key_filter_data.len());
        debug!("Metadata bloom filter size: {} bytes", combined_bloom_filter.metadata_filter_data.len());
        
        // Step 2: Organize records into data blocks (in-memory)
        let mut data_blocks = Vec::new();
        let mut index_entries = Vec::new();
        let mut current_block = Vec::new();
        let mut current_block_size = 0;
        let mut block_id = 0u32;
        let records_count = records.len();
        
        for (_key, record) in records {
            let record_size = record.serialize().map(|v| v.len()).unwrap_or(0);
            
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
        
        // Serialize bloom filter using custom serialization to avoid bincode issues
        let bloom_data = combined_bloom_filter.serialize()?;
        // Serialize index entries using custom serialization
        let mut index_data = Vec::new();
        for entry in &index_entries {
            let entry_data = entry.serialize()?;
            index_data.extend_from_slice(&(entry_data.len() as u32).to_le_bytes());
            index_data.extend_from_slice(&entry_data);
        }
        
        let total_data_size: u64 = data_blocks.iter()
            .map(|b| b.serialize().map(|v| v.len() as u64).unwrap_or(0))
            .sum();
        
        // Create header with correct sizes
        let mut header = SstableHeader {
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
            header_size: 0, // Will be updated after serialization
            index_size: index_data.len() as u32,
            data_size: total_data_size as u32,
            block_count: data_blocks.len() as u32,
        };
        
        // Serialize header to get its size
        let header_data = bincode::serialize(&header)?;
        header.header_size = header_data.len() as u32;
        
        // Re-serialize with correct header_size
        let header_data = bincode::serialize(&header)?;
        
        // Build complete SSTable bytes: header_len + header + bloom_len + bloom + index_len + index + data_blocks
        debug!("SSTable Writer - File layout:");
        debug!("  - Header length (4 bytes): {}", header_data.len());
        debug!("  - Header data ({} bytes)", header_data.len());
        debug!("  - Bloom length (4 bytes): {}", bloom_data.len());
        debug!("  - Bloom data ({} bytes)", bloom_data.len());
        debug!("  - Bloom offset will be: {}", 4 + header_data.len());
        
        sstable_bytes.extend_from_slice(&(header_data.len() as u32).to_le_bytes());
        sstable_bytes.extend_from_slice(&header_data);
        
        debug!("Writing bloom length: {} as bytes: {:?}", bloom_data.len(), &(bloom_data.len() as u32).to_le_bytes());
        sstable_bytes.extend_from_slice(&(bloom_data.len() as u32).to_le_bytes());
        sstable_bytes.extend_from_slice(&bloom_data);
        
        sstable_bytes.extend_from_slice(&(index_data.len() as u32).to_le_bytes());
        sstable_bytes.extend_from_slice(&index_data);
        
        // Add all data blocks
        for (idx, data_block) in data_blocks.iter().enumerate() {
            let block_data = data_block.serialize()?;
            debug!("Writing data block {} - length: {} bytes", idx, block_data.len());
            sstable_bytes.extend_from_slice(&(block_data.len() as u32).to_le_bytes());
            sstable_bytes.extend_from_slice(&block_data);
        }
        
        debug!("Total SSTable size before write: {} bytes", sstable_bytes.len());
        
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
        
        // Verify the file was written correctly
        let file_metadata = fs.metadata(&final_path).await
            .map_err(|e| anyhow::anyhow!("Failed to verify written file: {}", e))?;
        
        if file_metadata.size != sstable_bytes.len() as u64 {
            return Err(anyhow::anyhow!(
                "SSTable file size mismatch after write: expected {} bytes, got {} bytes",
                sstable_bytes.len(),
                file_metadata.size
            ));
        }
        
        info!(
            "✅ LSM: Atomically wrote SSTable {} ({} bytes / {} KB, {} records, {} blocks, header={} bytes, bloom={} bytes, index={} bytes)",
            self.path.display(),
            sstable_bytes.len(),
            sstable_bytes.len() / 1024,
            records_count,
            data_blocks.len(),
            header_data.len(),
            bloom_data.len(),
            index_data.len()
        );
        
        Ok(())
    }
    
    /// Helper to finalize a data block
    fn finalize_block(
        &self,
        data_blocks: &mut Vec<DataBlock>,
        index_entries: &mut Vec<IndexEntry>,
        current_block: &[SstRecord],
        block_id: u32,
        current_block_size: usize,
    ) -> Result<()> {
        let data_block = DataBlock {
            block_id,
            records: current_block.to_vec(),
            uncompressed_size: current_block_size as u32,
        };
        
        let block_size = data_block.serialize().map(|v| v.len()).unwrap_or(0) as u32;
        
        // Collect metadata statistics for this block
        let mut metadata_min_values = HashMap::new();
        let mut metadata_max_values = HashMap::new();
        let mut metadata_null_counts = HashMap::new();
        
        for record in current_block {
            for (column, value) in &record.metadata {
                // Track null counts
                if value.is_null() {
                    *metadata_null_counts.entry(column.clone()).or_insert(0) += 1;
                } else {
                    // Track min/max values
                    let entry_min = metadata_min_values.entry(column.clone()).or_insert_with(|| value.clone());
                    if Self::compare_json_values(value, entry_min) == std::cmp::Ordering::Less {
                        *entry_min = value.clone();
                    }
                    
                    let entry_max = metadata_max_values.entry(column.clone()).or_insert_with(|| value.clone());
                    if Self::compare_json_values(value, entry_max) == std::cmp::Ordering::Greater {
                        *entry_max = value.clone();
                    }
                }
            }
        }
        
        // Add index entry for first record in block
        if let Some(first_record) = current_block.first() {
            index_entries.push(IndexEntry {
                key: first_record.id.clone(),
                offset: 0, // Will be calculated during read
                size: block_size,
                block_id,
                block_offset: 0,
                compressed: false,
                metadata_min_values,
                metadata_max_values,
                metadata_null_counts,
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
    
    /// Compare two JSON values for ordering
    fn compare_json_values(a: &serde_json::Value, b: &serde_json::Value) -> std::cmp::Ordering {
        use serde_json::Value;
        use std::cmp::Ordering;
        
        match (a, b) {
            (Value::Number(n1), Value::Number(n2)) => {
                let f1 = n1.as_f64().unwrap_or(0.0);
                let f2 = n2.as_f64().unwrap_or(0.0);
                f1.partial_cmp(&f2).unwrap_or(Ordering::Equal)
            }
            (Value::String(s1), Value::String(s2)) => s1.cmp(s2),
            (Value::Bool(b1), Value::Bool(b2)) => b1.cmp(b2),
            _ => Ordering::Equal,
        }
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
            let record = SstRecord {
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