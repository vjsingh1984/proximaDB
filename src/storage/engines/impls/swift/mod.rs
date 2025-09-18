//! SWIFT Engine: Storage With Instant Fast Traversal - zero-overhead vector storage
//! Clean, forward-looking design - no backward compatibility (Release 1)
//!
//! ## How SWIFT Leverages Common Modules
//!
//! ### 1. Row-Based Module Integration (`fastlanes_blocks::`)
//! - **Hierarchical Blocks**: Uses `SuperBlock` and `DataBlock` from fastlanes_blocks for
//!   its unique three-tier hierarchy (SuperBlock → DataBlock → Records)
//! - **Index Structures**: Leverages `HierarchicalIndex` and `MultiLevelIndex` for
//!   efficient navigation of its deep block structure
//! - **Batch Operations**: Uses `RowBasedBatchOperations` with custom strategies
//!   optimized for hierarchical traversal
//! - **Compression**: Shared compression infrastructure with per-superblock tuning
//!
//! ### 2. SST Module Synergy (`sst::`)
//! - **Bloom Filters**: Reuses SST's `SstableBloomFilter` implementation
//! - **Compaction Logic**: Shares compaction strategies with SST
//! - **Reader Patterns**: Borrows efficient reading patterns from SST
//!
//! ### 3. Universal Adapter Integration (`universal::`)
//! - **Progressive Search**: Uses universal's Binary → INT8 → PQ → FP32 pipeline
//! - **Distance Computation**: All searches go through UniversalDistanceAdapter
//! - **Format Conversion**: Automatic conversion between hierarchical and flat formats
//! - **Hardware Optimization**: SIMD acceleration via universal adapter
//!
//! ### 4. Compute Module Integration (`compute::`)
//! - **Quantization**: Replaced local quantization_blocks with unified `StorageQuantizationEngine`
//! - **Distance Metrics**: Full suite of 13 metrics from `UnifiedDistanceCompute`
//! - **Memory Management**: Shared `VectorMemoryPool` for buffer reuse
//!
//! ## SWIFT-Specific Optimizations
//! - **Three-Tier Hierarchy**: Unique SuperBlock → DataBlock → Records structure
//! - **Billion-Scale Design**: Optimized for datasets with 1B+ vectors
//! - **Zero-Overhead Storage**: Minimal metadata overhead per vector
//! - **Instant Traversal**: O(log n) navigation through hierarchical indexes

pub mod engine;
pub mod hierarchical_blocks;
pub mod id_index;
// NOTE: quantization_blocks removed - using unified quantization from compute module
pub mod batch_operations;
pub mod optimized_operations;
pub mod progressive_search;
pub mod superblock_cache;
pub mod unified_metadata_serializer;
pub mod unified_reader;

// Re-export main engine type and cache
pub use engine::SwiftEngine;
pub use superblock_cache::{
    CachedSuperBlockMetadata, OptimalTreePath, SwiftSuperBlockCache, TreeNavigationHints,
};

use anyhow::{Result, anyhow};
// use std::collections::HashMap; // Unused import
use std::sync::Arc;
use tracing::debug;

use crate::core::compression::CompressionAlgorithm;
use crate::core::VectorRecord;

// SYNERGY: Reuse row-based bloom filter structures (shared with SST)
use crate::core::bloom::SstableBloomFilter;
// FastLanes encoding for columnar vector optimization
use crate::storage::engines::core::ops::fastlanes_encoding::FastLanesScheme;
// NOTE: Quantization now uses unified engine from compute module

// Import FastLanes common structures (SWIFT uses hierarchical structure)
// Note: FastLanesDataBlock provides the block structure with encoding support
use crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesDataBlock;

/// SWIFT-specific SuperBlock structure - hierarchical container for multiple data blocks
#[derive(Debug)]
pub struct SuperBlock {
    pub superblock_id: usize,
    pub name: String,
    pub blocks: Vec<FastLanesDataBlock>,
    pub superblock_encoding_marker: u8,
    pub centroid: Option<Vec<f32>>,
    pub quantized_signature: Vec<u8>,
    pub bloom_filter: Option<SstableBloomFilter>,
    pub record_count: u32, // Track total records in this superblock
    pub superblock_encoding_metadata: Option<crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesMetadata>,
}

