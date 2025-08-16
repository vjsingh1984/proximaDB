// SWIFT Engine: Storage With Instant Fast Traversal - zero-overhead vector storage
// Clean, forward-looking design - no backward compatibility (Release 1)

pub mod engine;
pub mod id_index;
pub mod hierarchical_blocks;
pub mod quantization_blocks;
pub mod progressive_search;
pub mod batch_operations;
pub mod optimized_operations;
pub mod unified_reader;

// Re-export main engine type
pub use engine::SwiftEngine;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use crate::core::{DistanceMetric, VectorRecord};
use crate::core::compression::CompressionAlgorithm;

// SYNERGY: Reuse SST's bloom filter structures
use crate::storage::engines::sst::{
    bloom_filter::SstableBloomFilter,
    quantization_compat::QuantizedSection,
};
// Quantization moved to universal adapter
use crate::storage::quantization::sst_adapter::SstQuantizationAdapter;

// Import row-based common structures
use crate::storage::engines::row_based::block_structures::{
    RowBasedDataBlock as DataBlock,
    SuperBlock,
};

/// Clean SST file structure - no legacy baggage
#[derive(Debug)]
pub struct SstFile {
    /// File header containing all metadata
    pub header: SstHeader,
    
    /// Three-tier hierarchy for billion-scale vectors
    pub superblocks: Vec<SuperBlock>,
    
    /// Global indexes for different access patterns
    pub id_index: id_index::IdIndex,
    pub quantized_index: quantization_blocks::QuantizedIndex,
    pub metadata_index: hierarchical_blocks::MetadataIndex,
    
    /// Memory management
    memory_manager: Arc<MemoryManager>,
}

/// SST header - all metadata in one place
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SstHeader {
    // File identification
    pub magic: [u8; 8],
    pub version: u32,
    pub file_id: Uuid,
    
    // Collection information
    pub collection_id: String,
    pub created_at: i64,
    pub compaction_level: u8,
    
    // Vector configuration
    pub dimension: usize,
    pub distance_metric: DistanceMetric,
    pub quantization_config: QuantizationConfig,
    
    // Record counts
    pub total_records: u64,
    pub deleted_records: u64,
    
    // Layout information
    pub superblock_count: u32,
    pub blocks_per_superblock: u32,
    pub records_per_block: u32,
    
    // Index offsets
    pub superblock_offset: u64,
    pub id_index_offset: u64,
    pub quantized_index_offset: u64,
    pub metadata_index_offset: u64,
    
    // Checksums
    pub header_checksum: u32,
    pub file_checksum: u64,
}

/// Quantization configuration - multi-level for progressive search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuantizationConfig {
    // Binary quantization
    pub enable_binary: bool,
    pub binary_threshold: f32,
    
    // INT8 quantization
    pub enable_int8: bool,
    pub int8_scale: f32,
    pub int8_zero_point: i8,
    
    // Product Quantization
    pub enable_pq: bool,
    pub pq_segments: u8,
    pub pq_bits: u8,
    pub pq_codebooks: Vec<Codebook>,
    
    // Compression
    pub compression_algorithm: CompressionAlgorithm,
    pub compression_level: u8,
}

/// PQ Codebook
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Codebook {
    pub segment_id: u8,
    pub dimension: usize,
    pub centroids: Vec<Vec<f32>>,
    pub distance_table: Vec<Vec<f32>>,
}

// SuperBlock and DataBlock are now imported from row_based common module
// Additional SWIFT-specific fields can be added via composition if needed

/// Column statistics for metadata filtering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnStats {
    pub column_name: String,
    pub null_count: u32,
    pub distinct_count: u32,
    pub min_value: serde_json::Value,
    pub max_value: serde_json::Value,
}

/// Memory manager for efficient resource usage
pub struct MemoryManager {
    max_memory_bytes: usize,
    current_usage: std::sync::atomic::AtomicUsize,
}

impl SstFile {
    /// Build blocks from vector records with universal adapters
    pub fn build_blocks_from_records_with_adapters(
        &mut self, 
        records: Vec<VectorRecord>,
        quantization_adapter: Option<&crate::storage::engines::common::UniversalQuantizationAdapter>,
        quantization_config: Option<&crate::storage::engines::common::UniversalQuantizationConfig>,
    ) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        
        // Group records into blocks (~2000 vectors per block)
        let records_per_block = self.header.records_per_block as usize;
        let mut block_id = 0;
        
