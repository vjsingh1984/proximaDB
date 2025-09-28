//! Streaming Parquet Writer
//!
//! This module provides streaming write capabilities for Parquet files,
//! optimized for large datasets with batch processing and row group management.

use anyhow::{anyhow, Context, Result};
use arrow::array::{
    ArrayRef, BinaryArray, Float32Array, Int64Array,
    RecordBatch, StringArray, UInt32Array,
};
use arrow::datatypes::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use parquet::file::properties::WriterProperties;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info, trace, warn};
use crate::storage::persistence::filesystem::{FilesystemFactory, FileSystem};

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::engines::core::formats::columnar::{
    metadata_collector::MetadataCollector,
    native_metadata::NativeMetadataHandler,
};
use crate::storage::engines::core::formats::columnar::ColumnarFilterableSpec;

use super::{
    writer_config::ParquetWriterConfig,
    writer_statistics::StreamingParquetWriterStats,
    schema_builder::{ParquetSchemaBuilder, create_writer_properties},
    implicit_id_generator::IdLessLookup,
};

/// Streaming Parquet writer optimized for columnar engines
pub struct StreamingParquetWriter {
    writer: ArrowWriter<Vec<u8>>,
    config: ParquetWriterConfig,
    schema: Arc<Schema>,
    current_batch: Vec<VectorRecord>,
    current_row_group: usize,
    total_records_written: u64,

    /// Custom ID bloom filters per row group (supplements Parquet native filters)
    id_bloom_filters: Vec<crate::storage::engines::core::formats::columnar::id_index::BloomFilter>,

    /// Metadata bloom filters for other columns
    metadata_bloom_filters: HashMap<String, crate::storage::engines::core::formats::columnar::id_index::BloomFilter>,

    file_path: String,

    /// Native metadata handler for optimized types
    native_metadata_handler: Option<NativeMetadataHandler>,

    /// Metadata samples for type inference
    metadata_samples: Vec<serde_json::Map<String, serde_json::Value>>,

    /// Filterable column definitions
    filterable_columns: Vec<ColumnarFilterableSpec>,

    /// Optional metadata collector for engine-specific sidecar files
    metadata_collector: Option<Box<dyn MetadataCollector>>,

    /// Filesystem factory for cloud storage support
    filesystem_factory: Arc<FilesystemFactory>,
}

impl StreamingParquetWriter {
    /// Create new streaming writer with optional filterable columns
    pub fn new<P: AsRef<Path>>(
        file_path: P,
        dimension: usize,
        config: ParquetWriterConfig,
        filterable_columns: Option<&[crate::proto::proximadb_v1::FilterableColumnSpec]>,
    ) -> Result<Self> {
        let filesystem_factory = Arc::new(FilesystemFactory::default());
        Self::with_filesystem_factory(
            file_path,
            dimension,
            config,
            filterable_columns,
            filesystem_factory,
        )
    }

