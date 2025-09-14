//! RAPTOR Metadata Serializer for Zero-Copy I/O System Integration
//!
//! This module provides efficient metadata serialization for RAPTOR files,
//! enabling zero-copy caching of file metadata to avoid repeated file reads.
//!
//! The cached metadata includes:
//! - File footer with centroids and bloom filters
//! - Row group metadata with P² and K² matrices
//! - P×K matrix coverage information for spillover detection
//! - Compression metadata for quantization

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::core::error::{ProximaDBError, StorageError};
use crate::storage::engines::core::io::zero_copy::traits::{
    DataRange, EngineMetadata, MetadataSerializer, QueryContext,
};
use crate::storage::persistence::filesystem::FilesystemFactory;

use super::common::{CentroidStats, VectorCentroidCompressionMetadata};

/// Cached RAPTOR metadata structure
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RaptorCachedMetadata {
    /// File size in bytes
    pub file_size: u64,

    /// Number of vectors in the file
    pub vector_count: usize,

    /// Vector dimension
    pub dimension: usize,

    /// Centroid statistics for boundary detection
    pub centroid_stats: Vec<CentroidStats>,

    /// Row group offsets for selective reading
    pub rowgroup_offsets: Vec<u64>,

    /// Bloom filter data for ID lookups
    pub bloom_filter_data: Vec<u8>,

    /// Compression metadata for quantization
    pub compression_metadata: VectorCentroidCompressionMetadata,

    /// File creation timestamp
    pub creation_timestamp: u64,

    /// P×K matrix coverage percentage (for spillover detection)
    pub pxk_coverage: f32,

    /// Whether file has HNSW index
    pub has_hnsw: bool,

    /// HNSW graph offset if present
    pub hnsw_offset: Option<u64>,
}

impl EngineMetadata for RaptorCachedMetadata {
    fn file_size(&self) -> u64 {
        self.file_size
    }

    fn estimated_selectivity(&self, query_context: &QueryContext) -> f32 {
        // Estimate based on query type and available indexes
        if !query_context.id_lookups.is_empty() {
            // ID lookup - check bloom filter would give exact answer
            // For now, assume moderate selectivity
            0.1
        } else if query_context.query_vector.is_some() {
            // Vector search - depends on k and total vectors
            let k = query_context.top_k.unwrap_or(10);
            (k as f32 / self.vector_count as f32).min(1.0)
        } else if !query_context.metadata_filters.is_empty() {
            // Metadata filtering - conservative estimate
            0.3
        } else {
            // Full scan
            1.0
        }
    }

    fn memory_footprint(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.bloom_filter_data.len()
            + self.centroid_stats.len() * std::mem::size_of::<CentroidStats>()
            + self.rowgroup_offsets.len() * std::mem::size_of::<u64>()
    }

    fn creation_timestamp(&self) -> Option<u64> {
        Some(self.creation_timestamp)
    }

    fn compression_ratio(&self) -> Option<f32> {
        // P×K matrix compression ratio based on coverage
        Some(1.0 / self.pxk_coverage)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn clone_box(&self) -> Box<dyn EngineMetadata> {
        Box::new(self.clone())
    }
}

/// RAPTOR metadata serializer implementation
pub struct RaptorMetadataSerializer {
    filesystem: Arc<FilesystemFactory>,
}

impl RaptorMetadataSerializer {
    pub fn new(filesystem: Arc<FilesystemFactory>) -> Self {
        Self { filesystem }
    }

