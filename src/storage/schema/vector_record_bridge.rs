//! # VectorRecordBridge - VectorRecord to Arrow RecordBatch Conversion
//!
//! Provides zero-copy conversion between VectorRecord (proto type) and Arrow RecordBatch.
//! This bridge enables seamless integration between ProximaDB's proto-first architecture
//! and the Arrow-native compute layer.
//!
//! ## Key Features
//!
//! - Zero-copy conversion where possible using Arrow's memory format
//! - Support for nested metadata fields (JSON -> Arrow Struct)
//! - Schema inference from VectorRecord metadata
//! - Batch conversion utilities for efficient bulk operations
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::storage::schema::vector_record_bridge::{
//!     VectorRecordBridge, DefaultVectorRecordBridge
//! };
//!
//! let bridge = DefaultVectorRecordBridge::new(schema);
//! let batch = bridge.records_to_batch(&records)?;
//! let records = bridge.batch_to_records(&batch)?;
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use arrow_array::{
    Array, ArrayRef, BooleanArray, FixedSizeListArray, Float32Array, Float64Array, Int64Array,
    RecordBatch, StringArray, StructArray,
    builder::{
        BooleanBuilder, FixedSizeListBuilder, Float32Builder, Float64Builder, Int64Builder,
        StringBuilder,
    },
};
use arrow_schema::{DataType, Field, Schema as ArrowSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use super::proxima_schema::{ProximaColumn, ProximaDataType, ProximaSchema, VectorElementType};
use crate::proto::proximadb_v1::sql_value::Value as ProtoSqlValueInner;
use crate::proto::proximadb_v1::{SqlValue, VectorRecord};
use crate::storage::formats::arrow_conversion::{json_to_sql_value, sql_value_to_json};

// ============================================================================
// VectorRecordBridge Trait
// ============================================================================

/// Trait for converting between VectorRecord and Arrow RecordBatch.
///
/// Implementations must ensure:
/// - Zero-copy conversion where possible
/// - Proper handling of nullable fields
/// - Correct metadata serialization/deserialization
/// - Schema compatibility validation
pub trait VectorRecordBridge: Send + Sync {
    /// Get the schema used for conversions.
    fn schema(&self) -> &ProximaSchema;

    /// Convert a slice of VectorRecords to an Arrow RecordBatch.
    ///
    /// # Arguments
    /// * `records` - Vector records to convert
    ///
    /// # Returns
    /// * Arrow RecordBatch containing all records
    fn records_to_batch(&self, records: &[VectorRecord]) -> Result<RecordBatch>;

    /// Convert an Arrow RecordBatch to VectorRecords.
    ///
    /// # Arguments
    /// * `batch` - Arrow RecordBatch to convert
    ///
    /// # Returns
    /// * Vec of VectorRecords extracted from the batch
    fn batch_to_records(&self, batch: &RecordBatch) -> Result<Vec<VectorRecord>>;

    /// Infer schema from a set of VectorRecords.
    ///
    /// Analyzes the metadata fields across records to determine
    /// the optimal schema for the collection.
    ///
    /// # Arguments
    /// * `records` - Sample records to analyze
    ///
    /// # Returns
    /// * Inferred ProximaSchema
    fn infer_schema_from_records(&self, records: &[VectorRecord]) -> Result<ProximaSchema>;

    /// Validate that records are compatible with the current schema.
    ///
    /// # Arguments
    /// * `records` - Records to validate
    ///
    /// # Returns
    /// * Ok(()) if compatible, Err with details otherwise
    fn validate_records(&self, records: &[VectorRecord]) -> Result<()>;
}

// ============================================================================
// DefaultVectorRecordBridge Implementation
// ============================================================================

/// Default implementation of VectorRecordBridge.
///
/// Uses the standard ProximaSchema for conversions and supports
/// both flat metadata (JSON string) and structured metadata (Arrow Struct).
#[derive(Debug, Clone)]
pub struct DefaultVectorRecordBridge {
    /// Schema for conversions
    schema: ProximaSchema,
    /// Metadata handling mode
    metadata_mode: MetadataMode,
    /// Whether to include vector data in conversions
    include_vectors: bool,
}

/// Mode for handling metadata in conversions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataMode {
    /// Store metadata as JSON string (simple, flexible)
    JsonString,
    /// Store metadata as Arrow Struct (typed, efficient)
    ArrowStruct,
    /// Auto-detect based on schema
    Auto,
}

impl Default for MetadataMode {
    fn default() -> Self {
        Self::JsonString
    }
}

impl DefaultVectorRecordBridge {
    /// Create a new bridge with the given schema.
    pub fn new(schema: ProximaSchema) -> Self {
        Self {
            schema,
            metadata_mode: MetadataMode::default(),
            include_vectors: true,
        }
    }

    /// Create a bridge with custom metadata mode.
    pub fn with_metadata_mode(mut self, mode: MetadataMode) -> Self {
        self.metadata_mode = mode;
        self
    }

    /// Create a bridge that excludes vector data (metadata-only queries).
    pub fn without_vectors(mut self) -> Self {
        self.include_vectors = false;
        self
    }

    /// Create a legacy VectorRecord bridge for backward compatibility.
    pub fn legacy(dimension: u32) -> Self {
        Self::new(ProximaSchema::vector_record_schema(dimension))
    }

    /// Get the vector dimension from the schema.
    fn vector_dimension(&self) -> Option<u32> {
        self.schema.vector_dimension()
    }

    /// Build ID array from records.
    fn build_id_array(&self, records: &[VectorRecord]) -> ArrayRef {
        Arc::new(StringArray::from(
            records.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ))
    }

    /// Build vector array from records.
    fn build_vector_array(&self, records: &[VectorRecord]) -> Result<ArrayRef> {
        let dimension = self.vector_dimension().unwrap_or(0) as usize;
        if dimension == 0 {
            return Err(anyhow!("Schema has no vector column"));
        }

        // Use FixedSizeListArray so each row has exactly one vector
        let values_builder = Float32Builder::with_capacity(records.len() * dimension);
        let mut builder = FixedSizeListBuilder::new(values_builder, dimension as i32);

        for record in records {
            if record.vector.len() != dimension {
                return Err(anyhow!(
                    "Vector dimension mismatch: expected {}, got {}",
                    dimension,
                    record.vector.len()
                ));
            }
            let values = builder.values();
            for &v in &record.vector {
                values.append_value(v);
            }
            builder.append(true);
        }

        Ok(Arc::new(builder.finish()))
    }

    /// Build metadata array from records based on mode.
    fn build_metadata_array(&self, records: &[VectorRecord]) -> Result<ArrayRef> {
        match self.metadata_mode {
            MetadataMode::JsonString | MetadataMode::Auto => {
                self.build_metadata_json_array(records)
            }
            MetadataMode::ArrowStruct => self.build_metadata_struct_array(records),
        }
    }

    /// Build metadata as JSON string array.
    fn build_metadata_json_array(&self, records: &[VectorRecord]) -> Result<ArrayRef> {
        let mut builder = StringBuilder::new();

        for record in records {
            if record.metadata.is_empty() {
                builder.append_null();
            } else {
                let json_map: HashMap<String, JsonValue> = record
                    .metadata
                    .iter()
                    .map(|(k, v)| (k.clone(), sql_value_to_json(v)))
                    .collect();
                let json_str = serde_json::to_string(&json_map)
                    .context("Failed to serialize metadata to JSON")?;
                builder.append_value(&json_str);
            }
        }

        Ok(Arc::new(builder.finish()))
    }

    /// Build metadata as Arrow Struct array.
    fn build_metadata_struct_array(&self, records: &[VectorRecord]) -> Result<ArrayRef> {
        // Collect all unique metadata keys and infer types
        let inferred = self.infer_metadata_schema(records)?;

        if inferred.is_empty() {
            // No metadata - return null array
            return self.build_metadata_json_array(records);
        }

        // Build arrays for each field
        let mut field_arrays: Vec<(Field, ArrayRef)> = Vec::new();

        for (key, data_type) in &inferred {
            let array = self.build_typed_metadata_field(records, key, data_type)?;
            let field = Field::new(key, data_type.clone(), true);
            field_arrays.push((field, array));
        }

        // Create struct array
        let fields: Vec<Field> = field_arrays.iter().map(|(f, _)| f.clone()).collect();
        let arrays: Vec<ArrayRef> = field_arrays.into_iter().map(|(_, a)| a).collect();

        let struct_array = StructArray::try_new(
            fields.into(),
            arrays,
            None, // No validity bitmap - all rows valid
        )?;

        Ok(Arc::new(struct_array))
    }

    /// Infer metadata schema from records.
    fn infer_metadata_schema(&self, records: &[VectorRecord]) -> Result<Vec<(String, DataType)>> {
        let mut schema: HashMap<String, DataType> = HashMap::new();

        for record in records {
            for (key, value) in &record.metadata {
                let dtype = self.infer_arrow_type_from_sql_value(value);
                schema.entry(key.clone()).or_insert(dtype);
            }
        }

        let mut result: Vec<_> = schema.into_iter().collect();
        result.sort_by(|a, b| a.0.cmp(&b.0)); // Stable ordering
        Ok(result)
    }

    /// Infer Arrow DataType from SqlValue.
    fn infer_arrow_type_from_sql_value(&self, value: &SqlValue) -> DataType {
        match &value.value {
            None => DataType::Null,
            Some(inner) => match inner {
                ProtoSqlValueInner::NullValue(_) => DataType::Null,
                ProtoSqlValueInner::BoolValue(_) => DataType::Boolean,
                ProtoSqlValueInner::Int64Value(_) => DataType::Int64,
                ProtoSqlValueInner::NumberValue(_) => DataType::Float64,
                ProtoSqlValueInner::StringValue(_) => DataType::Utf8,
                ProtoSqlValueInner::BytesValue(_) => DataType::Binary,
                ProtoSqlValueInner::ArrayValue(_) => DataType::Utf8, // Store as JSON
                ProtoSqlValueInner::ObjectValue(_) => DataType::Utf8, // Store as JSON
            },
        }
    }

    /// Build a typed metadata field array.
    fn build_typed_metadata_field(
        &self,
        records: &[VectorRecord],
        key: &str,
        data_type: &DataType,
    ) -> Result<ArrayRef> {
        let num_records = records.len();

        match data_type {
            DataType::Boolean => {
                let mut builder = BooleanBuilder::with_capacity(num_records);
                for record in records {
                    if let Some(value) = record.metadata.get(key) {
                        match &value.value {
                            Some(ProtoSqlValueInner::BoolValue(b)) => builder.append_value(*b),
                            _ => builder.append_null(),
                        }
                    } else {
                        builder.append_null();
                    }
                }
                Ok(Arc::new(builder.finish()))
            }
            DataType::Int64 => {
                let mut builder = Int64Builder::with_capacity(num_records);
                for record in records {
                    if let Some(value) = record.metadata.get(key) {
                        match &value.value {
                            Some(ProtoSqlValueInner::Int64Value(i)) => builder.append_value(*i),
                            _ => builder.append_null(),
                        }
                    } else {
                        builder.append_null();
                    }
                }
                Ok(Arc::new(builder.finish()))
            }
            DataType::Float64 => {
                let mut builder = Float64Builder::with_capacity(num_records);
                for record in records {
                    if let Some(value) = record.metadata.get(key) {
                        match &value.value {
                            Some(ProtoSqlValueInner::NumberValue(f)) => builder.append_value(*f),
                            _ => builder.append_null(),
                        }
                    } else {
                        builder.append_null();
                    }
                }
                Ok(Arc::new(builder.finish()))
            }
            DataType::Utf8 | _ => {
                // Default to string for all other types
                let mut builder = StringBuilder::new();
                for record in records {
                    if let Some(value) = record.metadata.get(key) {
                        let json = sql_value_to_json(value);
                        match json {
                            JsonValue::Null => builder.append_null(),
                            JsonValue::String(s) => builder.append_value(&s),
                            other => builder.append_value(other.to_string()),
                        }
                    } else {
                        builder.append_null();
                    }
                }
                Ok(Arc::new(builder.finish()))
            }
        }
    }

    /// Build timestamp array from records.
    fn build_timestamp_array(&self, records: &[VectorRecord]) -> ArrayRef {
        let timestamps: Vec<Option<i64>> = records
            .iter()
            .map(|r| r.timestamp.or(Some(chrono::Utc::now().timestamp_millis())))
            .collect();
        Arc::new(Int64Array::from(timestamps))
    }

    /// Build version array from records.
    fn build_version_array(&self, records: &[VectorRecord]) -> ArrayRef {
        let versions: Vec<Option<i64>> = records
            .iter()
            .map(|r| r.version.map(|v| v as i64))
            .collect();
        Arc::new(Int64Array::from(versions))
    }

    /// Extract ID from batch at given row.
    fn extract_id(&self, batch: &RecordBatch, row: usize) -> Result<String> {
        let id_col = batch
            .column_by_name("id")
            .ok_or_else(|| anyhow!("Missing 'id' column"))?;
        let id_array = id_col
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| anyhow!("'id' column is not StringArray"))?;

        Ok(id_array.value(row).to_string())
    }

    /// Extract vector from batch at given row.
    fn extract_vector(&self, batch: &RecordBatch, row: usize) -> Result<Vec<f32>> {
        let dimension = self.vector_dimension().unwrap_or(0) as usize;
        if dimension == 0 || !self.include_vectors {
            return Ok(vec![]);
        }

        let vector_col = batch
            .column_by_name("vector")
            .ok_or_else(|| anyhow!("Missing 'vector' column"))?;

        // Handle FixedSizeListArray (preferred format)
        if let Some(list_array) = vector_col.as_any().downcast_ref::<FixedSizeListArray>() {
            let values = list_array.value(row);
            if let Some(float_array) = values.as_any().downcast_ref::<Float32Array>() {
                return Ok(float_array.values().to_vec());
            }
        }

        // Handle flat Float32Array (legacy format - stored as contiguous memory)
        if let Some(float_array) = vector_col.as_any().downcast_ref::<Float32Array>() {
            let start = row * dimension;
            let end = start + dimension;
            if end <= float_array.len() {
                return Ok(float_array.values()[start..end].to_vec());
            }
        }

        // Handle FixedSizeBinary (vector stored as binary)
        if let Some(binary_array) = vector_col
            .as_any()
            .downcast_ref::<arrow_array::FixedSizeBinaryArray>()
        {
            let bytes = binary_array.value(row);
            // Convert bytes to f32 slice
            if bytes.len() == dimension * 4 {
                let floats: &[f32] = bytemuck::cast_slice(bytes);
                return Ok(floats.to_vec());
            }
        }

        Err(anyhow!("Could not extract vector from batch"))
    }

    /// Extract metadata from batch at given row.
    fn extract_metadata(
        &self,
        batch: &RecordBatch,
        row: usize,
    ) -> Result<HashMap<String, SqlValue>> {
        let metadata_col = match batch.column_by_name("metadata") {
            Some(col) => col,
            None => return Ok(HashMap::new()),
        };

        // Try StringArray (JSON mode)
        if let Some(string_array) = metadata_col.as_any().downcast_ref::<StringArray>() {
            if string_array.is_null(row) {
                return Ok(HashMap::new());
            }
            let json_str = string_array.value(row);
            let json_map: HashMap<String, JsonValue> =
                serde_json::from_str(json_str).unwrap_or_default();
            return Ok(json_map
                .into_iter()
                .map(|(k, v)| (k, json_to_sql_value(&v)))
                .collect());
        }

        // Try StructArray (structured mode)
        if let Some(struct_array) = metadata_col.as_any().downcast_ref::<StructArray>() {
            return self.extract_metadata_from_struct(struct_array, row);
        }

        Ok(HashMap::new())
    }

    /// Extract metadata from a StructArray.
    fn extract_metadata_from_struct(
        &self,
        struct_array: &StructArray,
        row: usize,
    ) -> Result<HashMap<String, SqlValue>> {
        let mut metadata = HashMap::new();

        for (i, field) in struct_array.fields().iter().enumerate() {
            let column = struct_array.column(i);
            let value = self.extract_field_value(column.as_ref(), row, field.data_type())?;
            if let Some(v) = value {
                metadata.insert(field.name().clone(), v);
            }
        }

        Ok(metadata)
    }

    /// Extract a single field value and convert to SqlValue.
    fn extract_field_value(
        &self,
        array: &dyn Array,
        row: usize,
        _data_type: &DataType,
    ) -> Result<Option<SqlValue>> {
        if array.is_null(row) {
            return Ok(None);
        }

        // Boolean
        if let Some(arr) = array.as_any().downcast_ref::<BooleanArray>() {
            return Ok(Some(SqlValue {
                value: Some(ProtoSqlValueInner::BoolValue(arr.value(row))),
            }));
        }

        // Int64
        if let Some(arr) = array.as_any().downcast_ref::<Int64Array>() {
            return Ok(Some(SqlValue {
                value: Some(ProtoSqlValueInner::Int64Value(arr.value(row))),
            }));
        }

        // Float64
        if let Some(arr) = array.as_any().downcast_ref::<Float64Array>() {
            return Ok(Some(SqlValue {
                value: Some(ProtoSqlValueInner::NumberValue(arr.value(row))),
            }));
        }

        // String
        if let Some(arr) = array.as_any().downcast_ref::<StringArray>() {
            return Ok(Some(SqlValue {
                value: Some(ProtoSqlValueInner::StringValue(arr.value(row).to_string())),
            }));
        }

        Ok(None)
    }

    /// Extract timestamp from batch at given row.
    fn extract_timestamp(&self, batch: &RecordBatch, row: usize) -> Option<i64> {
        batch
            .column_by_name("timestamp")
            .and_then(|col| col.as_any().downcast_ref::<Int64Array>())
            .and_then(|arr| {
                if arr.is_null(row) {
                    None
                } else {
                    Some(arr.value(row))
                }
            })
    }

    /// Extract version from batch at given row.
    fn extract_version(&self, batch: &RecordBatch, row: usize) -> Option<u32> {
        batch
            .column_by_name("version")
            .and_then(|col| col.as_any().downcast_ref::<Int64Array>())
            .and_then(|arr| {
                if arr.is_null(row) {
                    None
                } else {
                    Some(arr.value(row) as u32)
                }
            })
    }
}

