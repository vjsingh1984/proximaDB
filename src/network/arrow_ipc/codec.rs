// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Arrow <-> Proto codec for Flight protocol
//!
//! Reuses existing Arrow infrastructure from arrow_ipc_scanner.rs and unified_columnar_io.rs
//! for maximum code reuse and consistency.

use anyhow::{Context, Result};
use arrow_array::{
    Array, ArrayRef, BinaryArray, FixedSizeListArray, Float32Array, Int64Array, RecordBatch,
    StringArray, StructArray,
};
use arrow_flight::{FlightData, FlightDescriptor, Ticket};
use arrow_ipc::writer::IpcWriteOptions;
use arrow_schema::{DataType, Field, Fields, Schema};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::proto::proximadb_v1::{MetadataItem, SqlValue, VectorRecord, VectorSearchRequest};

/// Write mode for Arrow IPC operations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WriteMode {
    /// Use WAL for durability (30-50K vectors/sec)
    #[default]
    WAL,
    /// Direct engine write bypassing WAL (100-200K vectors/sec)
    Direct,
}

/// Metadata extracted from FlightDescriptor
#[derive(Debug, Clone)]
pub struct FlightRequestMetadata {
    /// Target collection for the Arrow Flight operation
    pub collection_id: String,
    /// Write mode (WAL or direct engine write)
    pub write_mode: WriteMode,
    /// Whether to trigger compaction after the write completes
    pub trigger_compaction: bool,
}

/// Arrow-Proto bidirectional codec
///
/// Reuses existing conversion functions from arrow_ipc_scanner.rs
pub struct ArrowProtoCodec;