        for chunk in records.chunks(records_per_block) {
            // Create quantized section for the block
            let quantized_section = QuantizedSection::default();
            
            // Create compression config
            let compression_config = crate::storage::engines::row_based::block_structures::BlockCompressionConfig::default();
            
            // Use row-based DataBlock constructor
            let mut block = DataBlock::new(
                chunk.to_vec(),
                quantized_section,
                compression_config,
            );
            
            // Set SWIFT-specific fields
            block.sequence_number = block_id as u64;
            
            // Build quantized representations for the block
            let vectors: Vec<Vec<f32>> = chunk.iter()
                .map(|r| r.vector.clone())
                .collect();
            
            // Use universal adapter if provided to quantize vectors
            if let (Some(adapter), Some(config)) = (quantization_adapter, quantization_config) {
                // Quantize vectors and update the quantized_section
                // The quantized_section is already part of the DataBlock
                // We need to populate it with the quantized data
                // TODO: Add quantization logic to populate block.quantized_section
            }
            
            // Update ID index
            for (idx, record) in chunk.iter().enumerate() {
                if let Some(id) = &record.id {
                    self.id_index.add(id.clone(), block_id, idx)?;
                }
            }
            
            // Group blocks into superblocks (64 blocks per superblock)
            let superblock_id = block_id / 64;
            if self.superblocks.len() <= superblock_id as usize {
                // Use row-based SuperBlock constructor
                let mut superblock = SuperBlock::new(superblock_id, format!("swift_sb_{}", superblock_id));
                
                // Initialize SWIFT-specific fields
                superblock.centroid = Some(vec![0.0; self.header.dimension]);
                superblock.quantized_signature = Vec::new();
                superblock.bloom_filter = SstableBloomFilter::new(10000, 0.01);
                
                self.superblocks.push(superblock);
            }
            
            self.superblocks[superblock_id as usize].blocks.push(block);
            self.superblocks[superblock_id as usize].record_count += chunk.len() as u32;
            
            block_id += 1;
        }
        
        // Update header statistics
        self.header.total_records = records.len() as u64;
        self.header.superblock_count = self.superblocks.len() as u32;
        
        // Build metadata indexes
        self.metadata_index.build_from_superblocks(&self.superblocks)?;
        