impl VectorRecordBridge for DefaultVectorRecordBridge {
    fn schema(&self) -> &ProximaSchema {
        &self.schema
    }

    fn records_to_batch(&self, records: &[VectorRecord]) -> Result<RecordBatch> {
        if records.is_empty() {
            return Err(anyhow!("Cannot create RecordBatch from empty records"));
        }

        // Build arrays for each column in the schema
        let id_array = self.build_id_array(records);
        let metadata_array = self.build_metadata_array(records)?;
        let timestamp_array = self.build_timestamp_array(records);
        let version_array = self.build_version_array(records);

        // Build schema and columns
        let mut fields = vec![Field::new("id", DataType::Utf8, false)];
        let mut columns: Vec<ArrayRef> = vec![id_array];

        // Add vector column if present in schema and requested
        if self.include_vectors && self.vector_dimension().is_some() {
            let dimension = self.vector_dimension().unwrap() as i32;

            // Store as FixedSizeListArray - each row contains one vector
            let vector_array = self.build_vector_array(records)?;

            // Use the array's actual data type to ensure schema matches
            fields.push(Field::new(
                "vector",
                vector_array.data_type().clone(),
                false,
            ));
            columns.push(vector_array);
        }

        // Add metadata column
        let metadata_dtype = if matches!(self.metadata_mode, MetadataMode::ArrowStruct) {
            metadata_array.data_type().clone()
        } else {
            DataType::Utf8
        };
        fields.push(Field::new("metadata", metadata_dtype, true));
        columns.push(metadata_array);

        // Add timestamp
        fields.push(Field::new("timestamp", DataType::Int64, true));
        columns.push(timestamp_array);

        // Add version
        fields.push(Field::new("version", DataType::Int64, true));
        columns.push(version_array);

        // Validate all columns have same length
        let num_records = records.len();
        for (i, (field, col)) in fields.iter().zip(columns.iter()).enumerate() {
            if col.len() != num_records {
                return Err(anyhow!(
                    "Column {} ({}) has wrong length: expected {}, got {}",
                    i,
                    field.name(),
                    num_records,
                    col.len()
                ));
            }
        }

        let arrow_schema = Arc::new(ArrowSchema::new(fields));
        RecordBatch::try_new(arrow_schema, columns)
            .context("Failed to create RecordBatch from VectorRecords")
    }

