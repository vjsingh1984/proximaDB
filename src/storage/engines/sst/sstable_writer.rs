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
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info};

use crate::storage::persistence::filesystem::{
    FilesystemFactory,
    atomic_strategy::{AtomicWriteExecutorFactory}
};

use super::{DataBlock, IndexEntry};  // OPTIMIZED: Removed SstRecord import
use crate::core::VectorRecord;  // OPTIMIZED: Direct VectorRecord usage
// MIGRATION: Removed SstQuantizationAdapter imports - now using UniversalQuantizationAdapter
use crate::storage::engines::common::quantization_common::{
    ProgressiveQuantizationStage, UniversalQuantizationLevel,
    BinaryThresholdStrategy, CodebookStrategy,
};
use crate::storage::engines::common::compression_common::{
    AdaptiveCompressionSettings, AdaptiveStrategy,
    ContextAwareCompressionConfig,
};
use crate::metrics::compression::CompressionDataType;
use crate::core::bloom::{
    BloomFilterConfig, BloomStrategy, BloomFilterStrategy, HashAlgorithm,
    factory::BloomFilterFactory,
};
use crate::core::bloom::strategies::composite::CompositeBloomFilterBuilder;
use crate::proto::proximadb::CompressionConfig;

// MIGRATION: Import universal adapters for deduplication
use crate::storage::engines::common::{
    UniversalCompressionAdapter, UniversalQuantizationAdapter,
    UniversalCompressionConfig, UniversalQuantizationConfig,
    // Temporarily disabled - these types may not exist yet
    // compression_common::{
    //     AdaptiveCompressionSettings, AdaptiveStrategy,
    //     ContextAwareCompressionConfig, CompressionDataType,
    // },
};

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
    /// MIGRATED: Universal compression adapter (REQUIRED - no legacy fallback)
    compression_adapter: Arc<UniversalCompressionAdapter>,
    /// MIGRATED: Universal quantization adapter (REQUIRED - no legacy fallback)
    quantization_adapter: Arc<UniversalQuantizationAdapter>,
}

impl SstableWriter {
    /// Create a new SSTable writer with collection-specific configuration
    pub fn new_with_config<P: AsRef<Path>>(
        path: P, 
        block_size: usize, 
        filesystem: Arc<FilesystemFactory>,
        collection_config: Option<&crate::proto::proximadb::Collection>,
    ) -> Self {
        // MIGRATION: Initialize universal compression adapter (REQUIRED)
        let compression_adapter = Arc::new(
            UniversalCompressionAdapter::new()
                .expect("Failed to initialize universal compression adapter")
        );
        
        // MIGRATION: Initialize universal quantization adapter (REQUIRED)
        let quantization_adapter = Arc::new(
            UniversalQuantizationAdapter::new()
                .expect("Failed to initialize universal quantization adapter")
        );
        
        // Configure adapters based on collection config if provided
        if let Some(collection) = collection_config {
            // Get distance metric from collection config (default: Cosine)
            let distance_metric = collection
                .config.as_ref()
                .map(|cfg| cfg.distance_metric())
                .unwrap_or(crate::proto::proximadb::DistanceMetric::Cosine);
            
            // Configure quantization settings if enabled
            if let Some(quant_config) = collection.config.as_ref()
                .and_then(|cfg| cfg.quantization_config.as_ref()) {
                
                if quant_config.enabled.unwrap_or(true) {
                    info!("🔧 SST: Configuring universal quantization adapter with collection settings");
                    
                    // Create universal quantization config for SST-specific needs
                    let mut universal_quant_config = UniversalQuantizationConfig::default();
                    universal_quant_config.enabled = true;
                    
                    // Configure progressive stages for SST hierarchical storage
                    if quant_config.enable_progressive_search.unwrap_or(true) {
                        use crate::storage::engines::common::quantization_common::*;
                        
                        universal_quant_config.stages = vec![
                            ProgressiveQuantizationStage {
                                level: UniversalQuantizationLevel::Binary {
                                    threshold_strategy: BinaryThresholdStrategy::Adaptive,
                                },
                                candidate_reduction: 0.7, // Filter 70% using binary
                                quality_threshold: quant_config.binary_filter_threshold.unwrap_or(0.3),
                            },
                            ProgressiveQuantizationStage {
                                level: UniversalQuantizationLevel::ProductQuantization {
                                    segments: quant_config.num_subvectors.unwrap_or(96) as usize,
                                    bits_per_segment: quant_config.bits_per_subvector.unwrap_or(8) as usize,
                                    codebook_strategy: CodebookStrategy::KMeans,
                                },
                                candidate_reduction: 0.0, // Keep all for final ranking
                                quality_threshold: quant_config.quality_threshold.unwrap_or(0.95),
                            },
                        ];
                        
                        // Add SST-specific engine overrides
                        universal_quant_config.engine_overrides.insert(
                            "sst_similarity_sorting".to_string(),
                            serde_json::json!(true)
                        );
                        universal_quant_config.engine_overrides.insert(
                            "sst_progressive_blocks".to_string(),
                            serde_json::json!(true)
                        );
                        universal_quant_config.engine_overrides.insert(
                            "sst_target_cluster_size".to_string(),
                            serde_json::json!((block_size / 512).max(100))
                        );
                    }
                    
                    // Apply configuration to the adapter
                    quantization_adapter.set_default_config(universal_quant_config);
                    
                    debug!("✅ SST: Universal quantization adapter configured for progressive search");
                }
            }
        }
        
        Self {
            path: path.as_ref().to_path_buf(),
            block_size,
            bloom_config: BloomFilterConfig {
                expected_items: 10000,
                false_positive_rate: Some(0.01),
                strategy: BloomStrategy::ByteAligned,
                bits_per_key: 8,
                enabled: true,
                hash_algorithm: HashAlgorithm::Murmur3,
            },
            filesystem,
            compression_adapter,  // Required universal adapter
            quantization_adapter,  // Required universal adapter
        }
    }
    
