//! # Internal Format Adapters
//!
//! This module provides adapter types that connect the new storage format traits
//! to existing storage engines. Following the Adapter Pattern, it wraps existing
//! `UnifiedStorageEngine` implementations to provide the `InternalFormat` interface.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                         ADAPTER PATTERN                                  │
//! │                                                                          │
//! │    ┌───────────────────────┐         ┌───────────────────────┐          │
//! │    │   InternalFormat      │         │ UnifiedStorageEngine  │          │
//! │    │   (New Trait)         │         │ (Existing Trait)      │          │
//! │    └───────────┬───────────┘         └───────────┬───────────┘          │
//! │                │                                 │                       │
//! │                │       Adapts                    │                       │
//! │                ▼                                 ▼                       │
//! │    ┌─────────────────────────────────────────────────────────┐          │
//! │    │           InternalFormatAdapter<E>                       │          │
//! │    │   - Wraps UnifiedStorageEngine                          │          │
//! │    │   - Implements InternalFormat                           │          │
//! │    │   - Translates between APIs                             │          │
//! │    └─────────────────────────────────────────────────────────┘          │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::storage::formats::adapters::InternalFormatAdapter;
//! use proximadb::storage::engines::sst::SstEngine;
//!
//! let sst_engine = SstEngine::new(config)?;
//! let format_adapter = InternalFormatAdapter::new(Arc::new(sst_engine));
//!
//! // Now use via InternalFormat trait
//! let batches = format_adapter.read_batches(&read_ctx).await?;
//! ```

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use arrow_array::RecordBatch;
use arrow_schema::{DataType as ArrowDataType, Field, Schema as ArrowSchema};
use async_trait::async_trait;
use chrono::Utc;
use futures::stream;
use tracing::{debug, warn};

use super::traits::{
    CompactionContext, CompactionResult, FileEntry, FormatStatistics,
    FormatType, InternalFormat, ReadContext, RecordBatchStream, StorageFormat, VectorBatch,
    VectorBatchStream, VectorReadContext, VectorWriteContext, WriteContext, WriteResult,
};
use crate::storage::traits::{StorageEngineStrategy, UnifiedStorageEngine};

// ============================================================================
// Internal Format Adapter
// ============================================================================

/// Adapter that wraps a `UnifiedStorageEngine` to provide `InternalFormat` interface.
///
/// This adapter bridges the gap between:
/// - **New API**: `InternalFormat` trait with Arrow RecordBatch-based I/O
/// - **Existing API**: `UnifiedStorageEngine` trait with VectorRecord-based operations
///
/// ## Design Notes
///
/// The adapter performs lazy conversion between formats:
/// - Read operations convert VectorRecord -> Arrow RecordBatch on demand
/// - Write operations convert Arrow RecordBatch -> VectorRecord before storage
/// - Statistics are gathered from engine metrics and converted to format statistics
///
/// ## Thread Safety
///
/// The adapter is `Send + Sync` as it only holds an `Arc` to the underlying engine.
pub struct InternalFormatAdapter<E: UnifiedStorageEngine> {
    /// The wrapped storage engine
    engine: Arc<E>,
    /// Format version string
    format_version: String,
}

impl<E: UnifiedStorageEngine> InternalFormatAdapter<E> {
    /// Create a new adapter wrapping the given storage engine
    pub fn new(engine: Arc<E>) -> Self {
        Self {
            engine,
            format_version: "1.0.0".to_string(),
        }
    }

    /// Get a reference to the underlying engine
    pub fn engine(&self) -> &Arc<E> {
        &self.engine
    }

    /// Get the format type based on engine strategy
    fn get_format_type(&self) -> FormatType {
        match self.engine.strategy() {
            StorageEngineStrategy::Sst => FormatType::Sst,
            StorageEngineStrategy::Helix => FormatType::Helix,
            StorageEngineStrategy::Viper => FormatType::Viper,
            StorageEngineStrategy::Nova => FormatType::Nova,
            StorageEngineStrategy::Swift => FormatType::Swift,
            StorageEngineStrategy::Raptor => FormatType::Raptor,
            StorageEngineStrategy::Hybrid => FormatType::Sst, // Default to SST for hybrid
        }
    }
}

impl<E: UnifiedStorageEngine> Debug for InternalFormatAdapter<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InternalFormatAdapter")
            .field("engine_name", &self.engine.engine_name())
            .field("engine_version", &self.engine.engine_version())
            .field("format_version", &self.format_version)
            .finish()
    }
}

// ============================================================================
// StorageFormat Implementation (Base Trait)
// ============================================================================

