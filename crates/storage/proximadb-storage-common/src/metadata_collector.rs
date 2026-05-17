//! Metadata collection trait for engine-specific sidecar files.
//!
//! Storage engines implement this to collect additional metadata during Parquet
//! writing for their own optimization (e.g., NOVA SuperBlocks, zone maps).

use anyhow::Result;
use arrow_array::RecordBatch;
use parquet::file::metadata::RowGroupMetaData;

/// Trait for collecting engine-specific metadata during Parquet writes
pub trait MetadataCollector: Send + Sync {
    fn on_row_group_start(&mut self, row_group_index: usize) -> Result<()>;

    fn on_batch_write(
        &mut self,
        batch: &RecordBatch,
        row_group_index: usize,
        batch_index_in_group: usize,
    ) -> Result<()>;

    fn on_row_group_complete(
        &mut self,
        row_group_index: usize,
        metadata: &RowGroupMetaData,
    ) -> Result<()>;

    fn finalize(&mut self, total_row_groups: usize) -> Result<()>;

    fn serialize_metadata(&self) -> Result<Vec<u8>>;

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

#[derive(Debug, Clone)]
pub struct MetadataCollectionConfig {
    pub compute_superblocks: bool,
    pub row_groups_per_superblock: usize,
    pub compute_vector_stats: bool,
    pub track_quantization_stats: bool,
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