        Ok(())
    }
    
    /// Legacy build blocks method (deprecated, use build_blocks_from_records_with_adapters)
    pub fn build_blocks_from_records(&mut self, records: Vec<VectorRecord>) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        
        // Group records into blocks (~2000 vectors per block)
        let records_per_block = self.header.records_per_block as usize;
        let mut block_id = 0;
        
        for chunk in records.chunks(records_per_block) {
            // Create quantized section for the block
            let quantized_section = QuantizedSection::default();
            
            // Create compression config
            let compression_config = crate::storage::engines::row_based::block_structures::BlockCompressionConfig::default();
            
            // Use row-based DataBlock constructor
            let mut block = DataBlock::new(
                chunk.to_vec(),
                quantized_section,
                compression_config,
            );
            
            // Set SWIFT-specific fields
            block.sequence_number = block_id as u64;
            
            // Build quantized representations for the block
            let vectors: Vec<Vec<f32>> = chunk.iter()
                .map(|r| r.vector.clone())
                .collect();
            block.quantized_block.quantize_vectors(&vectors, &self.header.quantization_config)?;
            
            // Update ID index
            for (idx, record) in chunk.iter().enumerate() {
                if let Some(id) = &record.id {
                    self.id_index.add(id.clone(), block_id, idx)?;
                }
            }
            
            // Group blocks into superblocks (64 blocks per superblock)
            let superblock_id = block_id / 64;
            if self.superblocks.len() <= superblock_id as usize {
                // Use row-based SuperBlock constructor
                let mut superblock = SuperBlock::new(superblock_id, format!("swift_sb_{}", superblock_id));
                
                // Initialize SWIFT-specific fields
                superblock.centroid = Some(vec![0.0; self.header.dimension]);
                superblock.quantized_signature = Vec::new();
                superblock.bloom_filter = SstableBloomFilter::new(10000, 0.01);
                
                self.superblocks.push(superblock);
            }
            
            self.superblocks[superblock_id as usize].blocks.push(block);
            self.superblocks[superblock_id as usize].record_count += chunk.len() as u32;
            
            block_id += 1;
        }
        
        // Update header statistics
        self.header.total_records = records.len() as u64;
        self.header.superblock_count = self.superblocks.len() as u32;
        
        // Build metadata indexes
        self.metadata_index.build_from_superblocks(&self.superblocks)?;
        
        Ok(())
    }
    
    /// Load a record at a specific location
    pub fn load_record_at_location(&self, location: &id_index::RecordLocation) -> Result<VectorRecord> {
        let superblock_id = location.block_id / 64;
        let block_idx = (location.block_id % 64) as usize;
        
        if superblock_id as usize >= self.superblocks.len() {
            return Err(anyhow!("Superblock {} not found", superblock_id));
        }
        
        let superblock = &self.superblocks[superblock_id as usize];
        if block_idx >= superblock.blocks.len() {
            return Err(anyhow!("Block {} not found in superblock", block_idx));
        }
        
        let block = &superblock.blocks[block_idx];
        if location.offset_in_block >= block.records.len() {
            return Err(anyhow!("Record offset {} out of bounds", location.offset_in_block));
        }
        
        Ok(block.records[location.offset_in_block].clone())
    }
    
    /// Create a new SST file - clean slate, no legacy
    pub fn new(collection_id: String, dimension: usize, distance_metric: DistanceMetric) -> Self {
        let header = SstHeader {
            magic: *b"PROXSST\0",
            version: 1,
            file_id: Uuid::new_v4(),
            collection_id,
            created_at: chrono::Utc::now().timestamp(),
            compaction_level: 0,
            dimension,
            distance_metric,
            quantization_config: QuantizationConfig::default(),
            total_records: 0,
            deleted_records: 0,
            superblock_count: 0,
            blocks_per_superblock: 64,
            records_per_block: 2000,
            superblock_offset: 0,
            id_index_offset: 0,
            quantized_index_offset: 0,
            metadata_index_offset: 0,
            header_checksum: 0,
            file_checksum: 0,
        };
        
        Self {
            header,
            superblocks: Vec::new(),
            id_index: id_index::IdIndex::new(),
            quantized_index: quantization_blocks::QuantizedIndex::new(dimension),
            metadata_index: hierarchical_blocks::MetadataIndex::new(),
            memory_manager: Arc::new(MemoryManager {
                max_memory_bytes: 4 * 1024 * 1024 * 1024, // 4GB
                current_usage: std::sync::atomic::AtomicUsize::new(0),
            }),
        }
    }
    
    /// Mode 1: AXIS-driven ID lookup
    pub async fn get_by_ids(&self, ids: &[String]) -> Result<Vec<VectorRecord>> {
        batch_operations::get_records_by_ids(self, ids).await
    }
    
    /// Mode 2: Index-free similarity search with progressive refinement
    pub async fn search_without_index(
        &self,
        query: &[f32],
        top_k: usize,
        filter: Option<MetadataFilter>,
    ) -> Result<Vec<VectorRecord>> {
        progressive_search::search_progressive(self, query, top_k, filter).await
    }
}

/// Metadata filter for queries
#[derive(Debug, Clone)]
pub struct MetadataFilter {
    pub conditions: Vec<FilterCondition>,
}

#[derive(Debug, Clone)]
pub enum FilterCondition {
    Equals(String, serde_json::Value),
    Range(String, serde_json::Value, serde_json::Value),
    In(String, Vec<serde_json::Value>),
    IsNull(String),
    IsNotNull(String),
}

impl Default for QuantizationConfig {
    fn default() -> Self {
        Self {
            enable_binary: true,
            binary_threshold: 0.0,
            enable_int8: true,
            int8_scale: 127.0,
            int8_zero_point: 0,
            enable_pq: true,
            pq_segments: 16,
            pq_bits: 8,
            pq_codebooks: Vec::new(),
            compression_algorithm: CompressionAlgorithm::Lz4,
            compression_level: 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_sst_file_creation() {
        let sst = SstFile::new(
            "test_collection".to_string(),
            768,
            DistanceMetric::Cosine,
        );
        
        assert_eq!(sst.header.collection_id, "test_collection");
        assert_eq!(sst.header.dimension, 768);
        assert_eq!(sst.header.version, 1);
        assert_eq!(sst.header.magic, *b"PROXSST\0");
    }
    
    #[test]
    fn test_quantization_config_default() {
        let config = QuantizationConfig::default();
        assert!(config.enable_binary);
        assert!(config.enable_int8);
        assert!(config.enable_pq);
        assert_eq!(config.pq_segments, 16);
        assert_eq!(config.pq_bits, 8);
    }
}