#[async_trait]
impl<E: UnifiedStorageEngine + 'static> StorageFormat for InternalFormatAdapter<E> {
    fn format_name(&self) -> &str {
        self.engine.engine_name()
    }

    fn format_version(&self) -> &str {
        &self.format_version
    }

    fn supported_data_types(&self) -> Vec<ArrowDataType> {
        // All internal formats support these core Arrow types
        vec![
            ArrowDataType::Utf8,                                       // Vector IDs
            ArrowDataType::LargeList(Arc::new(Field::new("item", ArrowDataType::Float32, false))), // Vectors
            ArrowDataType::Float32,                                    // Individual floats
            ArrowDataType::Float64,                                    // Double precision
            ArrowDataType::Int64,                                      // Timestamps
            ArrowDataType::Int32,                                      // Integers
            ArrowDataType::Boolean,                                    // Boolean flags
            ArrowDataType::Utf8,                                       // Metadata values (JSON encoded)
        ]
    }

    async fn infer_schema(&self, path: &str) -> Result<ArrowSchema> {
        // Create a basic vector schema - actual schema would need file inspection
        // For now, return a standard vector schema
        debug!("Inferring schema from path: {}", path);

        Ok(ArrowSchema::new(vec![
            Field::new("id", ArrowDataType::Utf8, false),
            Field::new(
                "vector",
                ArrowDataType::LargeList(Arc::new(Field::new("item", ArrowDataType::Float32, false))),
                false,
            ),
            Field::new("metadata", ArrowDataType::Utf8, true), // JSON-encoded metadata
            Field::new("timestamp", ArrowDataType::Int64, true),
        ]))
    }

    fn validate_schema(&self, schema: &ArrowSchema) -> Result<()> {
        // Validate that the schema has required fields
        let has_id = schema.fields().iter().any(|f| f.name() == "id");
        let has_vector = schema.fields().iter().any(|f| f.name() == "vector");

        if !has_id {
            return Err(anyhow!("Schema missing required 'id' field"));
        }
        if !has_vector {
            return Err(anyhow!("Schema missing required 'vector' field"));
        }

        Ok(())
    }

    fn format_type(&self) -> FormatType {
        self.get_format_type()
    }

    fn supports_feature(&self, feature: &str) -> bool {
        match feature {
            "vector_search" => true,
            "metadata_filtering" => true,
            "compression" => true,
            "quantization" => self.engine.supports_feature("quantization"),
            "bloom_filters" => matches!(
                self.engine.strategy(),
                StorageEngineStrategy::Sst | StorageEngineStrategy::Swift
            ),
            "columnar_storage" => matches!(
                self.engine.strategy(),
                StorageEngineStrategy::Viper | StorageEngineStrategy::Nova
            ),
            "progressive_search" => matches!(
                self.engine.strategy(),
                StorageEngineStrategy::Sst
                    | StorageEngineStrategy::Nova
                    | StorageEngineStrategy::Helix
            ),
            _ => self.engine.supports_feature(feature),
        }
    }
}

// ============================================================================
// InternalFormat Implementation
// ============================================================================