    fn batch_to_records(&self, batch: &RecordBatch) -> Result<Vec<VectorRecord>> {
        let num_rows = batch.num_rows();
        let mut records = Vec::with_capacity(num_rows);

        for row in 0..num_rows {
            let id = self.extract_id(batch, row)?;
            let vector = self.extract_vector(batch, row)?;
            let metadata = self.extract_metadata(batch, row)?;
            let timestamp = self.extract_timestamp(batch, row);
            let version = self.extract_version(batch, row);

            records.push(VectorRecord {
                id,
                vector,
                metadata,
                timestamp,
                version,
                ..Default::default()
            });
        }

        Ok(records)
    }

    fn infer_schema_from_records(&self, records: &[VectorRecord]) -> Result<ProximaSchema> {
        if records.is_empty() {
            return Err(anyhow!("Cannot infer schema from empty records"));
        }

        // Determine vector dimension
        let dimension = records.iter().map(|r| r.vector.len()).max().unwrap_or(0) as u32;

        if dimension == 0 {
            return Err(anyhow!("No vectors found in records"));
        }

        // Infer metadata schema
        let metadata_schema = self.infer_metadata_schema(records)?;

        // Build ProximaSchema
        let metadata_columns: Vec<(String, ProximaDataType)> = metadata_schema
            .into_iter()
            .map(|(name, arrow_type)| {
                let proxima_type = ProximaDataType::from_arrow_type(&arrow_type);
                (name, proxima_type)
            })
            .collect();

        let schema = ProximaSchema::with_metadata_columns(
            format!("inferred_{}", chrono::Utc::now().timestamp_millis()),
            dimension,
            metadata_columns,
        );

        Ok(schema)
    }

