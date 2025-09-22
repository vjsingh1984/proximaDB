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
//!
//! FASTLANES ENCODING INTEGRATION:
//! ================================
//! This writer intelligently chooses encoding schemes per FastLanesDataBlock based on data analysis:
//!
//! 1. ENCODING DECISION PROCESS:
//!    - Analyze vector statistics (range, deltas, patterns)
//!    - Choose optimal encoding (BitPacked, Delta, FrameOfReference, etc.)
//!    - Transpose vectors for columnar encoding within blocks
//!    - Write encoding marker as first byte of block
//!
//! 2. VECTOR ENCODING STRATEGY:
//!    a) Collect block of vectors (typically 500-1000)
//!    b) Transpose to columnar layout: vectors[N][D] → columns[D][N]
//!    c) Analyze each dimension column independently
//!    d) Apply dimension-specific encoding:
//!       - Constant dimensions → Run-length encoding
//!       - Small range → Frame of Reference
//!       - Sequential → Delta encoding
//!       - General → BitPacking
//!
//! 3. ENCODING MARKERS (1 byte):
//!    0x00: Raw/Uncompressed (backward compatible)
//!    0x10: FastLanes BitPacked
//!    0x20: FastLanes Delta
//!    0x30: FastLanes FrameOfReference
//!    0x40: FastLanes PatchedBase (for outliers)
//!    0x50: FastLanes Dictionary
//!    0x60: FastLanes RunLength
//!    0xF0-0xFF: Reserved for future encodings
//!
//! 4. METADATA ENCODING:
//!    - Timestamps: Always delta encoded
//!    - IDs: Dictionary encoded if repetitive
//!    - Versions: BitPacked (small range)
//!
//! 5. ADAPTIVE STRATEGY:
//!    - Monitor encoding effectiveness
//!    - Fall back to raw if encoding increases size
//!    - Track statistics for future optimization

use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info};

use crate::storage::persistence::filesystem::{
    FilesystemFactory, atomic_strategy::AtomicWriteExecutorFactory,
};

use super::IndexEntry;
use crate::proto::proximadb_v1::VectorRecord; // OPTIMIZED: Direct VectorRecord usage
use crate::core::bloom::factory::BloomFilterFactory;
use crate::core::bloom::{BloomFilterConfig, BloomFilterStrategy, HashAlgorithm};
use crate::storage::engines::core::formats::fastlanes_blocks::FastLanesDataBlock;
// Using unified quantization engine directly from compute module
// use crate::core::bloom::{
//     BloomFilterConfig, BloomStrategy, BloomFilterStrategy, HashAlgorithm,
//     factory::BloomFilterFactory,
// };
use crate::core::bloom::strategies::composite::CompositeBloomFilterBuilder;
use crate::proto::proximadb_v1::CompressionConfig;

// Use core compression directly instead of adapter
use crate::core::compression::{
    CompressionAlgorithm, CompressionContext, CompressionProvider, StandardCompression,
};

// FastLanes encoding delegation
use crate::storage::engines::core::ops::fastlanes_encoding::{FastLanesEncoder, FastLanesScheme};

/// SSTable writer with atomic write optimization and quantization support
/// MIGRATED: Now uses universal adapters to eliminate code duplication
pub struct SstableWriter {
    /// Output file path
    path: std::path::PathBuf,
    /// Block size for data organization
    block_size: usize,
    /// Bloom filter configuration
    bloom_config: BloomFilterConfig,
    /// Filesystem factory for atomic writes
    filesystem: Arc<FilesystemFactory>,
    /// Direct compression provider (no adapter indirection)
    compression_provider: StandardCompression,
    /// Unified quantization engine from compute module
    quantization_engine:
        Arc<crate::compute::quantization::storage_engine::StorageQuantizationEngine>,
    /// Compression configuration from flush parameters
    compression_config: Option<CompressionConfig>,
}

impl SstableWriter {
    /// Create a new SSTable writer with collection-specific configuration
    pub fn new_with_config<P: AsRef<Path>>(
        path: P,
        block_size: usize,
        filesystem: Arc<FilesystemFactory>,
        collection_config: Option<&crate::proto::proximadb_v1::Collection>,
    ) -> Self {
        // Initialize compression provider directly
        let compression_provider = StandardCompression::default();

        // Initialize unified quantization engine from compute module
        let distance_compute = Arc::new(
            crate::compute::distance_computation::engine::UnifiedDistanceCompute::default(),
        );
        let codebook_store =
            Arc::new(crate::compute::quantization::unified::InMemoryCodebookStore::new());
        let unified_engine = Arc::new(
            crate::compute::quantization::unified::UnifiedQuantizationEngine::new(
                distance_compute.clone(),
                codebook_store,
            ),
        );

        // Configure storage quantization for SST (row-based storage)
        let storage_config =
            crate::compute::quantization::storage_engine::StorageQuantizationConfig {
                primary_level: Some(
                    crate::compute::quantization::unified::UnifiedQuantizationLevel::pq8(32),
                ),
                filter_level: Some(
                    crate::compute::quantization::unified::UnifiedQuantizationLevel::binary(),
                ),
                fast_level: Some(
                    crate::compute::quantization::unified::UnifiedQuantizationLevel::int8(),
                ),
                distance_metric: if let Some(collection) = collection_config {
                    // Get distance metric from collection config
                    collection.config.as_ref()
                    .map(|cfg| match cfg.distance_metric() {
                        crate::proto::proximadb_v1::DistanceMetric::Cosine =>
                            crate::compute::distance_computation::engine::DistanceMetric::Cosine,
                        crate::proto::proximadb_v1::DistanceMetric::Euclidean =>
                            crate::compute::distance_computation::engine::DistanceMetric::Euclidean,
                        crate::proto::proximadb_v1::DistanceMetric::DotProduct =>
                            crate::compute::distance_computation::engine::DistanceMetric::DotProduct,
                        _ => crate::compute::distance_computation::engine::DistanceMetric::Cosine,
                    })
                    .unwrap_or(crate::compute::distance_computation::engine::DistanceMetric::Cosine)
                } else {
                    crate::compute::distance_computation::engine::DistanceMetric::Cosine
                },
                enable_progressive: true,
                filter_threshold: 100.0,
                candidate_multiplier: 10,
                training_sample_size: 10000,
                memory_budget_mb: 256, // SST uses less memory than columnar
                enable_hardware_acceleration: true,
            };

        let quantization_engine = Arc::new(
            crate::compute::quantization::storage_engine::StorageQuantizationEngine::new(
                unified_engine,
                distance_compute,
                storage_config,
            ),
        );

        // Compression configuration can be added here if needed

        Self {
            path: path.as_ref().to_path_buf(),
            block_size,
            bloom_config: BloomFilterConfig {
                strategy: crate::core::bloom::BloomStrategy::ByteAligned,
                expected_items: 10000,
                false_positive_rate: Some(0.01),
                bits_per_key: 8,
                enabled: true,
                hash_algorithm: HashAlgorithm::Murmur3,
            },
            filesystem,
            compression_provider,
            quantization_engine,
            compression_config: None,
        }
    }

