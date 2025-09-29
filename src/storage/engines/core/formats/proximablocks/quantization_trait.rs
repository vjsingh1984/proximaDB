use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;

use crate::proto::proximadb_v1::{VectorRecord, Collection};
use crate::storage::traits::{FlushParameters, FlushResult};
use crate::compute::quantization::precompute::{
    QuantizationPrecomputeService, QuantizedBatch, QuantizedVector
};
use crate::storage::engines::core::formats::proximablocks::{
    ProximaDataBlock, BlockCompressionConfig, VectorEncodingLayout, QuantizedSection
};
use crate::storage::engines::core::ops::unified_proxima_simd::EngineProfile;

/// Trait for ProximaDataBlock-based engines to handle quantization
#[async_trait]
pub trait ProximaBlockQuantization {
    /// Get the engine profile for SIMD optimization
    fn engine_profile(&self) -> EngineProfile;

    /// Get the block size configuration
    fn block_size_kb(&self) -> usize;

    /// Build ProximaDataBlocks with quantization support
    async fn build_blocks_with_quantization(
        &self,
        batch: QuantizedBatch,
        params: &FlushParameters,
    ) -> Result<Vec<ProximaDataBlock>> {
        let block_size = self.block_size_kb() * 1024;
        let mut blocks = Vec::new();
        let mut current_block_records = Vec::new();
        let mut current_block_quantized = Vec::new();
        let mut current_size = 0;

        // Group records into blocks
        for (record, quantized_opt) in batch.iter() {
            let record_size = self.estimate_record_size(record);

            if current_size + record_size > block_size && !current_block_records.is_empty() {
                // Create block from current batch
                let block = self.create_proxima_block_with_quantization(
                    current_block_records.clone(),
                    current_block_quantized.clone(),
                    blocks.len() as u32
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
            let block = self.create_proxima_block_with_quantization(
                current_block_records,
                current_block_quantized,
                blocks.len() as u32
            )?;
            blocks.push(block);
        }

        tracing::info!("📦 {}: Created {} ProximaDataBlocks with quantization",
            self.engine_name(), blocks.len());
        Ok(blocks)
    }

    /// Create a single ProximaDataBlock with quantization
    fn create_proxima_block_with_quantization(
        &self,
        records: Vec<VectorRecord>,
        quantized: Vec<Option<QuantizedVector>>,
        block_id: u32,
    ) -> Result<ProximaDataBlock> {
        // Determine optimal encoding layout based on dimension
        let dimension = records.first()
            .map(|r| r.values.len())
            .unwrap_or(0);

        let vector_layout = self.select_optimal_layout(dimension);
        tracing::debug!("📐 {}: Using vector layout {:?} for dimension {}",
            self.engine_name(), vector_layout, dimension);

        // Create block with engine-specific profile
        let compression_config = self.get_compression_config(vector_layout.clone());

        let mut block = ProximaDataBlock::new_with_engine_profile(
            records.clone(),
            compression_config,
            self.engine_profile()
        );
        block.block_id = block_id;

        // Add quantized section if we have quantization
        if quantized.iter().any(|q| q.is_some()) {
            let quantized_section = self.build_quantized_section(
                &quantized,
                vector_layout
            )?;
            block.quantized_section = Some(quantized_section);

            // Update metadata statistics
            if let Some(ref section) = block.quantized_section {
                block.metadata.quantization_stats.has_binary = section.binary_vectors.is_some();
                block.metadata.quantization_stats.has_int8 = section.int8_vectors.is_some();
                block.metadata.quantization_stats.has_pq = section.pq_vectors.is_some();
            }

            tracing::info!("🎯 {}: Added quantization to block {} (binary: {}, int8: {}, pq: {})",
                self.engine_name(),
                block_id,
                block.metadata.quantization_stats.has_binary,
                block.metadata.quantization_stats.has_int8,
                block.metadata.quantization_stats.has_pq
            );
        }

        Ok(block)
    }

    /// Build QuantizedSection from quantized vectors
    fn build_quantized_section(
        &self,
        quantized: &[Option<QuantizedVector>],
        layout: VectorEncodingLayout,
    ) -> Result<QuantizedSection> {
        let mut section = QuantizedSection {
            binary_vectors: None,
            int8_vectors: None,
            pq_vectors: None,
            codebooks: None,
        };

        // Check what types of quantization we have
        let has_binary = quantized.iter().any(|q| q.as_ref().map_or(false, |v| v.binary.is_some()));
        let has_int8 = quantized.iter().any(|q| q.as_ref().map_or(false, |v| v.int8.is_some()));
        let has_pq = quantized.iter().any(|q| q.as_ref().map_or(false, |v| v.pq8.is_some() || v.pq16.is_some()));

        // Apply columnar encoding based on layout
        match layout {
            VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector => {
                // Transpose quantized vectors to DxR columnar format
                if has_binary {
                    section.binary_vectors = Some(self.extract_binary_vectors(quantized)?);
                }
                if has_int8 {
                    section.int8_vectors = Some(self.transpose_int8_vectors(quantized)?);
                }
                if has_pq {
                    section.pq_vectors = Some(self.extract_pq_vectors(quantized)?);
                }
            },
            VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector => {
                // Group into 32D chunks for cache locality
                if has_binary {
                    section.binary_vectors = Some(self.extract_binary_vectors(quantized)?);
                }
                if has_int8 {
                    section.int8_vectors = Some(self.group_int8_vectors(quantized, 32)?);
                }
                if has_pq {
                    section.pq_vectors = Some(self.extract_pq_vectors(quantized)?);
                }
            },
            _ => {
                // Default: store as-is
                if has_binary {
                    section.binary_vectors = Some(self.extract_binary_vectors(quantized)?);
                }
                if has_int8 {
                    section.int8_vectors = Some(self.extract_int8_vectors(quantized)?);
                }
                if has_pq {
                    section.pq_vectors = Some(self.extract_pq_vectors(quantized)?);
                }
            }
        }

        // Extract codebooks if present
        if let Some(first_quantized) = quantized.iter().find_map(|q| q.as_ref()) {
            if let Some(ref codebooks) = first_quantized.codebooks {
                if let Some(ref pq_cb) = codebooks.pq_codebooks {
                    section.codebooks = Some(pq_cb.clone());
                }
            }
        }

        Ok(section)
    }

    /// Get compression configuration
    fn get_compression_config(&self, layout: VectorEncodingLayout) -> BlockCompressionConfig {
        BlockCompressionConfig {
            algorithm: crate::core::compression::CompressionAlgorithm::Lz4,
            compression_level: 3,
            enable_vector_compression: true,
            enable_metadata_compression: true,
            compression_threshold_bytes: 1024,
            dictionary_compression: false,
            vector_layout: layout,
            metadata_algorithm: None,
        }
    }

    /// Select optimal vector encoding layout based on dimension
    fn select_optimal_layout(&self, dimension: usize) -> VectorEncodingLayout {
        if dimension <= 128 {
            // Small dimensions: transpose for better SIMD
            VectorEncodingLayout::TransposeFieldEncodedAndCompressedVector
        } else if dimension <= 1024 {
            // Medium: use grouped for cache locality
            VectorEncodingLayout::GroupedFieldEncodedAndCompressedVector
        } else {
            // High dimensions: keep as-is
            VectorEncodingLayout::FullVector
        }
    }

    /// Estimate size of a vector record
    fn estimate_record_size(&self, record: &VectorRecord) -> usize {
        // ID + vector + metadata estimate
        record.id.len() + (record.values.len() * 4) + 100
    }

    /// Get engine name for logging
    fn engine_name(&self) -> &str;

    // Extraction helpers for different quantization types
    fn extract_binary_vectors(&self, quantized: &[Option<QuantizedVector>]) -> Result<Vec<Vec<u8>>> {
        Ok(quantized.iter()
            .filter_map(|q| q.as_ref().and_then(|v| v.binary.clone()))
            .collect())
    }

    fn extract_int8_vectors(&self, quantized: &[Option<QuantizedVector>]) -> Result<Vec<Vec<i8>>> {
        Ok(quantized.iter()
            .filter_map(|q| q.as_ref().and_then(|v| v.int8.clone()))
            .collect())
    }

    fn extract_pq_vectors(&self, quantized: &[Option<QuantizedVector>]) -> Result<Vec<Vec<u8>>> {
        Ok(quantized.iter()
            .filter_map(|q| q.as_ref().and_then(|v| v.pq8.clone().or(v.pq16.clone())))
            .collect())
    }

    // Transpose helpers for columnar encoding
    fn transpose_int8_vectors(&self, quantized: &[Option<QuantizedVector>]) -> Result<Vec<Vec<i8>>> {
        let vectors = self.extract_int8_vectors(quantized)?;
        if vectors.is_empty() {
            return Ok(vec![]);
        }

        let dimension = vectors[0].len();
        let mut transposed = vec![vec![0i8; vectors.len()]; dimension];

        for (vec_idx, vector) in vectors.iter().enumerate() {
            for (dim_idx, &value) in vector.iter().enumerate() {
                transposed[dim_idx][vec_idx] = value;
            }
        }

        Ok(transposed)
    }

    // Grouping helper for cache-friendly access
    fn group_int8_vectors(&self, quantized: &[Option<QuantizedVector>], group_size: usize) -> Result<Vec<Vec<i8>>> {
        let vectors = self.extract_int8_vectors(quantized)?;
        if vectors.is_empty() {
            return Ok(vec![]);
        }

        let dimension = vectors[0].len();
        let num_groups = (dimension + group_size - 1) / group_size;
        let mut grouped = Vec::with_capacity(num_groups * vectors.len());

        for group_idx in 0..num_groups {
            let start = group_idx * group_size;
            let end = ((group_idx + 1) * group_size).min(dimension);

            for vector in &vectors {
                let mut group = Vec::with_capacity(end - start);
                for dim_idx in start..end {
                    group.push(vector[dim_idx]);
                }
                grouped.push(group);
            }
        }

        Ok(grouped)
    }

    /// Perform flush with quantization
    async fn flush_with_quantization(
        &self,
        params: &FlushParameters,
    ) -> Result<FlushResult> {
        let start_time = std::time::Instant::now();

        tracing::info!(
            "🔄 {} FLUSH+QUANT: Starting flush with quantization for {} vectors",
            self.engine_name(),
            params.vector_records.len()
        );

        // Check if quantization is enabled
        let quantization_enabled = params.collection_config.as_ref()
            .map(|c| c.config.quantization.enabled)
            .unwrap_or(false);

        // Perform quantization if enabled
        let quantized_batch = if quantization_enabled {
            tracing::info!("⚡ {}: Quantizing vectors during flush", self.engine_name());
            let service = QuantizationPrecomputeService::global();
            let collection_config = params.collection_config.as_ref()
                .ok_or_else(|| anyhow::anyhow!("Collection config required for quantization"))?;

            service.quantize_for_flush(
                params.vector_records.clone(),
                collection_config
            ).await?
        } else {
            tracing::info!("⏭️ {}: Quantization disabled, using unquantized batch", self.engine_name());
            QuantizedBatch::unquantized(params.vector_records.clone())
        };

        // Build ProximaDataBlocks with quantization
        let blocks = self.build_blocks_with_quantization(quantized_batch, params).await?;

        // Write blocks to storage (delegate to engine-specific implementation)
        let flush_result = self.write_blocks_to_storage(blocks, params).await?;

        let duration = start_time.elapsed();
        tracing::info!(
            "✅ {} FLUSH+QUANT: Completed in {:.2}ms - {} vectors",
            self.engine_name(),
            duration.as_millis(),
            flush_result.entries_flushed.unwrap_or(0)
        );

        Ok(flush_result)
    }

    /// Write blocks to storage (engine-specific implementation)
    async fn write_blocks_to_storage(
        &self,
        blocks: Vec<ProximaDataBlock>,
        params: &FlushParameters,
    ) -> Result<FlushResult>;
}