    fn validate_records(&self, records: &[VectorRecord]) -> Result<()> {
        let expected_dim = self.vector_dimension();

        for (i, record) in records.iter().enumerate() {
            // Validate vector dimension
            if let Some(dim) = expected_dim {
                if record.vector.len() != dim as usize {
                    return Err(anyhow!(
                        "Record {} has wrong vector dimension: expected {}, got {}",
                        i,
                        dim,
                        record.vector.len()
                    ));
                }
            }

            // Validate ID is not empty
            if record.id.is_empty() {
                return Err(anyhow!("Record {} has empty ID", i));
            }
        }

        Ok(())
    }
}

// ============================================================================
// Schema Inference Utilities
// ============================================================================

/// Infer ProximaSchema from VectorRecord samples.
///
/// Analyzes metadata fields to determine optimal typing.
pub fn infer_schema_from_vector_records(
    records: &[VectorRecord],
    schema_id: String,
) -> Result<ProximaSchema> {
    if records.is_empty() {
        return Err(anyhow!("Cannot infer schema from empty records"));
    }

    let dimension = records.iter().map(|r| r.vector.len()).max().unwrap_or(0) as u32;

    if dimension == 0 {
        return Err(anyhow!("No vectors found in records"));
    }

    // Collect metadata field types
    let mut field_types: HashMap<String, ProximaDataType> = HashMap::new();

    for record in records {
        for (key, value) in &record.metadata {
            let inferred_type = infer_proxima_type_from_sql_value(value);
            field_types.entry(key.clone()).or_insert(inferred_type);
        }
    }

    // Convert to ordered list
    let mut metadata_fields: Vec<(String, ProximaDataType)> = field_types.into_iter().collect();
    metadata_fields.sort_by(|a, b| a.0.cmp(&b.0));

    Ok(ProximaSchema::with_metadata_columns(
        schema_id,
        dimension,
        metadata_fields,
    ))
}