    /// Extract metadata from RAPTOR file footer
    async fn extract_metadata(
        &self,
        file_path: &str,
        _collection_id: &str,
    ) -> Result<RaptorCachedMetadata> {
        let fs = self.filesystem.get_filesystem(file_path)?;

        // Open file to get its size
        let file_handle = fs.open_file(file_path, false).await?;
        let file_size = file_handle.file_size().await?;

        // Read footer (last 8 bytes contain footer size)
        let footer_size_data = fs.read_range(file_path, file_size - 8, 8).await?;
        let footer_size = u64::from_le_bytes(footer_size_data.try_into().map_err(|_| {
            ProximaDBError::Storage(StorageError::Corruption("Invalid footer size".to_string()))
        })?);

        // Read the actual footer
        let footer_offset = file_size - 8 - footer_size;
        let footer_data = fs.read_range(file_path, footer_offset, footer_size).await?;

        // Deserialize footer using bincode
        let footer: super::common::RaptorFooter =
            bincode::deserialize(&footer_data).map_err(|e| {
                ProximaDBError::Storage(StorageError::Serialization(format!(
                    "Failed to deserialize footer: {}",
                    e
                )))
            })?;

        // Extract bloom filter data if present
        let bloom_filter_data =
            if let Some(bloom_meta) = &footer.file_metadata.bloom_filter_metadata {
                fs.read_range(file_path, bloom_meta.offset, bloom_meta.size)
                    .await?
            } else {
                Vec::new()
            };

        // For now, create a basic compression metadata structure
        // In real implementation, this would come from the footer
        let compression_metadata = VectorCentroidCompressionMetadata {
            centroid_stats: vec![],
            global_min_distance: 0.0,
            global_max_distance: 1.0,
            global_mean_distance: 0.5,
            centroid_encodings: vec![],
        };

        // Calculate P×K coverage based on stored vs total distances
        let pxk_coverage = 0.1; // Default 10% coverage

        Ok(RaptorCachedMetadata {
            file_size,
            vector_count: footer.file_metadata.total_vectors,
            dimension: footer.file_metadata.dimension,
            centroid_stats: vec![], // Would extract from centroids
            rowgroup_offsets: footer.file_metadata.rowgroup_offsets.clone(),
            bloom_filter_data,
            compression_metadata,
            creation_timestamp: footer.file_metadata.created_at as u64,
            pxk_coverage,
            has_hnsw: false,   // HNSW obsolete - using Matrix Trinity (P² + K² + P×K)
            hnsw_offset: None, // No HNSW in RAPTOR
        })
    }
}

impl MetadataSerializer for RaptorMetadataSerializer {
    fn engine_id(&self) -> &'static str {
        "RAPTOR"
    }

    fn serialize_metadata(
        &self,
        file_path: &str,
        collection_id: &str,
    ) -> Result<Vec<u8>, ProximaDBError> {
        // Extract metadata (blocking for now as trait doesn't support async)
        let runtime = tokio::runtime::Handle::current();
        let metadata = runtime
            .block_on(self.extract_metadata(file_path, collection_id))
            .map_err(|e| {
                ProximaDBError::Storage(StorageError::SstStorage(format!(
                    "Failed to extract metadata: {}",
                    e
                )))
            })?;

        // Serialize using bincode for efficiency
        bincode::serialize(&metadata).map_err(|e| {
            ProximaDBError::Storage(StorageError::Serialization(format!(
                "Failed to serialize metadata: {}",
                e
            )))
        })
    }

    fn deserialize_metadata(&self, data: &[u8]) -> Result<Box<dyn EngineMetadata>, ProximaDBError> {
        let metadata: RaptorCachedMetadata = bincode::deserialize(data).map_err(|e| {
            ProximaDBError::Storage(StorageError::Serialization(format!(
                "Failed to deserialize metadata: {}",
                e
            )))
        })?;
        Ok(Box::new(metadata))
    }

    fn can_skip_file(&self, metadata: &dyn EngineMetadata, query_context: &QueryContext) -> bool {
        let raptor_meta = metadata
            .as_any()
            .downcast_ref::<RaptorCachedMetadata>()
            .expect("Invalid metadata type for RAPTOR");

        // Check bloom filter for ID lookups
        if !query_context.id_lookups.is_empty() && !raptor_meta.bloom_filter_data.is_empty() {
            // Check if any ID exists in bloom filter
            for id in &query_context.id_lookups {
                if self.check_bloom_simple(&raptor_meta.bloom_filter_data, id.as_bytes()) {
                    return false; // Can't skip - ID might exist
                }
            }
            return true; // Can skip - no IDs match bloom filter
        }

        // For vector search, never skip files with relevant vectors
        if query_context.query_vector.is_some() && raptor_meta.vector_count > 0 {
            return false;
        }

        // Default: don't skip
        false
    }

    fn get_required_ranges(
        &self,
        metadata: &dyn EngineMetadata,
        query_context: &QueryContext,
    ) -> Option<Vec<DataRange>> {
        let raptor_meta = metadata
            .as_any()
            .downcast_ref::<RaptorCachedMetadata>()
            .expect("Invalid metadata type for RAPTOR");

        // RAPTOR uses Matrix Trinity (P² + K² + P×K) instead of HNSW
        // For vector searches, we use the K² matrix for centroid navigation
        // and P×K matrix for spillover detection

        // For brute force search, read all row groups
        if query_context.query_vector.is_some() {
            // Read all row groups (simplified)
            if !raptor_meta.rowgroup_offsets.is_empty() {
                let ranges: Vec<DataRange> = raptor_meta
                    .rowgroup_offsets
                    .windows(2)
                    .map(|w| DataRange {
                        offset: w[0],
                        length: w[1] - w[0],
                        priority: 128, // Medium priority
                    })
                    .collect();
                return Some(ranges);
            }
        }

        None // Read entire file
    }
}