impl SuperBlock {
    pub fn new(id: usize, name: String) -> Self {
        Self {
            superblock_id: id,
            name,
            blocks: Vec::new(),
            superblock_encoding_marker: 0x00,
            centroid: None,
            quantized_signature: Vec::new(),
            bloom_filter: None,
            record_count: 0,
            superblock_encoding_metadata: None,
        }
    }

    pub fn add_block(&mut self, block: FastLanesDataBlock) {
        self.blocks.push(block);
    }
}

/// Placeholder for quantized index - now handled by unified compute module
#[derive(Debug)]
pub struct QuantizedIndex {
    dimension: usize,
}

impl QuantizedIndex {
    pub fn new(dimension: usize) -> Self {
        Self { dimension }
    }
}

/// SWIFT file structure - hierarchical superblock design
#[derive(Debug)]
pub struct SwiftFile {
    /// File header containing all metadata
    pub header: SwiftHeader,

    /// Three-tier hierarchy for billion-scale vectors
    pub superblocks: Vec<SuperBlock>,

    /// Global indexes for different access patterns
    pub id_index: id_index::IdIndex,
    pub quantized_index: QuantizedIndex,
    pub metadata_index: hierarchical_blocks::MetadataIndex,

    /// Memory management
    memory_manager: Arc<MemoryManager>,
}

/// Magic constant for SWIFT files (4 bytes)
pub const SWIFT_MAGIC: [u8; 4] = *b"SWFT";

