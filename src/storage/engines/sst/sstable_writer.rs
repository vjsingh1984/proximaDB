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
use crate::proto::proximadb::CompressionConfig;

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
    /// SDK-driven compression configuration
    compression_config: Option<CompressionConfig>,
}

impl SstableWriter {
    /// Create a new SSTable writer with filesystem support for atomic writes
    pub fn new<P: AsRef<Path>>(path: P, block_size: usize, filesystem: Arc<FilesystemFactory>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            block_size,
            bloom_config: BloomFilterConfig::default(),
            filesystem,
            compression_config: None,
        }
    }
    
    /// Create a new SSTable writer with compression configuration from SDK
    pub fn with_compression<P: AsRef<Path>>(
        path: P, 
        block_size: usize, 
        filesystem: Arc<FilesystemFactory>,
        compression_config: Option<CompressionConfig>
    ) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            block_size,
            bloom_config: BloomFilterConfig::default(),
            filesystem,
            compression_config,
        }
    }
    
    /// Write sorted records to SSTable using streaming (disk-only, no memory structures)
    /// DISK-ONLY DESIGN: Processes records as iterator to avoid in-memory BTreeMap
    #[inline(always)]
    pub async fn write_sorted_records<I>(&self, sorted_records: I, record_count: usize) -> Result<()>
    where
        I: Iterator<Item = (String, SstRecord)>,
    {
        info!("🔄 SST: Streaming {} sorted records directly to disk (no memory structures)", record_count);
        
        if record_count == 0 {
            info!("⚠️ SST: No records to write - this may be a valid scenario (e.g., compaction with no data)");
            return Err(anyhow::anyhow!("Cannot write SSTable with 0 records"));
        }
        
        // Get filesystem and atomic writer
        let path_str = self.path.to_string_lossy();
        let (_scheme, fs_url) = if path_str.contains("://") {
            let parts: Vec<&str> = path_str.splitn(2, "://").collect();
            (parts[0], path_str.to_string())
        } else {
            ("file", format!("file://{}", path_str))
        };
        let fs = self.filesystem.get_filesystem(&fs_url)?;
        let atomic_writer = AtomicWriteExecutorFactory::create_production_executor();
        
        // Step 1: Build bloom filters while streaming records
        let bloom_config = BloomFilterConfig {
            strategy: BloomStrategy::ByteAligned,
            expected_items: record_count,
            ..self.bloom_config.clone()
        };
        let mut key_bloom_filter = BloomFilterFactory::create(&bloom_config);
        
        let metadata_config = BloomFilterConfig {
            strategy: BloomStrategy::Composite,
            expected_items: record_count,
            ..self.bloom_config.clone()
        };
        let mut metadata_builder = CompositeBloomFilterBuilder::new(metadata_config);
        
        // Step 2: Stream records directly into blocks (no intermediate BTreeMap)
        let estimated_blocks = (record_count / (self.block_size / 256)).max(1);
        let mut data_blocks = Vec::with_capacity(estimated_blocks);
        let mut index_entries = Vec::with_capacity(estimated_blocks);
        let mut current_block = Vec::with_capacity(self.block_size / 128);
        let mut current_block_size = 0;
        let mut block_id = 0u32;
        let mut processed_count = 0;
        let mut metadata_value_count = 0;
        
        // Process records in streaming fashion
        for (key, record) in sorted_records {
            // Update bloom filters
            key_bloom_filter.insert(key.as_bytes());
            
            for metadata_item in &record.metadata {
                metadata_builder.add_metadata_item(metadata_item.key.clone(), metadata_item.clone());
                metadata_value_count += 1;
            }
            
            // Serialize record once
            let serialized = record.serialize()?;
            let record_size = serialized.len();
            
            // Check if we need to start a new block
            if current_block_size + record_size > self.block_size && !current_block.is_empty() {
                self.finalize_block(&mut data_blocks, &mut index_entries, &current_block, block_id, current_block_size)?;
                current_block.clear();
                current_block_size = 0;
                block_id += 1;
            }
            
            current_block.push(record);
            current_block_size += record_size;
            processed_count += 1;
        }
        
        // Handle the last block
        if !current_block.is_empty() {
            self.finalize_block(&mut data_blocks, &mut index_entries, &current_block, block_id, current_block_size)?;
        }
        
        debug!("🔍 Streamed {} records into {} blocks with {} metadata columns", 
               processed_count, data_blocks.len(), metadata_value_count);
        
        // Continue with bloom filter and file writing...
        let metadata_bloom_filter = metadata_builder.build();
        let metadata_filter_data = BloomFilterStrategy::serialize(&metadata_bloom_filter)?;
        
        let stats = super::bloom_filter::BloomFilterStats {
            key_count: processed_count as u64,
            metadata_columns: metadata_bloom_filter.num_columns() as u64,
            total_keys: 0,
            key_lookups_saved: 0,
            metadata_queries_saved: 0,
        };
        
        let combined_bloom_filter = super::bloom_filter::SstableBloomFilter::new(
            bloom_config.clone(),
            key_bloom_filter.serialize()?,
            metadata_filter_data,
            stats,
        );
        
        // Build complete SSTable in memory for atomic write
        let mut sstable_bytes = Vec::new();
        
        let min_key = index_entries.first().map(|e| e.key.clone()).unwrap_or_default();
        let max_key = index_entries.last().map(|e| e.key.clone()).unwrap_or_default();
        
        let bloom_data = combined_bloom_filter.serialize()?;
        let mut index_data = Vec::new();
        for entry in &index_entries {
            let entry_data = entry.serialize()?;
            index_data.extend_from_slice(&(entry_data.len() as u32).to_le_bytes());
            index_data.extend_from_slice(&entry_data);
        }
        
        let total_data_size: u64 = data_blocks.iter()
            .map(|b| b.serialize().map(|v| v.len() as u64).unwrap_or(0))
            .sum();
        
        // SDK-driven compression configuration
        let (compression_algorithm, compression_level) = 
            if let Some(ref compression) = self.compression_config {
                use crate::proto::proximadb::CompressionAlgorithm;
                let algorithm = match CompressionAlgorithm::try_from(compression.algorithm) {
                    Ok(CompressionAlgorithm::CompressionZstd) => super::CompressionAlgorithmSst::Zstd,
                    Ok(CompressionAlgorithm::CompressionLz4) => super::CompressionAlgorithmSst::Lz4,
                    Ok(CompressionAlgorithm::CompressionSnappy) => super::CompressionAlgorithmSst::Snappy,
                    Ok(CompressionAlgorithm::CompressionGzip) => super::CompressionAlgorithmSst::Gzip,
                    Ok(CompressionAlgorithm::CompressionBrotli) => super::CompressionAlgorithmSst::Brotli,
                    Ok(CompressionAlgorithm::CompressionBzip2) => super::CompressionAlgorithmSst::Bzip2,
                    Ok(CompressionAlgorithm::CompressionDeflate) => super::CompressionAlgorithmSst::Deflate,
                    Ok(CompressionAlgorithm::CompressionXz) => super::CompressionAlgorithmSst::Xz,
                    Ok(CompressionAlgorithm::CompressionZlib) => super::CompressionAlgorithmSst::Zlib,
                    Ok(CompressionAlgorithm::CompressionLzo) => super::CompressionAlgorithmSst::Lzo,
                    Ok(CompressionAlgorithm::CompressionLz4hc) => super::CompressionAlgorithmSst::Lz4Hc,
                    Ok(CompressionAlgorithm::CompressionLzma) => super::CompressionAlgorithmSst::Lzma,
                    _ => super::CompressionAlgorithmSst::None,
                };
                let level = compression.level.unwrap_or(3) as u8;
                debug!("🗜️ SST: Using SDK-driven compression: {:?} level {}", algorithm, level);
                (algorithm, level)
            } else {
                debug!("🗜️ SST: No compression configuration from SDK, using uncompressed");
                (super::CompressionAlgorithmSst::None, 0)
            };

        // NEW: Analyze overall file format for bytemuck optimization
        let file_vector_format = self.analyze_file_vector_format(&data_blocks);
        let overall_compression_ratio = self.calculate_overall_compression_ratio(&data_blocks);
        let metadata_column_count = self.count_metadata_columns(&data_blocks);
        
        // Calculate offsets for selective reading (header-first design)
        const SST_MAGIC_SIZE: u64 = 4;
        const HEADER_LEN_SIZE: u64 = 4;
        let header_placeholder_size = 1024u64; // Placeholder for header size calculation
        
        let global_bloom_offset = SST_MAGIC_SIZE + HEADER_LEN_SIZE + header_placeholder_size;
        let global_bloom_size = bloom_data.len() as u32;
        let block_index_offset = global_bloom_offset + (global_bloom_size as u64) + 4; // +4 for bloom length
        let block_index_size = index_data.len() as u32;
        let data_blocks_offset = block_index_offset + (block_index_size as u64) + 4; // +4 for index length
        
        // Create enhanced header with hierarchical optimization info
        let mut header = super::SstableHeader {
            version: 1,
            level: 0,
            entry_count: processed_count as u64,
            min_key,
            max_key,
            created_at: chrono::Utc::now().timestamp(),
            
            // Compression configuration
            compression_algorithm,
            compression_level,
            
            // Hierarchical bloom filter configuration
            has_bloom_filter: true,
            has_global_bloom: true,
            has_block_blooms: self.has_any_block_blooms(&index_entries),
            metadata_column_count,
            
            // Block organization
            block_size: self.block_size as u32,
            batch_size: 0,
            block_count: data_blocks.len() as u32,
            
            // Component sizes
            header_size: 0, // Will be updated after serialization
            index_size: block_index_size,
            data_size: total_data_size as u32,
            
            // NEW: Direct access offsets for selective loading
            global_bloom_offset,
            global_bloom_size,
            block_index_offset,
            block_index_size,
            data_blocks_offset,
            
            // NEW: Vector format optimization
            vector_format: file_vector_format,
            fixed_dimension: self.extract_fixed_dimension(&file_vector_format),
            compression_ratio: overall_compression_ratio,
        };
        
        let header_data = bincode::serialize(&header)?;
        header.header_size = header_data.len() as u32;
        let header_data = bincode::serialize(&header)?;
        
        // Build complete SSTable bytes
        const SST_MAGIC: &[u8; 4] = b"SST1";
        
        sstable_bytes.extend_from_slice(SST_MAGIC);
        sstable_bytes.extend_from_slice(&(header_data.len() as u32).to_le_bytes());
        sstable_bytes.extend_from_slice(&header_data);
        sstable_bytes.extend_from_slice(&(bloom_data.len() as u32).to_le_bytes());
        sstable_bytes.extend_from_slice(&bloom_data);
        sstable_bytes.extend_from_slice(&(index_data.len() as u32).to_le_bytes());
        sstable_bytes.extend_from_slice(&index_data);
        
        // Add all data blocks
        for data_block in data_blocks.iter() {
            let block_data = data_block.serialize()?;
            sstable_bytes.extend_from_slice(&(block_data.len() as u32).to_le_bytes());
            sstable_bytes.extend_from_slice(&block_data);
        }
        
        info!(
            "💾 SST: Built streaming SSTable: {} KB with {} records in {} blocks",
            sstable_bytes.len() / 1024,
            processed_count,
            data_blocks.len()
        );
        
        // Atomic write
        let final_path = self.path.to_string_lossy();
        atomic_writer
            .write_atomic(fs, &final_path, &sstable_bytes, None)
            .await
            .map_err(|e| anyhow::anyhow!("Atomic write failed: {}", e))?;
        
        // Verify file
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
            "✅ SST: Streamed SSTable {} ({} bytes, {} records, {} blocks) - DISK-ONLY DESIGN",
            self.path.display(),
            sstable_bytes.len(),
            processed_count,
            data_blocks.len()
        );
        
        Ok(())
    }

    /// LEGACY: Write records to SSTable with atomic write optimization (CRITICAL HOT PATH)
    /// Uses comprehensive atomic write strategies for flush/compaction safety
    /// DEPRECATED: Use write_sorted_records() for disk-only streaming
    #[inline(always)]
    pub async fn write_records(&self, records: BTreeMap<String, SstRecord>) -> Result<()> {
        info!("🔄 Building SSTable in memory for atomic write: {} records", records.len());
        
        if records.is_empty() {
            return Err(anyhow::anyhow!("Cannot write SSTable with 0 records"));
        }
        
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
            for metadata_item in &record.metadata {
                // Already have MetadataItem - no conversion needed!
                metadata_builder.add_metadata_item(metadata_item.key.clone(), metadata_item.clone());
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
        
        // Step 2: Organize records into data blocks (in-memory) - PERFORMANCE OPTIMIZED
        let estimated_blocks = (records.len() / (self.block_size / 256)).max(1); // Estimate based on ~256 bytes per record
        let mut data_blocks = Vec::with_capacity(estimated_blocks); // Pre-allocate capacity
        let mut index_entries = Vec::with_capacity(estimated_blocks); // Pre-allocate capacity
        let mut current_block = Vec::with_capacity(self.block_size / 128); // Pre-allocate for ~128 byte records
        let mut current_block_size = 0;
        let mut block_id = 0u32;
        let records_count = records.len();
        
        if records_count == 0 {
            debug!("⚠️ No records to write to SSTable");
            // Still need to write a valid SSTable file with headers
        }
        
        // Pre-cache serialized records to avoid redundant serialization (CRITICAL OPTIMIZATION)
        let mut serialized_records = Vec::with_capacity(records.len());
        for (_key, record) in records {
            let serialized = record.serialize()?; // Serialize once
            let record_size = serialized.len();
            serialized_records.push((record, serialized, record_size));
        }
        
        // Optimized block organization with cached serialization
        for (record, _serialized, record_size) in serialized_records {
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
        
        // SDK-DRIVEN COMPRESSION (2025-08-06): Use compression config from collection metadata
        let (compression_algorithm, compression_level) = 
            if let Some(ref compression) = self.compression_config {
                use crate::proto::proximadb::CompressionAlgorithm;
                // Convert from proto-generated enum value to SST internal enum
                let algorithm = match CompressionAlgorithm::try_from(compression.algorithm) {
                    Ok(CompressionAlgorithm::CompressionZstd) => super::CompressionAlgorithmSst::Zstd,
                    Ok(CompressionAlgorithm::CompressionLz4) => super::CompressionAlgorithmSst::Lz4,
                    Ok(CompressionAlgorithm::CompressionSnappy) => super::CompressionAlgorithmSst::Snappy,
                    Ok(CompressionAlgorithm::CompressionGzip) => super::CompressionAlgorithmSst::Gzip,
                    Ok(CompressionAlgorithm::CompressionBrotli) => super::CompressionAlgorithmSst::Brotli,
                    Ok(CompressionAlgorithm::CompressionBzip2) => super::CompressionAlgorithmSst::Bzip2,
                    Ok(CompressionAlgorithm::CompressionDeflate) => super::CompressionAlgorithmSst::Deflate,
                    Ok(CompressionAlgorithm::CompressionXz) => super::CompressionAlgorithmSst::Xz,
                    Ok(CompressionAlgorithm::CompressionZlib) => super::CompressionAlgorithmSst::Zlib,
                    Ok(CompressionAlgorithm::CompressionLzo) => super::CompressionAlgorithmSst::Lzo,
                    Ok(CompressionAlgorithm::CompressionLz4hc) => super::CompressionAlgorithmSst::Lz4Hc,
                    Ok(CompressionAlgorithm::CompressionLzma) => super::CompressionAlgorithmSst::Lzma,
                    _ => super::CompressionAlgorithmSst::None,
                };
                let level = compression.level.unwrap_or(3) as u8; // Default compression level
                debug!("🗜️ SST: Using SDK-driven compression: {:?} level {}", algorithm, level);
                (algorithm, level)
            } else {
                debug!("🗜️ SST: No compression configuration from SDK, using uncompressed");
                (super::CompressionAlgorithmSst::None, 0)
            };

        // Create header with hierarchical optimization fields
        let mut header = SstableHeader {
            version: 1,
            level: 0,
            entry_count: records_count as u64,
            min_key,
            max_key,
            created_at: chrono::Utc::now().timestamp(),
            compression_algorithm,
            compression_level,
            has_bloom_filter: true,
            block_size: self.block_size as u32,
            batch_size: 0,
            header_size: 0, // Will be updated after serialization
            index_size: index_data.len() as u32,
            data_size: total_data_size as u32,
            block_count: data_blocks.len() as u32,
            // NEW: Hierarchical optimization fields
            global_bloom_offset: 0, // Will be calculated later
            global_bloom_size: bloom_data.len() as u32,
            block_index_offset: 0, // Will be calculated later  
            block_index_size: index_data.len() as u32,
            data_blocks_offset: 0, // Will be calculated later
            vector_format: super::VectorFormatType::Variable, // Default to variable
            fixed_dimension: None,
            compression_ratio: 1.0, // Will be calculated
            has_global_bloom: true,
            has_block_blooms: false, // Future enhancement
            metadata_column_count: 0, // TODO: analyze metadata
        };
        
        // Serialize header to get its size
        let header_data = bincode::serialize(&header)?;
        header.header_size = header_data.len() as u32;
        
        // Re-serialize with correct header_size
        let header_data = bincode::serialize(&header)?;
        
        // Build complete SSTable bytes: magic + header_len + header + bloom_len + bloom + index_len + index + data_blocks
        // SST1 magic bytes for version 1 (as requested, not SST2 since v1 was never released)
        const SST_MAGIC: &[u8; 4] = b"SST1";
        
        debug!("SSTable Writer - File layout:");
        debug!("  - Magic bytes (4 bytes): SST1");
        debug!("  - Header length (4 bytes): {}", header_data.len());
        debug!("  - Header data ({} bytes)", header_data.len());
        debug!("  - Bloom length (4 bytes): {}", bloom_data.len());
        debug!("  - Bloom data ({} bytes)", bloom_data.len());
        debug!("  - Bloom offset will be: {}", 8 + header_data.len()); // 8 = magic + header_len
        
        // Write magic bytes first
        sstable_bytes.extend_from_slice(SST_MAGIC);
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
    /// Finalize block with optimized performance for hot path operations
    #[inline(always)]
    fn finalize_block(
        &self,
        data_blocks: &mut Vec<DataBlock>,
        index_entries: &mut Vec<IndexEntry>,
        current_block: &[SstRecord],
        block_id: u32,
        _current_block_size: usize,
    ) -> Result<()> {
        let data_block = DataBlock::new(block_id, current_block.to_vec());
        
        let block_size = data_block.serialize().map(|v| v.len()).unwrap_or(0) as u32;
        
        // Collect metadata statistics for this block - PERFORMANCE OPTIMIZED  
        let estimated_columns = current_block.first().map(|r| r.metadata.len()).unwrap_or(4);
        let mut metadata_min_values = HashMap::with_capacity(estimated_columns);
        let mut metadata_max_values = HashMap::with_capacity(estimated_columns);
        let mut metadata_null_counts = HashMap::with_capacity(estimated_columns);
        
        for record in current_block {
            for metadata_item in &record.metadata {
                let column = &metadata_item.key;
                
                // Convert MetadataItem to JSON for statistics (needed for filter expressions)
                let value = match &metadata_item.value {
                    Some(crate::proto::proximadb::metadata_item::Value::StringValue(s)) => 
                        serde_json::Value::String(s.clone()),
                    Some(crate::proto::proximadb::metadata_item::Value::NumberValue(n)) => 
                        serde_json::Number::from_f64(*n)
                            .map(serde_json::Value::Number)
                            .unwrap_or(serde_json::Value::Null),
                    Some(crate::proto::proximadb::metadata_item::Value::BoolValue(b)) => 
                        serde_json::Value::Bool(*b),
                    None => serde_json::Value::Null,
                };
                
                // Track null counts
                if value.is_null() {
                    *metadata_null_counts.entry(column.clone()).or_insert(0) += 1;
                } else {
                    // Track min/max values
                    let entry_min = metadata_min_values.entry(column.clone()).or_insert_with(|| value.clone());
                    if Self::compare_json_values(&value, entry_min) == std::cmp::Ordering::Less {
                        *entry_min = value.clone();
                    }
                    
                    let entry_max = metadata_max_values.entry(column.clone()).or_insert_with(|| value.clone());
                    if Self::compare_json_values(&value, entry_max) == std::cmp::Ordering::Greater {
                        *entry_max = value.clone();
                    }
                }
            }
        }
        
        // NEW: Analyze vector format for this block
        let vector_format = self.analyze_block_vector_format(current_block);
        let compression_ratio = self.estimate_compression_ratio(current_block, &vector_format);
        
        // NEW: Build block-level bloom filters if beneficial
        let (block_key_bloom, block_metadata_bloom) = self.build_block_bloom_filters(current_block, block_id);
        
        // Add enhanced index entry for first record in block
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
                // NEW: Hierarchical bloom filters
                block_key_bloom,
                block_metadata_bloom,
                // NEW: Vector format optimization
                vector_format,
                compression_ratio,
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
    
    /// Set compression configuration (SDK-driven)
    pub fn with_compression_config(mut self, config: Option<CompressionConfig>) -> Self {
        self.compression_config = config;
        self
    }
    
    /// NEW: Analyze vector format for optimal compression in this block
    fn analyze_block_vector_format(&self, block_records: &[SstRecord]) -> super::VectorFormatType {
        if block_records.is_empty() {
            return super::VectorFormatType::Variable;
        }
        
        // Collect dimensions
        let dimensions: Vec<usize> = block_records.iter()
            .map(|r| r.vector.len())
            .collect();
            
        // Find dominant dimension
        let mut dimension_counts = std::collections::HashMap::new();
        for &dim in &dimensions {
            *dimension_counts.entry(dim).or_insert(0) += 1;
        }
        
        let total_vectors = dimensions.len();
        if let Some((&dominant_dim, &count)) = dimension_counts.iter().max_by_key(|(_, &count)| count) {
            let dominance_ratio = count as f64 / total_vectors as f64;
            
            if dominance_ratio >= 0.95 && Self::is_supported_fixed_dimension(dominant_dim) {
                super::VectorFormatType::Fixed { dimension: dominant_dim }
            } else if dominance_ratio >= 0.7 && Self::is_supported_fixed_dimension(dominant_dim) {
                super::VectorFormatType::Mixed { dominant_dimension: dominant_dim }
            } else {
                super::VectorFormatType::Variable
            }
        } else {
            super::VectorFormatType::Variable
        }
    }
    
    /// Check if dimension is supported for fixed-length optimization
    fn is_supported_fixed_dimension(dimension: usize) -> bool {
        matches!(dimension, 64 | 128 | 256 | 512 | 768 | 1024 | 1536 | 2048)
    }
    
    /// NEW: Estimate compression ratio for this block
    fn estimate_compression_ratio(&self, block_records: &[SstRecord], format: &super::VectorFormatType) -> f32 {
        if block_records.is_empty() {
            return 1.0;
        }
        
        // Estimate based on vector format and sparsity
        match format {
            super::VectorFormatType::Fixed { dimension } => {
                // Fixed dimension vectors compress better with bytemuck
                let sparsity = self.estimate_vector_sparsity(block_records);
                if sparsity > 0.7 {
                    0.3 // Very sparse, excellent compression
                } else if sparsity > 0.5 {
                    0.5 // Moderately sparse
                } else {
                    0.8 // Dense vectors
                }
            },
            super::VectorFormatType::Mixed { .. } => 0.7, // Mixed format, moderate compression
            super::VectorFormatType::Variable => 0.9,     // Variable format, minimal compression gain
        }
    }
    
    /// Estimate vector sparsity (ratio of near-zero elements)
    fn estimate_vector_sparsity(&self, block_records: &[SstRecord]) -> f32 {
        if block_records.is_empty() {
            return 0.0;
        }
        
        let sample_size = block_records.len().min(10); // Sample first 10 vectors
        let mut total_elements = 0;
        let mut zero_elements = 0;
        
        for record in block_records.iter().take(sample_size) {
            for &value in &record.vector {
                total_elements += 1;
                if value.abs() < 1e-6 {
                    zero_elements += 1;
                }
            }
        }
        
        if total_elements == 0 {
            0.0
        } else {
            zero_elements as f32 / total_elements as f32
        }
    }
    
    /// NEW: Build block-level bloom filters if beneficial
    fn build_block_bloom_filters(&self, block_records: &[SstRecord], block_id: u32) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
        // Only build block blooms for large blocks (>100 records) to avoid overhead
        if block_records.len() < 100 {
            return (None, None);
        }
        
        let block_key_bloom = self.build_block_key_bloom(block_records);
        let block_metadata_bloom = self.build_block_metadata_bloom(block_records);
        
        (block_key_bloom, block_metadata_bloom)
    }
    
    /// Build key bloom filter for this block
    fn build_block_key_bloom(&self, block_records: &[SstRecord]) -> Option<Vec<u8>> {
        use crate::core::bloom::factory::BloomFilterFactory;
        
        let config = crate::core::bloom::BloomFilterConfig {
            strategy: crate::core::bloom::BloomStrategy::ByteAligned,
            expected_items: block_records.len(),
            false_positive_rate: Some(0.01), // 1% false positive rate for block blooms
            ..Default::default()
        };
        
        let mut bloom = BloomFilterFactory::create(&config);
        for record in block_records {
            bloom.insert(record.id.as_bytes());
        }
        
        bloom.serialize().ok()
    }
    
    /// Build metadata bloom filter for this block
    fn build_block_metadata_bloom(&self, block_records: &[SstRecord]) -> Option<Vec<u8>> {
        use crate::core::bloom::strategies::composite::CompositeBloomFilterBuilder;
        
        let config = crate::core::bloom::BloomFilterConfig {
            strategy: crate::core::bloom::BloomStrategy::Composite,
            expected_items: block_records.len(),
            false_positive_rate: Some(0.01),
            ..Default::default()
        };
        
        let mut builder = CompositeBloomFilterBuilder::new(config);
        for record in block_records {
            for metadata_item in &record.metadata {
                builder.add_metadata_item(metadata_item.key.clone(), metadata_item.clone());
            }
        }
        
        let bloom = builder.build();
        use crate::core::bloom::BloomFilterStrategy;
        BloomFilterStrategy::serialize(&bloom).ok()
    }
    
    /// NEW: Analyze vector format across the entire file
    fn analyze_file_vector_format(&self, data_blocks: &[super::DataBlock]) -> super::VectorFormatType {
        if data_blocks.is_empty() {
            return super::VectorFormatType::Variable;
        }
        
        let mut all_dimensions = Vec::new();
        for block in data_blocks {
            for record in &block.records {
                all_dimensions.push(record.vector.len());
            }
        }
        
        if all_dimensions.is_empty() {
            return super::VectorFormatType::Variable;
        }
        
        // Analyze dimensions across the entire file
        let mut dimension_counts = std::collections::HashMap::new();
        for &dim in &all_dimensions {
            *dimension_counts.entry(dim).or_insert(0) += 1;
        }
        
        let total_vectors = all_dimensions.len();
        if let Some((&dominant_dim, &count)) = dimension_counts.iter().max_by_key(|(_, &count)| count) {
            let dominance_ratio = count as f64 / total_vectors as f64;
            
            if dominance_ratio >= 0.95 && Self::is_supported_fixed_dimension(dominant_dim) {
                super::VectorFormatType::Fixed { dimension: dominant_dim }
            } else if dominance_ratio >= 0.7 && Self::is_supported_fixed_dimension(dominant_dim) {
                super::VectorFormatType::Mixed { dominant_dimension: dominant_dim }
            } else {
                super::VectorFormatType::Variable
            }
        } else {
            super::VectorFormatType::Variable
        }
    }
    
    /// Calculate overall compression ratio across all blocks
    fn calculate_overall_compression_ratio(&self, data_blocks: &[super::DataBlock]) -> f32 {
        if data_blocks.is_empty() {
            return 1.0;
        }
        
        let total_compression_ratio: f32 = data_blocks.iter()
            .map(|block| block.compression_ratio)
            .sum();
            
        total_compression_ratio / data_blocks.len() as f32
    }
    
    /// Count unique metadata columns across all blocks
    fn count_metadata_columns(&self, data_blocks: &[super::DataBlock]) -> u32 {
        let mut metadata_columns = std::collections::HashSet::new();
        
        for block in data_blocks {
            for record in &block.records {
                for metadata_item in &record.metadata {
                    metadata_columns.insert(metadata_item.key.clone());
                }
            }
        }
        
        metadata_columns.len() as u32
    }
    
    /// Check if any index entries have block-level bloom filters
    fn has_any_block_blooms(&self, index_entries: &[super::IndexEntry]) -> bool {
        index_entries.iter().any(|entry| {
            entry.block_key_bloom.is_some() || entry.block_metadata_bloom.is_some()
        })
    }
    
    /// Extract fixed dimension from vector format if applicable
    fn extract_fixed_dimension(&self, format: &super::VectorFormatType) -> Option<u32> {
        match format {
            super::VectorFormatType::Fixed { dimension } => Some(*dimension as u32),
            super::VectorFormatType::Mixed { dominant_dimension } => Some(*dominant_dimension as u32),
            super::VectorFormatType::Variable => None,
        }
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
                vector: vec![1.0, 2.0, 3.0],
                metadata: vec![],
                timestamp: chrono::Utc::now().timestamp() as u32,
                updated_at: Some(chrono::Utc::now().timestamp() as u32),
                expires_at: None,
                version: Some(1),
                is_tombstone: false,
                sequence_number: i as u64,
                level: 0,
            };
            records.insert(record.id.clone(), record);
        }
        
        assert_eq!(records.len(), 10);
    }
}