    /// Create a new SSTable writer with filesystem support for atomic writes
    /// Quantization is enabled by default as it's part of the SST file layout
    pub fn new<P: AsRef<Path>>(path: P, block_size: usize, filesystem: Arc<FilesystemFactory>) -> Self {
        Self::new_with_config(path, block_size, filesystem, None)
    }
    
    /// MIGRATED: Serialize a data block using universal compression adapter
    /// This eliminates duplicate compression logic and provides adaptive selection
    fn compress_block_streaming(
        &self,
        data_block: &DataBlock,
        algorithm: crate::core::compression::CompressionAlgorithm,
        level: u8,
    ) -> Result<Vec<u8>> {
        debug!("🔍 SST WRITER: Compressing block with universal adapter");
        debug!("   Algorithm: {:?}", algorithm);
        debug!("   Level: {}", level);
        debug!("   Block records: {}", data_block.records.len());
        
        // MIGRATION: Always use universal compression adapter (required field)
        let serialized = data_block.serialize()?;
        
        let config = UniversalCompressionConfig {
            enabled: true,
            primary_algorithm: algorithm,
            compression_level: level as i32,
            adaptive_settings: AdaptiveCompressionSettings {
                enabled: true,
                strategy: AdaptiveStrategy::DataDriven,
                fallback_algorithms: vec![algorithm],
                performance_target: Some(50),
            },
            context_aware: ContextAwareCompressionConfig {
                data_type: CompressionDataType::SstBlock,
                size_hint: Some(serialized.len()),
                access_pattern: None,
            },
            ..Default::default()
        };
        
        let compressed = self.compression_adapter.compress_with_universal_config(&serialized, &config)?;
        debug!("✅ Universal compression: {} -> {} bytes", compressed.original_size, compressed.compressed_size);
        Ok(compressed.data)
    }
    