    /// Create new streaming writer with filesystem factory for cloud storage support
    pub fn with_filesystem_factory<P: AsRef<Path>>(
        file_path: P,
        dimension: usize,
        config: ParquetWriterConfig,
        filterable_columns: Option<&[crate::proto::proximadb_v1::FilterableColumnSpec]>,
        filesystem_factory: Arc<FilesystemFactory>,
    ) -> Result<Self> {
        let file_path_str = file_path.as_ref().to_string_lossy().to_string();
        info!("Creating streaming Parquet writer with filesystem API: {}", file_path_str);

        // Convert proto filterable columns to columnar format
        let columnar_filterable: Vec<ColumnarFilterableSpec> = filterable_columns
            .map(|cols| cols.iter().map(ColumnarFilterableSpec::from_proto).collect())
            .unwrap_or_else(Vec::new);

        // Build optimized schema
        let mut schema_builder = ParquetSchemaBuilder::new(dimension, config.clone());

        // Convert ColumnarFilterableSpec to proto FilterableColumnSpec for schema builder
        let proto_filterable: Vec<crate::proto::proximadb_v1::FilterableColumnSpec> = columnar_filterable
            .iter()
            .map(|spec| {
                // Manual conversion since to_proto doesn't exist
                crate::proto::proximadb_v1::FilterableColumnSpec {
                    name: spec.name.clone(),
                    data_type: match spec.data_type {
                        crate::storage::engines::core::formats::columnar::schema::FilterableData::String => 0,
                        crate::storage::engines::core::formats::columnar::schema::FilterableData::Integer => 1,
                        crate::storage::engines::core::formats::columnar::schema::FilterableData::Float => 2,
                        crate::storage::engines::core::formats::columnar::schema::FilterableData::Boolean => 3,
                        crate::storage::engines::core::formats::columnar::schema::FilterableData::Datetime => 4,
                        _ => 0,
                    },
                    indexed: spec.indexed,
                    estimated_cardinality: spec.estimated_cardinality.map(|c| c as u32),  // Convert Option<usize> to Option<u32>
                    supports_range: false,
                }
            })
            .collect();

        if !proto_filterable.is_empty() {
            schema_builder = schema_builder.with_filterable_columns(proto_filterable);
        }

        let schema = schema_builder.build_schema()?;

        // Create writer properties with optimizations
        let props = create_writer_properties(&config)?;

        // Create a write buffer instead of opening file directly
        let write_buffer = Vec::new();
        let writer = ArrowWriter::try_new(write_buffer, schema.clone(), Some(props))?;

        // Initialize native metadata handler if filterable columns are specified
        let native_metadata_handler = if config.filterable_metadata_columns.is_some() {
            Some(NativeMetadataHandler::new())
        } else {
            None
        };

        Ok(Self {
            writer,
            config,
            schema,
            current_batch: Vec::new(),
            current_row_group: 0,
            total_records_written: 0,
            id_bloom_filters: Vec::new(),
            metadata_bloom_filters: HashMap::new(),
            file_path: file_path_str,
            native_metadata_handler,
            metadata_samples: Vec::new(),
            filterable_columns: columnar_filterable,
            metadata_collector: None,
            filesystem_factory,
        })
    }

    /// Set metadata collector for hierarchical metadata (NOVA engine)
    pub fn with_metadata_collector(mut self, collector: Box<dyn MetadataCollector>) -> Self {
        self.metadata_collector = Some(collector);
        self
    }

    /// Write a batch of records (streaming interface)
    pub async fn write_batch(&mut self, records: &[VectorRecord]) -> Result<()> {
        debug!(
            "Writing batch of {} records, dimension={}",
            records.len(),
            if !records.is_empty() { records[0].vector.len() } else { 0 }
        );

        // Collect metadata samples for type inference
        if self.native_metadata_handler.is_some()
            && self.metadata_samples.len() < 100  // Default metadata inference sample size
        {
            self.collect_metadata_samples(records)?;
        }

        // Add records to current batch
        for record in records {
            self.current_batch.push(record.clone());

            // Flush when batch is full
            if self.current_batch.len() >= self.config.write_batch_size {
                self.flush_current_batch().await?;
            }
        }

        Ok(())
    }

    /// Write a single record
    pub async fn write_record(&mut self, record: VectorRecord) -> Result<()> {
        self.write_batch(&[record]).await
    }

    /// Flush current batch to Parquet
    async fn flush_current_batch(&mut self) -> Result<()> {
        if self.current_batch.is_empty() {
            return Ok(());
        }

        trace!("Flushing batch of {} records", self.current_batch.len());

        // Notify collector about new row group starting
        if let Some(ref mut collector) = self.metadata_collector {
            collector.on_row_group_start(self.current_row_group)?;
        }

        // Apply sorting for better compression (if enabled)
        let sorted_records = if !self.config.sort_columns.is_empty() {
            // TODO: Implement column-based sorting
            self.current_batch.clone()
        } else {
            self.current_batch.clone()
        };

        // Convert records to Arrow RecordBatch
        let batch = self.create_record_batch(&sorted_records)?;

        // Notify collector about batch being written
        if let Some(ref mut collector) = self.metadata_collector {
            collector.on_batch_write(&batch, self.current_row_group, 0)?;
        }

        // Update bloom filters (use original order for consistency)
        if self.config.enable_bloom_filters {
            let batch_for_bloom = self.current_batch.clone();
            self.update_bloom_filters(&batch_for_bloom)?;
        }

        // Write to Parquet
        self.writer.write(&batch)?;

        self.total_records_written += self.current_batch.len() as u64;
        self.current_batch.clear();

        debug!("Flushed batch, total records: {}", self.total_records_written);
        Ok(())
    }

