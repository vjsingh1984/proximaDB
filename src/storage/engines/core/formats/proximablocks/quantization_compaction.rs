use std::sync::Arc;
use anyhow::{Result, Context};
use tracing::{info, debug, warn};

use crate::proto::proximadb_v1::{VectorRecord, Collection};
use crate::storage::traits::{CompactionParameters, CompactionResult};
use crate::compute::quantization::precompute::{
    QuantizationPrecomputeService, QuantizedBatch
};
use crate::storage::engines::core::formats::proximablocks::ProximaDataBlock;

/// Quantization-aware compaction for ProximaDataBlock engines
///
/// CRITICAL: During compaction, quantization MUST be recalculated because:
/// 1. Binary thresholds change with merged data distribution
/// 2. INT8 min/max scaling needs recalculation
/// 3. PQ codebooks must be retrained on combined dataset
/// 4. Each new row group needs its own quantization parameters
/// 5. L0→L1 and L1→L2 merges create different data distributions
pub struct QuantizationAwareCompaction {
    service: Arc<QuantizationPrecomputeService>,
}

impl QuantizationAwareCompaction {
    /// Create new quantization-aware compaction handler
    pub fn new() -> Self {
        Self {
            service: QuantizationPrecomputeService::global(),
        }
    }

    /// Compact blocks with quantization recalculation
    ///
    /// This is the critical operation that ensures quantization correctness
    /// after merging data from multiple blocks/files.
    pub async fn compact_with_quantization(
        &self,
        input_blocks: Vec<ProximaDataBlock>,
        params: &CompactionParameters,
        collection: &Collection,
    ) -> Result<Vec<ProximaDataBlock>> {
        info!(
            "🔄 COMPACTION+QUANT: Starting quantization-aware compaction of {} blocks",
            input_blocks.len()
        );

        // 1. Extract all records from input blocks
        let all_records = self.extract_all_records(input_blocks)?;
        info!("📊 Extracted {} total records for compaction", all_records.len());

        // 2. Merge and deduplicate records
        let merged_records = self.merge_and_deduplicate(all_records)?;
        info!("🔀 After deduplication: {} records", merged_records.len());

        // 3. CRITICAL: Recalculate quantization for merged data
        // This is essential because data distribution has changed
        let quantized_batch = if collection.config.quantization.enabled {
            warn!("⚠️ RECALCULATING quantization for merged data (required for correctness)");
            self.service.quantize_for_compaction(
                merged_records.clone(),
                collection
            ).await?
        } else {
            QuantizedBatch::unquantized(merged_records.clone())
        };

        // 4. Build new compacted blocks with fresh quantization
        let compacted_blocks = self.build_compacted_blocks(
            quantized_batch,
            params,
            collection
        ).await?;

        info!(
            "✅ COMPACTION+QUANT: Created {} compacted blocks with recalculated quantization",
            compacted_blocks.len()
        );

        Ok(compacted_blocks)
    }

    /// Extract all records from blocks
    fn extract_all_records(&self, blocks: Vec<ProximaDataBlock>) -> Result<Vec<VectorRecord>> {
        let mut all_records = Vec::new();

        for block in blocks {
            debug!("Extracting {} records from block {}", block.records.len(), block.block_id);
            all_records.extend(block.records);
        }

        Ok(all_records)
    }

    /// Merge and deduplicate records
    fn merge_and_deduplicate(&self, mut records: Vec<VectorRecord>) -> Result<Vec<VectorRecord>> {
        use std::collections::HashMap;

        // Sort by ID and version to handle updates
        records.sort_by(|a, b| {
            a.id.cmp(&b.id).then_with(|| a.version.cmp(&b.version))
        });

        // Keep only the latest version of each ID
        let mut deduped: HashMap<String, VectorRecord> = HashMap::new();

        for record in records {
            match deduped.get(&record.id) {
                Some(existing) if existing.version >= record.version => {
                    // Keep existing (newer or same version)
                    debug!("Skipping older version of {}", record.id);
                },
                _ => {
                    // Insert new or update with newer version
                    debug!("Keeping record {} version {}", record.id, record.version);
                    deduped.insert(record.id.clone(), record);
                }
            }
        }

        // Convert back to sorted vector
        let mut result: Vec<VectorRecord> = deduped.into_values().collect();
        result.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(result)
    }