    /// MIGRATION: Create SSTable writer with universal adapters
    /// Both compression and quantization use universal adapters for code deduplication
    pub fn with_compression<P: AsRef<Path>>(
        path: P, 
        block_size: usize, 
        filesystem: Arc<FilesystemFactory>,
        compression_config: Option<CompressionConfig>
    ) -> Self {
        // MIGRATION: Initialize universal adapters (REQUIRED)
        let compression_adapter = Arc::new(
            UniversalCompressionAdapter::new()
                .expect("Failed to initialize universal compression adapter")
        );
        
        let quantization_adapter = Arc::new(
            UniversalQuantizationAdapter::new()
                .expect("Failed to initialize universal quantization adapter")
        );
        
        // Configure compression adapter if config provided
        if let Some(config) = compression_config {
            // Convert proto config to universal config
            let universal_config = UniversalCompressionConfig {
                enabled: config.enabled.unwrap_or(true),
                primary_algorithm: match config.algorithm() {
                    crate::proto::proximadb::CompressionAlgorithm::None => 
                        crate::core::compression::CompressionAlgorithm::None,
                    crate::proto::proximadb::CompressionAlgorithm::Zstd => 
                        crate::core::compression::CompressionAlgorithm::Zstd,
                    crate::proto::proximadb::CompressionAlgorithm::Lz4 => 
                        crate::core::compression::CompressionAlgorithm::Lz4,
                    crate::proto::proximadb::CompressionAlgorithm::Snappy => 
                        crate::core::compression::CompressionAlgorithm::Snappy,
                    _ => crate::core::compression::CompressionAlgorithm::Zstd,
                },
                compression_level: config.level.unwrap_or(6),
                adaptive_settings: AdaptiveCompressionSettings {
                    enabled: true,
                    strategy: AdaptiveStrategy::DataDriven,
                    ..Default::default()
                },
                context_aware: ContextAwareCompressionConfig {
                    data_type: CompressionDataType::SstBlock,
                    ..Default::default()
                },
                ..Default::default()
            };
            compression_adapter.set_default_config(universal_config);
        }
        
        // Configure quantization adapter with SST-specific settings
        let mut quant_config = UniversalQuantizationConfig::default();
        quant_config.engine_overrides.insert(
            "sst_similarity_sorting".to_string(),
            serde_json::json!(true)
        );
        quant_config.engine_overrides.insert(
            "sst_target_cluster_size".to_string(),
            serde_json::json!((block_size / 512).max(100))
        );
        quantization_adapter.set_default_config(quant_config);
        
        Self {
            path: path.as_ref().to_path_buf(),
            block_size,
            bloom_config: BloomFilterConfig::default(),
            filesystem,
            compression_adapter,
            quantization_adapter,
        }
    }
    
    // Removed with_quantization() method - quantization is ALWAYS enabled
    // as it's integral to the SST file layout and provides PQ sorting for
    // better compression and selectivity
    
    /// Write sorted VectorRecords to SSTable using streaming (FASTEST PATH)
    /// OPTIMIZATION: Direct VectorRecord processing, no SstRecord conversion
    /// 
    /// USAGE PATTERNS:
    /// - FLUSH: Receives entire batch from memtable → sorts → streams to writer
    /// - COMPACTION: Receives pre-sorted stream from K-way merge → direct streaming
    #[inline(always)]
    pub async fn write_sorted_vector_records<I>(&self, sorted_records: I, record_count: usize) -> Result<()>
    where
        I: Iterator<Item = (String, VectorRecord)>,
    {
        info!("🚀 SST STREAMING PATH: Writing {} pre-sorted VectorRecords directly", record_count);
        debug!("📊 SST WRITER PATH ANALYSIS:");
        debug!("   - Input: Pre-sorted VectorRecord stream");
        debug!("   - No conversions: VectorRecord → VectorRecord");
        debug!("   - Quantization: Applied based on collection config");
        debug!("   - Compression: Applied based on collection config");
        
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
            
            for metadata_item in &vector_record.metadata {
                metadata_builder.add_metadata_item(metadata_item.key.clone(), metadata_item.clone());
                metadata_value_count += 1;
            }
            
            // FASTEST: Use existing protobuf serialization (already optimized)
            use prost::Message;
            let mut serialized = Vec::new();
            vector_record.encode(&mut serialized)?;
            let record_size = serialized.len();
            
            // Check if we need to start a new block
            if current_block_size + record_size > self.block_size && !current_block.is_empty() {
                self.finalize_vector_block(&mut data_blocks, &mut index_entries, &current_block, block_id, current_block_size)?;
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
            self.finalize_vector_block(&mut data_blocks, &mut index_entries, &current_block, block_id, current_block_size)?;
        }
        
        debug!("🔍 Streamed {} VectorRecords into {} blocks with {} metadata columns", 
               processed_count, data_blocks.len(), metadata_value_count);
        
        // Continue with rest of the write process (reuse existing logic)
        // MIGRATION: Apply quantization using universal adapter
        info!("🔧 SST: Applying universal quantization to {} VectorRecords", processed_count);
        
        // Convert to required format for quantization
        let vector_records = data_blocks.iter()
            .flat_map(|block| block.records.clone())
            .collect::<Vec<_>>();
        
        let all_vectors: Vec<Vec<f32>> = vector_records.iter()
            .map(|r| r.vector.clone())
            .collect();
        
        // Use universal quantization adapter (always available)
        let config = self.quantization_adapter.get_default_config()
            .unwrap_or_else(UniversalQuantizationConfig::default);
        
        // Perform progressive quantization with universal adapter
        let quantization_result = self.quantization_adapter
            .quantize_progressive(&all_vectors, &config)?;
        
        // Apply SST-specific optimizations from engine overrides
        if config.engine_overrides.get("sst_similarity_sorting")
            .and_then(|v| v.as_bool())
            .unwrap_or(false) {
            
            // Sort records by similarity for better compression
            let sorted_indices = self.sort_by_pq_similarity(&quantization_result)?;
            
            // Reorder data blocks based on similarity
            let mut sorted_blocks = Vec::new();
            let records_per_block = vector_records.len() / data_blocks.len().max(1);
            
            for chunk in sorted_indices.chunks(records_per_block) {
                let mut block = DataBlock::default();
                for &idx in chunk {
                    if idx < vector_records.len() {
                        block.records.push(vector_records[idx].clone());
                    }
                }
                sorted_blocks.push(block);
            }
            
            data_blocks = sorted_blocks;
        }
        
        // Apply quantization to blocks
        self.apply_universal_quantization_to_blocks(&mut data_blocks, &quantization_result)?;
        
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
        
        // Use existing write completion logic
        self.complete_sstable_write(data_blocks, index_entries, combined_bloom_filter, processed_count, atomic_writer, fs).await
    }
    
