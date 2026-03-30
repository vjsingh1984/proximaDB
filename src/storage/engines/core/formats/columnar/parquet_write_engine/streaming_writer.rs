//! Streaming Parquet Writer
//!
//! This module provides streaming write capabilities for Parquet files,
//! optimized for large datasets with batch processing and row group management.

use crate::storage::persistence::filesystem::FilesystemFactory;
use anyhow::{Result, anyhow};
use arrow::array::{ArrayRef, Float32Array, Int64Array, RecordBatch, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use parquet::arrow::ArrowWriter;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, error, info, trace};

use crate::proto::proximadb_v1::VectorRecord;
use crate::storage::engines::core::formats::columnar::ColumnarFilterableSpec;
use crate::storage::engines::core::formats::columnar::{
    metadata_collector::MetadataCollector, native_metadata::NativeMetadataHandler,
};

use super::{
    schema_builder::{ParquetSchemaBuilder, create_writer_properties},
    writer_config::ParquetWriterConfig,
    writer_statistics::StreamingParquetWriterStats,
};

/// Streaming Parquet writer optimized for columnar engines
pub struct StreamingParquetWriter {
    writer: ArrowWriter<Vec<u8>>,
    config: ParquetWriterConfig,
    schema: Arc<Schema>,
    dimension: usize,
    current_batch: Vec<VectorRecord>,
    current_row_group: usize,
    total_records_written: u64,

    /// Custom ID bloom filters per row group (supplements Parquet native filters)
    #[allow(dead_code)]
    id_bloom_filters: Vec<crate::storage::engines::core::formats::columnar::id_index::BloomFilter>,

    /// Metadata bloom filters for other columns
    #[allow(dead_code)]
    metadata_bloom_filters:
        HashMap<String, crate::storage::engines::core::formats::columnar::id_index::BloomFilter>,

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
    #[allow(dead_code)]
    filesystem_factory: Arc<FilesystemFactory>,
}

