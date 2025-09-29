//! SST Codebook Integration
//!
//! Integrates quantization codebook metadata storage into SST's ProximaBlock format.
//! Codebooks are stored in the footer for efficient file-level access.

use anyhow::{Context, Result};
use bytes::{Bytes, BytesMut};
use std::sync::Arc;
use tracing::{debug, info};

use crate::storage::engines::core::formats::codebook_metadata::{
    CodebookSerializer, ProximaBlockFooter, QuantizationCodebookMetadata,
};
use crate::compute::quantization::{
    unified::UnifiedQuantizationEngine,
    storage_engine::StorageQuantizationEngine,
};

/// SST-specific codebook manager
pub struct SstCodebookManager {
    serializer: CodebookSerializer,
    collection_id: String,
}

impl SstCodebookManager {
    /// Create new SST codebook manager
    pub fn new(collection_id: String) -> Self {
        Self {
            serializer: CodebookSerializer::new(),
            collection_id,
        }
    }

    /// Write SST file with codebook metadata in footer
    pub async fn write_sst_with_codebook(
        &self,
        data_blocks: Vec<Bytes>,
        index_data: Bytes,
        codebook_metadata: Option<QuantizationCodebookMetadata>,
    ) -> Result<Bytes> {
        let mut buffer = BytesMut::new();

        // Write data blocks
        let data_offset = 0;
        let mut current_offset = data_offset;
        for block in &data_blocks {
            buffer.extend_from_slice(block);
            current_offset += block.len() as u64;
        }

        // Write index
        let index_offset = current_offset;
        buffer.extend_from_slice(&index_data);
        current_offset += index_data.len() as u64;

        // Write codebook metadata if present
        let (codebook_offset, codebook_size) = if let Some(metadata) = codebook_metadata {
            let codebook_offset = current_offset;
            let codebook_bytes = self.serializer.serialize_for_footer(&metadata)?;
            buffer.extend_from_slice(&codebook_bytes);
            current_offset += codebook_bytes.len() as u64;
            (Some(codebook_offset), Some(codebook_bytes.len() as u64))
        } else {
            (None, None)
        };

        // Create and write footer
        let footer = ProximaBlockFooter {
            magic: ProximaBlockFooter::MAGIC,
            metadata_offset: 0,  // SST stores metadata separately
            metadata_size: 0,
            codebook_offset,
            codebook_size,
            index_offset,
            index_size: index_data.len() as u64,
            checksum: self.calculate_checksum(&buffer),
        };

        footer.write_to_buffer(&mut buffer);

        info!(
            "SST: Wrote file with {} data blocks, index at {}, codebook at {:?}",
            data_blocks.len(), index_offset, codebook_offset
        );

        Ok(buffer.freeze())
    }

    /// Read SST file and extract codebook metadata
    pub async fn read_codebook_from_sst(&self, file_data: &[u8]) -> Result<Option<QuantizationCodebookMetadata>> {
        // Check minimum size for footer
        if file_data.len() < ProximaBlockFooter::FOOTER_SIZE {
            return Ok(None);
        }

        // Read footer from end of file
        let footer_start = file_data.len() - ProximaBlockFooter::FOOTER_SIZE;
        let footer = ProximaBlockFooter::read_from_buffer(&file_data[footer_start..])?;

        // Validate magic
        if footer.magic != ProximaBlockFooter::MAGIC {
            return Err(anyhow::anyhow!("Invalid SST file: wrong magic number"));
        }

        // Extract codebook if present
        if let (Some(offset), Some(size)) = (footer.codebook_offset, footer.codebook_size) {
            let codebook_start = offset as usize;
            let codebook_end = codebook_start + size as usize;

            if codebook_end > file_data.len() - ProximaBlockFooter::FOOTER_SIZE {
                return Err(anyhow::anyhow!("Invalid codebook offset/size in footer"));
            }

            let codebook_bytes = &file_data[codebook_start..codebook_end];
            let metadata = self.serializer.deserialize_from_footer(codebook_bytes)?;

            debug!(
                "SST: Read codebook metadata for collection {} with {} PQ codebooks",
                metadata.collection_id,
                metadata.pq_codebooks.len()
            );

            Ok(Some(metadata))
        } else {
            Ok(None)
        }
    }