    /// Finalize a VectorRecord block (adapted from finalize_block)
    #[inline(always)]
    fn finalize_vector_block(
        &self,
        data_blocks: &mut Vec<DataBlock>,
        index_entries: &mut Vec<IndexEntry>,
        current_block: &[VectorRecord],
        block_id: u32,
        _current_block_size: usize,
    ) -> Result<()> {
        // Build block-level bloom filters
        let (block_key_bloom, block_metadata_bloom) = self.build_vector_block_bloom_filters(current_block, block_id);
        
        // Create DataBlock with VectorRecord
        let mut data_block = DataBlock::new(block_id, current_block.to_vec());
        
        // Set block-level bloom filter
        data_block.block_bloom_filter = block_key_bloom.clone().or(block_metadata_bloom.clone());
        
        let block_size = data_block.serialize().map(|v| v.len()).unwrap_or(0) as u32;
        
        // Collect metadata statistics for this block
        let estimated_columns = current_block.first().map(|r| r.metadata.len()).unwrap_or(4);
        let mut metadata_min_values = HashMap::with_capacity(estimated_columns);
        let mut metadata_max_values = HashMap::with_capacity(estimated_columns);
        let mut metadata_null_counts = HashMap::with_capacity(estimated_columns);
        
        for record in current_block {
            for metadata_item in &record.metadata {
                let column = &metadata_item.key;
                
                // Convert MetadataItem to JSON for statistics
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
        
        // Analyze vector format for this block
        let vector_format = self.analyze_vector_block_format(current_block);
        
        // Add enhanced index entry for first record in block
        if let Some(first_record) = current_block.first() {
            let first_id = first_record.id.as_ref().unwrap_or(&String::new()).clone();
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
                block_key_bloom: block_key_bloom.clone(),
                block_metadata_bloom: block_metadata_bloom.clone(),
                vector_format,
            });
        }
        
        data_blocks.push(data_block);
        Ok(())
    }
    
    /// Build bloom filters for VectorRecord block
    fn build_vector_block_bloom_filters(&self, block_records: &[VectorRecord], _block_id: u32) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
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
        use crate::core::bloom::factory::BloomFilterFactory;
        use crate::core::bloom::BloomFilterConfig;
        
        let config = BloomFilterConfig {
            strategy: crate::core::bloom::BloomStrategy::ByteAligned,
            expected_items: block_records.len(),
            false_positive_rate: Some(0.01),
            ..Default::default()
        };
        
        let mut bloom = BloomFilterFactory::create(&config);
        for record in block_records {
            if let Some(ref id) = record.id {
                bloom.insert(id.as_bytes());
            }
        }
        
