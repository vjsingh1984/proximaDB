//! Parquet Reader Core Implementation
//!
//! This module provides the core reader functionality for Parquet files,
//! including file access, record batch reading, and conversion to VectorRecords.

use anyhow::{Context, Result};
use arrow::record_batch::RecordBatch;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ProjectionMask;
use parquet::file::reader::{FileReader, SerializedFileReader};
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info, trace};
use crate::storage::persistence::filesystem::FileSystem;

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::engines::core::formats::columnar::constants::{FIELD_ID, FIELD_TIMESTAMP};
use crate::storage::engines::core::formats::columnar::unified_columnar_io::UnifiedColumnarReader;

use super::unified_reader::{UnifiedParquetReader, ReaderConfig};
use super::{QueryConfig, QueryStatistics};

/// Core Parquet reader implementation
pub struct ParquetReader {
    config: QueryConfig,
    stats: QueryStatistics,
}

impl ParquetReader {
    /// Create new reader with configuration
    pub fn new(config: QueryConfig) -> Self {
        Self {
            config,
            stats: QueryStatistics::default(),
        }
    }

    /// Read Parquet file and return all records
    pub fn read_all(&mut self, file_path: &str) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<VectorRecord>>> + Send + '_>> {
        let file_path = file_path.to_string();
        Box::pin(async move {
        info!("Reading all records from {}", file_path);

        let file = File::open(&file_path)
            .context("Failed to open Parquet file")?;

        let reader = SerializedFileReader::new(file)?;
        let metadata = reader.metadata();

        debug!(
            "File has {} row groups with {} total rows",
            metadata.num_row_groups(),
            metadata.file_metadata().num_rows()
        );

        // Use UnifiedParquetReader for actual reading
        // Create UnifiedCachingFilesystem for optimal performance
        let filesystem_factory = Arc::new(crate::storage::persistence::filesystem::FilesystemFactory::default());
        let base_fs = filesystem_factory.get_filesystem("file://")?;
        let cached_filesystem = Arc::new(
            crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem::new(
                base_fs,
                "default_collection".to_string(),
                "columnar".to_string(),
            )
        );
        let unified_reader = UnifiedParquetReader::new(
            vec![file_path.clone()],
            1024,
            filesystem_factory,
            cached_filesystem,
            "default_collection".to_string(),
            "columnar".to_string(),
        )?;

        // Read all records
        let records = unified_reader.read_all_records(0, None).await?;

        // Update statistics
        self.stats.files_read += 1;
        self.stats.records_read += records.len();
        self.stats.row_groups_read += metadata.num_row_groups() as usize;

        Ok(records)
        })
    }

    /// Read Parquet file using filesystem API
    pub async fn read_all_with_filesystem(
        &mut self,
        file_path: &str,
        filesystem: Arc<dyn FileSystem>,
    ) -> Result<Vec<VectorRecord>> {
        info!("Reading all records from {} using filesystem API", file_path);

        // For now, we'll read the entire file into memory and create a byte slice
        // This is not optimal for large files, but works with the current Parquet reader API
        // TODO: In the future, implement streaming readers that work with async I/O
        let file_data = filesystem.read(file_path).await
            .context("Failed to read Parquet file from filesystem")?;

        // Use bytes::Bytes which implements ChunkReader
        let bytes = bytes::Bytes::from(file_data);
        let reader = SerializedFileReader::new(bytes.clone())?;
        let metadata = reader.metadata();

        debug!(
            "File has {} row groups with {} total rows",
            metadata.num_row_groups(),
            metadata.file_metadata().num_rows()
        );

        // Read data using Arrow ParquetRecordBatchReader
        let builder = ParquetRecordBatchReaderBuilder::try_new(bytes)?;
        let arrow_reader = builder.build()?;

        let mut all_records = Vec::new();
        for batch_result in arrow_reader {
            let batch = batch_result?;
            let records = self.batch_to_records(batch)?;
            all_records.extend(records);
        }

        // Update statistics
        self.stats.files_read += 1;
        self.stats.records_read += all_records.len();
        self.stats.row_groups_read += metadata.num_row_groups() as usize;

        Ok(all_records)
    }

    /// Read specific row groups
    pub async fn read_row_groups(
        &mut self,
        file_path: &str,
        row_groups: &[usize],
    ) -> Result<Vec<VectorRecord>> {
        debug!(
            "Reading row groups {:?} from {}",
            row_groups, file_path
        );

        let mut all_records = Vec::new();

        for &row_group in row_groups {
            let file = File::open(file_path)?;
            let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
            let reader = builder
                .with_row_groups(vec![row_group])
                .build()?;

            for batch_result in reader {
                let batch = batch_result?;
                let records = self.batch_to_records(batch)?;
                all_records.extend(records);
            }
        }

        self.stats.row_groups_read += row_groups.len();
        self.stats.records_read += all_records.len();

        Ok(all_records)
    }

    /// Read with column projection
    pub async fn read_projected(
        &mut self,
        file_path: &str,
        columns: &[String],
    ) -> Result<RecordBatch> {
        debug!(
            "Reading projected columns {:?} from {}",
            columns, file_path
        );

        let file = File::open(file_path)?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)?;
        let schema = builder.schema();

        // Build projection mask
        let column_indices: Vec<usize> = columns
            .iter()
            .filter_map(|name| schema.index_of(name).ok())
            .collect();

        let projection = ProjectionMask::roots(
            builder.parquet_schema(),
            column_indices.clone(),
        );

        let reader = builder
            .with_projection(projection)
            .build()?;

        // Read first batch for now (could be extended)
        let batch = reader
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("No data in file"))??;

        self.stats.columns_projected += columns.len();

        Ok(batch)
    }

    /// Convert Arrow RecordBatch to VectorRecords
    fn batch_to_records(&self, batch: RecordBatch) -> Result<Vec<VectorRecord>> {
        // This is a simplified implementation
        // The full implementation would handle all columns properly

        let num_rows = batch.num_rows();
        let mut records = Vec::with_capacity(num_rows);

        // Extract ID column
        let id_array = batch
            .column_by_name(FIELD_ID)
            .ok_or_else(|| anyhow::anyhow!("ID column not found"))?
            .as_any()
            .downcast_ref::<arrow::array::StringArray>()
            .ok_or_else(|| anyhow::anyhow!("ID column is not string type"))?;

        // Extract timestamp column
        let timestamp_array = batch
            .column_by_name(FIELD_TIMESTAMP)
            .ok_or_else(|| anyhow::anyhow!("Timestamp column not found"))?
            .as_any()
            .downcast_ref::<arrow::array::Int64Array>()
            .ok_or_else(|| anyhow::anyhow!("Timestamp column is not i64 type"))?;

        for row in 0..num_rows {
            let record = VectorRecord {
                id: id_array.value(row).to_string(),
                timestamp: timestamp_array.value(row) as i64,
                vector: vec![], // Would extract from vector column
                metadata: Default::default(), // Would extract from metadata columns
                ..Default::default()
            };
            records.push(record);
        }

        Ok(records)
    }

    /// Get current statistics
    pub fn get_statistics(&self) -> QueryStatistics {
        self.stats.clone()
    }
}

/// Builder for ParquetReader
pub struct ReaderBuilder {
    config: QueryConfig,
}

impl ReaderBuilder {
    /// Create new builder
    pub fn new() -> Self {
        Self {
            config: QueryConfig {
                enable_pushdown: true,
                enable_projection: true,
                enable_statistics: true,
                cache_strategy: super::CacheStrategy::LRU,
                limit: None,
                enable_parallel: true,
                parallel_workers: 4,
            },
        }
    }

    /// Set query configuration
    pub fn with_config(mut self, config: QueryConfig) -> Self {
        self.config = config;
        self
    }

    /// Enable/disable predicate pushdown
    pub fn with_pushdown(mut self, enable: bool) -> Self {
        self.config.enable_pushdown = enable;
        self
    }

    /// Set result limit
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.config.limit = Some(limit);
        self
    }

    /// Build the reader
    pub fn build(self) -> ParquetReader {
        ParquetReader::new(self.config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reader_builder() {
        let reader = ReaderBuilder::new()
            .with_pushdown(false)
            .with_limit(100)
            .build();

        assert!(!reader.config.enable_pushdown);
        assert_eq!(reader.config.limit, Some(100));
    }
}