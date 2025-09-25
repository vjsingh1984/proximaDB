//! Metadata collection trait for engine-specific sidecar files
//!
//! This module provides a trait that storage engines can implement to collect
//! additional metadata during Parquet writing for their own optimization purposes.
//! For example, NOVA uses this to build SuperBlocks and hierarchical zone maps.

use anyhow::Result;
use arrow_array::RecordBatch;
use parquet::file::metadata::RowGroupMetaData;
use std::sync::Arc;

/// Trait for collecting engine-specific metadata during Parquet writes
///
/// Implementations can gather statistics during the write process without
/// needing to re-read the file, enabling efficient sidecar metadata generation.
pub trait MetadataCollector: Send + Sync {
    /// Called when a new row group starts
    fn on_row_group_start(&mut self, row_group_index: usize) -> Result<()>;

    /// Called for each batch of records before they're written
    /// This is where most statistics collection happens
    fn on_batch_write(
        &mut self,
        batch: &RecordBatch,
        row_group_index: usize,
        batch_index_in_group: usize,
    ) -> Result<()>;

    /// Called when a row group is completed with its Parquet metadata
    fn on_row_group_complete(
        &mut self,
        row_group_index: usize,
        metadata: &RowGroupMetaData,
    ) -> Result<()>;

    /// Called after all data is written to finalize statistics
    fn finalize(&mut self, total_row_groups: usize) -> Result<()>;

    /// Serialize collected metadata to bytes for sidecar file
    fn serialize_metadata(&self) -> Result<Vec<u8>>;

    /// Get the sidecar file extension (e.g., "nova_meta")
    fn sidecar_extension(&self) -> &str;
}

/// No-op collector for engines that don't need sidecar files
pub struct NoOpCollector;

impl MetadataCollector for NoOpCollector {
    fn on_row_group_start(&mut self, _row_group_index: usize) -> Result<()> {
        Ok(())
    }

    fn on_batch_write(
        &mut self,
        _batch: &RecordBatch,
        _row_group_index: usize,
        _batch_index_in_group: usize,
    ) -> Result<()> {
        Ok(())
    }

    fn on_row_group_complete(
        &mut self,
        _row_group_index: usize,
        _metadata: &RowGroupMetaData,
    ) -> Result<()> {
        Ok(())
    }

    fn finalize(&mut self, _total_row_groups: usize) -> Result<()> {
        Ok(())
    }

    fn serialize_metadata(&self) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }

    fn sidecar_extension(&self) -> &str {
        ""
    }
}

/// Configuration for metadata collection
#[derive(Debug, Clone)]
pub struct MetadataCollectionConfig {
    /// Whether to compute SuperBlock statistics (for NOVA)
    pub compute_superblocks: bool,

    /// Number of row groups per SuperBlock
    pub row_groups_per_superblock: usize,

    /// Whether to compute enhanced vector statistics
    pub compute_vector_stats: bool,

    /// Whether to track quantization effectiveness
    pub track_quantization_stats: bool,

    /// Sample rate for expensive statistics (1.0 = all, 0.1 = 10%)
    pub stats_sample_rate: f32,
}

impl Default for MetadataCollectionConfig {
    fn default() -> Self {
        Self {
            compute_superblocks: true,
            row_groups_per_superblock: 10,
            compute_vector_stats: true,
            track_quantization_stats: true,
            stats_sample_rate: 1.0,
        }
    }
}