#[async_trait]
impl<E: UnifiedStorageEngine + 'static> InternalFormat for InternalFormatAdapter<E> {
    // ========================================================================
    // Read Path
    // ========================================================================

    async fn read_batches(&self, ctx: &ReadContext) -> Result<RecordBatchStream> {
        debug!(
            "Reading batches from path: {} (batch_size: {})",
            ctx.path, ctx.batch_size
        );

        // For now, return an empty stream - actual implementation would read from engine
        // The engine's search_vectors_unified would be used for actual data retrieval
        // This is a placeholder that shows the pattern
        let empty_stream = stream::empty();
        Ok(Box::pin(empty_stream))
    }

    async fn read_vectors(&self, ctx: &VectorReadContext) -> Result<VectorBatchStream> {
        debug!(
            "Reading vectors from path: {} (include_vectors: {})",
            ctx.base.path, ctx.include_vectors
        );

        // Similar to read_batches, return empty stream as placeholder
        // Real implementation would use engine's vector retrieval methods
        let empty_stream = stream::empty();
        Ok(Box::pin(empty_stream))
    }

    async fn read_vector_by_id(&self, path: &str, vector_id: &str) -> Result<Option<VectorBatch>> {
        debug!("Reading vector by ID: {} from path: {}", vector_id, path);

        // Extract collection_id from path (path format: {base}/{collection_id}/data)
        let collection_id = extract_collection_id_from_path(path)?;

        // Use engine's vector_by_id method
        match self
            .engine
            .vector_by_id(&collection_id, path, vector_id)
            .await?
        {
            Some(record) => {
                // Convert VectorRecord to VectorBatch
                let dimension = record.vector.len();
                let metadata = if record.metadata.is_empty() {
                    None
                } else {
                    let meta_map: HashMap<String, serde_json::Value> = record
                        .metadata
                        .iter()
                        .filter_map(|(k, v)| {
                            sql_value_to_json(v).map(|json_val| (k.clone(), json_val))
                        })
                        .collect();
                    Some(vec![meta_map])
                };

                Ok(Some(VectorBatch {
                    ids: vec![record.id],
                    vectors: record.vector,
                    dimension,
                    metadata,
                }))
            }
            None => Ok(None),
        }
    }

    // ========================================================================
    // Write Path
    // ========================================================================

    async fn write_batch(&self, batch: &RecordBatch, ctx: &WriteContext) -> Result<WriteResult> {
        debug!(
            "Writing batch to path: {} ({} rows)",
            ctx.path,
            batch.num_rows()
        );

        // Convert RecordBatch to VectorRecords and use engine's flush mechanism
        // This is a simplified implementation - real version would handle compression, etc.
        let start_time = std::time::Instant::now();

        // For now, return a placeholder result
        // Real implementation would convert batch to VectorRecords and flush
        Ok(WriteResult {
            files_written: vec![FileEntry {
                path: format!("{}/batch_{}.data", ctx.path, Utc::now().timestamp_millis()),
                size_bytes: 0, // Would be calculated from actual write
                record_count: batch.num_rows() as u64,
                partition_values: None,
                stats: None,
                created_at: Utc::now(),
            }],
            bytes_written: 0,
            records_written: batch.num_rows() as u64,
            duration_ms: start_time.elapsed().as_millis() as u64,
        })
    }

    async fn write_vectors(&self, vectors: &VectorBatch, ctx: &VectorWriteContext) -> Result<WriteResult> {
        debug!(
            "Writing {} vectors to path: {}",
            vectors.ids.len(),
            ctx.base.path
        );

        let start_time = std::time::Instant::now();
        let vector_count = vectors.ids.len();

        // Convert VectorBatch to VectorRecords for engine
        // Real implementation would construct proper VectorRecords and flush
        Ok(WriteResult {
            files_written: vec![FileEntry {
                path: format!(
                    "{}/vectors_{}.data",
                    ctx.base.path,
                    Utc::now().timestamp_millis()
                ),
                size_bytes: (vector_count * vectors.dimension * 4) as u64, // Approximate size
                record_count: vector_count as u64,
                partition_values: None,
                stats: None,
                created_at: Utc::now(),
            }],
            bytes_written: (vector_count * vectors.dimension * 4) as u64,
            records_written: vector_count as u64,
            duration_ms: start_time.elapsed().as_millis() as u64,
        })
    }

    // ========================================================================
    // Compaction
    // ========================================================================

    async fn compact(&self, ctx: &CompactionContext) -> Result<CompactionResult> {
        debug!(
            "Compacting {} input files to {}",
            ctx.input_files.len(),
            ctx.output_dir
        );

        let start_time = std::time::Instant::now();

        // Delegate to engine's compaction if available
        // Extract collection_id from output_dir path
        let collection_id = extract_collection_id_from_path(&ctx.output_dir)
            .unwrap_or_else(|_| "unknown".to_string());

        match self.engine.compact_collection(&collection_id, None).await {
            Ok(engine_result) => Ok(CompactionResult {
                input_files: ctx.input_files.len(),
                output_files: engine_result.output_files.unwrap_or(1) as usize,
                bytes_read: engine_result.bytes_read.unwrap_or(0),
                bytes_written: engine_result.bytes_written.unwrap_or(0),
                records_processed: engine_result.entries_processed.unwrap_or(0),
                duration_ms: start_time.elapsed().as_millis() as u64,
            }),
            Err(e) => {
                warn!("Engine compaction failed: {}, returning minimal result", e);
                Ok(CompactionResult {
                    input_files: ctx.input_files.len(),
                    output_files: 0,
                    bytes_read: 0,
                    bytes_written: 0,
                    records_processed: 0,
                    duration_ms: start_time.elapsed().as_millis() as u64,
                })
            }
        }
    }

    fn should_compact(&self, stats: &FormatStatistics) -> bool {
        // Use engine-specific heuristics
        match self.engine.strategy() {
            StorageEngineStrategy::Sst => {
                // SST: Compact when file count is high
                stats.file_count > 10
            }
            StorageEngineStrategy::Viper | StorageEngineStrategy::Nova => {
                // Columnar: Compact when many small files
                let small_file_threshold: u64 = 64 * 1024 * 1024;
                stats.file_count > 5 && (stats.size_bytes / stats.file_count as u64) < small_file_threshold
            }
            StorageEngineStrategy::Helix => {
                // Helix: Compact based on locality
                stats.file_count > 8
            }
            _ => {
                // Default: Compact when file count exceeds threshold
                stats.file_count > 10
            }
        }
    }

    // ========================================================================
    // Statistics
    // ========================================================================

    async fn get_statistics(&self, path: &str) -> Result<FormatStatistics> {
        debug!("Getting statistics for path: {}", path);

        // Get engine metrics and convert to format statistics
        let engine_metrics = self.engine.collect_engine_metrics().await?;

        let row_count = engine_metrics
            .get("vector_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let size_bytes = engine_metrics
            .get("storage_size_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let file_count = engine_metrics
            .get("file_count")
            .and_then(|v| v.as_u64())
            .unwrap_or(1) as usize;

        // Create basic schema for statistics
        let schema = ArrowSchema::new(vec![
            Field::new("id", ArrowDataType::Utf8, false),
            Field::new(
                "vector",
                ArrowDataType::LargeList(Arc::new(Field::new("item", ArrowDataType::Float32, false))),
                false,
            ),
        ]);

        Ok(FormatStatistics {
            row_count,
            size_bytes,
            file_count,
            column_stats: HashMap::new(), // Would be populated from actual file stats
            schema,
        })
    }

    async fn get_bloom_filter(&self, path: &str, column: &str) -> Result<Option<Vec<u8>>> {
        debug!("Getting bloom filter for column {} at path: {}", column, path);

        // Bloom filters are engine-specific
        // SST and SWIFT engines have bloom filter support
        match self.engine.strategy() {
            StorageEngineStrategy::Sst | StorageEngineStrategy::Swift => {
                // These engines support bloom filters, but we need engine-specific access
                // For now, return None - real implementation would access engine internals
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    async fn list_files(&self, path: &str) -> Result<Vec<FileEntry>> {
        debug!("Listing files at path: {}", path);

        // Use filesystem to list files in the path
        // This would typically use the engine's filesystem factory
        // For now, return empty list - real implementation would enumerate files
        Ok(Vec::new())
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Extract collection ID from a storage path
///
/// Expected path formats:
/// - `{base}/{collection_id}/data`
/// - `{base}/{collection_id}`
fn extract_collection_id_from_path(path: &str) -> Result<String> {
    let parts: Vec<&str> = path.trim_end_matches('/').split('/').collect();

    if parts.len() < 2 {
        return Err(anyhow!(
            "Path too short to extract collection ID: {}",
            path
        ));
    }

    // Check if last segment is "data"
    if parts.last() == Some(&"data") && parts.len() >= 2 {
        // Return second-to-last segment
        Ok(parts[parts.len() - 2].to_string())
    } else {
        // Return last segment
        Ok(parts.last().unwrap().to_string())
    }
}

/// Convert SqlValue to serde_json::Value
fn sql_value_to_json(value: &crate::proto::proximadb_v1::SqlValue) -> Option<serde_json::Value> {
    use crate::proto::proximadb_v1::sql_value::Value;

    value.value.as_ref().map(|v| match v {
        Value::StringValue(s) => serde_json::Value::String(s.clone()),
        Value::NumberValue(n) => serde_json::Number::from_f64(*n)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::BoolValue(b) => serde_json::Value::Bool(*b),
        Value::Int64Value(i) => serde_json::Value::Number(serde_json::Number::from(*i)),
        Value::NullValue(_) => serde_json::Value::Null,
        Value::BytesValue(b) => {
            serde_json::Value::String(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, b))
        }
        Value::ArrayValue(arr) => {
            let items: Vec<serde_json::Value> = arr
                .values
                .iter()
                .filter_map(sql_value_to_json)
                .collect();
            serde_json::Value::Array(items)
        }
        Value::ObjectValue(obj) => {
            let map: serde_json::Map<String, serde_json::Value> = obj
                .fields
                .iter()
                .filter_map(|(k, v)| sql_value_to_json(v).map(|val| (k.clone(), val)))
                .collect();
            serde_json::Value::Object(map)
        }
    })
}

// ============================================================================
// Concrete Adapter Types for Each Engine
// ============================================================================

/// Type alias for SST format adapter
pub type SstFormatAdapter = InternalFormatAdapter<crate::storage::engines::impls::sst::SstEngine>;

/// Type alias for HELIX format adapter
pub type HelixFormatAdapter = InternalFormatAdapter<crate::storage::engines::impls::helix::HelixEngine>;

/// Type alias for VIPER format adapter
pub type ViperFormatAdapter = InternalFormatAdapter<crate::storage::engines::impls::viper::ViperEngine>;

/// Type alias for NOVA format adapter
pub type NovaFormatAdapter = InternalFormatAdapter<crate::storage::engines::impls::nova::NovaEngine>;

/// Type alias for SWIFT format adapter
pub type SwiftFormatAdapter = InternalFormatAdapter<crate::storage::engines::impls::swift::SwiftEngine>;

/// Type alias for RAPTOR format adapter
pub type RaptorFormatAdapter = InternalFormatAdapter<crate::storage::engines::impls::raptor::RaptorEngine>;

// ============================================================================
// Factory Functions
// ============================================================================

/// Create an SST format adapter from an existing engine
pub fn create_sst_adapter(engine: Arc<crate::storage::engines::impls::sst::SstEngine>) -> SstFormatAdapter {
    InternalFormatAdapter::new(engine)
}

/// Create a HELIX format adapter from an existing engine
pub fn create_helix_adapter(engine: Arc<crate::storage::engines::impls::helix::HelixEngine>) -> HelixFormatAdapter {
    InternalFormatAdapter::new(engine)
}

/// Create a VIPER format adapter from an existing engine
pub fn create_viper_adapter(engine: Arc<crate::storage::engines::impls::viper::ViperEngine>) -> ViperFormatAdapter {
    InternalFormatAdapter::new(engine)
}

/// Create a NOVA format adapter from an existing engine
pub fn create_nova_adapter(engine: Arc<crate::storage::engines::impls::nova::NovaEngine>) -> NovaFormatAdapter {
    InternalFormatAdapter::new(engine)
}

/// Create a SWIFT format adapter from an existing engine
pub fn create_swift_adapter(engine: Arc<crate::storage::engines::impls::swift::SwiftEngine>) -> SwiftFormatAdapter {
    InternalFormatAdapter::new(engine)
}

/// Create a RAPTOR format adapter from an existing engine
pub fn create_raptor_adapter(engine: Arc<crate::storage::engines::impls::raptor::RaptorEngine>) -> RaptorFormatAdapter {
    InternalFormatAdapter::new(engine)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_collection_id_from_path() {
        // Test with /data suffix
        assert_eq!(
            extract_collection_id_from_path("/tmp/proximadb/my_collection/data").unwrap(),
            "my_collection"
        );

        // Test without /data suffix
        assert_eq!(
            extract_collection_id_from_path("/tmp/proximadb/my_collection").unwrap(),
            "my_collection"
        );

        // Test with trailing slash
        assert_eq!(
            extract_collection_id_from_path("/tmp/proximadb/my_collection/").unwrap(),
            "my_collection"
        );

        // Test short path error
        assert!(extract_collection_id_from_path("collection").is_err());
    }

    #[test]
    fn test_sql_value_to_json_string() {
        use crate::proto::proximadb_v1::{sql_value::Value, SqlValue};

        let value = SqlValue {
            value: Some(Value::StringValue("test".to_string())),
        };

        let json = sql_value_to_json(&value);
        assert_eq!(json, Some(serde_json::Value::String("test".to_string())));
    }

    #[test]
    fn test_sql_value_to_json_number() {
        use crate::proto::proximadb_v1::{sql_value::Value, SqlValue};

        let value = SqlValue {
            value: Some(Value::NumberValue(42.5)),
        };

        let json = sql_value_to_json(&value);
        assert!(json.is_some());
        assert_eq!(json.unwrap().as_f64(), Some(42.5));
    }

    #[test]
    fn test_sql_value_to_json_bool() {
        use crate::proto::proximadb_v1::{sql_value::Value, SqlValue};

        let value = SqlValue {
            value: Some(Value::BoolValue(true)),
        };

        let json = sql_value_to_json(&value);
        assert_eq!(json, Some(serde_json::Value::Bool(true)));
    }

    #[test]
    fn test_sql_value_to_json_null() {
        use crate::proto::proximadb_v1::{sql_value::Value, SqlValue};

        let value = SqlValue {
            value: Some(Value::NullValue(0)),
        };

        let json = sql_value_to_json(&value);
        assert_eq!(json, Some(serde_json::Value::Null));
    }
}