    /// Build compacted blocks with fresh quantization
    async fn build_compacted_blocks(
        &self,
        batch: QuantizedBatch,
        params: &CompactionParameters,
        collection: &Collection,
    ) -> Result<Vec<ProximaDataBlock>> {
        use crate::storage::engines::core::formats::proximablocks::{
            BlockCompressionConfig, VectorEncodingLayout
        };
        use crate::storage::engines::core::ops::unified_proxima_simd::EngineProfile;

        let block_size = params.target_block_size_bytes.unwrap_or(1024 * 1024); // 1MB default
        let mut blocks = Vec::new();
        let mut current_block_records = Vec::new();
        let mut current_block_quantized = Vec::new();
        let mut current_size = 0usize;

        // Determine optimal encoding layout
        let dimension = batch.records.first()
            .map(|r| r.values.len())
            .unwrap_or(128);
        let layout = self.select_compaction_layout(dimension);

        for (record, quantized_opt) in batch.iter() {
            let record_size = self.estimate_record_size(record);

            if current_size + record_size > block_size && !current_block_records.is_empty() {
                // Create block from current batch
                let block = self.create_compacted_block(
                    current_block_records.clone(),
                    current_block_quantized.clone(),
                    blocks.len() as u32,
                    layout.clone(),
                    params.compaction_level
                )?;
                blocks.push(block);

                // Reset for next block
                current_block_records.clear();
                current_block_quantized.clear();
                current_size = 0;
            }

            current_block_records.push(record.clone());
            current_block_quantized.push(quantized_opt.clone());
            current_size += record_size;
        }

        // Create final block if there are remaining records
        if !current_block_records.is_empty() {
            let block = self.create_compacted_block(
                current_block_records,
                current_block_quantized,
                blocks.len() as u32,
                layout,
                params.compaction_level
            )?;
            blocks.push(block);
        }

        Ok(blocks)
    }

    /// Create a compacted block with recalculated quantization
    fn create_compacted_block(
        &self,
        records: Vec<VectorRecord>,
        quantized: Vec<Option<crate::compute::quantization::precompute::QuantizedVector>>,
        block_id: u32,
        layout: VectorEncodingLayout,
        compaction_level: u8,
    ) -> Result<ProximaDataBlock> {
        use crate::storage::engines::core::formats::proximablocks::{
            BlockCompressionConfig, QuantizedSection
        };
        use crate::storage::engines::core::ops::unified_proxima_simd::EngineProfile;

        // Use higher compression for compacted blocks
        let compression_config = BlockCompressionConfig {
            algorithm: crate::core::compression::CompressionAlgorithm::Zstd,
            compression_level: 6, // Higher compression for compacted data
            enable_vector_compression: true,
            enable_metadata_compression: true,
            compression_threshold_bytes: 512,
            dictionary_compression: compaction_level >= 2, // Use dictionary for L2+
            vector_layout: layout,
            metadata_algorithm: None,
        };

        let mut block = ProximaDataBlock::new_with_engine_profile(
            records.clone(),
            compression_config,
            EngineProfile::SST // Can be parameterized
        );
        block.block_id = block_id;
        block.metadata.compaction_level = compaction_level;

        // Add recalculated quantization if present
        if quantized.iter().any(|q| q.is_some()) {
            let quantized_section = self.build_quantized_section(&quantized, layout)?;
            block.quantized_section = Some(quantized_section);

            // Update statistics
            if let Some(ref section) = block.quantized_section {
                block.metadata.quantization_stats.has_binary = section.binary_vectors.is_some();
                block.metadata.quantization_stats.has_int8 = section.int8_vectors.is_some();
                block.metadata.quantization_stats.has_pq = section.pq_vectors.is_some();

                info!(
                    "📊 Compacted block {} has recalculated quantization: binary={}, int8={}, pq={}",
                    block_id,
                    block.metadata.quantization_stats.has_binary,
                    block.metadata.quantization_stats.has_int8,
                    block.metadata.quantization_stats.has_pq
                );
            }
        }

        Ok(block)
    }