    /// Extract codebook from quantization engine for current collection
    pub async fn extract_codebook_from_engine(
        &self,
        engine: &UnifiedQuantizationEngine,
    ) -> Result<QuantizationCodebookMetadata> {
        self.serializer.extract_from_engine(engine, &self.collection_id).await
    }

    /// Create codebook metadata from quantization config
    pub fn create_from_config(
        &self,
        config: &crate::proto::proximadb_v1::QuantizationConfig,
        dimension: usize,
    ) -> QuantizationCodebookMetadata {
        self.serializer.create_from_config(&self.collection_id, config, dimension)
    }

    /// Calculate checksum for buffer
    fn calculate_checksum(&self, buffer: &[u8]) -> u32 {
        // Simple CRC32 checksum
        let mut hasher = crc32fast::Hasher::new();
        hasher.update(buffer);
        hasher.finalize()
    }

    /// Optimize codebook placement in file
    /// Places codebooks after index for efficient loading
    pub fn optimize_layout(&self, file_size: usize) -> LayoutRecommendation {
        LayoutRecommendation {
            codebook_placement: if file_size > 100_000_000 {
                // For large files, place at end for streaming
                CodebookPlacement::Footer
            } else {
                // For small files, can load entire file
                CodebookPlacement::Footer
            },
            compression_recommended: file_size > 10_000_000,
            cache_priority: CachePriority::High,
        }
    }
}

/// Layout recommendation for codebook storage
#[derive(Debug)]
pub struct LayoutRecommendation {
    pub codebook_placement: CodebookPlacement,
    pub compression_recommended: bool,
    pub cache_priority: CachePriority,
}

#[derive(Debug)]
pub enum CodebookPlacement {
    Footer,
    // Future: Could support Header or Separate file
}

#[derive(Debug)]
pub enum CachePriority {
    High,
    Medium,
    Low,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::engines::core::formats::codebook_metadata::{
        BinaryCodebook, Int8Codebook, PqCodebook, PqTrainingConfig,
    };
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_sst_codebook_write_read() {
        let manager = SstCodebookManager::new("test_collection".to_string());

        // Create test codebook metadata
        let mut pq_codebooks = HashMap::new();
        pq_codebooks.insert(
            "pq8_16".to_string(),
            PqCodebook {
                num_subvectors: 16,
                bits_per_code: 8,
                centroids: vec![vec![vec![0.1; 4]; 256]; 16],
                dimension: 64,
                subvector_dim: 4,
                num_centroids: 256,
                training_config: PqTrainingConfig {
                    num_iterations: 100,
                    seed: Some(42),
                    distance_metric: "euclidean".to_string(),
                },
            },
        );

        let metadata = QuantizationCodebookMetadata {
            collection_id: "test_collection".to_string(),
            binary_codebook: Some(BinaryCodebook {
                threshold: 0.5,
                mean: None,
                dimension: 64,
            }),
            int8_codebook: Some(Int8Codebook {
                scale: 0.01,
                zero_point: 0,
                min_value: -1.0,
                max_value: 1.0,
                dimension: 64,
            }),
            pq_codebooks,
            created_at: 1234567890,
            training_samples: 1000,
            schema_version: 1,
        };

        // Create test data
        let data_blocks = vec![
            Bytes::from(vec![1u8; 1000]),
            Bytes::from(vec![2u8; 1000]),
        ];
        let index_data = Bytes::from(vec![3u8; 500]);

        // Write SST with codebook
        let sst_data = manager
            .write_sst_with_codebook(data_blocks, index_data, Some(metadata.clone()))
            .await
            .unwrap();

        // Read back codebook
        let read_metadata = manager
            .read_codebook_from_sst(&sst_data)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(read_metadata.collection_id, metadata.collection_id);
        assert_eq!(read_metadata.pq_codebooks.len(), 1);
        assert!(read_metadata.binary_codebook.is_some());
        assert!(read_metadata.int8_codebook.is_some());
    }

    #[test]
    fn test_layout_optimization() {
        let manager = SstCodebookManager::new("test_collection".to_string());

        // Test small file
        let small_rec = manager.optimize_layout(1_000_000);
        assert!(!small_rec.compression_recommended);

        // Test large file
        let large_rec = manager.optimize_layout(200_000_000);
        assert!(large_rec.compression_recommended);
        assert!(matches!(large_rec.codebook_placement, CodebookPlacement::Footer));
    }
}