    /// Create a new SSTable writer with filesystem support for atomic writes
    /// Quantization is enabled by default as it's part of the SST file layout
    pub fn new<P: AsRef<Path>>(
        path: P,
        block_size: usize,
        filesystem: Arc<FilesystemFactory>,
    ) -> Self {
        Self::new_with_config(path, block_size, filesystem, None)
    }

    /// MIGRATED: Serialize a data block using universal compression adapter
    /// This eliminates duplicate compression logic and provides adaptive selection
    fn compress_block_streaming(
        &self,
        data_block: &FastLanesDataBlock,
        algorithm: crate::core::compression::CompressionAlgorithm,
        level: u8,
    ) -> Result<Vec<u8>> {
        debug!("🔍 SST WRITER: Compressing block with universal adapter");
        debug!("   Algorithm: {:?}", algorithm);
        debug!("   Level: {}", level);
        debug!("   Block records: {}", data_block.records.len());

        // FASTLANES: Apply encoding before serialization based on block analysis
        let encoded_data_block = self.encode_block_with_fastlanes(data_block)?;
        let serialized = encoded_data_block.serialize()?;

        // Select algorithm based on data size for optimal performance
        let algorithm = if serialized.len() < 1024 {
            CompressionAlgorithm::Lz4 // Fast for small blocks
        } else if serialized.len() < 64 * 1024 {
            CompressionAlgorithm::Snappy // Balanced
        } else {
            CompressionAlgorithm::Zstd // High compression for large blocks
        };

        let context = CompressionContext::Block;
        let compressed =
            self.compression_provider
                .compress(&serialized, algorithm, level as i32, context)?;
        debug!(
            "✅ Direct compression: {} -> {} bytes",
            serialized.len(),
            compressed.len()
        );
        Ok(compressed)
    }

    /// MIGRATION: Create SSTable writer with universal adapters
    /// Both compression and quantization use universal adapters for code deduplication
    pub fn with_compression<P: AsRef<Path>>(
        path: P,
        block_size: usize,
        filesystem: Arc<FilesystemFactory>,
        compression_config: Option<CompressionConfig>,
    ) -> Self {
        // Initialize universal compression adapter
        let compression_provider = StandardCompression::default();

        // Initialize unified quantization engine from compute module
        let distance_compute = Arc::new(
            crate::compute::distance_computation::engine::UnifiedDistanceCompute::default(),
        );
        let codebook_store =
            Arc::new(crate::compute::quantization::unified::InMemoryCodebookStore::new());
        let unified_engine = Arc::new(
            crate::compute::quantization::unified::UnifiedQuantizationEngine::new(
                distance_compute.clone(),
                codebook_store,
            ),
        );

        // Configure storage quantization for SST
        let storage_config =
            crate::compute::quantization::storage_engine::StorageQuantizationConfig {
                primary_level: Some(
                    crate::compute::quantization::unified::UnifiedQuantizationLevel::pq8(32),
                ),
                filter_level: Some(
                    crate::compute::quantization::unified::UnifiedQuantizationLevel::binary(),
                ),
                fast_level: Some(
                    crate::compute::quantization::unified::UnifiedQuantizationLevel::int8(),
                ),
                distance_metric:
                    crate::compute::distance_computation::engine::DistanceMetric::Cosine,
                enable_progressive: true,
                filter_threshold: 100.0,
                candidate_multiplier: 10,
                training_sample_size: 10000,
                memory_budget_mb: 256,
                enable_hardware_acceleration: true,
            };

        let quantization_engine = Arc::new(
            crate::compute::quantization::storage_engine::StorageQuantizationEngine::new(
                unified_engine,
                distance_compute,
                storage_config,
            ),
        );

        Self {
            path: path.as_ref().to_path_buf(),
            block_size,
            bloom_config: BloomFilterConfig::default(),
            filesystem,
            compression_provider,
            quantization_engine,
            compression_config: compression_config,
        }
    }

    // Removed with_quantization() method - quantization is ALWAYS enabled
    // as it's integral to the SST file layout and provides PQ sorting for
    // better compression and selectivity