/// SWIFT header - all metadata in one place
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SwiftHeader {
    // File identification
    pub magic: [u8; 4],
    pub version: u32,

    // Collection information
    pub collection_id: String,
    pub timestamp: i64,
    pub compaction_level: u8,

    // Vector configuration
    pub dimension: usize,
    pub distance_metric: String,
    pub quantization: QuantizationConfig,

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

impl Default for SwiftHeader {
    fn default() -> Self {
        Self {
            magic: SWIFT_MAGIC,
            version: 1,
            collection_id: String::new(),
            timestamp: 0,
            compaction_level: 0,
            dimension: 0,
            distance_metric: "cosine".to_string(), // TODO: Use proper enum conversion
            quantization: QuantizationConfig::default(),
            total_records: 0,
            deleted_records: 0,
            superblock_count: 0,
            blocks_per_superblock: 10,
            records_per_block: 1000,
            superblock_offset: 4096, // After header
            id_index_offset: 0,
            quantized_index_offset: 0,
            metadata_index_offset: 0,
            header_checksum: 0,
            file_checksum: 0,
        }
    }
}

/// DEPRECATED: Being replaced with unified config
/// Use crate::core::unified_config::EngineQuantizationConfig instead
#[derive(Debug, Clone)]
pub struct QuantizationConfigOLD {
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

impl Default for QuantizationConfigOLD {
    fn default() -> Self {
        Self {
            enable_binary: false,
            binary_threshold: 0.5,
            enable_int8: false,
            int8_scale: 1.0,
            int8_zero_point: 0,
            enable_pq: false,
            pq_segments: 8,
            pq_bits: 8,
            pq_codebooks: Vec::new(),
            compression_algorithm: CompressionAlgorithm::None,
            compression_level: 0,
        }
    }
}

/// Use proto-generated config directly - no more duplicates!
pub use crate::proto::proximadb_v1::QuantizationConfig;

/// PQ Codebook
#[derive(Debug, Clone)]
pub struct Codebook {
    pub segment_id: u8,
    pub dimension: usize,
    pub centroids: Vec<Vec<f32>>,
    pub distance_table: Vec<Vec<f32>>,
}

// SuperBlock and DataBlock are now imported from fastlanes_blocks common module
// Additional SWIFT-specific fields can be added via composition if needed

/// Column statistics for metadata filtering
#[derive(Debug, Clone)]
pub struct ColumnStats {
    pub column_name: String,
    pub null_count: u32,
    pub distinct_count: u32,
    pub min_value: serde_json::Value,
    pub max_value: serde_json::Value,
}

/// Memory manager for efficient resource usage
#[derive(Debug)]
pub struct MemoryManager {
    max_memory_bytes: usize,
    current_usage: std::sync::atomic::AtomicUsize,
}

impl SwiftFile {
    /// Build blocks from vector records with universal adapters
    pub fn build_blocks_from_records_with_adapters(
        &mut self,
        records: Vec<VectorRecord>,
        quantization_engine: Option<
            &crate::compute::quantization::storage_engine::StorageQuantizationEngine,
        >,
        quantization_config: Option<
            &crate::compute::quantization::storage_engine::StorageQuantizationConfig,
        >,
    ) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        // Group records into blocks (~2000 vectors per block)
        let records_per_block = self.header.records_per_block as usize;
        let mut block_id = 0;

        for chunk in records.chunks(records_per_block) {
            // Create compression config
            let compression_config = crate::storage::engines::core::formats::fastlanes_blocks::block_structures::BlockCompressionConfig::default();

            // Use row-based DataBlock constructor
            let block = FastLanesDataBlock::new(chunk.to_vec(), compression_config);

            // Build quantized representations for the block
            // Note: quantize_batch requires owned vectors for now
            let vectors: Vec<Vec<f32>> = chunk.iter().map(|r| r.vector.clone()).collect();

            // Use quantization engine if provided to quantize vectors
            if let (Some(engine), Some(config)) = (quantization_engine, quantization_config) {
                // Quantize vectors using the storage quantization engine
                use tokio::runtime::Handle;
                let quantized_vectors = Handle::current()
                    .block_on(async { engine.quantize_batch(&vectors, None).await })
                    .map_err(|e| anyhow::anyhow!("Failed to quantize vectors: {}", e))?;

                // Encode quantized vectors with FastLanes based on quantization level
                use crate::compute::quantization::types::QuantizationLevel;
                use crate::storage::engines::core::ops::fastlanes_encoding::FastLanesEncoder;

                // Use BitPacked scheme as default for vector encoding
                let fastlanes_encoder =
                    FastLanesEncoder::new(FastLanesScheme::BitPacked { bits: 32 });

                // Process each quantized vector and encode with appropriate FastLanes method
                for (idx, quantized) in quantized_vectors.iter().enumerate() {
                    // Access primary quantization data if available
                    let encoded_data = if let Some(ref primary) = quantized.primary {
                        match &primary.quantization_level.level_type {
                            None | Some(QuantizationLevel::None(_)) => {
                                // Full precision FP32 - use FastLanes float encoding
                                fastlanes_encoder.encode_f32(&vectors[idx])?
                            }
                            Some(QuantizationLevel::Binary(_)) => {
                                // Binary quantization - use FastLanes binary encoding
                                fastlanes_encoder.encode_binary(&primary.data)?
                            }
                            Some(QuantizationLevel::Scalar(config)) if config.bits == 8 => {
                                // INT8 quantization - use FastLanes INT8 encoding
                                let int8_data: Vec<i8> =
                                    primary.data.iter().map(|&b| b as i8).collect();
                                fastlanes_encoder.encode_int8(&int8_data)?
                            }
                            Some(QuantizationLevel::Pq(config)) if config.bits_per_code == 4 => {
                                // PQ4 quantization - use FastLanes PQ4 encoding
                                fastlanes_encoder
                                    .encode_pq4(&primary.data, config.num_subvectors as usize)?
                            }
                            Some(QuantizationLevel::Pq(config)) if config.bits_per_code == 8 => {
                                // PQ8 quantization - use FastLanes PQ8 encoding
                                fastlanes_encoder
                                    .encode_pq8(&primary.data, config.num_subvectors as usize)?
                            }
                            _ => {
                                // Fallback to raw quantized data
                                primary.data.clone()
                            }
                        }
                    } else {
                        // No quantization available, fallback to FP32 encoding
                        fastlanes_encoder.encode_f32(&vectors[idx])?
                    };

                    // Store encoded data in the block's quantized section
                    // The DataBlock should have a quantized_section field for this
                    // For now, we'll store it as part of the block metadata
                    // TODO: Update DataBlock structure to include quantized_section field
                }
            }

            // Update ID index
            for (idx, record) in chunk.iter().enumerate() {
                if !record.id.is_empty() {
                    self.id_index.add(record.id.clone(), block_id as u32, idx)?;
                }
            }

            // Group blocks into superblocks (64 blocks per superblock)
            let superblock_id = block_id / 64;
            if self.superblocks.len() <= superblock_id {
                // Use row-based SuperBlock constructor
                let mut superblock =
                    SuperBlock::new(superblock_id, format!("swift_sb_{}", superblock_id));

                // FASTLANES: Set SuperBlock-level encoding for hierarchical compression
                // SWIFT benefits from encoding 10K vectors together for better compression
                superblock.superblock_encoding_marker = 0x80; // SWIFT SuperBlock encoding

                // Initialize SWIFT-specific fields
                superblock.centroid = Some(vec![0.0; self.header.dimension]);
                superblock.quantized_signature = Vec::new();

                // Initialize bloom filter with default configuration
                let bloom_config = crate::core::bloom::BloomFilterConfig {
                    bits_per_key: 10,
                    false_positive_rate: Some(0.01),
                    expected_items: 1000,
                    strategy: crate::core::bloom::BloomStrategy::ByteAligned,
                    enabled: true,
                    hash_algorithm: crate::core::bloom::HashAlgorithm::default(),
                };
                let bloom_stats = crate::core::bloom::BloomFilterStats::default();
                superblock.bloom_filter = Some(SstableBloomFilter::new(
                    bloom_config,
                    Vec::new(), // Empty key filter data initially
                    Vec::new(), // Empty metadata filter data initially
                    bloom_stats,
                ));

                self.superblocks.push(superblock);
            }

            self.superblocks[superblock_id].blocks.push(block);
            self.superblocks[superblock_id].record_count += chunk.len() as u32;

            block_id += 1;
        }

        // Update header statistics
        self.header.total_records = records.len() as u64;
        self.header.superblock_count = self.superblocks.len() as u32;

        // Build metadata indexes
        // TODO: Fix SuperBlock type mismatch - MetadataIndex expects different SuperBlock type
        // self.metadata_index.build_from_superblocks(&self.superblocks[..])?;

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
            // Create compression config
            let compression_config = crate::storage::engines::core::formats::fastlanes_blocks::block_structures::BlockCompressionConfig::default();

            // Use row-based DataBlock constructor
            let block = FastLanesDataBlock::new(chunk.to_vec(), compression_config);

            // Build quantized representations for the block
            // Note: Quantization is handled by the unified quantization system
            // This method should receive quantization parameters like build_blocks_from_records_with_adapters
            let vectors: Vec<Vec<f32>> = chunk.iter().map(|r| r.vector.clone()).collect();

            // TODO: Add quantization support by passing quantization_engine and config as parameters
            // For now, store vectors without quantization
            debug!("Stored {} vectors in block {} (quantization TODO)", vectors.len(), block_id);

            // Update ID index
            for (idx, record) in chunk.iter().enumerate() {
                if !record.id.is_empty() {
                    self.id_index.add(record.id.clone(), block_id as u32, idx)?;
                }
            }

            // Group blocks into superblocks (64 blocks per superblock)
            let superblock_id = block_id / 64;
            if self.superblocks.len() <= superblock_id as usize {
                // Use row-based SuperBlock constructor
                let mut superblock =
                    SuperBlock::new(superblock_id, format!("swift_sb_{}", superblock_id));

                // FASTLANES: Set SuperBlock-level encoding for hierarchical compression
                // SWIFT benefits from encoding 10K vectors together for better compression
                superblock.superblock_encoding_marker = 0x80; // SWIFT SuperBlock encoding

                // Initialize SWIFT-specific fields
                superblock.centroid = Some(vec![0.0; self.header.dimension]);
                superblock.quantized_signature = Vec::new();

                // Initialize bloom filter with default configuration
                let bloom_config = crate::core::bloom::BloomFilterConfig {
                    bits_per_key: 10,
                    false_positive_rate: Some(0.01),
                    expected_items: 1000,
                    strategy: crate::core::bloom::BloomStrategy::ByteAligned,
                    enabled: true,
                    hash_algorithm: crate::core::bloom::HashAlgorithm::default(),
                };
                let bloom_stats = crate::core::bloom::BloomFilterStats::default();
                superblock.bloom_filter = Some(SstableBloomFilter::new(
                    bloom_config,
                    Vec::new(), // Empty key filter data initially
                    Vec::new(), // Empty metadata filter data initially
                    bloom_stats,
                ));

                self.superblocks.push(superblock);
            }

            self.superblocks[superblock_id].blocks.push(block);
            self.superblocks[superblock_id].record_count += chunk.len() as u32;

            block_id += 1;
        }

        // Update header statistics
        self.header.total_records = records.len() as u64;
        self.header.superblock_count = self.superblocks.len() as u32;

        // Build metadata indexes
        // TODO: Fix SuperBlock type mismatch - MetadataIndex expects different SuperBlock type
        // self.metadata_index.build_from_superblocks(&self.superblocks[..])?;

        Ok(())
    }

    /// Load a record at a specific location
    pub fn load_record_at_location(
        &self,
        location: &id_index::RecordLocation,
    ) -> Result<VectorRecord> {
        let superblock_id = location.superblock_idx;
        let block_idx = location.block_idx as usize;

        if superblock_id as usize >= self.superblocks.len() {
            return Err(anyhow!("Superblock {} not found", superblock_id));
        }

        let superblock = &self.superblocks[superblock_id as usize];
        if block_idx >= superblock.blocks.len() {
            return Err(anyhow!("Block {} not found in superblock", block_idx));
        }

        let block = &superblock.blocks[block_idx];
        if location.offset_in_block as usize >= block.records.len() {
            return Err(anyhow!(
                "Record offset {} out of bounds",
                location.offset_in_block
            ));
        }

        Ok(block.records[location.offset_in_block as usize].clone())
    }

    /// Create a new SWIFT file - clean slate, no legacy
    pub fn new(collection_id: String, dimension: usize, distance_metric: String) -> Self {
        let header = SwiftHeader {
            magic: SWIFT_MAGIC,
            version: 1,
            collection_id,
            timestamp: chrono::Utc::now().timestamp(),
            compaction_level: 0,
            dimension,
            distance_metric,
            quantization: QuantizationConfig::default(),
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
            // Quantized index now handled by unified compute module
            quantized_index: QuantizedIndex::new(dimension),
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

    /// FASTLANES: Optimize SuperBlock encoding for columnar SIMD and hierarchical compression
    /// Uses columnar layout for maximum SIMD efficiency and optimized I/O
    fn finalize_superblock_encoding(&mut self) {
        use crate::storage::engines::core::formats::fastlanes_blocks::block_structures::FastLanesMetadata;
        // use crate::core::hardware_capabilities::HardwareCapabilities; // Unused import

        let hw_caps = crate::core::hardware_capabilities::get_hardware_capabilities();

        for superblock in &mut self.superblocks {
            // Columnar analysis: transpose vectors to analyze dimension-wise
            let mut all_vectors = Vec::new();
            for block in &superblock.blocks {
                for record in &block.records {
                    all_vectors.push(&record.vector);
                }
            }

            if all_vectors.is_empty() {
                continue;
            }

            let dimension = all_vectors[0].len();
            let vector_count = all_vectors.len();

            // COLUMNAR ANALYSIS: Transpose to dimension-major layout for SIMD
            let mut columnar_stats = Vec::new();
            for dim in 0..dimension {
                let mut values: Vec<f32> = all_vectors.iter().map(|v| v[dim]).collect();
                values.sort_by(|a, b| a.partial_cmp(b).unwrap());

                let min_val = values[0];
                let max_val = values[values.len() - 1];
                let range = max_val - min_val;

                // Calculate delta compression potential
                let mut sum_delta = 0.0f32;
                for i in 1..values.len() {
                    sum_delta += (values[i] - values[i - 1]).abs();
                }
                let avg_delta = sum_delta / (values.len() - 1) as f32;

                columnar_stats.push((min_val, max_val, range, avg_delta));
            }

            // SIMD-optimized scheme selection based on hardware capabilities
            let (marker, scheme) = if hw_caps.cpu.simd.has_avx512 {
                // AVX-512: 16x f32 SIMD operations
                Self::select_avx512_scheme(&columnar_stats, vector_count)
            } else if hw_caps.cpu.simd.has_avx2 {
                // AVX2: 8x f32 SIMD operations
                Self::select_avx2_scheme(&columnar_stats, vector_count)
            } else if hw_caps.cpu.simd.has_sse {
                // SSE: 4x f32 SIMD operations
                Self::select_sse_scheme(&columnar_stats, vector_count)
            } else {
                // Fallback: scalar optimized
                Self::select_scalar_scheme(&columnar_stats, vector_count)
            };

            // Calculate global statistics for metadata
            let (global_min, global_max) = columnar_stats.iter().fold(
                (f32::MAX, f32::MIN),
                |(min_acc, max_acc), &(min_val, max_val, _, _)| {
                    (min_acc.min(min_val), max_acc.max(max_val))
                },
            );

            superblock.superblock_encoding_marker = marker;
            superblock.superblock_encoding_metadata = Some(FastLanesMetadata {
                scheme,
                dimension,
                vector_count,
                bits_per_value: Some(((global_max - global_min).log2().ceil() as u8).max(1)),
                base_value: Some(global_min as i64),
                dict_size: None,
                patch_count: None,
                min_value: global_min,
                max_value: global_max,
                range_bits: ((global_max - global_min).log2().ceil() as u8).max(1),
                compression_ratio: 1.0, // Will be calculated during actual encoding
            });

            // Update child blocks to inherit SuperBlock columnar encoding
            for block in &mut superblock.blocks {
                // Child blocks inherit columnar SIMD optimization
                if block.encoding_marker == 0x00 || block.encoding_marker == 0x10 {
                    block.encoding_marker = 0xFF; // Inherit columnar SIMD from SuperBlock
                    block.encoding_metadata = None; // Use SuperBlock columnar metadata
                }
            }
        }
    }

    /// AVX-512 optimized scheme selection for 16-wide SIMD
    fn select_avx512_scheme(
        stats: &[(f32, f32, f32, f32)],
        vector_count: usize,
    ) -> (u8, FastLanesScheme) {
        let avg_range =
            stats.iter().map(|(_, _, range, _)| *range).sum::<f32>() / stats.len() as f32;
        let avg_delta =
            stats.iter().map(|(_, _, _, delta)| *delta).sum::<f32>() / stats.len() as f32;

        if avg_range < 1e-6 {
            (0x96, FastLanesScheme::RunLength) // AVX-512 run-length
        } else if avg_delta < avg_range / 16.0 {
            // Excellent delta compression for AVX-512
            (
                0x92,
                FastLanesScheme::Delta {
                    base: stats[0].0 as i64,
                },
            )
        } else if avg_range < 32.0 {
            // Frame-of-reference for 16-wide SIMD
            (
                0x93,
                FastLanesScheme::FrameOfReference {
                    reference: stats[0].0 as i64,
                    bits: (avg_range.log2().ceil() as u8).max(6), // Optimized for AVX-512
                },
            )
        } else {
            // High-precision BitPacking for AVX-512
            (
                0x91,
                FastLanesScheme::BitPacked {
                    bits: (avg_range.log2().ceil() as u8).max(10),
                },
            )
        }
    }

    /// AVX2 optimized scheme selection for 8-wide SIMD
    fn select_avx2_scheme(
        stats: &[(f32, f32, f32, f32)],
        vector_count: usize,
    ) -> (u8, FastLanesScheme) {
        let avg_range =
            stats.iter().map(|(_, _, range, _)| *range).sum::<f32>() / stats.len() as f32;
        let avg_delta =
            stats.iter().map(|(_, _, _, delta)| *delta).sum::<f32>() / stats.len() as f32;

        if avg_range < 1e-6 {
            (0x86, FastLanesScheme::RunLength) // AVX2 run-length
        } else if avg_delta < avg_range / 8.0 {
            // Good delta compression for AVX2
            (
                0x82,
                FastLanesScheme::Delta {
                    base: stats[0].0 as i64,
                },
            )
        } else if avg_range < 64.0 {
            // Frame-of-reference for 8-wide SIMD
            (
                0x83,
                FastLanesScheme::FrameOfReference {
                    reference: stats[0].0 as i64,
                    bits: (avg_range.log2().ceil() as u8).max(8),
                },
            )
        } else {
            // Standard BitPacking for AVX2
            (
                0x81,
                FastLanesScheme::BitPacked {
                    bits: (avg_range.log2().ceil() as u8).max(12),
                },
            )
        }
    }

    /// SSE optimized scheme selection for 4-wide SIMD
    fn select_sse_scheme(
        stats: &[(f32, f32, f32, f32)],
        vector_count: usize,
    ) -> (u8, FastLanesScheme) {
        let avg_range =
            stats.iter().map(|(_, _, range, _)| *range).sum::<f32>() / stats.len() as f32;
        let avg_delta =
            stats.iter().map(|(_, _, _, delta)| *delta).sum::<f32>() / stats.len() as f32;

        if avg_range < 1e-6 {
            (0x76, FastLanesScheme::RunLength) // SSE run-length
        } else if avg_delta < avg_range / 4.0 {
            // Modest delta compression for SSE
            (
                0x72,
                FastLanesScheme::Delta {
                    base: stats[0].0 as i64,
                },
            )
        } else if avg_range < 128.0 {
            // Conservative frame-of-reference for 4-wide SIMD
            (
                0x73,
                FastLanesScheme::FrameOfReference {
                    reference: stats[0].0 as i64,
                    bits: (avg_range.log2().ceil() as u8).max(10),
                },
            )
        } else {
            // Basic BitPacking for SSE
            (
                0x71,
                FastLanesScheme::BitPacked {
                    bits: (avg_range.log2().ceil() as u8).max(14),
                },
            )
        }
    }

    /// Scalar optimized scheme selection (no SIMD)
    fn select_scalar_scheme(
        stats: &[(f32, f32, f32, f32)],
        vector_count: usize,
    ) -> (u8, FastLanesScheme) {
        let avg_range =
            stats.iter().map(|(_, _, range, _)| *range).sum::<f32>() / stats.len() as f32;
        let avg_delta =
            stats.iter().map(|(_, _, _, delta)| *delta).sum::<f32>() / stats.len() as f32;

        if avg_range < 1e-6 {
            (0x66, FastLanesScheme::RunLength) // Scalar run-length
        } else if avg_delta < avg_range / 2.0 {
            // Simple delta compression
            (
                0x62,
                FastLanesScheme::Delta {
                    base: stats[0].0 as i64,
                },
            )
        } else if avg_range < 256.0 {
            // Basic frame-of-reference
            (
                0x63,
                FastLanesScheme::FrameOfReference {
                    reference: stats[0].0 as i64,
                    bits: (avg_range.log2().ceil() as u8).max(12),
                },
            )
        } else {
            // Conservative BitPacking
            (
                0x61,
                FastLanesScheme::BitPacked {
                    bits: (avg_range.log2().ceil() as u8).max(16),
                },
            )
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swift_file_creation() {
        let sst = SwiftFile::new("test_collection".to_string(), 768, "cosine".to_string());

        assert_eq!(sst.header.collection_id, "test_collection");
        assert_eq!(sst.header.dimension, 768);
        assert_eq!(sst.header.version, 1);
        assert_eq!(sst.header.magic, SWIFT_MAGIC);
    }

    #[test]
    fn test_quantization_config_default() {
        let config = QuantizationConfig::default();
        // The proto config structure is different
        assert!(config.enabled);
    }
}