/// Infer ProximaDataType from SqlValue.
fn infer_proxima_type_from_sql_value(value: &SqlValue) -> ProximaDataType {
    match &value.value {
        None => ProximaDataType::String, // Default to string for null
        Some(inner) => match inner {
            ProtoSqlValueInner::NullValue(_) => ProximaDataType::String,
            ProtoSqlValueInner::BoolValue(_) => ProximaDataType::Boolean,
            ProtoSqlValueInner::Int64Value(_) => ProximaDataType::Int64,
            ProtoSqlValueInner::NumberValue(_) => ProximaDataType::Float64,
            ProtoSqlValueInner::StringValue(_) => ProximaDataType::String,
            ProtoSqlValueInner::BytesValue(_) => ProximaDataType::Binary,
            ProtoSqlValueInner::ArrayValue(_) => ProximaDataType::Json,
            ProtoSqlValueInner::ObjectValue(_) => ProximaDataType::Json,
        },
    }
}

// ============================================================================
// Avro-Style Schema Serialization
// ============================================================================

/// Avro-style schema representation for ProximaSchema.
///
/// Enables schema serialization compatible with schema registries
/// and provides stable schema fingerprinting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvroStyleSchema {
    /// Schema type (always "record" for ProximaSchema)
    #[serde(rename = "type")]
    pub schema_type: String,
    /// Namespace (collection identifier)
    pub namespace: String,
    /// Schema name
    pub name: String,
    /// Schema fields
    pub fields: Vec<AvroStyleField>,
    /// Optional aliases
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aliases: Option<Vec<String>>,
    /// Custom metadata
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub metadata: HashMap<String, String>,
}

/// Avro-style field representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvroStyleField {
    /// Field name
    pub name: String,
    /// Field type (Avro type union for nullable)
    #[serde(rename = "type")]
    pub field_type: AvroStyleType,
    /// Optional default value
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    /// Optional documentation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    /// Original column ID (ProximaDB extension)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column_id: Option<i32>,
}

/// Avro-style type representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AvroStyleType {
    /// Simple type name
    Simple(String),
    /// Union type (for nullable)
    Union(Vec<String>),
    /// Complex type with properties
    Complex {
        #[serde(rename = "type")]
        type_name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        items: Option<Box<AvroStyleType>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        values: Option<Box<AvroStyleType>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        dimension: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        precision: Option<u8>,
        #[serde(skip_serializing_if = "Option::is_none")]
        scale: Option<i8>,
    },
}

impl ProximaSchema {
    /// Serialize to Avro-style schema format.
    pub fn to_avro_style(&self) -> AvroStyleSchema {
        let fields: Vec<AvroStyleField> = self
            .columns
            .iter()
            .filter(|c| !c.is_deleted)
            .map(|col| col.to_avro_field())
            .collect();

        AvroStyleSchema {
            schema_type: "record".to_string(),
            namespace: "com.proximadb".to_string(),
            name: self.schema_id.clone(),
            fields,
            aliases: None,
            metadata: self.metadata.clone(),
        }
    }

