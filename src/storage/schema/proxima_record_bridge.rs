//! # ProximaRecordBridge - ProximaRecord to Arrow RecordBatch Conversion
//!
//! Provides conversion between the canonical ProximaRecord envelope and Arrow
//! RecordBatch values for the Arrow-native compute layer.
//!
//! ## Key Features
//!
//! - Zero-copy conversion where possible using Arrow's memory format
//! - Support for nested metadata fields (JSON -> Arrow Struct)
//! - Schema inference from ProximaRecord properties
//! - Batch conversion utilities for efficient bulk operations
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::storage::schema::proxima_record_bridge::{
//!     ProximaRecordBridge, DefaultProximaRecordBridge
//! };
//!
//! let bridge = DefaultProximaRecordBridge::new(schema);
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
use proximadb_data_model::ProximaValue;
use proximadb_records::{EmbeddingCell, ProximaRecord, ProximaTree, ProximaTreeNode};
use serde_json::Value as JsonValue;

use super::proxima_schema::{ProximaDataType, ProximaSchema};

// ============================================================================
// ProximaRecordBridge Trait
// ============================================================================

/// Trait for converting between ProximaRecord and Arrow RecordBatch.
///
/// Implementations must ensure:
/// - Zero-copy conversion where possible
/// - Proper handling of nullable fields
/// - Correct metadata serialization/deserialization
/// - Schema compatibility validation
pub trait ProximaRecordBridge: Send + Sync {
    /// Get the schema used for conversions.
    fn schema(&self) -> &ProximaSchema;

    /// Convert a slice of ProximaRecords to an Arrow RecordBatch.
    ///
    /// # Arguments
    /// * `records` - Canonical records to convert
    ///
    /// # Returns
    /// * Arrow RecordBatch containing all records
    fn records_to_batch(&self, records: &[ProximaRecord]) -> Result<RecordBatch>;

    /// Convert an Arrow RecordBatch to ProximaRecords.
    ///
    /// # Arguments
    /// * `batch` - Arrow RecordBatch to convert
    ///
    /// # Returns
    /// * Vec of ProximaRecords extracted from the batch
    fn batch_to_records(&self, batch: &RecordBatch) -> Result<Vec<ProximaRecord>>;

    /// Infer schema from a set of ProximaRecords.
    ///
    /// Analyzes the metadata fields across records to determine
    /// the optimal schema for the collection.
    ///
    /// # Arguments
    /// * `records` - Sample records to analyze
    ///
    /// # Returns
    /// * Inferred ProximaSchema
    fn infer_schema_from_records(&self, records: &[ProximaRecord]) -> Result<ProximaSchema>;

    /// Validate that records are compatible with the current schema.
    ///
    /// # Arguments
    /// * `records` - Records to validate
    ///
    /// # Returns
    /// * Ok(()) if compatible, Err with details otherwise
    fn validate_records(&self, records: &[ProximaRecord]) -> Result<()>;
}

// ============================================================================
// DefaultProximaRecordBridge Implementation
// ============================================================================

/// Default implementation of ProximaRecordBridge.
///
/// Uses the standard ProximaSchema for conversions and supports
/// both flat metadata (JSON string) and structured metadata (Arrow Struct).
#[derive(Debug, Clone)]
pub struct DefaultProximaRecordBridge {
    /// Schema for conversions
    schema: ProximaSchema,
    /// Metadata handling mode
    metadata_mode: MetadataMode,
    /// Whether to include vector data in conversions
    include_vectors: bool,
}

/// Mode for handling metadata in conversions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MetadataMode {
    /// Store metadata as JSON string (simple, flexible)
    #[default]
    JsonString,
    /// Store metadata as Arrow Struct (typed, efficient)
    ArrowStruct,
    /// Auto-detect based on schema
    Auto,
}

impl DefaultProximaRecordBridge {
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

    /// Create a bridge with the historical vector schema shape.
    pub fn legacy(dimension: u32) -> Self {
        Self::new(ProximaSchema::vector_record_schema(dimension))
    }