impl ArrowProtoCodec {
    /// Create standard vector schema for ProximaDB
    ///
    /// Schema:
    /// - id: Utf8 (required)
    /// - vector: FixedSizeList<Float32>(dimension) (required)
    /// - metadata: Struct<key: Utf8, value: Utf8> (optional)
    /// - timestamp: Int64 (optional)
    /// - score: Float32 (for search results, optional)
    pub fn create_vector_schema(dimension: usize) -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, false)),
                    dimension as i32,
                ),
                false,
            ),
            Field::new(
                "metadata",
                DataType::Struct(Fields::from(vec![
                    Field::new("key", DataType::Utf8, false),
                    Field::new("value", DataType::Utf8, true),
                ])),
                true,
            ),
            Field::new("timestamp", DataType::Int64, true),
            Field::new("score", DataType::Float32, true),
        ]))
    }

    /// Convert Arrow RecordBatch to VectorRecord protos
    ///
    /// Reuses existing batch_to_vector_records from arrow_ipc_scanner.rs
    pub fn batches_to_vector_records(batches: Vec<RecordBatch>) -> Result<Vec<VectorRecord>> {
        let mut all_records = Vec::new();

        for batch in batches {
            let records = Self::batch_to_vector_records(&batch)?;
            all_records.extend(records);
        }

        Ok(all_records)
    }

    /// Convert single RecordBatch to VectorRecords
    ///
    /// This is adapted from arrow_ipc_scanner.rs::batch_to_vector_records
    fn batch_to_vector_records(batch: &RecordBatch) -> Result<Vec<VectorRecord>> {
        let mut records = Vec::with_capacity(batch.num_rows());

        // Extract id column
        let id_array = batch
            .column_by_name("id")
            .context("Missing 'id' column")?
            .as_any()
            .downcast_ref::<StringArray>()
            .context("'id' column is not StringArray")?;

        // Extract vector column (supports FixedSizeList or Binary)
        let vector_array = batch
            .column_by_name("vector")
            .context("Missing 'vector' column")?;

        let vectors = Self::extract_vectors(vector_array, batch.num_rows())?;

        // Extract optional metadata
        let metadata_items = if let Some(meta_col) = batch.column_by_name("metadata") {
            Self::extract_metadata(meta_col, batch.num_rows())?
        } else {
            vec![Vec::new(); batch.num_rows()]
        };

        // Extract optional timestamp
        let timestamps = if let Some(ts_col) = batch.column_by_name("timestamp") {
            if let Some(ts_array) = ts_col.as_any().downcast_ref::<Int64Array>() {
                (0..batch.num_rows())
                    .map(|i| {
                        if ts_array.is_null(i) {
                            None
                        } else {
                            Some(ts_array.value(i))
                        }
                    })
                    .collect()
            } else {
                vec![None; batch.num_rows()]
            }
        } else {
            vec![None; batch.num_rows()]
        };

        // Build VectorRecord objects
        for i in 0..batch.num_rows() {
            // Convert Vec<MetadataItem> to HashMap<String, SqlValue>
            let metadata_map = metadata_items[i]
                .iter()
                .filter_map(|item| {
                    item.value.as_ref().map(|meta_value| {
                        use crate::proto::proximadb_v1::metadata_item::Value as MetaValue;
                        use crate::proto::proximadb_v1::sql_value::Value as SqlVal;

                        let sql_value = match meta_value {
                            MetaValue::StringValue(s) => SqlValue {
                                value: Some(SqlVal::StringValue(s.clone())),
                            },
                            MetaValue::NumberValue(n) => SqlValue {
                                value: Some(SqlVal::NumberValue(*n)),
                            },
                            MetaValue::BoolValue(b) => SqlValue {
                                value: Some(SqlVal::BoolValue(*b)),
                            },
                        };
                        (item.key.clone(), sql_value)
                    })
                })
                .collect();

            records.push(VectorRecord {
                id: id_array.value(i).to_string(),
                vector: vectors[i].clone(),
                metadata: metadata_map,
                timestamp: timestamps[i],
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            });
        }

        Ok(records)
    }

    /// Extract vectors from Arrow array (handles multiple formats)
    fn extract_vectors(array: &ArrayRef, num_rows: usize) -> Result<Vec<Vec<f32>>> {
        // Try FixedSizeList<Float32> first (standard format)
        if let Some(list_array) = array.as_any().downcast_ref::<FixedSizeListArray>() {
            let value_length = list_array.value_length() as usize;
            let flat_array = list_array
                .values()
                .as_any()
                .downcast_ref::<Float32Array>()
                .context("FixedSizeList values not Float32Array")?;

            return Ok((0..num_rows)
                .map(|i| {
                    let start = i * value_length;
                    flat_array.values()[start..start + value_length].to_vec()
                })
                .collect());
        }

        // Fallback: Binary format (serialized f32 vectors)
        if let Some(binary_array) = array.as_any().downcast_ref::<BinaryArray>() {
            return (0..num_rows)
                .map(|i| {
                    let bytes = binary_array.value(i);
                    Self::deserialize_vector(bytes)
                })
                .collect();
        }

        // Fallback: Direct Float32Array (for 1D vectors)
        if let Some(float_array) = array.as_any().downcast_ref::<Float32Array>() {
            let dim = float_array.len() / num_rows;
            return Ok((0..num_rows)
                .map(|i| float_array.values()[i * dim..(i + 1) * dim].to_vec())
                .collect());
        }

        Err(anyhow::anyhow!(
            "Unsupported vector column type: {:?}",
            array.data_type()
        ))
    }

    /// Deserialize f32 vector from binary (little-endian)
    fn deserialize_vector(bytes: &[u8]) -> Result<Vec<f32>> {
        if !bytes.len().is_multiple_of(4) {
            return Err(anyhow::anyhow!("Invalid vector binary data length"));
        }

        Ok(bytes
            .chunks_exact(4)
            .map(|chunk| {
                let arr = [chunk[0], chunk[1], chunk[2], chunk[3]];
                f32::from_le_bytes(arr)
            })
            .collect())
    }

    /// Extract metadata from Arrow struct column
    fn extract_metadata(array: &ArrayRef, num_rows: usize) -> Result<Vec<Vec<MetadataItem>>> {
        // Handle StructArray with key/value fields
        if let Some(struct_array) = array.as_any().downcast_ref::<StructArray>() {
            let key_array = struct_array
                .column_by_name("key")
                .context("Missing 'key' field in metadata struct")?
                .as_any()
                .downcast_ref::<StringArray>()
                .context("'key' field not StringArray")?;

            let value_array = struct_array
                .column_by_name("value")
                .context("Missing 'value' field in metadata struct")?
                .as_any()
                .downcast_ref::<StringArray>()
                .context("'value' field not StringArray")?;

            // Assume each row has one metadata item (or null)
            return Ok((0..num_rows)
                .map(|i| {
                    if struct_array.is_null(i) {
                        Vec::new()
                    } else {
                        use crate::proto::proximadb_v1::metadata_item::Value;
                        vec![MetadataItem {
                            key: key_array.value(i).to_string(),
                            value: Some(Value::StringValue(value_array.value(i).to_string())),
                        }]
                    }
                })
                .collect());
        }

        // Fallback: StringArray with JSON metadata
        if let Some(string_array) = array.as_any().downcast_ref::<StringArray>() {
            return (0..num_rows)
                .map(|i| {
                    if string_array.is_null(i) {
                        Ok(Vec::new())
                    } else {
                        let json_str = string_array.value(i);
                        Self::parse_metadata_json(json_str)
                    }
                })
                .collect();
        }

        Ok(vec![Vec::new(); num_rows])
    }

    /// Parse JSON metadata string
    fn parse_metadata_json(json: &str) -> Result<Vec<MetadataItem>> {
        let map: HashMap<String, serde_json::Value> = serde_json::from_str(json)?;

        Ok(map
            .into_iter()
            .map(|(key, value)| MetadataItem {
                key,
                value: Some(Self::json_value_to_metadata_value(value)),
            })
            .collect())
    }

    /// Convert serde_json::Value to metadata_item::Value
    fn json_value_to_metadata_value(
        value: serde_json::Value,
    ) -> crate::proto::proximadb_v1::metadata_item::Value {
        use crate::proto::proximadb_v1::metadata_item::Value;

        match value {
            serde_json::Value::String(s) => Value::StringValue(s),
            serde_json::Value::Number(n) => {
                if let Some(f) = n.as_f64() {
                    Value::NumberValue(f)
                } else {
                    Value::StringValue(n.to_string())
                }
            }
            serde_json::Value::Bool(b) => Value::BoolValue(b),
            _ => Value::StringValue(value.to_string()),
        }
    }

    /// Convert VectorRecords to Arrow RecordBatch (for search results)
    pub fn vector_records_to_batch(
        records: Vec<VectorRecord>,
        dimension: usize,
    ) -> Result<RecordBatch> {
        if records.is_empty() {
            return Err(anyhow::anyhow!("Cannot create batch from empty records"));
        }

        let schema = Self::create_vector_schema(dimension);

        // Build id array
        let id_array = StringArray::from_iter_values(records.iter().map(|r| r.id.as_str()));

        // Build vector array (FixedSizeList<Float32>)
        let mut vector_values = Vec::with_capacity(records.len() * dimension);
        for record in &records {
            vector_values.extend_from_slice(&record.vector);
        }
        let flat_array = Arc::new(Float32Array::from(vector_values)) as ArrayRef;
        let vector_field = Arc::new(Field::new("item", DataType::Float32, false));
        let vector_array =
            FixedSizeListArray::new(vector_field, dimension as i32, flat_array, None);

        // Build metadata array (Struct<key, value>)
        let metadata_array = Self::build_metadata_struct_array(&records)?;

        // Build timestamp array
        let timestamp_array = Int64Array::from(
            records
                .iter()
                .map(|r| r.timestamp)
                .collect::<Vec<Option<i64>>>(),
        );

        // Build score array (None for insert, Some for search)
        let score_array = Float32Array::from(vec![None; records.len()]);

        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(id_array),
                Arc::new(vector_array),
                Arc::new(metadata_array),
                Arc::new(timestamp_array),
                Arc::new(score_array),
            ],
        )
        .context("Failed to create RecordBatch")
    }

    /// Build StructArray for metadata
    fn build_metadata_struct_array(records: &[VectorRecord]) -> Result<StructArray> {
        let mut keys = Vec::new();
        let mut values = Vec::new();

        for record in records {
            if record.metadata.is_empty() {
                keys.push(None);
                values.push(None);
            } else {
                // Take first metadata item (simplification)
                if let Some((key, value)) = record.metadata.iter().next() {
                    keys.push(Some(key.as_str()));
                    values.push(Some(Self::sql_value_to_string(value)));
                } else {
                    keys.push(None);
                    values.push(None);
                }
            }
        }

        let key_array = StringArray::from(keys);
        let value_array = StringArray::from(values);

        Ok(StructArray::from(vec![
            (
                Arc::new(Field::new("key", DataType::Utf8, false)),
                Arc::new(key_array) as ArrayRef,
            ),
            (
                Arc::new(Field::new("value", DataType::Utf8, true)),
                Arc::new(value_array) as ArrayRef,
            ),
        ]))
    }

    /// Convert SqlValue to string
    fn sql_value_to_string(value: &SqlValue) -> String {
        use crate::proto::proximadb_v1::sql_value::Value;

        match &value.value {
            Some(Value::StringValue(s)) => s.clone(),
            Some(Value::Int64Value(i)) => i.to_string(),
            Some(Value::NumberValue(f)) => f.to_string(),
            Some(Value::BoolValue(b)) => b.to_string(),
            Some(Value::NullValue(_)) => "null".to_string(),
            Some(Value::BytesValue(b)) => format!("{:?}", b),
            Some(Value::ArrayValue(_)) => "[array]".to_string(),
            Some(Value::ObjectValue(_)) => "{object}".to_string(),
            None => "null".to_string(),
        }
    }

    /// Parse FlightDescriptor to extract request metadata
    pub fn parse_descriptor(descriptor: &FlightDescriptor) -> Result<FlightRequestMetadata> {
        let path = descriptor
            .path
            .first()
            .context("FlightDescriptor path is empty")?;

        let collection_id = path.clone();

        // Parse command from descriptor.cmd if present
        let (write_mode, trigger_compaction) = if !descriptor.cmd.is_empty() {
            let cmd: HashMap<String, String> = serde_json::from_slice(&descriptor.cmd)?;
            let mode = match cmd.get("write_mode").map(|s| s.as_str()) {
                Some("direct") => WriteMode::Direct,
                _ => WriteMode::WAL,
            };
            let compact = cmd
                .get("trigger_compaction")
                .and_then(|s| s.parse().ok())
                .unwrap_or(false);
            (mode, compact)
        } else {
            (WriteMode::WAL, false)
        };

        Ok(FlightRequestMetadata {
            collection_id,
            write_mode,
            trigger_compaction,
        })
    }

    /// Parse Ticket to extract search request
    pub fn ticket_to_search_request(ticket: &Ticket) -> Result<VectorSearchRequest> {
        serde_json::from_slice(&ticket.ticket).context("Failed to parse search request from ticket")
    }

    /// Convert FlightData to RecordBatch
    pub fn flight_data_to_batch(data: &FlightData) -> Result<RecordBatch> {
        arrow_flight::utils::flight_data_to_arrow_batch(
            data,
            Arc::new(Schema::empty()), // Schema sent separately
            &Default::default(),
        )
        .context("Failed to convert FlightData to RecordBatch")
    }

    /// Convert RecordBatch to FlightData
    pub fn batch_to_flight_data(
        batch: &RecordBatch,
        _options: &IpcWriteOptions,
    ) -> Result<Vec<FlightData>> {
        Ok(arrow_flight::utils::batches_to_flight_data(
            &batch.schema(),
            vec![batch.clone()],
        )?)
    }

    /// Convert RecordBatch to FlightData with compression support
    ///
    /// This method encodes a RecordBatch into Arrow IPC FlightData format,
    /// applying the specified compression codec if provided.
    ///
    /// ## Parameters
    /// - `batch`: The RecordBatch to encode
    /// - `compression`: Optional compression type (LZ4_FRAME or ZSTD)
    ///
    /// ## Returns
    /// A vector of FlightData messages including schema and data
    ///
    /// ## Example
    /// ```rust,ignore
    /// use arrow_ipc::CompressionType;
    ///
    /// let flight_data = ArrowProtoCodec::batch_to_flight_data_with_compression(
    ///     &batch,
    ///     Some(CompressionType::LZ4_FRAME),
    /// )?;
    /// ```
    pub fn batch_to_flight_data_with_compression(
        batch: &RecordBatch,
        compression: Option<arrow_ipc::CompressionType>,
    ) -> Result<Vec<FlightData>> {
        use arrow_ipc::writer::{CompressionContext, DictionaryTracker, IpcDataGenerator};

        let schema = batch.schema();

        // Create write options with compression if specified
        let options = match compression {
            Some(codec) => IpcWriteOptions::default()
                .try_with_compression(Some(codec))
                .context("Failed to configure compression")?,
            None => IpcWriteOptions::default(),
        };

        // Create schema message
        let schema_flight_data: FlightData =
            arrow_flight::SchemaAsIpc::new(&schema, &options).into();

        // Encode the batch with compression
        let data_gen = IpcDataGenerator::default();
        let mut dictionary_tracker = DictionaryTracker::new(false);
        let mut compression_context = CompressionContext::default();

        let (encoded_dictionaries, encoded_batch) = data_gen.encode(
            batch,
            &mut dictionary_tracker,
            &options,
            &mut compression_context,
        )?;

        // Build the result: schema + dictionaries + data
        let mut stream = Vec::with_capacity(1 + encoded_dictionaries.len() + 1);
        stream.push(schema_flight_data);
        stream.extend(encoded_dictionaries.into_iter().map(Into::into));
        stream.push(encoded_batch.into());

        Ok(stream)
    }

    /// Convert multiple RecordBatches to FlightData with compression support
    ///
    /// This is a convenience method for encoding multiple batches with the same
    /// compression settings. The schema is sent only once at the beginning.
    pub fn batches_to_flight_data_with_compression(
        batches: &[RecordBatch],
        compression: Option<arrow_ipc::CompressionType>,
    ) -> Result<Vec<FlightData>> {
        if batches.is_empty() {
            return Ok(Vec::new());
        }

        use arrow_ipc::writer::{CompressionContext, DictionaryTracker, IpcDataGenerator};

        let schema = batches[0].schema();

        // Create write options with compression if specified
        let options = match compression {
            Some(codec) => IpcWriteOptions::default()
                .try_with_compression(Some(codec))
                .context("Failed to configure compression")?,
            None => IpcWriteOptions::default(),
        };

        // Create schema message
        let schema_flight_data: FlightData =
            arrow_flight::SchemaAsIpc::new(&schema, &options).into();

        let data_gen = IpcDataGenerator::default();
        let mut dictionary_tracker = DictionaryTracker::new(false);
        let mut compression_context = CompressionContext::default();

        // Estimate capacity
        let mut stream = Vec::with_capacity(1 + batches.len() * 2);
        stream.push(schema_flight_data);

        // Encode each batch
        for batch in batches {
            let (encoded_dictionaries, encoded_batch) = data_gen.encode(
                batch,
                &mut dictionary_tracker,
                &options,
                &mut compression_context,
            )?;

            stream.extend(encoded_dictionaries.into_iter().map(Into::into));
            stream.push(encoded_batch.into());
        }

        Ok(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{ArrayRef, FixedSizeListArray, Float32Array, Int64Array, StringArray};
    use arrow_schema::{DataType, Field};

    #[test]
    fn test_create_vector_schema() {
        let schema = ArrowProtoCodec::create_vector_schema(384);
        assert_eq!(schema.fields().len(), 5);
        assert_eq!(schema.field(0).name(), "id");
        assert_eq!(schema.field(1).name(), "vector");
    }

    #[test]
    fn test_deserialize_vector() {
        let vec = vec![1.0f32, 2.0, 3.0];
        let mut bytes = Vec::new();
        for v in &vec {
            bytes.extend_from_slice(&v.to_le_bytes());
        }

        let result = ArrowProtoCodec::deserialize_vector(&bytes).unwrap();
        assert_eq!(result, vec);
    }

    /// Helper to create a test RecordBatch
    fn create_test_batch(num_rows: usize, dimension: i32) -> RecordBatch {
        // Create id column
        let ids: Vec<String> = (0..num_rows).map(|i| format!("id_{}", i)).collect();
        let id_array = StringArray::from(ids);

        // Create vector column
        let flat_values: Vec<f32> = (0..(num_rows * dimension as usize))
            .map(|i| (i as f32) * 0.1)
            .collect();
        let values_array = Arc::new(Float32Array::from(flat_values)) as ArrayRef;
        let vector_field = Arc::new(Field::new("item", DataType::Float32, false));
        let vector_array = FixedSizeListArray::new(vector_field, dimension, values_array, None);

        // Create timestamp column
        let timestamps: Vec<i64> = (0..num_rows).map(|i| i as i64 * 1000).collect();
        let timestamp_array = Int64Array::from(timestamps);

        // Create schema
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, false)),
                    dimension,
                ),
                false,
            ),
            Field::new("timestamp", DataType::Int64, true),
        ]));

        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(id_array),
                Arc::new(vector_array),
                Arc::new(timestamp_array),
            ],
        )
        .expect("Failed to create test batch")
    }

    #[test]
    fn test_batch_to_flight_data_no_compression() {
        let batch = create_test_batch(10, 4);
        let flight_data = ArrowProtoCodec::batch_to_flight_data_with_compression(&batch, None)
            .expect("Failed to convert batch");

        // Should have schema + data messages
        assert!(!flight_data.is_empty());
        // First message should be schema
        assert!(!flight_data[0].data_header.is_empty());
    }

    #[test]
    fn test_batch_to_flight_data_with_lz4_compression() {
        let batch = create_test_batch(100, 128);
        let flight_data = ArrowProtoCodec::batch_to_flight_data_with_compression(
            &batch,
            Some(arrow_ipc::CompressionType::LZ4_FRAME),
        )
        .expect("Failed to convert batch with LZ4");

        assert!(!flight_data.is_empty());
    }

    #[test]
    fn test_batch_to_flight_data_with_zstd_compression() {
        let batch = create_test_batch(100, 128);
        let flight_data = ArrowProtoCodec::batch_to_flight_data_with_compression(
            &batch,
            Some(arrow_ipc::CompressionType::ZSTD),
        )
        .expect("Failed to convert batch with ZSTD");

        assert!(!flight_data.is_empty());
    }

    #[test]
    fn test_batches_to_flight_data_empty() {
        let flight_data = ArrowProtoCodec::batches_to_flight_data_with_compression(&[], None)
            .expect("Failed to convert empty batches");

        assert!(flight_data.is_empty());
    }

    #[test]
    fn test_batches_to_flight_data_multiple_batches() {
        let batch1 = create_test_batch(50, 64);
        let batch2 = create_test_batch(30, 64);
        let batches = vec![batch1, batch2];

        let flight_data = ArrowProtoCodec::batches_to_flight_data_with_compression(
            &batches,
            Some(arrow_ipc::CompressionType::LZ4_FRAME),
        )
        .expect("Failed to convert batches");

        // Should have schema + data for each batch
        assert!(flight_data.len() >= 3); // At least schema + 2 data messages
    }

    #[test]
    fn test_flight_ticket_serialization() {
        // Create a VectorSearchRequest and serialize it into a Ticket
        let search_request = VectorSearchRequest {
            collection_id: "test_collection".to_string(),
            top_k: 10,
            ..Default::default()
        };

        let ticket_bytes =
            serde_json::to_vec(&search_request).expect("Failed to serialize search request");
        let ticket = Ticket {
            ticket: ticket_bytes.into(),
        };

        // Deserialize back
        let parsed = ArrowProtoCodec::ticket_to_search_request(&ticket)
            .expect("Failed to parse ticket back to search request");
        assert_eq!(parsed.collection_id, "test_collection");
        assert_eq!(parsed.top_k, 10);
    }

    #[test]
    fn test_flight_descriptor_creation() {
        // Create a FlightDescriptor with a collection path and parse it
        let descriptor = FlightDescriptor {
            r#type: 0,
            path: vec!["my_collection".to_string()],
            cmd: Default::default(),
        };

        let metadata =
            ArrowProtoCodec::parse_descriptor(&descriptor).expect("Failed to parse descriptor");
        assert_eq!(metadata.collection_id, "my_collection");
        assert_eq!(metadata.write_mode, WriteMode::WAL);
        assert!(!metadata.trigger_compaction);
    }

    #[test]
    fn test_arrow_batch_to_vector_records() {
        // Create a test batch with known data and convert to VectorRecords
        let batch = create_test_batch(3, 4);

        let records = ArrowProtoCodec::batch_to_vector_records(&batch)
            .expect("Failed to convert batch to records");

        assert_eq!(records.len(), 3);
        assert_eq!(records[0].id, "id_0");
        assert_eq!(records[1].id, "id_1");
        assert_eq!(records[2].id, "id_2");

        // Verify vector dimensions
        assert_eq!(records[0].vector.len(), 4);
        assert_eq!(records[1].vector.len(), 4);

        // Verify timestamps
        assert_eq!(records[0].timestamp, Some(0));
        assert_eq!(records[1].timestamp, Some(1000));
        assert_eq!(records[2].timestamp, Some(2000));
    }

    #[test]
    fn test_vector_records_to_arrow_batch() {
        use crate::proto::proximadb_v1::sql_value::Value as SqlVal;

        let mut meta_a = HashMap::new();
        meta_a.insert(
            "color".to_string(),
            SqlValue {
                value: Some(SqlVal::StringValue("red".to_string())),
            },
        );
        let mut meta_b = HashMap::new();
        meta_b.insert(
            "color".to_string(),
            SqlValue {
                value: Some(SqlVal::StringValue("blue".to_string())),
            },
        );

        let records = vec![
            VectorRecord {
                id: "vec_a".to_string(),
                vector: vec![1.0, 2.0, 3.0],
                metadata: meta_a,
                timestamp: Some(100),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            },
            VectorRecord {
                id: "vec_b".to_string(),
                vector: vec![4.0, 5.0, 6.0],
                metadata: meta_b,
                timestamp: Some(200),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            },
        ];

        let batch = ArrowProtoCodec::vector_records_to_batch(records, 3)
            .expect("Failed to convert records to batch");

        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.num_columns(), 5); // id, vector, metadata, timestamp, score

        // Verify id column
        let id_col = batch
            .column_by_name("id")
            .expect("Missing id column")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("id is not StringArray");
        assert_eq!(id_col.value(0), "vec_a");
        assert_eq!(id_col.value(1), "vec_b");
    }

    #[test]
    fn test_multimodel_codec_vector() {
        // Test round-trip: VectorRecords -> RecordBatch -> VectorRecords
        use crate::proto::proximadb_v1::sql_value::Value as SqlVal;

        let mut meta_1 = HashMap::new();
        meta_1.insert(
            "tag".to_string(),
            SqlValue {
                value: Some(SqlVal::StringValue("alpha".to_string())),
            },
        );
        let mut meta_2 = HashMap::new();
        meta_2.insert(
            "tag".to_string(),
            SqlValue {
                value: Some(SqlVal::StringValue("beta".to_string())),
            },
        );

        let original_records = vec![
            VectorRecord {
                id: "rt_1".to_string(),
                vector: vec![0.5, 1.5, 2.5, 3.5],
                metadata: meta_1,
                timestamp: Some(1000),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            },
            VectorRecord {
                id: "rt_2".to_string(),
                vector: vec![4.5, 5.5, 6.5, 7.5],
                metadata: meta_2,
                timestamp: Some(2000),
                updated_at: None,
                expires_at: None,
                version: None,
                source: None,
            },
        ];

        // Encode to batch
        let batch = ArrowProtoCodec::vector_records_to_batch(original_records.clone(), 4)
            .expect("Failed to encode to batch");

        // Decode back
        let decoded =
            ArrowProtoCodec::batch_to_vector_records(&batch).expect("Failed to decode from batch");

        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].id, "rt_1");
        assert_eq!(decoded[1].id, "rt_2");
        assert_eq!(decoded[0].vector, vec![0.5, 1.5, 2.5, 3.5]);
        assert_eq!(decoded[1].vector, vec![4.5, 5.5, 6.5, 7.5]);
    }

    #[test]
    fn test_multimodel_codec_document() {
        // Test document schema from multimodel_codec and verify field types
        use crate::network::arrow_ipc::multimodel_codec::document_schema;

        let schema = document_schema();
        assert_eq!(schema.fields().len(), 5);

        // Verify round-trip: build a RecordBatch from document schema, read it back
        let id_array = StringArray::from(vec!["doc_1", "doc_2"]);
        let doc_array = StringArray::from(vec![r#"{"title":"Hello"}"#, r#"{"title":"World"}"#]);
        let version_array = Int64Array::from(vec![1i64, 2]);
        let collection_array = StringArray::from(vec!["col_a", "col_a"]);
        let updated_array = Int64Array::from(vec![1000i64, 2000]);

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(id_array) as ArrayRef,
                Arc::new(doc_array) as ArrayRef,
                Arc::new(version_array) as ArrayRef,
                Arc::new(collection_array) as ArrayRef,
                Arc::new(updated_array) as ArrayRef,
            ],
        )
        .expect("Failed to create document batch");

        assert_eq!(batch.num_rows(), 2);

        let ids = batch
            .column_by_name("id")
            .expect("Missing id")
            .as_any()
            .downcast_ref::<StringArray>()
            .expect("id not StringArray");
        assert_eq!(ids.value(0), "doc_1");
        assert_eq!(ids.value(1), "doc_2");
    }

    #[test]
    fn test_flight_info_metadata() {
        // Verify that FlightDescriptor with command metadata is parsed correctly
        let cmd = serde_json::json!({
            "write_mode": "direct",
            "trigger_compaction": "true"
        });

        let descriptor = FlightDescriptor {
            r#type: 0,
            path: vec!["high_perf_collection".to_string()],
            cmd: serde_json::to_vec(&cmd)
                .expect("Failed to serialize cmd")
                .into(),
        };

        let metadata = ArrowProtoCodec::parse_descriptor(&descriptor)
            .expect("Failed to parse descriptor with metadata");
        assert_eq!(metadata.collection_id, "high_perf_collection");
        assert_eq!(metadata.write_mode, WriteMode::Direct);
        assert!(metadata.trigger_compaction);
    }

    #[test]
    fn test_empty_batch_handling() {
        // Verify empty records produce an error (not a panic)
        let result = ArrowProtoCodec::vector_records_to_batch(vec![], 128);
        assert!(result.is_err(), "Empty records should return an error");

        // Verify empty batch list conversion works without panic
        let result = ArrowProtoCodec::batches_to_vector_records(vec![]);
        assert!(result.is_ok());
        assert!(result.expect("Should succeed for empty batches").is_empty());

        // Verify empty batch list to flight data doesn't panic
        let result = ArrowProtoCodec::batches_to_flight_data_with_compression(&[], None);
        assert!(result.is_ok());
        assert!(
            result
                .expect("Should succeed for empty flight data")
                .is_empty()
        );
    }

    #[test]
    fn test_compression_reduces_size() {
        // Create a batch with repetitive data that compresses well
        let num_rows = 1000;
        let dimension = 128;

        // Create highly compressible data (all zeros)
        let ids: Vec<String> = (0..num_rows).map(|i| format!("id_{}", i)).collect();
        let id_array = StringArray::from(ids);

        let flat_values: Vec<f32> = vec![0.0f32; num_rows * dimension];
        let values_array = Arc::new(Float32Array::from(flat_values)) as ArrayRef;
        let vector_field = Arc::new(Field::new("item", DataType::Float32, false));
        let vector_array =
            FixedSizeListArray::new(vector_field, dimension as i32, values_array, None);

        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, false)),
                    dimension as i32,
                ),
                false,
            ),
        ]));

        let batch = RecordBatch::try_new(schema, vec![Arc::new(id_array), Arc::new(vector_array)])
            .expect("Failed to create batch");

        // Get uncompressed size
        let uncompressed =
            ArrowProtoCodec::batches_to_flight_data_with_compression(&[batch.clone()], None)
                .expect("Failed uncompressed");

        // Get compressed size
        let compressed = ArrowProtoCodec::batches_to_flight_data_with_compression(
            &[batch],
            Some(arrow_ipc::CompressionType::LZ4_FRAME),
        )
        .expect("Failed compressed");

        // Calculate total data size
        let uncompressed_size: usize = uncompressed.iter().map(|fd| fd.data_body.len()).sum();
        let compressed_size: usize = compressed.iter().map(|fd| fd.data_body.len()).sum();

        // Compressed should be smaller for highly repetitive data
        // Note: For very small data, compression overhead might make it larger
        if uncompressed_size > 1024 {
            assert!(
                compressed_size <= uncompressed_size,
                "Compressed ({}) should be <= uncompressed ({}) for repetitive data",
                compressed_size,
                uncompressed_size
            );
        }
    }
}
