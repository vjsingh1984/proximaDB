//! Batch Parquet Writer
//!
//! This module provides batch writing capabilities for Parquet files,
//! optimized for bulk operations where all data is available upfront.

use anyhow::{Context, Result};
use std::path::Path;
use tracing::{info, debug};

use crate::proto::proximadb_v1::{VectorRecord, FilterableColumnSpec};
use crate::storage::engines::core::formats::columnar::metadata_collector::MetadataCollector;

use super::{
    writer_config::ParquetWriterConfig,
    writer_statistics::StreamingParquetWriterStats,
    streaming_writer::StreamingParquetWriter,
};

/// Batch Parquet writer for bulk operations
pub struct BatchParquetWriter {
    config: ParquetWriterConfig,
    file_path: String,
    dimension: usize,
    filterable_columns: Option<Vec<FilterableColumnSpec>>,
    metadata_collector: Option<Box<dyn MetadataCollector>>,
}

impl BatchParquetWriter {
    /// Create new batch writer
    pub fn new<P: AsRef<Path>>(
        file_path: P,
        dimension: usize,
        config: ParquetWriterConfig,
    ) -> Self {
        Self {
            config,
            file_path: file_path.as_ref().to_string_lossy().to_string(),
            dimension,
            filterable_columns: None,
            metadata_collector: None,
        }
    }

    /// Set filterable columns for the writer
    pub fn with_filterable_columns(mut self, columns: Vec<FilterableColumnSpec>) -> Self {
        self.filterable_columns = Some(columns);
        self
    }

    /// Set metadata collector for hierarchical metadata (NOVA engine)
    pub fn with_metadata_collector(mut self, collector: Box<dyn MetadataCollector>) -> Self {
        self.metadata_collector = Some(collector);
        self
    }

    /// Write all records at once with optional metadata collection
    pub async fn write_all(
        &mut self,
        records: &[VectorRecord],
    ) -> Result<(StreamingParquetWriterStats, Option<Box<dyn MetadataCollector>>)> {
        info!(
            "Batch writing {} records to {}",
            records.len(),
            self.file_path
        );

        // Create streaming writer with batch configuration
        let mut writer = StreamingParquetWriter::new(
            &self.file_path,
            self.dimension,
            self.config.clone(),
            self.filterable_columns.as_deref(),
        )?;

        // Add metadata collector if provided
        if let Some(collector) = self.metadata_collector.take() {
            writer = writer.with_metadata_collector(collector);
        }

        // Calculate optimal batch size based on row group size
        let batch_size = self.config.write_batch_size.min(self.config.row_group_size);

        // Write records in batches
        for chunk in records.chunks(batch_size) {
            writer.write_batch(chunk).await
                .context("Failed to write batch")?;
        }

        // Finalize and get statistics
        writer.finalize().await
    }

    /// Write all records and return only statistics (convenience method)
    pub async fn write_all_simple(
        &mut self,
        records: &[VectorRecord],
    ) -> Result<StreamingParquetWriterStats> {
        let (stats, _) = self.write_all(records).await?;
        Ok(stats)
    }
}

/// Builder for BatchParquetWriter
pub struct BatchWriterBuilder {
    file_path: Option<String>,
    dimension: Option<usize>,
    config: ParquetWriterConfig,
    filterable_columns: Option<Vec<FilterableColumnSpec>>,
    metadata_collector: Option<Box<dyn MetadataCollector>>,
}

impl BatchWriterBuilder {
    /// Create new builder
    pub fn new() -> Self {
        Self {
            file_path: None,
            dimension: None,
            config: ParquetWriterConfig::default(),
            filterable_columns: None,
            metadata_collector: None,
        }
    }

    /// Set file path
    pub fn with_path<P: AsRef<Path>>(mut self, path: P) -> Self {
        self.file_path = Some(path.as_ref().to_string_lossy().to_string());
        self
    }

    /// Set vector dimension
    pub fn with_dimension(mut self, dimension: usize) -> Self {
        self.dimension = Some(dimension);
        self
    }

    /// Set configuration
    pub fn with_config(mut self, config: ParquetWriterConfig) -> Self {
        self.config = config;
        self
    }

    /// Set filterable columns
    pub fn with_filterable_columns(mut self, columns: Vec<FilterableColumnSpec>) -> Self {
        self.filterable_columns = Some(columns);
        self
    }

    /// Set metadata collector
    pub fn with_metadata_collector(mut self, collector: Box<dyn MetadataCollector>) -> Self {
        self.metadata_collector = Some(collector);
        self
    }

    /// Build the writer
    pub fn build(self) -> Result<BatchParquetWriter> {
        let file_path = self.file_path
            .ok_or_else(|| anyhow::anyhow!("File path is required"))?;
        let dimension = self.dimension
            .ok_or_else(|| anyhow::anyhow!("Dimension is required"))?;

        let mut writer = BatchParquetWriter::new(file_path, dimension, self.config);

        if let Some(columns) = self.filterable_columns {
            writer = writer.with_filterable_columns(columns);
        }

        if let Some(collector) = self.metadata_collector {
            writer = writer.with_metadata_collector(collector);
        }

        Ok(writer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_batch_writer_basic() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_batch.parquet");

        let config = ParquetWriterConfig::default();
        let mut writer = BatchParquetWriter::new(&file_path, 128, config);

        let records = vec![
            VectorRecord {
                id: "test_1".to_string(),
                vector: vec![1.0; 128],
                metadata: Default::default(),
                timestamp: 0,
                ..Default::default()
            },
            VectorRecord {
                id: "test_2".to_string(),
                vector: vec![2.0; 128],
                metadata: Default::default(),
                timestamp: 1,
                ..Default::default()
            },
        ];

        let stats = writer.write_all_simple(&records).await.unwrap();
        assert_eq!(stats.total_records, 2);
        assert!(stats.compressed_size > 0);
    }

    #[tokio::test]
    async fn test_batch_writer_with_filterable_columns() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_batch_filterable.parquet");

        let config = ParquetWriterConfig::default();
        let columns = vec![
            FilterableColumnSpec {
                name: "category".to_string(),
                data_type: 0, // STRING type
                indexed: false,
                supports_range: false,
                estimated_cardinality: Some(100),
            },
        ];

        let mut writer = BatchParquetWriter::new(&file_path, 64, config)
            .with_filterable_columns(columns);

        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            "category".to_string(),
            crate::proto::proximadb_v1::SqlValue {
                value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(
                    "test_category".to_string()
                )),
            },
        );

        let records = vec![
            VectorRecord {
                id: "test_1".to_string(),
                vector: vec![1.0; 64],
                metadata: metadata.clone(),
                timestamp: 0,
                ..Default::default()
            },
        ];

        let stats = writer.write_all_simple(&records).await.unwrap();
        assert_eq!(stats.total_records, 1);
        assert_eq!(stats.filterable_columns_count, 1);
    }

    #[test]
    fn test_batch_writer_builder() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test_builder.parquet");

        let writer = BatchWriterBuilder::new()
            .with_path(&file_path)
            .with_dimension(256)
            .with_config(ParquetWriterConfig::for_analytics())
            .build()
            .unwrap();

        assert_eq!(writer.dimension, 256);
        assert_eq!(writer.file_path, file_path.to_string_lossy());
    }
}