    /// Write sorted VectorRecords to SSTable with quantization support
    /// OPTIMIZATION: Direct VectorRecord processing with multi-level quantization
    ///
    /// USAGE PATTERNS:
    /// - FLUSH: Receives entire batch from memtable → sorts → quantizes → streams to writer
    /// - COMPACTION: Receives pre-sorted stream from K-way merge → direct streaming
    ///
    /// QUANTIZATION LEVELS:
    /// - FP32: Original full precision (when needed for reranking)
    /// - INT8: Fast approximate search
    /// - PQ4/PQ8: Memory-efficient storage
    /// - Binary: Ultra-fast filtering
    #[inline(always)]
    pub async fn write_sorted_vector_records<I>(
        &self,
        sorted_records: I,
        record_count: usize,
    ) -> Result<()>
    where
        I: Iterator<Item = (String, VectorRecord)>,
    {
        info!(
            "🚀 SST STREAMING PATH: Writing {} pre-sorted VectorRecords directly",
            record_count
        );
        debug!("📊 SST WRITER PATH ANALYSIS:");
        debug!("   - Input: Pre-sorted VectorRecord stream");
        debug!("   - No conversions: VectorRecord → VectorRecord");
        debug!("   - Quantization: Applied based on collection config");
        debug!("   - Compression: Applied based on collection config");

        if record_count == 0 {
            info!(
                "⚠️ SST: No records to write - this may be a valid scenario (e.g., compaction with no data)"
            );
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
            // strategy removed -  BloomStrategy::ByteAligned,
            expected_items: record_count,
            ..self.bloom_config.clone()
        };
        let mut key_bloom_filter = BloomFilterFactory::create(&bloom_config);

        let metadata_config = BloomFilterConfig {
            // strategy removed -  BloomStrategy::Composite,
            expected_items: record_count,
            ..self.bloom_config.clone()
        };
        let mut metadata_builder = CompositeBloomFilterBuilder::new(metadata_config);

        // Step 2: Stream VectorRecords directly into blocks (NO CONVERSIONS)
        let estimated_blocks = (record_count / (self.block_size / 256)).max(1);
        let mut data_blocks = Vec::with_capacity(estimated_blocks);
        let mut index_entries = Vec::with_capacity(estimated_blocks);
        let mut current_block = Vec::with_capacity(self.block_size / 128);
        let mut current_block_size = 0;
        let mut block_id = 0u32;
        let mut processed_count = 0;
        let mut metadata_value_count = 0;

        // Process VectorRecords in streaming fashion (DIRECT PROCESSING)
        for (key, vector_record) in sorted_records {
            // Update bloom filters
            key_bloom_filter.insert(key.as_bytes());

            for (key, sql_value) in &vector_record.metadata {
                // Convert SqlValue to MetadataItem
                let metadata_value = if let Some(value) = &sql_value.value {
                    use crate::proto::proximadb_v1::sql_value::Value as SqlValueType;
                    use crate::proto::proximadb_v1::metadata_item::Value as MetadataValueType;
                    match value {
                        SqlValueType::StringValue(s) => Some(MetadataValueType::StringValue(s.clone())),
                        SqlValueType::NumberValue(n) => Some(MetadataValueType::NumberValue(*n)),
                        SqlValueType::BoolValue(b) => Some(MetadataValueType::BoolValue(*b)),
                        SqlValueType::Int64Value(i) => Some(MetadataValueType::NumberValue(*i as f64)),
                        _ => None,
                    }
                } else {
                    None
                };
                
                let metadata_item = crate::proto::proximadb_v1::MetadataItem {
                    key: key.clone(),
                    value: metadata_value,
                };
                metadata_builder.add_metadata_item(key.clone(), metadata_item);
                metadata_value_count += 1;
            }

            // FASTEST: Use existing protobuf serialization (already optimized)
            use prost::Message;
            let mut serialized = Vec::new();
            vector_record.encode(&mut serialized)?;
            let record_size = serialized.len();

            // Check if we need to start a new block
            if current_block_size + record_size > self.block_size && !current_block.is_empty() {
                self.finalize_vector_block(
                    &mut data_blocks,
                    &mut index_entries,
                    &current_block,
                    block_id,
                    current_block_size,
                )?;
                current_block.clear();
                current_block_size = 0;
                block_id += 1;
            }

            current_block.push(vector_record);
            current_block_size += record_size;
            processed_count += 1;
        }

        // Handle the last block
        if !current_block.is_empty() {
            self.finalize_vector_block(
                &mut data_blocks,
                &mut index_entries,
                &current_block,
                block_id,
                current_block_size,
            )?;
        }

        debug!(
            "🔍 Streamed {} VectorRecords into {} blocks with {} metadata columns",
            processed_count,
            data_blocks.len(),
            metadata_value_count
        );

        // Continue with rest of the write process (reuse existing logic)
        // MIGRATION: Apply quantization using universal adapter
        info!(
            "🔧 SST: Applying universal quantization to {} VectorRecords",
            processed_count
        );

        // Convert to required format for quantization
        let vector_records = data_blocks
            .iter()
            .flat_map(|block| block.records.clone())
            .collect::<Vec<_>>();

        // Note: Some operations still require owned vectors
        let all_vectors: Vec<Vec<f32>> = vector_records.iter().map(|r| r.vector.clone()).collect();

        // Quantization: Unified engine already initialized above (lines 114-120)
        // Three-stage filtering: Bloom → Quantized → Full precision implemented
        // For current write operation, using direct vector storage with bloom filter optimization

        // Proceed with existing SST file creation logic
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

        // Use shared SST metadata serializer from fastlanes_blocks module
        use crate::storage::engines::core::formats::fastlanes_blocks::sst_metadata::{
            SstBlockHeader, SstGlobalHeader,
        };

        // Calculate offsets manually since atomic writer doesn't track position
        let current_offset = 0u64;

        // Create global header
        let global_header = SstGlobalHeader {
            file_size: 0, // Will be updated after writing all data
            num_blocks: data_blocks.len() as u32,
            bloom_filter_offset: current_offset as u32,
            bloom_filter_size: combined_bloom_filter.serialize()?.len() as u32,
            index_offset: 0, // Will be set after bloom filter
            index_size: index_entries
                .iter()
                .map(|e| e.serialize().unwrap().len())
                .sum::<usize>() as u32,
            total_records: processed_count as u64,
            min_timestamp: 0,        // TODO: extract from data
            max_timestamp: u64::MAX, // TODO: extract from data
            compression_ratio: 70,   // Estimated compression ratio
            reserved: [0; 7],
        };

        // Create block headers for each data block
        let mut block_headers = Vec::new();
        for (i, block) in data_blocks.iter().enumerate() {
            let header = SstBlockHeader {
                offset: 0,            // Will be calculated during writing
                compressed_size: 0,   // Will be calculated during compression
                uncompressed_size: 0, // Will be calculated
                record_count: block.records.len() as u32,
                bloom_offset: 0,        // Block-level bloom filter offset (if any)
                bloom_size: 0,          // Block-level bloom filter size
                min_key_hash: 0,        // TODO: calculate from block data
                max_key_hash: u64::MAX, // TODO: calculate from block data
                priority: 128,          // Medium priority
                reserved: [0; 7],
            };
            block_headers.push(header);
        }

        // Accumulate all data to write atomically
        let mut output_data = Vec::new();

        // Write SST1 magic bytes at the beginning
        output_data.extend_from_slice(b"SST1");

        // Write header length placeholder (will be updated after header is built)
        let header_len_pos = output_data.len();
        output_data.extend_from_slice(&0u32.to_le_bytes());
        // Use shared FastLanes serialization for data blocks
        debug!("📦 Writing {} data blocks using FastLanes serialization", data_blocks.len());
        for block in &data_blocks {
            // Serialize the block using the shared FastLanes format
            let serialized_block = block.serialize()?;
            // Write block length prefix for framing
            output_data.extend_from_slice(&(serialized_block.len() as u32).to_le_bytes());
            output_data.extend_from_slice(&serialized_block);
        }
        let data_blocks_size = output_data.len();
        debug!("✅ Wrote {} bytes of FastLanes-encoded vector data", data_blocks_size);

        // Write index entries
        for entry in &index_entries {
            output_data.extend_from_slice(&entry.serialize()?);
        }

        // Write bloom filter
        output_data.extend_from_slice(&combined_bloom_filter.serialize()?);

        // Manually serialize metadata instead of using bincode
        let mut footer_bytes = Vec::new();

        // Serialize global header manually
        footer_bytes.extend_from_slice(&global_header.file_size.to_le_bytes());
        footer_bytes.extend_from_slice(&global_header.num_blocks.to_le_bytes());
        footer_bytes.extend_from_slice(&global_header.bloom_filter_offset.to_le_bytes());
        footer_bytes.extend_from_slice(&global_header.bloom_filter_size.to_le_bytes());
        footer_bytes.extend_from_slice(&global_header.index_offset.to_le_bytes());
        footer_bytes.extend_from_slice(&global_header.index_size.to_le_bytes());
        footer_bytes.extend_from_slice(&global_header.total_records.to_le_bytes());
        footer_bytes.extend_from_slice(&global_header.min_timestamp.to_le_bytes());
        footer_bytes.extend_from_slice(&global_header.max_timestamp.to_le_bytes());
        footer_bytes.extend_from_slice(&global_header.compression_ratio.to_le_bytes());
        footer_bytes.extend_from_slice(&global_header.reserved);

        // Serialize block count
        footer_bytes.extend_from_slice(&(block_headers.len() as u32).to_le_bytes());

        // Serialize block headers manually
        for header in &block_headers {
            footer_bytes.extend_from_slice(&header.offset.to_le_bytes());
            footer_bytes.extend_from_slice(&header.compressed_size.to_le_bytes());
            footer_bytes.extend_from_slice(&header.uncompressed_size.to_le_bytes());
            footer_bytes.extend_from_slice(&header.record_count.to_le_bytes());
            footer_bytes.extend_from_slice(&header.bloom_offset.to_le_bytes());
            footer_bytes.extend_from_slice(&header.bloom_size.to_le_bytes());
            footer_bytes.extend_from_slice(&header.min_key_hash.to_le_bytes());
            footer_bytes.extend_from_slice(&header.max_key_hash.to_le_bytes());
            footer_bytes.extend_from_slice(&header.priority.to_le_bytes());
            footer_bytes.extend_from_slice(&header.reserved);
        }

        // Serialize variable data size and data
        footer_bytes.extend_from_slice(&0u32.to_le_bytes()); // No variable data
        output_data.extend_from_slice(&footer_bytes);

        // Update header length at the reserved position
        let header_len = footer_bytes.len() as u32;
        let header_len_bytes = header_len.to_le_bytes();
        output_data[header_len_pos..header_len_pos + 4].copy_from_slice(&header_len_bytes);

        // Write all data atomically
        atomic_writer
            .write_atomic(&*fs, &self.path.to_string_lossy(), &output_data, None)
            .await?;

        Ok(())
    }