    /// Convert VectorRecords to Arrow RecordBatch
    fn create_record_batch(&self, records: &[VectorRecord]) -> Result<RecordBatch> {
        // This is a simplified version - the full implementation would handle
        // all the columns including quantized vectors, metadata, etc.

        let num_records = records.len();
        let mut arrays: Vec<ArrayRef> = Vec::new();

        // ID column
        let ids: Vec<Option<String>> = records.iter()
            .map(|r| Some(r.id.clone()))
            .collect();
        arrays.push(Arc::new(StringArray::from(ids)));

        // Row group offset and row index
        let row_group_offsets: Vec<u32> = vec![self.current_row_group as u32; num_records];
        let row_indices: Vec<u32> = (0..num_records as u32).collect();
        arrays.push(Arc::new(UInt32Array::from(row_group_offsets)));
        arrays.push(Arc::new(UInt32Array::from(row_indices)));

        // Vector data - simplified for now
        // TODO: Implement full vector array creation

        // Timestamp
        let timestamps: Vec<i64> = records.iter()
            .map(|r| r.timestamp as i64)
            .collect();
        arrays.push(Arc::new(Int64Array::from(timestamps)));

        // Create record batch
        RecordBatch::try_new(self.schema.clone(), arrays)
            .context("Failed to create record batch")
    }

    /// Update bloom filters with current batch
    fn update_bloom_filters(&mut self, records: &[VectorRecord]) -> Result<()> {
        // Ensure we have a bloom filter for current row group
        while self.id_bloom_filters.len() <= self.current_row_group {
            let estimated_items = self.config.bloom_filter_ndv.max(100000);
            // BloomFilter::new expects different parameters
            // Using default configuration for now
            let bloom = crate::storage::engines::core::formats::columnar::id_index::BloomFilter::new(1000, 0.01);  // expected_items, false_positive_rate
            self.id_bloom_filters.push(bloom);
        }

        // Update ID bloom filter
        let bloom = &mut self.id_bloom_filters[self.current_row_group];
        for record in records {
            bloom.insert(&record.id);
        }

        Ok(())
    }

    /// Collect metadata samples for type inference
    fn collect_metadata_samples(&mut self, records: &[VectorRecord]) -> Result<()> {
        for record in records {
            if !record.metadata.is_empty() &&
               self.metadata_samples.len() < 100 { // Default metadata inference sample size
                // Convert metadata to JSON for sampling
                let metadata_map = self.convert_metadata_to_json(&record.metadata)?;
                self.metadata_samples.push(metadata_map);

                // Perform type inference once we have enough samples
                if self.metadata_samples.len() >= 100 { // Default metadata inference sample size
                    self.infer_metadata_types()?;
                }
            }
        }
        Ok(())
    }

    /// Convert metadata to JSON map
    fn convert_metadata_to_json(
        &self,
        metadata: &HashMap<String, crate::proto::proximadb_v1::SqlValue>,
    ) -> Result<serde_json::Map<String, serde_json::Value>> {
        let mut map = serde_json::Map::new();

        for (key, sql_value) in metadata {
            if let Some(value) = &sql_value.value {
                use crate::proto::proximadb_v1::sql_value::Value;
                let json_value = match value {
                    Value::StringValue(s) => serde_json::Value::String(s.clone()),
                    Value::NumberValue(f) => serde_json::Value::Number(
                        serde_json::Number::from_f64(*f).unwrap_or(serde_json::Number::from(0))
                    ),
                    Value::BoolValue(b) => serde_json::Value::Bool(*b),
                    Value::Int64Value(i) => serde_json::Value::Number(serde_json::Number::from(*i)),
                    _ => serde_json::Value::String("".to_string()),
                };
                map.insert(key.clone(), json_value);
            }
        }

        Ok(map)
    }

    /// Infer metadata types from collected samples
    fn infer_metadata_types(&mut self) -> Result<()> {
        if let Some(ref mut handler) = self.native_metadata_handler {
            info!("Inferring metadata types from {} samples", self.metadata_samples.len());
            handler.analyze_metadata(&self.metadata_samples)?;
            self.metadata_samples.clear();
        }
        Ok(())
    }