    /// Deserialize from Avro-style schema format.
    pub fn from_avro_style(avro: &AvroStyleSchema) -> Result<Self> {
        let columns: Vec<ProximaColumn> = avro
            .fields
            .iter()
            .enumerate()
            .map(|(idx, field)| field.to_proxima_column((idx + 1) as i32))
            .collect::<Result<Vec<_>>>()?;

        let fingerprint = Self::compute_fingerprint_for_columns(&columns);
        let now_ms = chrono::Utc::now().timestamp_millis();

        Ok(Self {
            schema_id: avro.name.clone(),
            version: 1,
            parent_schema_id: None,
            columns,
            primary_key: vec![1], // Default to first column
            fingerprint,
            metadata: avro.metadata.clone(),
            created_at_ms: now_ms,
            is_legacy_vector_record: false,
        })
    }

    /// Serialize to JSON string.
    pub fn to_avro_json(&self) -> Result<String> {
        serde_json::to_string_pretty(&self.to_avro_style())
            .context("Failed to serialize schema to Avro-style JSON")
    }

    /// Deserialize from JSON string.
    pub fn from_avro_json(json: &str) -> Result<Self> {
        let avro: AvroStyleSchema =
            serde_json::from_str(json).context("Failed to parse Avro-style JSON")?;
        Self::from_avro_style(&avro)
    }
}

impl ProximaColumn {
    /// Convert to Avro-style field.
    fn to_avro_field(&self) -> AvroStyleField {
        let field_type = self.data_type.to_avro_type(self.nullable);

        AvroStyleField {
            name: self.name.clone(),
            field_type,
            default: self.default_value.as_ref().map(|d| match d {
                super::proxima_schema::DefaultValue::Literal(s) => {
                    serde_json::from_str(s).unwrap_or(JsonValue::String(s.clone()))
                }
                super::proxima_schema::DefaultValue::Expression(s) => JsonValue::String(s.clone()),
                super::proxima_schema::DefaultValue::AutoGenerate(_) => JsonValue::Null,
            }),
            doc: self.comment.clone(),
            column_id: Some(self.id),
        }
    }
}

impl AvroStyleField {
    /// Convert to ProximaColumn.
    fn to_proxima_column(&self, default_id: i32) -> Result<ProximaColumn> {
        let (data_type, nullable) = self.field_type.to_proxima_type()?;

        Ok(ProximaColumn {
            id: self.column_id.unwrap_or(default_id),
            name: self.name.clone(),
            data_type,
            nullable,
            default_value: self
                .default
                .as_ref()
                .map(|v| super::proxima_schema::DefaultValue::Literal(v.to_string())),
            comment: self.doc.clone(),
            metadata: HashMap::new(),
            is_deleted: false,
            original_id: None,
        })
    }
}

impl ProximaDataType {
    /// Convert to Avro-style type.
    fn to_avro_type(&self, nullable: bool) -> AvroStyleType {
        let base_type = match self {
            ProximaDataType::Boolean => AvroStyleType::Simple("boolean".to_string()),
            ProximaDataType::Int8 | ProximaDataType::Int16 | ProximaDataType::Int32 => {
                AvroStyleType::Simple("int".to_string())
            }
            ProximaDataType::Int64 => AvroStyleType::Simple("long".to_string()),
            ProximaDataType::UInt8 | ProximaDataType::UInt16 | ProximaDataType::UInt32 => {
                AvroStyleType::Simple("int".to_string())
            }
            ProximaDataType::UInt64 => AvroStyleType::Simple("long".to_string()),
            ProximaDataType::Float32 => AvroStyleType::Simple("float".to_string()),
            ProximaDataType::Float64 => AvroStyleType::Simple("double".to_string()),
            ProximaDataType::Decimal { precision, scale } => AvroStyleType::Complex {
                type_name: "bytes".to_string(),
                items: None,
                values: None,
                dimension: None,
                precision: Some(*precision),
                scale: Some(*scale),
            },
            ProximaDataType::String | ProximaDataType::Uuid | ProximaDataType::Json => {
                AvroStyleType::Simple("string".to_string())
            }
            ProximaDataType::Binary => AvroStyleType::Simple("bytes".to_string()),
            ProximaDataType::Date => AvroStyleType::Simple("int".to_string()), // days since epoch
            ProximaDataType::Time { .. } => AvroStyleType::Simple("long".to_string()),
            ProximaDataType::Timestamp { .. } => AvroStyleType::Simple("long".to_string()),
            ProximaDataType::List { element } => AvroStyleType::Complex {
                type_name: "array".to_string(),
                items: Some(Box::new(element.to_avro_type(true))),
                values: None,
                dimension: None,
                precision: None,
                scale: None,
            },
            ProximaDataType::Map { key: _, value } => AvroStyleType::Complex {
                type_name: "map".to_string(),
                items: None,
                values: Some(Box::new(value.to_avro_type(true))),
                dimension: None,
                precision: None,
                scale: None,
            },
            ProximaDataType::Struct { .. } => {
                // Structs are complex - serialize as string for simplicity
                AvroStyleType::Simple("string".to_string())
            }
            ProximaDataType::Vector { dimension, .. } => AvroStyleType::Complex {
                type_name: "fixed".to_string(),
                items: None,
                values: None,
                dimension: Some(*dimension),
                precision: None,
                scale: None,
            },
            ProximaDataType::SparseVector { .. } => AvroStyleType::Simple("bytes".to_string()),
            ProximaDataType::BinaryVector { dimension } => AvroStyleType::Complex {
                type_name: "fixed".to_string(),
                items: None,
                values: None,
                dimension: Some(*dimension),
                precision: None,
                scale: None,
            },
            ProximaDataType::QuantizedInt8Vector { dimension, .. } => AvroStyleType::Complex {
                type_name: "fixed".to_string(),
                items: None,
                values: None,
                dimension: Some(*dimension),
                precision: None,
                scale: None,
            },
            ProximaDataType::QuantizedPQVector { dimension, .. } => AvroStyleType::Complex {
                type_name: "fixed".to_string(),
                items: None,
                values: None,
                dimension: Some(*dimension),
                precision: None,
                scale: None,
            },
            ProximaDataType::QuantizedBinaryVector { dimension } => AvroStyleType::Complex {
                type_name: "fixed".to_string(),
                items: None,
                values: None,
                dimension: Some(*dimension),
                precision: None,
                scale: None,
            },
        };

        if nullable {
            match base_type {
                AvroStyleType::Simple(t) => AvroStyleType::Union(vec!["null".to_string(), t]),
                _ => base_type, // Complex types already handle nullability
            }
        } else {
            base_type
        }
    }
}