    /// Get the vector dimension from the schema.
    fn vector_dimension(&self) -> Option<u32> {
        self.schema.vector_dimension()
    }

    /// Convert canonical records into an Arrow RecordBatch.
    pub fn proxima_records_to_batch(&self, records: &[ProximaRecord]) -> Result<RecordBatch> {
        self.records_to_batch(records)
    }

    /// Convert an Arrow RecordBatch into canonical records.
    pub fn batch_to_proxima_records(&self, batch: &RecordBatch) -> Result<Vec<ProximaRecord>> {
        self.batch_to_records(batch)
    }

    /// Build ID array from records.
    fn build_id_array(&self, records: &[ProximaRecord]) -> ArrayRef {
        Arc::new(StringArray::from(
            records.iter().map(|r| r.oid.as_str()).collect::<Vec<_>>(),
        ))
    }

    /// Build vector array from records.
    fn build_vector_array(&self, records: &[ProximaRecord]) -> Result<ArrayRef> {
        let dimension =
            self.vector_dimension()
                .ok_or_else(|| anyhow!("Schema has no vector dimension"))? as usize;
        if dimension == 0 {
            return Err(anyhow!("Schema has no vector column"));
        }

        // Use FixedSizeListArray so each row has exactly one vector
        let values_builder = Float32Builder::with_capacity(records.len() * dimension);
        let mut builder = FixedSizeListBuilder::new(values_builder, dimension as i32);

        for record in records {
            let vector = record
                .embeddings
                .first()
                .map(|embedding| embedding.as_fp32_slice())
                .unwrap_or(&[]);
            if vector.len() != dimension {
                return Err(anyhow!(
                    "Vector dimension mismatch: expected {}, got {}",
                    dimension,
                    vector.len()
                ));
            }
            let values = builder.values();
            for &v in vector {
                values.append_value(v);
            }
            builder.append(true);
        }

        Ok(Arc::new(builder.finish()))
    }

    /// Build metadata array from records based on mode.
    fn build_metadata_array(&self, records: &[ProximaRecord]) -> Result<ArrayRef> {
        match self.metadata_mode {
            MetadataMode::JsonString | MetadataMode::Auto => {
                self.build_metadata_json_array(records)
            }
            MetadataMode::ArrowStruct => self.build_metadata_struct_array(records),
        }
    }

    /// Build metadata as JSON string array.
    fn build_metadata_json_array(&self, records: &[ProximaRecord]) -> Result<ArrayRef> {
        let mut builder = StringBuilder::new();

        for record in records {
            if record.props.is_empty() {
                builder.append_null();
            } else {
                let json_map: serde_json::Map<String, JsonValue> = record
                    .props
                    .iter()
                    .map(|(k, v)| (k.clone(), Self::tree_node_to_json(v)))
                    .collect();
                let json_str = serde_json::to_string(&JsonValue::Object(json_map))
                    .context("Failed to serialize metadata to JSON")?;
                builder.append_value(&json_str);
            }
        }

        Ok(Arc::new(builder.finish()))
    }