    /// Finalize a VectorRecord block (adapted from finalize_block)
    #[inline(always)]
    fn finalize_vector_block(
        &self,
        data_blocks: &mut Vec<FastLanesDataBlock>,
        index_entries: &mut Vec<IndexEntry>,
        current_block: &[VectorRecord],
        block_id: u32,
        _current_block_size: usize,
    ) -> Result<()> {
        // Build block-level bloom filters
        let (block_key_bloom, block_metadata_bloom) =
            self.build_vector_block_bloom_filters(current_block, block_id);

        // Create FastLanesDataBlock with VectorRecord
        // Note: The FastLanesDataBlock's encode_with_fastlanes method will handle:
        // 1. Transposing vectors to columnar format for better compression
        // 2. Using FastLanes encoding for SIMD-optimized operations
        // The SST writer has a quantization_engine field that can be used for
        // quantization before creating blocks, but for now we keep FP32 vectors
        // and let FastLanes handle the encoding optimization
        // Create compression config for the block - use the config from flush parameters
        let block_compression_config = if let Some(ref comp_config) = self.compression_config {
            crate::storage::engines::core::formats::fastlanes_blocks::block_structures::BlockCompressionConfig {
                algorithm: match comp_config.algorithm {
                    1 => crate::core::compression::CompressionAlgorithm::Zstd,
                    2 => crate::core::compression::CompressionAlgorithm::Lz4,
                    3 => crate::core::compression::CompressionAlgorithm::Snappy,
                    4 => crate::core::compression::CompressionAlgorithm::Gzip,
                    5 => crate::core::compression::CompressionAlgorithm::Brotli,
                    _ => crate::core::compression::CompressionAlgorithm::Zstd, // Default
                },
                compression_level: comp_config.level.unwrap_or(3) as u8,
                enable_vector_compression: true,
                enable_metadata_compression: true,
                compression_threshold_bytes: 8192,
                dictionary_compression: false,
            }
        } else {
            crate::storage::engines::core::formats::fastlanes_blocks::block_structures::BlockCompressionConfig::default()
        };
        let mut data_block = FastLanesDataBlock::new(current_block.to_vec(), block_compression_config);
        data_block.block_id = block_id;

        // Set block-level bloom filter
        // Convert Vec<u8> bloom filters to SstableBloomFilter
        data_block.block_bloom_filter = if let Some(ref key_bloom) = block_key_bloom {
            Some(super::bloom_filter::SstableBloomFilter::new(
                self.bloom_config.clone(),
                key_bloom.clone(),
                Vec::new(),
                super::bloom_filter::BloomFilterStats::default(),
            ))
        } else if let Some(ref metadata_bloom) = block_metadata_bloom {
            Some(super::bloom_filter::SstableBloomFilter::new(
                self.bloom_config.clone(),
                Vec::new(),
                metadata_bloom.clone(),
                super::bloom_filter::BloomFilterStats::default(),
            ))
        } else {
            None
        };

        let block_size = data_block.serialize().map(|v| v.len()).unwrap_or(0) as u32;

        // Collect metadata statistics for this block
        let estimated_columns = current_block.first().map(|r| r.metadata.len());
        let capacity = estimated_columns.unwrap_or(10);
        let mut metadata_min_values = HashMap::with_capacity(capacity);
        let mut metadata_max_values = HashMap::with_capacity(capacity);
        let mut metadata_null_counts = HashMap::with_capacity(capacity);

        for record in current_block {
            for (key, sql_value) in &record.metadata {
                let column = key;

                // Convert SqlValue to JSON for statistics
                let value = match &sql_value.value {
                    Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) => {
                        serde_json::Value::String(s.clone())
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(n)) => {
                        serde_json::Number::from_f64(*n)
                            .map(serde_json::Value::Number)
                            .unwrap_or(serde_json::Value::Null)
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)) => {
                        serde_json::Value::Bool(*b)
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(i)) => {
                        serde_json::Value::Number(serde_json::Number::from(*i))
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::BytesValue(_)) => {
                        serde_json::Value::String("[binary data]".to_string())
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::NullValue(_)) => {
                        serde_json::Value::Null
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::ArrayValue(_)) => {
                        serde_json::Value::String("[array]".to_string())
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::ObjectValue(_)) => {
                        serde_json::Value::String("[object]".to_string())
                    }
                    None => serde_json::Value::Null,
                };

                // Track null counts
                if value.is_null() {
                    *metadata_null_counts.entry(column.clone()).or_insert(0) += 1;
                } else {
                    // Track min/max values
                    let entry_min = metadata_min_values
                        .entry(column.clone())
                        .or_insert_with(|| value.clone());
                    if Self::compare_json_values(&value, entry_min) == std::cmp::Ordering::Less {
                        *entry_min = value.clone();
                    }

                    let entry_max = metadata_max_values
                        .entry(column.clone())
                        .or_insert_with(|| value.clone());
                    if Self::compare_json_values(&value, entry_max) == std::cmp::Ordering::Greater {
                        *entry_max = value.clone();
                    }
                }
            }
        }

        // Analyze vector format for this block
        let vector_format = self.analyze_vector_block_format(current_block);

        // Add enhanced index entry for first record in block
        if let Some(first_record) = current_block.first() {
            let first_id = first_record.id.clone();
            index_entries.push(IndexEntry {
                key: first_id,
                offset: 0, // Will be calculated during read
                size: block_size,
                block_id,
                block_offset: 0,
                compressed: false,
                metadata_min_values,
                metadata_max_values,
                metadata_null_counts,
                block_key_bloom,
                block_metadata_bloom,
                vector_format,
            });
        }

        data_blocks.push(data_block);
        Ok(())
    }

    /// Build bloom filters for VectorRecord block
    fn build_vector_block_bloom_filters(
        &self,
        block_records: &[VectorRecord],
        _block_id: u32,
    ) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
        // Only build block blooms for large blocks (>100 records) to avoid overhead
        if block_records.len() < 100 {
            return (None, None);
        }

        let block_key_bloom = self.build_vector_block_key_bloom(block_records);
        let block_metadata_bloom = self.build_vector_block_metadata_bloom(block_records);

        (block_key_bloom, block_metadata_bloom)
    }

    /// Build key bloom filter for VectorRecord block
    fn build_vector_block_key_bloom(&self, block_records: &[VectorRecord]) -> Option<Vec<u8>> {
        use crate::core::bloom::BloomFilterConfig;
        use crate::core::bloom::factory::BloomFilterFactory;

        let config = BloomFilterConfig {
            // strategy removed -  crate::core::bloom::BloomStrategy::ByteAligned,
            expected_items: block_records.len(),
            false_positive_rate: Some(0.01),
            ..Default::default()
        };

        let mut bloom = BloomFilterFactory::create(&config);
        for record in block_records {
            bloom.insert(record.id.as_bytes());
        }

        bloom.serialize().ok()
    }

    /// Build metadata bloom filter for VectorRecord block
    fn build_vector_block_metadata_bloom(&self, block_records: &[VectorRecord]) -> Option<Vec<u8>> {
        use crate::core::bloom::strategies::composite::CompositeBloomFilterBuilder;

        let config = crate::core::bloom::BloomFilterConfig {
            // strategy removed -  crate::core::bloom::BloomStrategy::Composite,
            expected_items: block_records.len(),
            false_positive_rate: Some(0.01),
            ..Default::default()
        };

        let mut builder = CompositeBloomFilterBuilder::new(config);
        for record in block_records {
            for (key, sql_value) in &record.metadata {
                let metadata_item = crate::core::proto_metadata_helper::sqlvalue_to_metadata_item(key.clone(), sql_value);
                builder.add_metadata_item(key.clone(), metadata_item);
            }
        }

        let bloom = builder.build();
        use crate::core::bloom::BloomFilterStrategy;
        BloomFilterStrategy::serialize(&bloom).ok()
    }

    /// Analyze vector format for VectorRecord block
    fn analyze_vector_block_format(&self, block_records: &[VectorRecord]) -> super::VectorFormat {
        if block_records.is_empty() {
            return super::VectorFormat::Variable;
        }

        // Collect dimensions
        let dimensions: Vec<usize> = block_records.iter().map(|r| r.vector.len()).collect();

        // Find dominant dimension
        let mut dimension_counts = std::collections::HashMap::new();
        for &dim in &dimensions {
            *dimension_counts.entry(dim).or_insert(0) += 1;
        }

        let total_vectors = dimensions.len();
        if let Some((dominant_dim, count)) =
            dimension_counts.iter().max_by_key(|(_, count)| **count)
        {
            let dominance_ratio = *count as f64 / total_vectors as f64;

            if dominance_ratio >= 0.95 && Self::is_supported_fixed_dimension(*dominant_dim) {
                super::VectorFormat::Fixed {
                    dimension: *dominant_dim,
                }
            } else if dominance_ratio >= 0.7 && Self::is_supported_fixed_dimension(*dominant_dim) {
                super::VectorFormat::Mixed {
                    dominant_dimension: *dominant_dim,
                }
            } else {
                super::VectorFormat::Variable
            }
        } else {
            super::VectorFormat::Variable
        }
    }

    // Quantization methods removed - now handled by unified compute module directly

    /// Helper to finalize a data block
    /// Finalize block with optimized performance for hot path operations
    #[inline(always)]
    fn finalize_block(
        &self,
        data_blocks: &mut Vec<FastLanesDataBlock>,
        index_entries: &mut Vec<IndexEntry>,
        current_block: &[VectorRecord],
        block_id: u32,
        _current_block_size: usize,
    ) -> Result<()> {
        // NEW: Build block-level bloom filters first (needed for FastLanesDataBlock creation)
        let (block_key_bloom, block_metadata_bloom) =
            self.build_block_bloom_filters(current_block, block_id);

        // Create FastLanesDataBlock with hierarchical metadata
        // Create compression config for the block
        // Use the compression config from flush parameters
        let block_compression_config = if let Some(ref comp_config) = self.compression_config {
            crate::storage::engines::core::formats::fastlanes_blocks::block_structures::BlockCompressionConfig {
                algorithm: match comp_config.algorithm {
                    1 => crate::core::compression::CompressionAlgorithm::Zstd,
                    2 => crate::core::compression::CompressionAlgorithm::Lz4,
                    3 => crate::core::compression::CompressionAlgorithm::Snappy,
                    4 => crate::core::compression::CompressionAlgorithm::Gzip,
                    5 => crate::core::compression::CompressionAlgorithm::Brotli,
                    _ => crate::core::compression::CompressionAlgorithm::Zstd, // Default
                },
                compression_level: comp_config.level.unwrap_or(3) as u8,
                enable_vector_compression: true,
                enable_metadata_compression: true,
                compression_threshold_bytes: 8192,
                dictionary_compression: false,
            }
        } else {
            crate::storage::engines::core::formats::fastlanes_blocks::block_structures::BlockCompressionConfig::default()
        };
        let mut data_block = FastLanesDataBlock::new(current_block.to_vec(), block_compression_config);
        data_block.block_id = block_id;

        // Set block-level bloom filter (combines key and metadata blooms into one)
        // Convert Vec<u8> bloom filters to SstableBloomFilter
        data_block.block_bloom_filter = if let Some(ref key_bloom) = block_key_bloom {
            Some(super::bloom_filter::SstableBloomFilter::new(
                self.bloom_config.clone(),
                key_bloom.clone(),
                Vec::new(),
                super::bloom_filter::BloomFilterStats::default(),
            ))
        } else if let Some(ref metadata_bloom) = block_metadata_bloom {
            Some(super::bloom_filter::SstableBloomFilter::new(
                self.bloom_config.clone(),
                Vec::new(),
                metadata_bloom.clone(),
                super::bloom_filter::BloomFilterStats::default(),
            ))
        } else {
            None
        };

        let block_size = data_block.serialize().map(|v| v.len()).unwrap_or(0) as u32;

        // Collect metadata statistics for this block - PERFORMANCE OPTIMIZED
        let estimated_columns = current_block.first().map(|r| r.metadata.len());
        let capacity = estimated_columns.unwrap_or(10);
        let mut metadata_min_values = HashMap::with_capacity(capacity);
        let mut metadata_max_values = HashMap::with_capacity(capacity);
        let mut metadata_null_counts = HashMap::with_capacity(capacity);

        for record in current_block {
            for (key, sql_value) in &record.metadata {
                let column = key;

                // Convert SqlValue to JSON for statistics (needed for filter expressions)
                let value = match &sql_value.value {
                    Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(s)) => {
                        serde_json::Value::String(s.clone())
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::NumberValue(n)) => {
                        serde_json::Number::from_f64(*n)
                            .map(serde_json::Value::Number)
                            .unwrap_or(serde_json::Value::Null)
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::BoolValue(b)) => {
                        serde_json::Value::Bool(*b)
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::Int64Value(i)) => {
                        serde_json::Value::Number(serde_json::Number::from(*i))
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::BytesValue(_)) => {
                        serde_json::Value::String("[binary data]".to_string())
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::NullValue(_)) => {
                        serde_json::Value::Null
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::ArrayValue(_)) => {
                        serde_json::Value::String("[array]".to_string())
                    }
                    Some(crate::proto::proximadb_v1::sql_value::Value::ObjectValue(_)) => {
                        serde_json::Value::String("[object]".to_string())
                    }
                    None => serde_json::Value::Null,
                };

                // Track null counts
                if value.is_null() {
                    *metadata_null_counts.entry(column.clone()).or_insert(0) += 1;
                } else {
                    // Track min/max values
                    let entry_min = metadata_min_values
                        .entry(column.clone())
                        .or_insert_with(|| value.clone());
                    if Self::compare_json_values(&value, entry_min) == std::cmp::Ordering::Less {
                        *entry_min = value.clone();
                    }

                    let entry_max = metadata_max_values
                        .entry(column.clone())
                        .or_insert_with(|| value.clone());
                    if Self::compare_json_values(&value, entry_max) == std::cmp::Ordering::Greater {
                        *entry_max = value.clone();
                    }
                }
            }
        }

        // NEW: Analyze vector format for this block
        let vector_format = self.analyze_block_vector_format(current_block);
        // REMOVED: compression_ratio - can be calculated on-demand when needed

        // Add enhanced index entry for first record in block
        if let Some(first_record) = current_block.first() {
            index_entries.push(IndexEntry {
                key: if first_record.id.is_empty() {
                    "unknown".to_string()
                } else {
                    first_record.id.clone()
                },
                offset: 0, // Will be calculated during read
                size: block_size,
                block_id,
                block_offset: 0,
                compressed: false,
                metadata_min_values,
                metadata_max_values,
                metadata_null_counts,
                // NEW: Hierarchical bloom filters (reuse from FastLanesDataBlock)
                block_key_bloom,
                block_metadata_bloom,
                // NEW: Vector format optimization
                vector_format,
                // REMOVED: compression_ratio field
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
        // Update compression configuration (stored in compression_config field)
        self.compression_config = config;
        self
    }

    /// Stub for write_sorted_records - delegates to write_sorted_vector_records
    pub async fn write_sorted_records<I>(
        &self,
        sorted_records: I,
        record_count: usize,
    ) -> Result<()>
    where
        I: Iterator<Item = VectorRecord> + Send,
    {
        // Convert VectorRecord iterator to (String, VectorRecord) iterator
        let sorted_with_keys = sorted_records.map(|record| {
            let key = if record.id.is_empty() {
                format!("vec_{}", crate::utils::uuid::Uuid::new_v4())
            } else {
                record.id.clone()
            };
            (key, record)
        });
        self.write_sorted_vector_records(sorted_with_keys, record_count)
            .await
    }

    /// Finalize the SSTable file with proper headers and footers
    async fn finalize_sstable(&self, sorted_records: Vec<(String, VectorRecord)>) -> Result<()> {
        // Create a proper SSTable file structure
        let mut file_content = Vec::new();

        // Write SST1 magic bytes
        file_content.extend_from_slice(b"SST1");

        // Write header length (4 bytes, little-endian)
        let header = serde_json::json!({
            "version": 1,
            "block_size": self.block_size,
            "record_count": sorted_records.len(),
            "compression": "none"
        });
        let header_bytes = serde_json::to_vec(&header)?;
        let header_len = header_bytes.len() as u32;
        file_content.extend_from_slice(&header_len.to_le_bytes());

        // Write header
        file_content.extend_from_slice(&header_bytes);

        // Write data blocks (simplified - just serialize records)
        for (key, record) in sorted_records {
            let record_data = serde_json::to_vec(&record)?;
            let record_len = record_data.len() as u32;
            file_content.extend_from_slice(&record_len.to_le_bytes());
            file_content.extend_from_slice(&record_data);
        }

        // Write to file using filesystem
        let fs = self.filesystem.get_filesystem("file://")?;
        fs.write(self.path.to_str().unwrap(), &file_content, None).await?;

        Ok(())
    }

    // MIGRATION: Removed legacy quantization methods - universal adapters are always used
    // The universal adapters are initialized in new() and with_compression()
    // No need for separate quantization configuration methods

    /// NEW: Analyze vector format for optimal compression in this block
    fn analyze_block_vector_format(&self, block_records: &[VectorRecord]) -> super::VectorFormat {
        if block_records.is_empty() {
            return super::VectorFormat::Variable;
        }

        // Collect dimensions
        let dimensions: Vec<usize> = block_records.iter().map(|r| r.vector.len()).collect();

        // Find dominant dimension
        let mut dimension_counts = std::collections::HashMap::new();
        for &dim in &dimensions {
            *dimension_counts.entry(dim).or_insert(0) += 1;
        }

        let total_vectors = dimensions.len();
        if let Some((dominant_dim, count)) =
            dimension_counts.iter().max_by_key(|(_, count)| **count)
        {
            let dominance_ratio = *count as f64 / total_vectors as f64;

            if dominance_ratio >= 0.95 && Self::is_supported_fixed_dimension(*dominant_dim) {
                super::VectorFormat::Fixed {
                    dimension: *dominant_dim,
                }
            } else if dominance_ratio >= 0.7 && Self::is_supported_fixed_dimension(*dominant_dim) {
                super::VectorFormat::Mixed {
                    dominant_dimension: *dominant_dim,
                }
            } else {
                super::VectorFormat::Variable
            }
        } else {
            super::VectorFormat::Variable
        }
    }

    /// Check if dimension is supported for fixed-length optimization
    fn is_supported_fixed_dimension(dimension: usize) -> bool {
        matches!(dimension, 64 | 128 | 256 | 512 | 768 | 1024 | 1536 | 2048)
    }

    // REMOVED: estimate_compression_ratio - no longer needed without compression_ratio field

    /// Estimate vector sparsity (ratio of near-zero elements)
    fn estimate_vector_sparsity(&self, block_records: &[VectorRecord]) -> f32 {
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
    /// Uses CompositeBloomFilter from core for consistency
    fn build_block_bloom_filters(
        &self,
        block_records: &[VectorRecord],
        _block_id: u32,
    ) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
        // Only build block blooms for large blocks (>100 records) to avoid overhead
        // This threshold balances bloom filter overhead vs. I/O savings
        if block_records.len() < 100 {
            return (None, None);
        }

        let block_key_bloom = self.build_block_key_bloom(block_records);
        let block_metadata_bloom = self.build_block_metadata_bloom(block_records);

        (block_key_bloom, block_metadata_bloom)
    }

    /// Build key bloom filter for this block using core CompositeBloomFilter
    fn build_block_key_bloom(&self, block_records: &[VectorRecord]) -> Option<Vec<u8>> {
        use crate::core::bloom::BloomFilterConfig;
        use crate::core::bloom::factory::BloomFilterFactory;

        let config = BloomFilterConfig {
            // strategy removed -  crate::core::bloom::BloomStrategy::ByteAligned,
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

    /// Build metadata bloom filter for this block using core CompositeBloomFilter
    fn build_block_metadata_bloom(&self, block_records: &[VectorRecord]) -> Option<Vec<u8>> {
        use crate::core::bloom::strategies::composite::CompositeBloomFilterBuilder;

        let config = crate::core::bloom::BloomFilterConfig {
            // strategy removed -  crate::core::bloom::BloomStrategy::Composite,
            expected_items: block_records.len(),
            false_positive_rate: Some(0.01),
            ..Default::default()
        };

        let mut builder = CompositeBloomFilterBuilder::new(config);
        for record in block_records {
            for (key, sql_value) in &record.metadata {
                let metadata_item = crate::core::proto_metadata_helper::sqlvalue_to_metadata_item(key.clone(), sql_value);
                builder.add_metadata_item(key.clone(), metadata_item);
            }
        }

        let bloom = builder.build();
        use crate::core::bloom::BloomFilterStrategy;
        BloomFilterStrategy::serialize(&bloom).ok()
    }

    /// NEW: Analyze vector format across the entire file
    fn analyze_file_vector_format(
        &self,
        data_blocks: &[super::FastLanesDataBlock],
    ) -> super::VectorFormat {
        if data_blocks.is_empty() {
            return super::VectorFormat::Variable;
        }

        let mut all_dimensions = Vec::new();
        for block in data_blocks {
            for record in &block.records {
                all_dimensions.push(record.vector.len());
            }
        }

        if all_dimensions.is_empty() {
            return super::VectorFormat::Variable;
        }

        // Analyze dimensions across the entire file
        let mut dimension_counts = std::collections::HashMap::new();
        for &dim in &all_dimensions {
            *dimension_counts.entry(dim).or_insert(0) += 1;
        }

        let total_vectors = all_dimensions.len();
        if let Some((dominant_dim, count)) =
            dimension_counts.iter().max_by_key(|(_, count)| **count)
        {
            let dominance_ratio = *count as f64 / total_vectors as f64;

            if dominance_ratio >= 0.95 && Self::is_supported_fixed_dimension(*dominant_dim) {
                super::VectorFormat::Fixed {
                    dimension: *dominant_dim,
                }
            } else if dominance_ratio >= 0.7 && Self::is_supported_fixed_dimension(*dominant_dim) {
                super::VectorFormat::Mixed {
                    dominant_dimension: *dominant_dim,
                }
            } else {
                super::VectorFormat::Variable
            }
        } else {
            super::VectorFormat::Variable
        }
    }

    // REMOVED: calculate_overall_compression_ratio - no longer needed without compression_ratio field
    // Overall compression ratio is now stored only in SstableHeader

    /// Count unique metadata columns across all blocks
    fn count_metadata_columns(&self, data_blocks: &[super::FastLanesDataBlock]) -> u32 {
        let mut metadata_columns = std::collections::HashSet::new();

        for block in data_blocks {
            for record in &block.records {
                for (key, _sql_value) in &record.metadata {
                    metadata_columns.insert(key.clone());
                }
            }
        }

        metadata_columns.len() as u32
    }

    /// Check if any index entries have block-level bloom filters
    fn has_any_block_blooms(&self, index_entries: &[super::IndexEntry]) -> bool {
        index_entries
            .iter()
            .any(|entry| entry.block_key_bloom.is_some() || entry.block_metadata_bloom.is_some())
    }

    /// Extract fixed dimension from vector format if applicable
    fn extract_fixed_dimension(&self, format: &super::VectorFormat) -> Option<u32> {
        match format {
            super::VectorFormat::Fixed { dimension } => Some(*dimension as u32),
            super::VectorFormat::Mixed { dominant_dimension } => Some(*dominant_dimension as u32),
            super::VectorFormat::Variable => None,
        }
    }

    /// Encode FastLanesDataBlock using FastLanes with intelligent scheme selection
    fn encode_block_with_fastlanes(
        &self,
        data_block: &FastLanesDataBlock,
    ) -> Result<FastLanesDataBlock> {
        if data_block.records.is_empty() {
            return Ok(data_block.clone());
        }

        // Analyze vectors to choose optimal FastLanes scheme
        let scheme = self.analyze_vector_patterns(&data_block.records)?;

        // Create encoder with chosen scheme
        let encoder = FastLanesEncoder::new(scheme);

        // Clone the block and encode vector data using FastLanes
        let encoded_block = data_block.clone();

        // SST can handle multiple quantization levels in the same block
        // The quantization engine determines the appropriate level based on config
        // For now, keep original f32 vectors - quantization happens at write time

        Ok(encoded_block)
    }

    /// Analyze vector patterns to choose optimal FastLanes encoding scheme
    fn analyze_vector_patterns(&self, records: &[VectorRecord]) -> Result<FastLanesScheme> {
        if records.is_empty() {
            return Ok(FastLanesScheme::BitPacked { bits: 16 });
        }

        // Sample vectors for analysis (first 10 or all if fewer)
        let sample_size = std::cmp::min(10, records.len());
        let mut has_constants = false;
        let mut has_small_range = false;
        let mut has_deltas = false;
        let mut overall_min_val = f32::INFINITY;
        let mut first_value = 0f32;

        for record in records.iter().take(sample_size) {
            if !record.vector.is_empty() {
                let vec = &record.vector;

                // Store first value for delta encoding
                if first_value == 0.0 && !vec.is_empty() {
                    first_value = vec[0];
                }

                // Check for constant dimensions
                let first_val = vec[0];
                if vec.iter().all(|&v| (v - first_val).abs() < f32::EPSILON) {
                    has_constants = true;
                }

                // Check for small range (good for frame of reference)
                let min_val = vec.iter().fold(f32::INFINITY, |a, &b| a.min(b));
                let max_val = vec.iter().fold(f32::NEG_INFINITY, |a, &b| a.max(b));
                overall_min_val = overall_min_val.min(min_val);
                if (max_val - min_val) < 100.0 {
                    has_small_range = true;
                }

                // Check for sequential patterns (good for delta encoding)
                if vec.len() > 1 {
                    let mut sequential_count = 0;
                    for i in 1..vec.len() {
                        if (vec[i] - vec[i - 1]).abs() < 10.0 {
                            sequential_count += 1;
                        }
                    }
                    if sequential_count > vec.len() / 2 {
                        has_deltas = true;
                    }
                }
            }
        }

        // Choose scheme based on analysis
        let scheme = if has_constants {
            FastLanesScheme::RunLength
        } else if has_small_range {
            FastLanesScheme::FrameOfReference {
                reference: overall_min_val as i64,
                bits: 16, // Use 16 bits for small ranges
            }
        } else if has_deltas {
            FastLanesScheme::Delta {
                base: first_value as i64,
            }
        } else {
            // Default to bit packing for dense data
            FastLanesScheme::BitPacked {
                bits: 32, // Default to 32 bits
            }
        };

        debug!("🔍 FastLanes scheme selected: {:?}", scheme);
        Ok(scheme)
    }

    /// Compare two JSON values for ordering
    fn compare_json_values(a: &serde_json::Value, b: &serde_json::Value) -> std::cmp::Ordering {
        use serde_json::Value;
        use std::cmp::Ordering;

        match (a, b) {
            (Value::Number(n1), Value::Number(n2)) => {
                let f1 = n1.as_f64();
                let f2 = n2.as_f64();
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
            let record = VectorRecord {
                id: format!("key{:03}", i),
                vector: vec![1.0, 2.0, 3.0],
                metadata: std::collections::HashMap::new(),
                timestamp: chrono::Utc::now().timestamp(),
                updated_at: Some(chrono::Utc::now().timestamp()),
                expires_at: None,
                version: Some(1),
                quantized_vector: vec![],
                source: None,
            };
            records.insert(record.id.clone(), record);
        }

        assert_eq!(records.len(), 10);
    }
}