impl AvroStyleType {
    /// Convert to ProximaDataType.
    fn to_proxima_type(&self) -> Result<(ProximaDataType, bool)> {
        match self {
            AvroStyleType::Simple(t) => {
                let dtype = match t.as_str() {
                    "null" => ProximaDataType::String,
                    "boolean" => ProximaDataType::Boolean,
                    "int" => ProximaDataType::Int32,
                    "long" => ProximaDataType::Int64,
                    "float" => ProximaDataType::Float32,
                    "double" => ProximaDataType::Float64,
                    "bytes" => ProximaDataType::Binary,
                    "string" => ProximaDataType::String,
                    _ => ProximaDataType::String, // Default
                };
                Ok((dtype, false))
            }
            AvroStyleType::Union(types) => {
                let nullable = types.contains(&"null".to_string());
                let non_null: Vec<_> = types.iter().filter(|t| *t != "null").collect();
                if let Some(t) = non_null.first() {
                    let (dtype, _) = AvroStyleType::Simple((*t).clone()).to_proxima_type()?;
                    Ok((dtype, nullable))
                } else {
                    Ok((ProximaDataType::String, true))
                }
            }
            AvroStyleType::Complex {
                type_name,
                items,
                values,
                dimension,
                precision,
                scale,
            } => {
                let dtype = match type_name.as_str() {
                    "array" => {
                        let element = items
                            .as_ref()
                            .map(|i| i.to_proxima_type().map(|(t, _)| t))
                            .transpose()?
                            .unwrap_or(ProximaDataType::String);
                        ProximaDataType::List {
                            element: Box::new(element),
                        }
                    }
                    "map" => {
                        let value = values
                            .as_ref()
                            .map(|v| v.to_proxima_type().map(|(t, _)| t))
                            .transpose()?
                            .unwrap_or(ProximaDataType::String);
                        ProximaDataType::Map {
                            key: Box::new(ProximaDataType::String),
                            value: Box::new(value),
                        }
                    }
                    "fixed" => {
                        if let Some(dim) = dimension {
                            ProximaDataType::Vector {
                                dimension: *dim,
                                element_type: VectorElementType::Float32,
                            }
                        } else {
                            ProximaDataType::Binary
                        }
                    }
                    "bytes" if precision.is_some() => ProximaDataType::Decimal {
                        precision: precision.unwrap_or(38),
                        scale: scale.unwrap_or(0),
                    },
                    _ => ProximaDataType::String,
                };
                Ok((dtype, false))
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::proximadb_v1::SqlArray;

    fn create_test_record(id: &str, dim: usize) -> VectorRecord {
        let mut metadata = HashMap::new();
        metadata.insert(
            "name".to_string(),
            SqlValue {
                value: Some(ProtoSqlValueInner::StringValue("test".to_string())),
            },
        );
        metadata.insert(
            "score".to_string(),
            SqlValue {
                value: Some(ProtoSqlValueInner::NumberValue(0.95)),
            },
        );
        metadata.insert(
            "count".to_string(),
            SqlValue {
                value: Some(ProtoSqlValueInner::Int64Value(42)),
            },
        );
        metadata.insert(
            "active".to_string(),
            SqlValue {
                value: Some(ProtoSqlValueInner::BoolValue(true)),
            },
        );

        VectorRecord {
            id: id.to_string(),
            vector: (0..dim).map(|i| i as f32 * 0.1).collect(),
            metadata,
            timestamp: Some(chrono::Utc::now().timestamp_millis()),
            version: Some(1),
            ..Default::default()
        }
    }

    #[test]
    fn test_records_to_batch_roundtrip() {
        let schema = ProximaSchema::vector_record_schema(128);
        let bridge = DefaultVectorRecordBridge::new(schema);

        let records = vec![
            create_test_record("vec_1", 128),
            create_test_record("vec_2", 128),
        ];

        let batch = bridge.records_to_batch(&records).unwrap();
        assert_eq!(batch.num_rows(), 2);

        let recovered = bridge.batch_to_records(&batch).unwrap();
        assert_eq!(recovered.len(), 2);
        assert_eq!(recovered[0].id, "vec_1");
        assert_eq!(recovered[1].id, "vec_2");
        assert_eq!(recovered[0].vector.len(), 128);
    }

    #[test]
    fn test_metadata_json_mode() {
        let schema = ProximaSchema::vector_record_schema(64);
        let bridge =
            DefaultVectorRecordBridge::new(schema).with_metadata_mode(MetadataMode::JsonString);

        let records = vec![create_test_record("test", 64)];
        let batch = bridge.records_to_batch(&records).unwrap();

        // Check metadata column is string type
        let metadata_col = batch.column_by_name("metadata").unwrap();
        assert!(
            metadata_col
                .as_any()
                .downcast_ref::<StringArray>()
                .is_some()
        );
    }

    #[test]
    fn test_metadata_struct_mode() {
        let schema = ProximaSchema::vector_record_schema(64);
        let bridge =
            DefaultVectorRecordBridge::new(schema).with_metadata_mode(MetadataMode::ArrowStruct);

        let records = vec![create_test_record("test", 64)];
        let batch = bridge.records_to_batch(&records).unwrap();

        // Check metadata column is struct type
        let metadata_col = batch.column_by_name("metadata").unwrap();
        assert!(
            metadata_col
                .as_any()
                .downcast_ref::<StructArray>()
                .is_some()
        );
    }

    #[test]
    fn test_schema_inference() {
        let schema = ProximaSchema::vector_record_schema(256);
        let bridge = DefaultVectorRecordBridge::new(schema);

        let records = vec![create_test_record("a", 256), create_test_record("b", 256)];

        let inferred = bridge.infer_schema_from_records(&records).unwrap();
        assert_eq!(inferred.vector_dimension(), Some(256));
        assert!(inferred.active_column_count() >= 3); // id, vector, timestamp + metadata fields
    }

    #[test]
    fn test_validate_records() {
        let schema = ProximaSchema::vector_record_schema(128);
        let bridge = DefaultVectorRecordBridge::new(schema);

        let valid_records = vec![create_test_record("valid", 128)];
        assert!(bridge.validate_records(&valid_records).is_ok());

        let invalid_records = vec![VectorRecord {
            id: "".to_string(), // Empty ID
            vector: vec![0.1; 128],
            ..Default::default()
        }];
        assert!(bridge.validate_records(&invalid_records).is_err());

        let wrong_dim_records = vec![VectorRecord {
            id: "wrong_dim".to_string(),
            vector: vec![0.1; 64], // Wrong dimension
            ..Default::default()
        }];
        assert!(bridge.validate_records(&wrong_dim_records).is_err());
    }

    #[test]
    fn test_avro_style_serialization() {
        let schema = ProximaSchema::vector_record_schema(512);
        let avro = schema.to_avro_style();

        assert_eq!(avro.schema_type, "record");
        assert_eq!(avro.name, "vector_record_v0");
        assert!(!avro.fields.is_empty());

        // Round-trip test
        let json = schema.to_avro_json().unwrap();
        let recovered = ProximaSchema::from_avro_json(&json).unwrap();
        assert_eq!(recovered.vector_dimension(), Some(512));
    }

    #[test]
    fn test_legacy_bridge() {
        let bridge = DefaultVectorRecordBridge::legacy(768);
        assert_eq!(bridge.vector_dimension(), Some(768));
        assert!(bridge.schema().is_legacy_vector_record);
    }

    #[test]
    fn test_without_vectors() {
        let schema = ProximaSchema::vector_record_schema(128);
        let bridge = DefaultVectorRecordBridge::new(schema).without_vectors();

        let records = vec![create_test_record("test", 128)];
        let batch = bridge.records_to_batch(&records).unwrap();

        // Vector column should not be present
        assert!(batch.column_by_name("vector").is_none());
    }

    #[test]
    fn test_infer_schema_from_vector_records() {
        let records = vec![create_test_record("a", 384), create_test_record("b", 384)];

        let schema = infer_schema_from_vector_records(&records, "test_schema".to_string()).unwrap();
        assert_eq!(schema.vector_dimension(), Some(384));
        assert_eq!(schema.schema_id, "test_schema");
    }

    #[test]
    fn test_empty_metadata() {
        let schema = ProximaSchema::vector_record_schema(64);
        let bridge = DefaultVectorRecordBridge::new(schema);

        let records = vec![VectorRecord {
            id: "empty_meta".to_string(),
            vector: vec![0.1; 64],
            metadata: HashMap::new(),
            timestamp: Some(1234567890),
            ..Default::default()
        }];

        let batch = bridge.records_to_batch(&records).unwrap();
        let recovered = bridge.batch_to_records(&batch).unwrap();

        assert_eq!(recovered[0].id, "empty_meta");
        assert!(recovered[0].metadata.is_empty());
    }

    #[test]
    fn test_nested_metadata() {
        let schema = ProximaSchema::vector_record_schema(32);
        let bridge =
            DefaultVectorRecordBridge::new(schema).with_metadata_mode(MetadataMode::JsonString);

        let mut metadata = HashMap::new();
        metadata.insert(
            "nested".to_string(),
            SqlValue {
                value: Some(ProtoSqlValueInner::ArrayValue(SqlArray {
                    values: vec![
                        SqlValue {
                            value: Some(ProtoSqlValueInner::Int64Value(1)),
                        },
                        SqlValue {
                            value: Some(ProtoSqlValueInner::Int64Value(2)),
                        },
                    ],
                })),
            },
        );

        let records = vec![VectorRecord {
            id: "nested".to_string(),
            vector: vec![0.1; 32],
            metadata,
            ..Default::default()
        }];

        let batch = bridge.records_to_batch(&records).unwrap();
        let recovered = bridge.batch_to_records(&batch).unwrap();

        assert!(recovered[0].metadata.contains_key("nested"));
    }
}