    /// Build metadata as Arrow Struct array.
    fn build_metadata_struct_array(&self, records: &[ProximaRecord]) -> Result<ArrayRef> {
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
    fn infer_metadata_schema(&self, records: &[ProximaRecord]) -> Result<Vec<(String, DataType)>> {
        let mut schema: HashMap<String, DataType> = HashMap::new();

        for record in records {
            for (key, value) in &record.props {
                let dtype = self.infer_arrow_type_from_tree_node(value);
                schema.entry(key.clone()).or_insert(dtype);
            }
        }

        let mut result: Vec<_> = schema.into_iter().collect();
        result.sort_by(|a, b| a.0.cmp(&b.0)); // Stable ordering
        Ok(result)
    }

    /// Infer Arrow DataType from a canonical property node.
    fn infer_arrow_type_from_tree_node(&self, node: &ProximaTreeNode) -> DataType {
        match node {
            ProximaTreeNode::Value(value) => self.infer_arrow_type_from_proxima_value(value),
            ProximaTreeNode::Object(_) => DataType::Utf8,
        }
    }

    /// Infer Arrow DataType from ProximaValue.
    fn infer_arrow_type_from_proxima_value(&self, value: &ProximaValue) -> DataType {
        match value {
            ProximaValue::Null => DataType::Null,
            ProximaValue::Boolean(_) => DataType::Boolean,
            ProximaValue::Int8(_)
            | ProximaValue::Int16(_)
            | ProximaValue::Int32(_)
            | ProximaValue::Int64(_)
            | ProximaValue::UInt8(_)
            | ProximaValue::UInt16(_)
            | ProximaValue::UInt32(_)
            | ProximaValue::UInt64(_) => DataType::Int64,
            ProximaValue::Float16(_) | ProximaValue::Float32(_) | ProximaValue::Float64(_) => {
                DataType::Float64
            }
            ProximaValue::String(_) | ProximaValue::Symbol(_) => DataType::Utf8,
            ProximaValue::Binary(_) | ProximaValue::BinaryVector(_) => DataType::Binary,
            _ => DataType::Utf8,
        }
    }

    /// Build a typed metadata field array.
    fn build_typed_metadata_field(
        &self,
        records: &[ProximaRecord],
        key: &str,
        data_type: &DataType,
    ) -> Result<ArrayRef> {
        let num_records = records.len();

        match data_type {
            DataType::Boolean => {
                let mut builder = BooleanBuilder::with_capacity(num_records);
                for record in records {
                    if let Some(ProximaValue::Boolean(value)) = self.property_value(record, key) {
                        builder.append_value(*value);
                    } else {
                        builder.append_null();
                    }
                }
                Ok(Arc::new(builder.finish()))
            }
            DataType::Int64 => {
                let mut builder = Int64Builder::with_capacity(num_records);
                for record in records {
                    if let Some(value) = self.property_value(record, key) {
                        if let Some(int_value) = Self::proxima_value_to_i64(value) {
                            builder.append_value(int_value);
                        } else {
                            builder.append_null();
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
                    if let Some(value) = self.property_value(record, key) {
                        if let Some(float_value) = Self::proxima_value_to_f64(value) {
                            builder.append_value(float_value);
                        } else {
                            builder.append_null();
                        }
                    } else {
                        builder.append_null();
                    }
                }
                Ok(Arc::new(builder.finish()))
            }
            _ => {
                // Default to string for all other types
                let mut builder = StringBuilder::new();
                for record in records {
                    if let Some(node) = record.props.get(key) {
                        let json = Self::tree_node_to_json(node);
                        match json {
                            JsonValue::Null => builder.append_null(),
                            JsonValue::String(value) => builder.append_value(&value),
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
    fn build_timestamp_array(&self, records: &[ProximaRecord]) -> ArrayRef {
        let timestamps: Vec<Option<i64>> = records
            .iter()
            .map(|r| Some(r.created_at_ns / 1_000_000))
            .collect();
        Arc::new(Int64Array::from(timestamps))
    }

    /// Build version array from records.
    fn build_version_array(&self, records: &[ProximaRecord]) -> ArrayRef {
        let versions: Vec<Option<i64>> = records
            .iter()
            .map(|r| Some(r.record_version as i64))
            .collect();
        Arc::new(Int64Array::from(versions))
    }

    fn property_value<'a>(&self, record: &'a ProximaRecord, key: &str) -> Option<&'a ProximaValue> {
        match record.props.get(key)? {
            ProximaTreeNode::Value(value) => Some(value),
            ProximaTreeNode::Object(_) => None,
        }
    }

    fn tree_node_to_json(node: &ProximaTreeNode) -> JsonValue {
        match node {
            ProximaTreeNode::Value(value) => Self::proxima_value_to_json(value),
            ProximaTreeNode::Object(tree) => JsonValue::Object(
                tree.iter()
                    .map(|(key, node)| (key.clone(), Self::tree_node_to_json(node)))
                    .collect(),
            ),
        }
    }

    fn proxima_value_to_json(value: &ProximaValue) -> JsonValue {
        match value {
            ProximaValue::Boolean(value) => JsonValue::Bool(*value),
            ProximaValue::Int8(value) => JsonValue::from(*value),
            ProximaValue::Int16(value) => JsonValue::from(*value),
            ProximaValue::Int32(value) => JsonValue::from(*value),
            ProximaValue::Int64(value) => JsonValue::from(*value),
            ProximaValue::UInt8(value) => JsonValue::from(*value),
            ProximaValue::UInt16(value) => JsonValue::from(*value),
            ProximaValue::UInt32(value) => JsonValue::from(*value),
            ProximaValue::UInt64(value) => JsonValue::from(*value),
            ProximaValue::Float16(value) | ProximaValue::Float32(value) => JsonValue::from(*value),
            ProximaValue::Float64(value) => JsonValue::from(*value),
            ProximaValue::String(value)
            | ProximaValue::Symbol(value)
            | ProximaValue::Decimal(value) => JsonValue::String(value.clone()),
            ProximaValue::Json(value) | ProximaValue::Jsonb(value) => value.clone(),
            ProximaValue::Array(values) => {
                JsonValue::Array(values.iter().map(Self::proxima_value_to_json).collect())
            }
            ProximaValue::Map(values) | ProximaValue::Struct(values) => JsonValue::Object(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), Self::proxima_value_to_json(value)))
                    .collect(),
            ),
            ProximaValue::Null => JsonValue::Null,
            other => serde_json::to_value(other).unwrap_or(JsonValue::Null),
        }
    }

    fn json_to_tree_node(value: JsonValue) -> ProximaTreeNode {
        match value {
            JsonValue::Object(map) => ProximaTreeNode::Object(
                map.into_iter()
                    .map(|(key, value)| (key, Self::json_to_tree_node(value)))
                    .collect(),
            ),
            other => ProximaTreeNode::Value(Self::json_to_proxima_value(other)),
        }
    }

    fn json_to_proxima_value(value: JsonValue) -> ProximaValue {
        match value {
            JsonValue::Null => ProximaValue::Null,
            JsonValue::Bool(value) => ProximaValue::Boolean(value),
            JsonValue::Number(value) => {
                if let Some(int_value) = value.as_i64() {
                    ProximaValue::Int64(int_value)
                } else if let Some(float_value) = value.as_f64() {
                    ProximaValue::Float64(float_value)
                } else {
                    ProximaValue::Null
                }
            }
            JsonValue::String(value) => ProximaValue::String(value),
            JsonValue::Array(values) => ProximaValue::Array(
                values
                    .into_iter()
                    .map(Self::json_to_proxima_value)
                    .collect(),
            ),
            JsonValue::Object(values) => ProximaValue::Map(
                values
                    .into_iter()
                    .map(|(key, value)| (key, Self::json_to_proxima_value(value)))
                    .collect(),
            ),
        }
    }

    fn proxima_value_to_i64(value: &ProximaValue) -> Option<i64> {
        match value {
            ProximaValue::Int8(value) => Some(*value as i64),
            ProximaValue::Int16(value) => Some(*value as i64),
            ProximaValue::Int32(value) => Some(*value as i64),
            ProximaValue::Int64(value) => Some(*value),
            ProximaValue::UInt8(value) => Some(*value as i64),
            ProximaValue::UInt16(value) => Some(*value as i64),
            ProximaValue::UInt32(value) => Some(*value as i64),
            ProximaValue::UInt64(value) => i64::try_from(*value).ok(),
            _ => None,
        }
    }

    fn proxima_value_to_f64(value: &ProximaValue) -> Option<f64> {
        match value {
            ProximaValue::Float16(value) | ProximaValue::Float32(value) => Some(*value as f64),
            ProximaValue::Float64(value) => Some(*value),
            _ => Self::proxima_value_to_i64(value).map(|value| value as f64),
        }
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
        let dimension =
            self.vector_dimension()
                .ok_or_else(|| anyhow!("Schema has no vector dimension"))? as usize;
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
    fn extract_metadata(&self, batch: &RecordBatch, row: usize) -> Result<ProximaTree> {
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
            let json_map: HashMap<String, JsonValue> = serde_json::from_str(json_str)
                .with_context(|| format!("Failed to parse metadata JSON: {}", json_str))?;
            return Ok(json_map
                .into_iter()
                .map(|(k, v)| (k, Self::json_to_tree_node(v)))
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
    ) -> Result<ProximaTree> {
        let mut metadata = HashMap::new();

        for (i, field) in struct_array.fields().iter().enumerate() {
            let column = struct_array.column(i);
            let value = self.extract_field_value(column.as_ref(), row, field.data_type())?;
            if let Some(v) = value {
                metadata.insert(field.name().clone(), ProximaTreeNode::Value(v));
            }
        }

        Ok(metadata)
    }

    /// Extract a single field value and convert to ProximaValue.
    fn extract_field_value(
        &self,
        array: &dyn Array,
        row: usize,
        _data_type: &DataType,
    ) -> Result<Option<ProximaValue>> {
        if array.is_null(row) {
            return Ok(None);
        }

        // Boolean
        if let Some(arr) = array.as_any().downcast_ref::<BooleanArray>() {
            return Ok(Some(ProximaValue::Boolean(arr.value(row))));
        }

        // Int64
        if let Some(arr) = array.as_any().downcast_ref::<Int64Array>() {
            return Ok(Some(ProximaValue::Int64(arr.value(row))));
        }

        // Float64
        if let Some(arr) = array.as_any().downcast_ref::<Float64Array>() {
            return Ok(Some(ProximaValue::Float64(arr.value(row))));
        }

        // String
        if let Some(arr) = array.as_any().downcast_ref::<StringArray>() {
            return Ok(Some(ProximaValue::String(arr.value(row).to_string())));
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

impl ProximaRecordBridge for DefaultProximaRecordBridge {
    fn schema(&self) -> &ProximaSchema {
        &self.schema
    }

    fn records_to_batch(&self, records: &[ProximaRecord]) -> Result<RecordBatch> {
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
        if self.include_vectors
            && let Some(dimension) = self.vector_dimension()
        {
            let _dimension = dimension as i32;

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
            .context("Failed to create RecordBatch from ProximaRecords")
    }

    fn batch_to_records(&self, batch: &RecordBatch) -> Result<Vec<ProximaRecord>> {
        let num_rows = batch.num_rows();
        let mut records = Vec::with_capacity(num_rows);

        for row in 0..num_rows {
            let id = self.extract_id(batch, row)?;
            let vector = self.extract_vector(batch, row)?;
            let props = self.extract_metadata(batch, row)?;
            let timestamp = self.extract_timestamp(batch, row);
            let version = self.extract_version(batch, row);
            let created_at_ns = timestamp
                .map(|ts| ts.saturating_mul(1_000_000))
                .unwrap_or_else(|| {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos() as i64
                });

            let embeddings = if vector.is_empty() {
                Vec::new()
            } else {
                vec![EmbeddingCell {
                    model_id: "default".to_string(),
                    modality: "dense_vector".to_string(),
                    dim: vector.len() as u32,
                    values: proximadb_records::EmbeddingValues::Fp32(vector),
                    ..Default::default()
                }]
            };

            records.push(ProximaRecord {
                oid: id.clone(),
                local_id: Some(id),
                created_at_ns,
                updated_at_ns: created_at_ns,
                record_version: version.unwrap_or(0) as u64,
                props,
                embeddings,
                ..ProximaRecord::default()
            });
        }

        Ok(records)
    }

    fn infer_schema_from_records(&self, records: &[ProximaRecord]) -> Result<ProximaSchema> {
        if records.is_empty() {
            return Err(anyhow!("Cannot infer schema from empty records"));
        }

        // Determine vector dimension
        let dimension = records
            .iter()
            .map(|r| {
                r.embeddings
                    .first()
                    .map(|embedding| embedding.values.len())
                    .unwrap_or_default()
            })
            .max()
            .ok_or_else(|| anyhow!("Cannot infer schema from empty records"))?
            as u32;

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

    fn validate_records(&self, records: &[ProximaRecord]) -> Result<()> {
        let expected_dim = self.vector_dimension();

        for (i, record) in records.iter().enumerate() {
            // Validate vector dimension
            if let Some(dim) = expected_dim
                && record
                    .embeddings
                    .first()
                    .map(|embedding| embedding.values.len())
                    .unwrap_or_default()
                    != dim as usize
            {
                let actual_dim = record
                    .embeddings
                    .first()
                    .map(|embedding| embedding.values.len())
                    .unwrap_or_default();
                return Err(anyhow!(
                    "Record {} has wrong vector dimension: expected {}, got {}",
                    i,
                    dim,
                    actual_dim
                ));
            }

            // Validate ID is not empty
            if record.oid.is_empty() {
                return Err(anyhow!("Record {} has empty ID", i));
            }
        }

        Ok(())
    }
}

// ============================================================================
// Schema Inference Utilities
// ============================================================================

/// Infer ProximaSchema from ProximaRecord samples.
///
/// Analyzes property fields to determine optimal typing.
pub fn infer_schema_from_proxima_records(
    records: &[ProximaRecord],
    schema_id: String,
) -> Result<ProximaSchema> {
    if records.is_empty() {
        return Err(anyhow!("Cannot infer schema from empty records"));
    }

    let dimension = records
        .iter()
        .map(|r| {
            r.embeddings
                .first()
                .map(|embedding| embedding.values.len())
                .unwrap_or_default()
        })
        .max()
        .ok_or_else(|| anyhow!("Cannot infer schema from empty records"))?
        as u32;

    if dimension == 0 {
        return Err(anyhow!("No vectors found in records"));
    }

    // Collect metadata field types
    let mut field_types: HashMap<String, ProximaDataType> = HashMap::new();

    for record in records {
        for (key, value) in &record.props {
            let inferred_type = infer_proxima_type_from_tree_node(value);
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

/// Infer ProximaDataType from a property tree node.
fn infer_proxima_type_from_tree_node(value: &ProximaTreeNode) -> ProximaDataType {
    match value {
        ProximaTreeNode::Object(_) => ProximaDataType::Json,
        ProximaTreeNode::Value(value) => infer_proxima_type_from_value(value),
    }
}

/// Infer ProximaDataType from ProximaValue.
fn infer_proxima_type_from_value(value: &ProximaValue) -> ProximaDataType {
    match value {
        ProximaValue::Boolean(_) => ProximaDataType::Boolean,
        ProximaValue::Int8(_)
        | ProximaValue::Int16(_)
        | ProximaValue::Int32(_)
        | ProximaValue::Int64(_)
        | ProximaValue::UInt8(_)
        | ProximaValue::UInt16(_)
        | ProximaValue::UInt32(_)
        | ProximaValue::UInt64(_) => ProximaDataType::Int64,
        ProximaValue::Float16(_) | ProximaValue::Float32(_) | ProximaValue::Float64(_) => {
            ProximaDataType::Float64
        }
        ProximaValue::Binary(_) | ProximaValue::BinaryVector(_) => ProximaDataType::Binary,
        ProximaValue::Array(_)
        | ProximaValue::Map(_)
        | ProximaValue::Struct(_)
        | ProximaValue::Json(_)
        | ProximaValue::Jsonb(_) => ProximaDataType::Json,
        _ => ProximaDataType::String,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_record(id: &str, dim: usize) -> ProximaRecord {
        let mut props = HashMap::new();
        props.insert(
            "name".to_string(),
            ProximaTreeNode::Value(ProximaValue::String("test".to_string())),
        );
        props.insert(
            "score".to_string(),
            ProximaTreeNode::Value(ProximaValue::Float64(0.95)),
        );
        props.insert(
            "count".to_string(),
            ProximaTreeNode::Value(ProximaValue::Int64(42)),
        );
        props.insert(
            "active".to_string(),
            ProximaTreeNode::Value(ProximaValue::Boolean(true)),
        );

        let timestamp_ns = chrono::Utc::now()
            .timestamp_millis()
            .saturating_mul(1_000_000);
        ProximaRecord {
            oid: id.to_string(),
            local_id: Some(id.to_string()),
            props,
            created_at_ns: timestamp_ns,
            updated_at_ns: timestamp_ns,
            record_version: 1,
            embeddings: vec![EmbeddingCell {
                model_id: "test".to_string(),
                modality: "dense_vector".to_string(),
                dim: dim as u32,
                values: proximadb_records::EmbeddingValues::Fp32((0..dim).map(|i| i as f32 * 0.1).collect()),
                ..Default::default()
            }],
            ..ProximaRecord::default()
        }
    }

    #[test]
    fn test_records_to_batch_roundtrip() {
        let schema = ProximaSchema::vector_record_schema(128);
        let bridge = DefaultProximaRecordBridge::new(schema);

        let records = vec![
            create_test_record("vec_1", 128),
            create_test_record("vec_2", 128),
        ];

        let batch = bridge
            .records_to_batch(&records)
            .expect("Failed to convert records to batch");
        assert_eq!(batch.num_rows(), 2);

        let recovered = bridge
            .batch_to_records(&batch)
            .expect("Failed to convert batch to records");
        assert_eq!(recovered.len(), 2);
        assert_eq!(recovered[0].oid, "vec_1");
        assert_eq!(recovered[1].oid, "vec_2");
        assert_eq!(recovered[0].embeddings[0].values.len(), 128);
    }

    #[test]
    fn test_metadata_json_mode() {
        let schema = ProximaSchema::vector_record_schema(64);
        let bridge =
            DefaultProximaRecordBridge::new(schema).with_metadata_mode(MetadataMode::JsonString);

        let records = vec![create_test_record("test", 64)];
        let batch = bridge
            .records_to_batch(&records)
            .expect("Failed to convert records to batch");

        // Check metadata column is string type
        let metadata_col = batch
            .column_by_name("metadata")
            .expect("Batch should have metadata column");
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
            DefaultProximaRecordBridge::new(schema).with_metadata_mode(MetadataMode::ArrowStruct);

        let records = vec![create_test_record("test", 64)];
        let batch = bridge
            .records_to_batch(&records)
            .expect("Failed to convert records to batch");

        // Check metadata column is struct type
        let metadata_col = batch
            .column_by_name("metadata")
            .expect("Batch should have metadata column");
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
        let bridge = DefaultProximaRecordBridge::new(schema);

        let records = vec![create_test_record("a", 256), create_test_record("b", 256)];

        let inferred = bridge
            .infer_schema_from_records(&records)
            .expect("Failed to infer schema from records");
        assert_eq!(inferred.vector_dimension(), Some(256));
        assert!(inferred.active_column_count() >= 3); // id, vector, timestamp + metadata fields
    }

    #[test]
    fn test_validate_records() {
        let schema = ProximaSchema::vector_record_schema(128);
        let bridge = DefaultProximaRecordBridge::new(schema);

        let valid_records = vec![create_test_record("valid", 128)];
        assert!(bridge.validate_records(&valid_records).is_ok());

        let invalid_records = vec![ProximaRecord {
            oid: "".to_string(), // Empty ID
            embeddings: vec![EmbeddingCell {
                model_id: "test".to_string(),
                modality: "dense_vector".to_string(),
                dim: 128,
                values: proximadb_records::EmbeddingValues::Fp32(vec![0.1; 128]),
                ..Default::default()
            }],
            ..ProximaRecord::default()
        }];
        assert!(bridge.validate_records(&invalid_records).is_err());

        let wrong_dim_records = vec![ProximaRecord {
            oid: "wrong_dim".to_string(),
            embeddings: vec![EmbeddingCell {
                model_id: "test".to_string(),
                modality: "dense_vector".to_string(),
                dim: 64,
                values: proximadb_records::EmbeddingValues::Fp32(vec![0.1; 64]), // Wrong dimension
                ..Default::default()
            }],
            ..ProximaRecord::default()
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
        let json = schema
            .to_avro_json()
            .expect("Failed to serialize schema to Avro JSON");
        let recovered = ProximaSchema::from_avro_json(&json)
            .expect("Failed to deserialize schema from Avro JSON");
        assert_eq!(recovered.vector_dimension(), Some(512));
    }

    #[test]
    fn test_legacy_bridge() {
        let bridge = DefaultProximaRecordBridge::legacy(768);
        assert_eq!(bridge.vector_dimension(), Some(768));
        assert!(bridge.schema().is_legacy_vector_record);
    }

    #[test]
    fn test_without_vectors() {
        let schema = ProximaSchema::vector_record_schema(128);
        let bridge = DefaultProximaRecordBridge::new(schema).without_vectors();

        let records = vec![create_test_record("test", 128)];
        let batch = bridge
            .records_to_batch(&records)
            .expect("Failed to convert records to batch");

        // Vector column should not be present
        assert!(batch.column_by_name("vector").is_none());
    }

    #[test]
    fn test_infer_schema_from_proxima_records() {
        let records = vec![create_test_record("a", 384), create_test_record("b", 384)];

        let schema = infer_schema_from_proxima_records(&records, "test_schema".to_string())
            .expect("Failed to infer schema from records");
        assert_eq!(schema.vector_dimension(), Some(384));
        assert_eq!(schema.schema_id, "test_schema");
    }

    #[test]
    fn test_empty_metadata() {
        let schema = ProximaSchema::vector_record_schema(64);
        let bridge = DefaultProximaRecordBridge::new(schema);

        let records = vec![ProximaRecord {
            oid: "empty_meta".to_string(),
            embeddings: vec![EmbeddingCell {
                model_id: "test".to_string(),
                modality: "dense_vector".to_string(),
                dim: 64,
                values: proximadb_records::EmbeddingValues::Fp32(vec![0.1; 64]),
                ..Default::default()
            }],
            created_at_ns: 1_234_567_890_000_000,
            updated_at_ns: 1_234_567_890_000_000,
            ..ProximaRecord::default()
        }];

        let batch = bridge
            .records_to_batch(&records)
            .expect("Failed to convert records to batch");
        let recovered = bridge
            .batch_to_records(&batch)
            .expect("Failed to convert batch to records");

        assert_eq!(recovered[0].oid, "empty_meta");
        assert!(recovered[0].props.is_empty());
    }

    #[test]
    fn test_nested_metadata() {
        let schema = ProximaSchema::vector_record_schema(32);
        let bridge =
            DefaultProximaRecordBridge::new(schema).with_metadata_mode(MetadataMode::JsonString);

        let mut props = HashMap::new();
        props.insert(
            "nested".to_string(),
            ProximaTreeNode::Value(ProximaValue::Array(vec![
                ProximaValue::Int64(1),
                ProximaValue::Int64(2),
            ])),
        );

        let records = vec![ProximaRecord {
            oid: "nested".to_string(),
            props,
            embeddings: vec![EmbeddingCell {
                model_id: "test".to_string(),
                modality: "dense_vector".to_string(),
                dim: 32,
                values: proximadb_records::EmbeddingValues::Fp32(vec![0.1; 32]),
                ..Default::default()
            }],
            ..ProximaRecord::default()
        }];

        let batch = bridge
            .records_to_batch(&records)
            .expect("Failed to convert records to batch");
        let recovered = bridge
            .batch_to_records(&batch)
            .expect("Failed to convert batch to records");

        assert!(recovered[0].props.contains_key("nested"));
    }
}