impl StreamingParquetWriter {
    /// Create new streaming writer with optional filterable columns
    pub async fn new<P: AsRef<Path>>(
        file_path: P,
        dimension: usize,
        config: ParquetWriterConfig,
        filterable_columns: Option<&[crate::proto::proximadb_v1::FilterableColumnSpec]>,
    ) -> Result<Self> {
        let filesystem_factory = Arc::new(FilesystemFactory::create_default().await?);
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
        info!(
            "Creating streaming Parquet writer with filesystem API: {}",
            file_path_str
        );

        // Convert proto filterable columns to columnar format
        let columnar_filterable: Vec<ColumnarFilterableSpec> = filterable_columns
            .map(|cols| {
                cols.iter()
                    .map(ColumnarFilterableSpec::from_proto)
                    .collect()
            })
            .unwrap_or_else(Vec::new);

        // Build optimized schema
        let mut schema_builder = ParquetSchemaBuilder::new(dimension, config.clone());

        // Convert ColumnarFilterableSpec to proto FilterableColumnSpec for schema builder
        let proto_filterable: Vec<crate::proto::proximadb_v1::FilterableColumnSpec> =
            columnar_filterable
                .iter()
                .map(|spec| {
                    // Manual conversion since to_proto doesn't exist
                    crate::proto::proximadb_v1::FilterableColumnSpec {
                    name: spec.name.clone(),
                    data_type: match spec.data_type {
                        crate::storage::engines::core::formats::columnar::schema::FilterableData::String => 1,   // FILTERABLE_STRING = 1
                        crate::storage::engines::core::formats::columnar::schema::FilterableData::Integer => 2,  // FILTERABLE_INTEGER = 2
                        crate::storage::engines::core::formats::columnar::schema::FilterableData::Float => 3,    // FILTERABLE_FLOAT = 3
                        crate::storage::engines::core::formats::columnar::schema::FilterableData::Boolean => 4,  // FILTERABLE_BOOLEAN = 4
                        crate::storage::engines::core::formats::columnar::schema::FilterableData::Datetime => 5, // FILTERABLE_DATETIME = 5
                        _ => 0,  // FILTERABLE_DATA_TYPE_UNSPECIFIED = 0
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
            dimension,
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
            if !records.is_empty() {
                records[0].vector.len()
            } else {
                0
            }
        );

        // Collect metadata samples for type inference
        if self.native_metadata_handler.is_some() && self.metadata_samples.len() < 100
        // Default metadata inference sample size
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

        debug!(
            "Flushed batch, total records: {}",
            self.total_records_written
        );
        Ok(())
    }

    /// Convert VectorRecords to Arrow RecordBatch
    fn create_record_batch(&self, records: &[VectorRecord]) -> Result<RecordBatch> {
        // This is a simplified version - the full implementation would handle
        // all the columns including quantized vectors, metadata, etc.

        let num_records = records.len();
        let mut arrays: Vec<ArrayRef> = Vec::new();

        // ID column
        let ids: Vec<Option<String>> = records.iter().map(|r| Some(r.id.clone())).collect();
        arrays.push(Arc::new(StringArray::from(ids)));

        // Row group offset and row index
        let row_group_offsets: Vec<u32> = vec![self.current_row_group as u32; num_records];
        let row_indices: Vec<u32> = (0..num_records as u32).collect();
        arrays.push(Arc::new(UInt32Array::from(row_group_offsets)));
        arrays.push(Arc::new(UInt32Array::from(row_indices)));

        // Vector data - Create List array of Float32 (non-nullable items)
        // Create fixed-size list array (more efficient for vectors with known dimension)
        use arrow_array::{FixedSizeListArray, Float32Array};

        // Flatten all vectors into a single array
        let mut values = Vec::with_capacity(records.len() * self.dimension);
        for record in records {
            // Ensure vector has correct dimension
            if record.vector.len() != self.dimension {
                return Err(anyhow::anyhow!(
                    "Vector dimension mismatch: expected {}, got {}",
                    self.dimension,
                    record.vector.len()
                ));
            }
            values.extend_from_slice(&record.vector);
        }

        let values_array = Float32Array::from(values);
        let fixed_list_array = FixedSizeListArray::try_new(
            Arc::new(Field::new("item", DataType::Float32, false)),
            self.dimension as i32,
            Arc::new(values_array),
            None,
        )
        .map_err(|e| anyhow::anyhow!("Failed to create fixed-size list array: {}", e))?;

        arrays.push(Arc::new(fixed_list_array));

        // Add quantization arrays if enabled using UnifiedQuantizationEngine
        if self.config.quantization.enable_binary.unwrap_or(false)
            || self.config.quantization.enable_int8.unwrap_or(false)
            || self.config.quantization.enable_pq.unwrap_or(false)
        {
            self.add_quantization_arrays(&mut arrays, records)?;
        }

        // Timestamp
        let timestamps: Vec<i64> = records
            .iter()
            .map(|r| r.timestamp.unwrap_or(0))
            .collect();
        arrays.push(Arc::new(Int64Array::from(timestamps)));

        // Updated at (optional)
        let updated_at: Vec<Option<i64>> = records
            .iter()
            .map(|r| r.updated_at)
            .collect();
        arrays.push(Arc::new(Int64Array::from(updated_at)));

        // Expires at (optional)
        let expires_at: Vec<Option<i64>> = records
            .iter()
            .map(|r| r.expires_at)
            .collect();
        arrays.push(Arc::new(Int64Array::from(expires_at)));

        // Version (optional)
        let versions: Vec<Option<u32>> = records.iter().map(|r| r.version).collect();
        arrays.push(Arc::new(UInt32Array::from(versions)));

        // Source (optional)
        let sources: Vec<Option<String>> = records.iter().map(|r| r.source.clone()).collect();
        arrays.push(Arc::new(StringArray::from(sources)));

        // Add filterable column arrays if specified
        if !self.filterable_columns.is_empty() {
            use crate::storage::engines::core::formats::columnar::schema::FilterableData;
            use arrow_array::{BooleanArray, Float64Array, Int64Array};

            debug!(
                "Writing {} filterable columns for {} records",
                self.filterable_columns.len(),
                records.len()
            );
            for col_spec in &self.filterable_columns {
                // Create array based on column data type with proper typing
                let array: ArrayRef = match col_spec.data_type {
                    FilterableData::String => {
                        let values: Vec<Option<String>> = records
                            .iter()
                            .map(|r| {
                                r.metadata.get(&col_spec.name).and_then(|sql_value| {
                                    if let Some(value) = &sql_value.value {
                                        use crate::proto::proximadb_v1::sql_value::Value;
                                        match value {
                                            Value::StringValue(s) => Some(s.clone()),
                                            _ => None,
                                        }
                                    } else {
                                        None
                                    }
                                })
                            })
                            .collect();
                        Arc::new(StringArray::from(values))
                    }
                    FilterableData::Integer => {
                        let values: Vec<Option<i64>> = records
                            .iter()
                            .map(|r| {
                                r.metadata.get(&col_spec.name).and_then(|sql_value| {
                                    if let Some(value) = &sql_value.value {
                                        use crate::proto::proximadb_v1::sql_value::Value;
                                        match value {
                                            Value::Int64Value(i) => Some(*i),
                                            Value::NumberValue(f) => Some(*f as i64),
                                            _ => None,
                                        }
                                    } else {
                                        None
                                    }
                                })
                            })
                            .collect();
                        Arc::new(Int64Array::from(values))
                    }
                    FilterableData::Float => {
                        let values: Vec<Option<f64>> = records
                            .iter()
                            .map(|r| {
                                r.metadata.get(&col_spec.name).and_then(|sql_value| {
                                    if let Some(value) = &sql_value.value {
                                        use crate::proto::proximadb_v1::sql_value::Value;
                                        match value {
                                            Value::NumberValue(f) => Some(*f),
                                            Value::Int64Value(i) => Some(*i as f64),
                                            _ => None,
                                        }
                                    } else {
                                        None
                                    }
                                })
                            })
                            .collect();
                        Arc::new(Float64Array::from(values))
                    }
                    FilterableData::Boolean => {
                        let values: Vec<Option<bool>> = records
                            .iter()
                            .map(|r| {
                                r.metadata.get(&col_spec.name).and_then(|sql_value| {
                                    if let Some(value) = &sql_value.value {
                                        use crate::proto::proximadb_v1::sql_value::Value;
                                        match value {
                                            Value::BoolValue(b) => Some(*b),
                                            _ => None,
                                        }
                                    } else {
                                        None
                                    }
                                })
                            })
                            .collect();
                        Arc::new(BooleanArray::from(values))
                    }
                    FilterableData::Datetime => {
                        // Timestamp as Int64 (microseconds)
                        let values: Vec<Option<i64>> = records
                            .iter()
                            .map(|r| {
                                r.metadata.get(&col_spec.name).and_then(|sql_value| {
                                    if let Some(value) = &sql_value.value {
                                        use crate::proto::proximadb_v1::sql_value::Value;
                                        match value {
                                            Value::Int64Value(i) => Some(*i),
                                            _ => None,
                                        }
                                    } else {
                                        None
                                    }
                                })
                            })
                            .collect();
                        Arc::new(Int64Array::from(values))
                    }
                    _ => {
                        // Default to string for unsupported types
                        let values: Vec<Option<String>> = records
                            .iter()
                            .map(|r| {
                                r.metadata.get(&col_spec.name).and_then(|sql_value| {
                                    if let Some(value) = &sql_value.value {
                                        use crate::proto::proximadb_v1::sql_value::Value;
                                        match value {
                                            Value::StringValue(s) => Some(s.clone()),
                                            _ => None,
                                        }
                                    } else {
                                        None
                                    }
                                })
                            })
                            .collect();
                        Arc::new(StringArray::from(values))
                    }
                };

                arrays.push(array);
            }
        }

        // Extra metadata - Create a Map array matching the schema
        // The schema expects a struct with "key" and "value" fields
        use arrow_array::{MapArray, StructArray};

        // Create struct array for the entries
        let key_field = Field::new("key", DataType::Utf8, false);
        let value_field = Field::new("value", DataType::Utf8, true);
        let struct_fields = vec![key_field.clone(), value_field.clone()];

        // Collect all metadata entries from all records
        let mut all_keys = Vec::new();
        let mut all_values = Vec::new();
        let mut map_offsets = Vec::new();
        let mut current_offset = 0i32;

        // Build set of filterable column names for exclusion
        let filterable_names: std::collections::HashSet<String> = self
            .filterable_columns
            .iter()
            .map(|col| col.name.clone())
            .collect();

        debug!(
            "Writing metadata for {} records (excluding {} filterable columns)",
            records.len(),
            filterable_names.len()
        );

        // Build the key-value pairs for each record's metadata
        for (idx, record) in records.iter().enumerate() {
            map_offsets.push(current_offset);
            let metadata_count = record.metadata.len();

            if idx < 3 || metadata_count > 0 {
                trace!(
                    "Record {} (id={}) has {} metadata entries",
                    idx, record.id, metadata_count
                );
            }

            // Add only non-filterable metadata entries for this record
            // Filterable metadata is already in typed columns
            for (key, sql_value) in &record.metadata {
                // Skip if this is a filterable column (already in typed column)
                if filterable_names.contains(key) {
                    if idx < 3 {
                        trace!("  Skipping filterable column: {}", key);
                    }
                    continue;
                }

                all_keys.push(key.clone());

                // Convert SqlValue to string representation
                let value_str = if let Some(value) = &sql_value.value {
                    use crate::proto::proximadb_v1::sql_value::Value;
                    match value {
                        Value::StringValue(s) => Some(s.clone()),
                        Value::NumberValue(f) => Some(f.to_string()),
                        Value::BoolValue(b) => Some(b.to_string()),
                        Value::Int64Value(i) => Some(i.to_string()),
                        _ => Some("".to_string()),
                    }
                } else {
                    None
                };

                all_values.push(value_str);
                current_offset += 1;

                if idx < 3 {
                    trace!("  Added metadata {}={:?}", key, all_values.last());
                }
            }
        }
        map_offsets.push(current_offset); // Final offset

        debug!("Total metadata entries written: {}", all_keys.len());

        // Create the struct array with all key-value pairs
        let keys_array = StringArray::from(all_keys);
        let values_array = StringArray::from(all_values);

        let struct_array = StructArray::try_new(
            struct_fields.into(),
            vec![Arc::new(keys_array), Arc::new(values_array)],
            None,
        )
        .map_err(|e| anyhow::anyhow!("Failed to create struct array: {}", e))?;

        // Create offsets buffer from the offsets vector
        let offsets =
            unsafe { arrow_buffer::OffsetBuffer::<i32>::new_unchecked(map_offsets.into()) };

        let map_field = Field::new(
            "entries",
            DataType::Struct(vec![key_field, value_field].into()),
            false,
        );

        let map_array = MapArray::new(Arc::new(map_field), offsets, struct_array, None, false);

        arrays.push(Arc::new(map_array));

        // Create record batch
        let array_count = arrays.len();
        let result = RecordBatch::try_new(self.schema.clone(), arrays);
        match result {
            Ok(batch) => Ok(batch),
            Err(e) => {
                error!("RecordBatch creation error: {}", e);
                error!(
                    "Schema field count: {}, Array count: {}",
                    self.schema.fields().len(),
                    array_count
                );
                for (i, field) in self.schema.fields().iter().enumerate() {
                    error!("  Field {}: {} ({:?})", i, field.name(), field.data_type());
                }
                Err(anyhow::anyhow!("Failed to create record batch: {}", e))
            }
        }
    }

    /// Add quantization arrays to the record batch using UnifiedQuantizationEngine
    fn add_quantization_arrays(
        &self,
        arrays: &mut Vec<ArrayRef>,
        records: &[VectorRecord],
    ) -> Result<()> {
        use crate::compute::distance_computation::engine::UnifiedDistanceCompute;
        use crate::compute::quantization::unified::{
            InMemoryCodebookStore, UnifiedQuantizationEngine,
        };
        use arrow::array::BinaryBuilder;

        // Create a quantization engine for this batch with in-memory codebook
        let distance_compute = Arc::new(UnifiedDistanceCompute::default());
        let codebook_store = Arc::new(InMemoryCodebookStore::new());
        let engine = UnifiedQuantizationEngine::new(distance_compute, codebook_store);

        // Binary quantization
        if self.config.quantization.enable_binary.unwrap_or(false) {
            let mut builder = BinaryBuilder::new();
            for record in records {
                let binary_vec = engine.quantize_to_binary(&record.vector)?;
                builder.append_value(&binary_vec);
            }
            arrays.push(Arc::new(builder.finish()));
        }

        // INT8 quantization
        if self.config.quantization.enable_int8.unwrap_or(false) {
            let mut int8_builder = BinaryBuilder::new();
            let mut scale_values = Vec::with_capacity(records.len());
            let mut min_values = Vec::with_capacity(records.len());
            let mut max_values = Vec::with_capacity(records.len());

            for record in records {
                let (int8_vec, min, max) = engine.quantize_to_u8(&record.vector)?;
                int8_builder.append_value(&int8_vec);

                // Calculate scale from min/max
                let range = max - min;
                let scale = if range > 0.0 { range / 255.0 } else { 1.0 };

                scale_values.push(scale);
                min_values.push(min);
                max_values.push(max);
            }

            arrays.push(Arc::new(int8_builder.finish()));
            arrays.push(Arc::new(Float32Array::from(scale_values)));
            arrays.push(Arc::new(Float32Array::from(min_values)));
            arrays.push(Arc::new(Float32Array::from(max_values)));
        }

        // PQ quantization
        if self.config.quantization.enable_pq.unwrap_or(false) {
            // TODO: Implement PQ quantization
            // PQ requires async codebook training and proper sidecar file storage
            // For now, store null values
            let mut pq_builder = BinaryBuilder::new();
            for _ in records {
                pq_builder.append_null();
            }
            arrays.push(Arc::new(pq_builder.finish()));
        }

        Ok(())
    }

    /// Update bloom filters with current batch
    fn update_bloom_filters(&mut self, records: &[VectorRecord]) -> Result<()> {
        // Ensure we have a bloom filter for current row group
        while self.id_bloom_filters.len() <= self.current_row_group {
            let _estimated_items = self.config.bloom_filter_ndv.max(100000);
            // BloomFilter::new expects different parameters
            // Using default configuration for now
            let bloom =
                crate::storage::engines::core::formats::columnar::id_index::BloomFilter::new(
                    1000, 0.01,
                ); // expected_items, false_positive_rate
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
            if !record.metadata.is_empty() && self.metadata_samples.len() < 100 {
                // Default metadata inference sample size
                // Convert metadata to JSON for sampling
                let metadata_map = self.convert_metadata_to_json(&record.metadata)?;
                self.metadata_samples.push(metadata_map);

                // Perform type inference once we have enough samples
                if self.metadata_samples.len() >= 100 {
                    // Default metadata inference sample size
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
                        serde_json::Number::from_f64(*f).unwrap_or(serde_json::Number::from(0)),
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
            info!(
                "Inferring metadata types from {} samples",
                self.metadata_samples.len()
            );
            handler.analyze_metadata(&self.metadata_samples)?;
            self.metadata_samples.clear();
        }
        Ok(())
    }

    /// Finalize the writer and return statistics and data
    pub async fn finalize(
        mut self,
    ) -> Result<(
        StreamingParquetWriterStats,
        Vec<u8>,
        Option<Box<dyn MetadataCollector>>,
    )> {
        // Flush any remaining records
        if !self.current_batch.is_empty() {
            self.flush_current_batch().await?;
        }

        // Finish writing and get the written bytes
        // The finish() method consumes the writer and returns the bytes
        let written_data = self.writer.into_inner()?;

        // Notify collector about finalization (use current_row_group + 1 as total)
        let total_row_groups = self.current_row_group + 1;
        if let Some(ref mut collector) = self.metadata_collector {
            collector.finalize(total_row_groups)?;
        }

        // Calculate statistics from written data
        let file_size = written_data.len() as u64;

        // Calculate compression ratio by comparing vector column compressed size to uncompressed size
        // Uncompressed vector size = dimensions × 4 bytes × record_count
        let uncompressed_vector_size =
            (self.dimension * 4 * self.total_records_written as usize) as u64;

        // Extract compressed vector column size from Parquet metadata
        let compressed_vector_size =
            extract_vector_column_compressed_size(&written_data, uncompressed_vector_size)?;

        // Calculate compression ratio as space savings: 1 - (compressed/uncompressed)
        // This aligns with standard compression literature
        // 0.0 = no compression, 0.5 = 50% space savings, 0.9 = 90% space savings
        let compression_ratio = if uncompressed_vector_size > 0 {
            1.0 - (compressed_vector_size as f64 / uncompressed_vector_size as f64)
        } else {
            0.0
        };

        let stats = StreamingParquetWriterStats {
            // File information
            file_path: self.file_path.clone(),
            file_size,
            total_row_groups,
            // Record statistics
            total_records: self.total_records_written as usize,
            unique_ids: 0, // Would need to track this
            duplicate_ids: 0,
            uncompressed_size: uncompressed_vector_size as usize,
            compressed_size: compressed_vector_size as usize,
            vector_data_size: compressed_vector_size as usize,
            metadata_size: 0,
            compression_ratio,
            vector_compression_ratio: compression_ratio,
            metadata_compression_ratio: 0.0,
            row_groups_written: total_row_groups,
            avg_row_group_size: if total_row_groups > 0 {
                self.total_records_written as usize / total_row_groups
            } else {
                0
            },
            min_row_group_size: 0,
            max_row_group_size: 0,
            bloom_filter_count: self.id_bloom_filters.len(),
            bloom_filter_total_size: 0,
            write_duration: std::time::Duration::default(),
            compression_duration: std::time::Duration::default(),
            index_build_duration: std::time::Duration::default(),
            throughput_records_per_sec: 0.0,
            throughput_mb_per_sec: 0.0,
            quantization_enabled: self.config.quantization.enabled.unwrap_or(false),
            quantization_levels: vec![],
            quantization_space_saved: 0,
            filterable_columns_count: self.filterable_columns.len(),
            records_with_metadata: 0,
            avg_metadata_fields: 0.0,
            // TD-040: Vector bounds computed during write (populated by caller)
            vector_norm_min: None,
            vector_norm_max: None,
            vector_norm_mean: None,
            vector_component_min: None,
            vector_component_max: None,
        };

        Ok((stats, written_data, self.metadata_collector))
    }
}

/// Extract compressed size of vector column from Parquet metadata
fn extract_vector_column_compressed_size(
    parquet_data: &[u8],
    fallback_uncompressed: u64,
) -> Result<u64> {
    use crate::storage::engines::core::formats::columnar::constants::FIELD_VECTOR_FP32;
    use bytes::Bytes;
    use parquet::file::reader::FileReader;
    use parquet::file::serialized_reader::SerializedFileReader;

    // Create a reader from the bytes
    let bytes = Bytes::copy_from_slice(parquet_data);
    let reader = SerializedFileReader::new(bytes)?;

    // Get metadata
    let metadata = reader.metadata();

    // Find the vector column using constant
    let mut total_compressed_size = 0u64;

    // Iterate through row groups
    for rg_idx in 0..metadata.num_row_groups() {
        let row_group = metadata.row_group(rg_idx);

        // Find the vector column in this row group
        for col_idx in 0..row_group.num_columns() {
            let column = row_group.column(col_idx);
            let column_path = column.column_path();
            let path_str = column_path.string();

            // Check if this is the vector column (matches FIELD_VECTOR_FP32 or its nested paths)
            if path_str.starts_with(FIELD_VECTOR_FP32) {
                // Get compressed size from column metadata
                total_compressed_size += column.compressed_size() as u64;
            }
        }
    }

    // If we couldn't find the column or size is 0, return fallback
    if total_compressed_size == 0 {
        Ok(fallback_uncompressed)
    } else {
        Ok(total_compressed_size)
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
    pub fn with_filterable_columns(
        mut self,
        columns: Vec<crate::proto::proximadb_v1::FilterableColumnSpec>,
    ) -> Self {
        self.filterable_columns = Some(columns);
        self
    }

    /// Set filesystem factory
    pub fn with_filesystem_factory(mut self, filesystem_factory: Arc<FilesystemFactory>) -> Self {
        self.filesystem_factory = Some(filesystem_factory);
        self
    }

    /// Build the writer
    pub async fn build(self) -> Result<StreamingParquetWriter> {
        let file_path = self
            .file_path
            .ok_or_else(|| anyhow!("File path is required"))?;
        let dimension = self
            .dimension
            .ok_or_else(|| anyhow!("Dimension is required"))?;

        match self.filesystem_factory {
            Some(factory) => StreamingParquetWriter::with_filesystem_factory(
                file_path,
                dimension,
                self.config,
                self.filterable_columns.as_deref(),
                factory,
            ),
            None => {
                StreamingParquetWriter::new(
                    file_path,
                    dimension,
                    self.config,
                    self.filterable_columns.as_deref(),
                )
                .await
            }
        }
    }
}
