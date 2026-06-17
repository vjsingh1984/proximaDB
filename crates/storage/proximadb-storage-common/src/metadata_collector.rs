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

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::RecordBatch;
    use arrow_schema::Schema;
    use std::sync::Arc;

    #[test]
    fn noop_collector_accepts_write_lifecycle_and_emits_no_sidecar() {
        let mut collector = NoOpCollector;
        let batch = RecordBatch::new_empty(Arc::new(Schema::empty()));

        collector.on_row_group_start(3).unwrap();
        collector.on_batch_write(&batch, 3, 1).unwrap();
        collector.finalize(4).unwrap();

        assert_eq!(collector.serialize_metadata().unwrap(), Vec::<u8>::new());
        assert_eq!(collector.sidecar_extension(), "");
    }

    #[test]
    fn default_metadata_collection_config_enables_engine_sidecar_statistics() {
        let config = MetadataCollectionConfig::default();

        assert!(config.compute_superblocks);
        assert_eq!(config.row_groups_per_superblock, 10);
        assert!(config.compute_vector_stats);
        assert!(config.track_quantization_stats);
        assert_eq!(config.stats_sample_rate, 1.0);
    }
}