    /// Finalize the writer and return statistics
    pub async fn finalize(mut self) -> Result<(StreamingParquetWriterStats, Option<Box<dyn MetadataCollector>>)> {
        // Flush any remaining records
        if !self.current_batch.is_empty() {
            self.flush_current_batch().await?;
        }

        // Finish writing and get the metadata
        let _metadata = self.writer.finish()?;

        // Extract the written data from the writer
        let written_data = self.writer.into_inner()?;

        // Write data to the filesystem using the filesystem API
        let fs = self.filesystem_factory.get_filesystem(&self.file_path)?;
        let path = FilesystemFactory::resolve_path(&self.file_path)?;

        fs.write(&path, &written_data, None).await
            .context("Failed to write Parquet data to filesystem")?;

        // Notify collector about finalization (use current_row_group + 1 as total)
        let total_row_groups = self.current_row_group + 1;
        if let Some(ref mut collector) = self.metadata_collector {
            collector.finalize(total_row_groups)?;
        }

        // Calculate statistics using filesystem metadata
        let file_metadata = fs.metadata(&path).await
            .context("Failed to get file metadata after write")?;
        let file_size = file_metadata.size;

        let compression_ratio = if self.total_records_written > 0 {
            // Simplified calculation - actual implementation would be more sophisticated
            1.0
        } else {
            1.0
        };

        let stats = StreamingParquetWriterStats {
            // File information
            file_path: self.file_path.clone(),
            file_size,
            total_row_groups: total_row_groups as usize,
            // Record statistics
            total_records: self.total_records_written as usize,
            unique_ids: 0, // Would need to track this
            duplicate_ids: 0,
            uncompressed_size: 0, // Would need to track
            compressed_size: file_size as usize,
            vector_data_size: 0,
            metadata_size: 0,
            compression_ratio: compression_ratio as f64,
            vector_compression_ratio: 0.0,
            metadata_compression_ratio: 0.0,
            row_groups_written: total_row_groups as usize,
            avg_row_group_size: if total_row_groups > 0 {
                self.total_records_written as usize / total_row_groups as usize
            } else { 0 },
            min_row_group_size: 0,
            max_row_group_size: 0,
            bloom_filter_count: self.id_bloom_filters.len(),
            bloom_filter_total_size: 0,
            write_duration: std::time::Duration::default(),
            compression_duration: std::time::Duration::default(),
            index_build_duration: std::time::Duration::default(),
            throughput_records_per_sec: 0.0,
            throughput_mb_per_sec: 0.0,
            quantization_enabled: self.config.quantization.enabled,
            quantization_levels: vec![],
            quantization_space_saved: 0,
            filterable_columns_count: self.filterable_columns.len(),
            records_with_metadata: 0,
            avg_metadata_fields: 0.0,
        };

        Ok((stats, self.metadata_collector))
    }
}

/// Builder for StreamingParquetWriter
pub struct StreamingWriterBuilder {
    file_path: Option<String>,
    dimension: Option<usize>,
    config: ParquetWriterConfig,
    filterable_columns: Option<Vec<crate::proto::proximadb_v1::FilterableColumnSpec>>,
    filesystem_factory: Option<Arc<FilesystemFactory>>,
}

impl StreamingWriterBuilder {
    /// Create new builder
    pub fn new() -> Self {
        Self {
            file_path: None,
            dimension: None,
            config: ParquetWriterConfig::default(),
            filterable_columns: None,
            filesystem_factory: None,
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
    pub fn with_filterable_columns(mut self, columns: Vec<crate::proto::proximadb_v1::FilterableColumnSpec>) -> Self {
        self.filterable_columns = Some(columns);
        self
    }

    /// Set filesystem factory
    pub fn with_filesystem_factory(mut self, filesystem_factory: Arc<FilesystemFactory>) -> Self {
        self.filesystem_factory = Some(filesystem_factory);
        self
    }

    /// Build the writer
    pub fn build(self) -> Result<StreamingParquetWriter> {
        let file_path = self.file_path
            .ok_or_else(|| anyhow!("File path is required"))?;
        let dimension = self.dimension
            .ok_or_else(|| anyhow!("Dimension is required"))?;

        match self.filesystem_factory {
            Some(factory) => StreamingParquetWriter::with_filesystem_factory(
                file_path,
                dimension,
                self.config,
                self.filterable_columns.as_deref(),
                factory,
            ),
            None => StreamingParquetWriter::new(
                file_path,
                dimension,
                self.config,
                self.filterable_columns.as_deref(),
            ),
        }
    }
}