    /// Select optimal layout for compacted data
    fn select_compaction_layout(&self, dimension: usize) -> VectorEncodingLayout {
        use crate::storage::engines::core::formats::proximablocks::VectorEncodingLayout;

        // For compacted data, balance compression vs decode speed
        // Benchmark data (12-pattern): GroupedField 19-22% compression, FullVector 18-20%
        // Compacted blocks are read less frequently, so slightly prefer compression
        if dimension <= 256 {
            // Small dimensions: Transpose for better dimensional correlation exploitation
            VectorEncodingLayout::TransposeFieldEncodedBlockCompressedVector
        } else if dimension <= 1536 {
            // Medium dimensions: GroupedField for best compression (19-22%)
            // Trade-off: ~1-2% better compression vs slower decode than FullVector
            VectorEncodingLayout::GroupedFieldEncodedBlockCompressedVector
        } else {
            // Large dimensions (>1536): FullVector for better decode performance
            // High-dimensional vectors benefit less from grouping strategies
            VectorEncodingLayout::FullVector
        }
    }

    /// Build quantized section from recalculated quantization
    fn build_quantized_section(
        &self,
        quantized: &[Option<crate::compute::quantization::precompute::QuantizedVector>],
        layout: VectorEncodingLayout,
    ) -> Result<crate::storage::engines::core::formats::proximablocks::QuantizedSection> {
        use crate::storage::engines::core::formats::proximablocks::QuantizedSection;

        let mut section = QuantizedSection {
            binary_vectors: None,
            int8_vectors: None,
            pq_vectors: None,
            codebooks: None,
        };

        // Extract quantized vectors
        let has_binary = quantized.iter().any(|q| q.as_ref().map_or(false, |v| v.binary.is_some()));
        let has_int8 = quantized.iter().any(|q| q.as_ref().map_or(false, |v| v.int8.is_some()));
        let has_pq = quantized.iter().any(|q| q.as_ref().map_or(false, |v| v.pq8.is_some() || v.pq16.is_some()));

        if has_binary {
            section.binary_vectors = Some(
                quantized.iter()
                    .filter_map(|q| q.as_ref().and_then(|v| v.binary.clone()))
                    .collect()
            );
        }

        if has_int8 {
            section.int8_vectors = Some(
                quantized.iter()
                    .filter_map(|q| q.as_ref().and_then(|v| v.int8.clone()))
                    .collect()
            );
        }

        if has_pq {
            section.pq_vectors = Some(
                quantized.iter()
                    .filter_map(|q| q.as_ref().and_then(|v| v.pq8.clone().or(v.pq16.clone())))
                    .collect()
            );

            // Extract codebooks
            if let Some(first_quantized) = quantized.iter().find_map(|q| q.as_ref()) {
                if let Some(ref codebooks) = first_quantized.codebooks {
                    if let Some(ref pq_cb) = codebooks.pq_codebooks {
                        section.codebooks = Some(pq_cb.clone());
                    }
                }
            }
        }

        Ok(section)
    }

    fn estimate_record_size(&self, record: &VectorRecord) -> usize {
        record.id.len() + (record.values.len() * 4) + 100
    }
}

impl Default for QuantizationAwareCompaction {
    fn default() -> Self {
        Self::new()
    }
}

/// Results from quantization-aware compaction
pub struct QuantizedCompactionResult {
    pub compacted_blocks: Vec<ProximaDataBlock>,
    pub total_records: usize,
    pub records_deduplicated: usize,
    pub quantization_recalculated: bool,
    pub new_codebooks_generated: bool,
}