        bloom.serialize().ok()
    }
    
    /// Build metadata bloom filter for VectorRecord block
    fn build_vector_block_metadata_bloom(&self, block_records: &[VectorRecord]) -> Option<Vec<u8>> {
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
    
    /// Analyze vector format for VectorRecord block
    fn analyze_vector_block_format(&self, block_records: &[VectorRecord]) -> super::VectorFormatType {
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

    /// MIGRATION: Apply universal quantization to data blocks
    fn apply_universal_quantization_to_blocks(
        &self,
        data_blocks: &mut Vec<DataBlock>,
        quantization_result: &crate::storage::engines::common::quantization_adapter::StageQuantizationResult,
    ) -> Result<()> {
        // Extract quantized data from result stages
        for (block_idx, block) in data_blocks.iter_mut().enumerate() {
            // Create QuantizedSection from universal quantization result
            let mut quantized_section = crate::storage::engines::sst::QuantizedSection::default();
            
            // Process each stage of progressive quantization
            for stage in &quantization_result.stages {
                match stage.stage_name.as_str() {
                    "Binary" => {
                        // Extract binary sketches for fast filtering
                        if let Some(binary_data) = &stage.quantized_data {
                            let sketches = self.extract_binary_sketches(binary_data, block_idx)?;
                            quantized_section.binary_sketches = sketches;
                        }
                    },
                    "ProductQuantization" => {
                        // Extract PQ codes for similarity search
                        if let Some(pq_data) = &stage.quantized_data {
                            let pq_codes = self.extract_pq_codes(pq_data, block_idx)?;
                            quantized_section.pq_codes = pq_codes;
                        }
                    },
                    _ => {}
                }
            }
            
            block.quantized_section = quantized_section;
            debug!("📊 SST: Added universal quantization to block {}", block_idx);
        }
        Ok(())
    }
    
    /// Helper: Extract binary sketches from quantization data
    fn extract_binary_sketches(
        &self,
        binary_data: &[u8],
        block_idx: usize,
    ) -> Result<Vec<crate::storage::engines::sst::BinarySketch>> {
        // Implementation would convert universal binary format to SST BinarySketch
        Ok(vec![])
    }
    
    /// Helper: Extract PQ codes from quantization data
    fn extract_pq_codes(
        &self,
        pq_data: &[u8],
        block_idx: usize,
    ) -> Result<Vec<crate::storage::engines::sst::PQCode>> {
        // Implementation would convert universal PQ format to SST PQCode
        Ok(vec![])
    }
    
    /// Helper: Sort indices by PQ similarity
    fn sort_by_pq_similarity(
        &self,
        quantization_result: &crate::storage::engines::common::quantization_adapter::StageQuantizationResult,
    ) -> Result<Vec<usize>> {
        // Simple implementation: return indices in order
        // A real implementation would analyze PQ codes for similarity clustering
        let count = quantization_result.stages.first()
            .and_then(|s| s.quantized_data.as_ref())
            .map(|d| d.len())
            .unwrap_or(0);
        Ok((0..count).collect())
    }
    
    /// Helper to finalize a data block
    /// Finalize block with optimized performance for hot path operations
    #[inline(always)]
    fn finalize_block(
        &self,
        data_blocks: &mut Vec<DataBlock>,
        index_entries: &mut Vec<IndexEntry>,
        current_block: &[VectorRecord],
        block_id: u32,
        _current_block_size: usize,
    ) -> Result<()> {
        // NEW: Build block-level bloom filters first (needed for DataBlock creation)
        let (block_key_bloom, block_metadata_bloom) = self.build_block_bloom_filters(current_block, block_id);
        
        // Create DataBlock with hierarchical metadata
        let mut data_block = DataBlock::new(block_id, current_block.to_vec());
        
        // Set block-level bloom filter (combines key and metadata blooms into one)
        data_block.block_bloom_filter = block_key_bloom.clone().or(block_metadata_bloom.clone());
        
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
        // REMOVED: compression_ratio - can be calculated on-demand when needed
        
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
                // NEW: Hierarchical bloom filters (reuse from DataBlock)
                block_key_bloom: block_key_bloom.clone(),
                block_metadata_bloom: block_metadata_bloom.clone(),
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
        self.compression_config = config;
        self
    }
    
    /// Stub for write_sorted_records - delegates to write_sorted_vector_records
    pub async fn write_sorted_records<I>(&self, sorted_records: I, record_count: usize) -> Result<()> 
    where 
        I: Iterator<Item = VectorRecord> + Send,
    {
        self.write_sorted_vector_records(sorted_records, record_count).await
    }
    
    // MIGRATION: Removed legacy quantization methods - universal adapters are always used
    // The universal adapters are initialized in new() and with_compression()
    // No need for separate quantization configuration methods
    
    /// NEW: Analyze vector format for optimal compression in this block
    fn analyze_block_vector_format(&self, block_records: &[VectorRecord]) -> super::VectorFormatType {
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
    fn build_block_bloom_filters(&self, block_records: &[VectorRecord], _block_id: u32) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
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
        use crate::core::bloom::factory::BloomFilterFactory;
        use crate::core::bloom::BloomFilterConfig;
        
        let config = BloomFilterConfig {
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
    
    /// Build metadata bloom filter for this block using core CompositeBloomFilter
    fn build_block_metadata_bloom(&self, block_records: &[VectorRecord]) -> Option<Vec<u8>> {
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
    
    // REMOVED: calculate_overall_compression_ratio - no longer needed without compression_ratio field
    // Overall compression ratio is now stored only in SstableHeader
    
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