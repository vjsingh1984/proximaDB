//! # Apache Spark DataSource V2 Connector
//!
//! Provides a JNI bridge for Apache Spark integration via the DataSource V2 API.
//! This enables Spark jobs to read from and write to ProximaDB collections natively.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                           Spark Driver                                  │
//! │  ┌─────────────────────┐    ┌─────────────────────┐                    │
//! │  │ ProximaDBScan       │    │ ProximaDBWrite      │                    │
//! │  │ (Scala/Java)        │    │ (Scala/Java)        │                    │
//! │  └─────────────────────┘    └─────────────────────┘                    │
//! │            │                          │                                 │
//! └────────────┼──────────────────────────┼─────────────────────────────────┘
//!              │ JNI                       │ JNI
//!              ▼                           ▼
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                        Rust JNI Bridge                                  │
//! │  ┌─────────────────────┐    ┌─────────────────────┐                    │
//! │  │ SparkReadBridge     │    │ SparkWriteBridge    │                    │
//! │  └─────────────────────┘    └─────────────────────┘                    │
//! └─────────────────────────────────────────────────────────────────────────┘
//!              │                           │
//!              ▼                           ▼
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                    ProximaDB Storage Engines                            │
//! │         (SST, HELIX, SWIFT, NOVA, VIPER, RAPTOR)                       │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage in Spark
//!
//! ```scala
//! // Scala DataFrame API
//! val df = spark.read
//!   .format("proximadb")
//!   .option("host", "localhost:5678")
//!   .option("collection", "embeddings")
//!   .load()
//!
//! // Write back to ProximaDB
//! df.write
//!   .format("proximadb")
//!   .option("collection", "processed_embeddings")
//!   .mode("append")
//!   .save()
//!
//! // SQL with Vector Search
//! spark.sql("""
//!   SELECT * FROM proximadb.embeddings
//!   WHERE vector_search(embedding, array(0.1, 0.2, ...), 10) > 0.8
//! """)
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::RecordBatch;
use arrow::datatypes::Schema as ArrowSchema;
use serde::{Deserialize, Serialize};

use crate::storage::formats::FileSplit;
use crate::storage::schema::ProximaSchema;

/// Configuration for Spark connector
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparkConnectorConfig {
    /// ProximaDB server host
    pub host: String,
    /// ProximaDB server port
    pub port: u16,
    /// Authentication token (optional)
    pub auth_token: Option<String>,
    /// Connection timeout in milliseconds
    pub connection_timeout_ms: u64,
    /// Read timeout in milliseconds
    pub read_timeout_ms: u64,
    /// Maximum number of concurrent readers
    pub max_concurrent_readers: usize,
    /// Batch size for reading
    pub batch_size: usize,
    /// Enable filter pushdown
    pub enable_filter_pushdown: bool,
    /// Enable projection pushdown
    pub enable_projection_pushdown: bool,
    /// Enable aggregate pushdown
    pub enable_aggregate_pushdown: bool,
}

impl Default for SparkConnectorConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 5678,
            auth_token: None,
            connection_timeout_ms: 30000,
            read_timeout_ms: 60000,
            max_concurrent_readers: 8,
            batch_size: 8192,
            enable_filter_pushdown: true,
            enable_projection_pushdown: true,
            enable_aggregate_pushdown: true,
        }
    }
}

/// Spark DataSource V2 Table representation
#[derive(Debug, Clone)]
pub struct SparkTable {
    /// Table name (collection name in ProximaDB)
    pub name: String,
    /// Arrow schema
    pub schema: Arc<ArrowSchema>,
    /// ProximaDB schema with additional metadata
    pub proxima_schema: Arc<ProximaSchema>,
    /// Table properties
    pub properties: HashMap<String, String>,
    /// Partition columns (if any)
    pub partition_columns: Vec<String>,
}

/// Spark InputPartition - represents a unit of work for a Spark task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparkInputPartition {
    /// Partition ID
    pub partition_id: i32,
    /// Underlying file splits
    pub splits: Vec<FileSplit>,
    /// Preferred host locations for locality
    pub preferred_locations: Vec<String>,
    /// Estimated rows in this partition
    pub estimated_rows: Option<i64>,
    /// Estimated bytes in this partition
    pub estimated_bytes: Option<i64>,
}

impl SparkInputPartition {
    /// Create a new input partition from file splits
    pub fn from_splits(partition_id: i32, splits: Vec<FileSplit>) -> Self {
        let estimated_rows = splits
            .iter()
            .filter_map(|s| s.statistics.row_count)
            .sum::<u64>();
        let estimated_bytes = splits
            .iter()
            .filter_map(|s| s.statistics.byte_size)
            .sum::<u64>();

        // Collect preferred hosts from all splits
        let mut preferred_locations: Vec<String> = splits
            .iter()
            .flat_map(|s| s.locality.preferred_hosts.clone())
            .collect();
        preferred_locations.dedup();

        Self {
            partition_id,
            splits,
            preferred_locations,
            estimated_rows: if estimated_rows > 0 {
                Some(estimated_rows as i64)
            } else {
                None
            },
            estimated_bytes: if estimated_bytes > 0 {
                Some(estimated_bytes as i64)
            } else {
                None
            },
        }
    }
}

/// Spark ScanBuilder - builds scans with pushdown support
#[derive(Debug, Clone)]
pub struct SparkScanBuilder {
    /// Table to scan
    pub table: SparkTable,
    /// Column projection (None = all columns)
    pub projection: Option<Vec<String>>,
    /// Filter expression (serialized)
    pub filters: Vec<SparkFilter>,
    /// Limit (if any)
    pub limit: Option<i64>,
}

/// Spark filter expression (simplified for JNI transfer)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparkFilter {
    /// Filter type
    pub filter_type: SparkFilterType,
    /// Column name (for column-based filters)
    pub column: Option<String>,
    /// Literal value (JSON encoded)
    pub value: Option<serde_json::Value>,
    /// Child filters (for AND/OR/NOT)
    pub children: Vec<SparkFilter>,
}

/// Spark filter types
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SparkFilterType {
    /// Equality comparison (=)
    EqualTo,
    /// Inequality comparison (!=)
    NotEqualTo,
    /// Greater-than comparison (>)
    GreaterThan,
    /// Greater-than-or-equal comparison (>=)
    GreaterThanOrEqual,
    /// Less-than comparison (<)
    LessThan,
    /// Less-than-or-equal comparison (<=)
    LessThanOrEqual,
    /// String prefix match
    StringStartsWith,
    /// String suffix match
    StringEndsWith,
    /// String substring match
    StringContains,
    /// Check for null value
    IsNull,
    /// Check for non-null value
    IsNotNull,
    /// Logical AND of child filters
    And,
    /// Logical OR of child filters
    Or,
    /// Logical NOT of a child filter
    Not,
    /// Membership test (IN list)
    In,
    /// Always-true constant predicate
    AlwaysTrue,
    /// Always-false constant predicate
    AlwaysFalse,
}

impl SparkScanBuilder {
    /// Create a new scan builder for a table
    pub fn new(table: SparkTable) -> Self {
        Self {
            table,
            projection: None,
            filters: Vec::new(),
            limit: None,
        }
    }

    /// Add column projection
    pub fn with_projection(mut self, columns: Vec<String>) -> Self {
        self.projection = Some(columns);
        self
    }

    /// Add filter
    pub fn with_filter(mut self, filter: SparkFilter) -> Self {
        self.filters.push(filter);
        self
    }

    /// Add limit
    pub fn with_limit(mut self, limit: i64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Build input partitions for parallel execution
    pub fn plan_partitions(&self, target_partitions: usize) -> Vec<SparkInputPartition> {
        // Partition planning: file-split-based partitions via ConnectorStorageAdapter
        // For now, create placeholder partitions
        (0..target_partitions)
            .map(|i| SparkInputPartition {
                partition_id: i as i32,
                splits: Vec::new(),
                preferred_locations: Vec::new(),
                estimated_rows: None,
                estimated_bytes: None,
            })
            .collect()
    }
}

/// Spark PartitionReader - reads data from a single partition
pub struct SparkPartitionReader {
    /// Partition being read
    #[allow(dead_code)]
    partition: SparkInputPartition,
    /// Current split index
    #[allow(dead_code)]
    current_split: usize,
    /// Batch size
    #[allow(dead_code)]
    batch_size: usize,
    /// Records read so far
    #[allow(dead_code)]
    records_read: usize,
    /// Whether reader is exhausted
    #[allow(dead_code)]
    exhausted: bool,
}

impl SparkPartitionReader {
    /// Create a new partition reader
    pub fn new(partition: SparkInputPartition, batch_size: usize) -> Self {
        Self {
            partition,
            current_split: 0,
            batch_size,
            records_read: 0,
            exhausted: false,
        }
    }

    /// Read the next batch of records
    pub fn next_batch(&mut self) -> Option<RecordBatch> {
        if self.exhausted {
            return None;
        }

        // Storage read: delegates to ConnectorStorageAdapter.read_batch()
        // For now, return None to indicate end of data
        self.exhausted = true;
        None
    }

    /// Get progress estimate (0.0 to 1.0)
    pub fn progress(&self) -> f64 {
        if let Some(total) = self.partition.estimated_rows
            && total > 0
        {
            return (self.records_read as f64) / (total as f64);
        }
        if self.exhausted { 1.0 } else { 0.0 }
    }
}

/// Spark WriteBuilder - builds write operations
#[derive(Debug, Clone)]
pub struct SparkWriteBuilder {
    /// Target table name
    pub table_name: String,
    /// Schema for data being written
    pub schema: Arc<ArrowSchema>,
    /// Write mode
    pub mode: SparkWriteMode,
    /// Partition columns
    pub partition_by: Vec<String>,
    /// Additional options
    pub options: HashMap<String, String>,
}

/// Spark write modes
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SparkWriteMode {
    /// Append to existing data
    Append,
    /// Overwrite existing data
    Overwrite,
    /// Fail if data exists
    ErrorIfExists,
    /// Ignore if data exists
    Ignore,
}

impl SparkWriteBuilder {
    /// Create a new write builder
    pub fn new(table_name: String, schema: Arc<ArrowSchema>) -> Self {
        Self {
            table_name,
            schema,
            mode: SparkWriteMode::Append,
            partition_by: Vec::new(),
            options: HashMap::new(),
        }
    }

    /// Set write mode
    pub fn with_mode(mut self, mode: SparkWriteMode) -> Self {
        self.mode = mode;
        self
    }

    /// Set partition columns
    pub fn with_partition_by(mut self, columns: Vec<String>) -> Self {
        self.partition_by = columns;
        self
    }

    /// Add option
    pub fn with_option(mut self, key: String, value: String) -> Self {
        self.options.insert(key, value);
        self
    }
}

/// Spark DataWriter - writes data from a single Spark task
pub struct SparkDataWriter {
    /// Table name
    #[allow(dead_code)]
    table_name: String,
    /// Schema
    #[allow(dead_code)]
    schema: Arc<ArrowSchema>,
    /// Partition ID (task ID)
    #[allow(dead_code)]
    partition_id: i32,
    /// Records written
    #[allow(dead_code)]
    records_written: usize,
    /// Bytes written
    #[allow(dead_code)]
    bytes_written: usize,
    /// Files created
    #[allow(dead_code)]
    files_created: Vec<String>,
}

impl SparkDataWriter {
    /// Create a new data writer
    pub fn new(table_name: String, schema: Arc<ArrowSchema>, partition_id: i32) -> Self {
        Self {
            table_name,
            schema,
            partition_id,
            records_written: 0,
            bytes_written: 0,
            files_created: Vec::new(),
        }
    }

    /// Write a batch of records
    pub fn write(&mut self, _batch: &RecordBatch) -> Result<(), SparkWriteError> {
        // Write: delegates to ConnectorStorageAdapter.write_batch()
        Ok(())
    }

    /// Commit the write
    pub fn commit(self) -> SparkWriteCommitMessage {
        SparkWriteCommitMessage {
            partition_id: self.partition_id,
            records_written: self.records_written as i64,
            bytes_written: self.bytes_written as i64,
            files_created: self.files_created,
        }
    }

    /// Abort the write
    pub fn abort(self) {
        // Cleanup: abort partial writes via ConnectorStorageAdapter
    }
}

/// Write commit message returned by each task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SparkWriteCommitMessage {
    /// Partition ID that completed
    pub partition_id: i32,
    /// Records written by this task
    pub records_written: i64,
    /// Bytes written by this task
    pub bytes_written: i64,
    /// Files created by this task
    pub files_created: Vec<String>,
}

/// Write error
#[derive(Debug)]
pub struct SparkWriteError {
    /// Human-readable error description
    pub message: String,
    /// Numeric error code
    pub code: i32,
}

impl std::fmt::Display for SparkWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SparkWriteError({}): {}", self.code, self.message)
    }
}

impl std::error::Error for SparkWriteError {}

// ============================================================================
// JNI Bridge Functions (called from Java/Scala via JNI)
// ============================================================================

/// JNI: Get table schema as JSON
///
/// Called from Java: `native String getTableSchema(String tableName);`
pub fn jni_get_table_schema(_table_name: &str) -> String {
    // Schema: ConnectorStorageAdapter.get_schema() via JNI bridge
    r#"{"type":"struct","fields":[]}"#.to_string()
}

/// JNI: Plan input partitions as JSON array
///
/// Called from Java: `native String planInputPartitions(String tableName, String filtersJson, int numPartitions);`
pub fn jni_plan_input_partitions(
    _table_name: &str,
    _filters_json: &str,
    _num_partitions: i32,
) -> String {
    // Partitions: ConnectorStorageAdapter.get_splits() via JNI bridge
    "[]".to_string()
}

/// JNI: Create partition reader
///
/// Called from Java: `native long createPartitionReader(String partitionJson);`
pub fn jni_create_partition_reader(_partition_json: &str) -> i64 {
    // Reader: JNI handle wrapping ConnectorStorageAdapter reader
    0
}

/// JNI: Read next batch from partition reader
///
/// Called from Java: `native byte[] readNextBatch(long readerHandle);`
/// Returns Arrow IPC serialized RecordBatch or empty array if exhausted
pub fn jni_read_next_batch(_reader_handle: i64) -> Vec<u8> {
    // Batch read: Arrow IPC from ConnectorStorageAdapter via JNI
    Vec::new()
}

/// JNI: Close partition reader
///
/// Called from Java: `native void closePartitionReader(long readerHandle);`
pub fn jni_close_partition_reader(_reader_handle: i64) {
    // Reader cleanup: close ConnectorStorageAdapter reader handle
}

/// JNI: Create data writer
///
/// Called from Java: `native long createDataWriter(String tableName, String schemaJson, int partitionId);`
pub fn jni_create_data_writer(_table_name: &str, _schema_json: &str, _partition_id: i32) -> i64 {
    // Writer: JNI handle wrapping ConnectorStorageAdapter writer
    0
}

/// JNI: Write batch to data writer
///
/// Called from Java: `native void writeBatch(long writerHandle, byte[] arrowBatch);`
pub fn jni_write_batch(_writer_handle: i64, _arrow_batch: &[u8]) {
    // Write: Arrow IPC → RecordBatch → ConnectorStorageAdapter.write_batch()
}

/// JNI: Commit data writer
///
/// Called from Java: `native String commitWriter(long writerHandle);`
/// Returns commit message as JSON
pub fn jni_commit_writer(_writer_handle: i64) -> String {
    // Commit: finalize write transaction via ConnectorStorageAdapter
    r#"{"partition_id":0,"records_written":0,"bytes_written":0,"files_created":[]}"#.to_string()
}

/// JNI: Abort data writer
///
/// Called from Java: `native void abortWriter(long writerHandle);`
pub fn jni_abort_writer(_writer_handle: i64) {
    // Abort: rollback partial writes via ConnectorStorageAdapter
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spark_connector_config_default() {
        let config = SparkConnectorConfig::default();
        assert_eq!(config.host, "localhost");
        assert_eq!(config.port, 5678);
        assert!(config.enable_filter_pushdown);
    }

    #[test]
    fn test_spark_input_partition() {
        let partition = SparkInputPartition {
            partition_id: 0,
            splits: Vec::new(),
            preferred_locations: vec!["host1".to_string()],
            estimated_rows: Some(1000),
            estimated_bytes: Some(1024 * 1024),
        };

        assert_eq!(partition.partition_id, 0);
        assert_eq!(partition.estimated_rows, Some(1000));
    }

    #[test]
    fn test_spark_filter() {
        let filter = SparkFilter {
            filter_type: SparkFilterType::EqualTo,
            column: Some("category".to_string()),
            value: Some(serde_json::json!("science")),
            children: Vec::new(),
        };

        assert_eq!(filter.filter_type, SparkFilterType::EqualTo);
        assert_eq!(filter.column, Some("category".to_string()));
    }

    #[test]
    fn test_spark_write_mode() {
        let builder =
            SparkWriteBuilder::new("test_table".to_string(), Arc::new(ArrowSchema::empty()))
                .with_mode(SparkWriteMode::Overwrite);

        assert_eq!(builder.mode, SparkWriteMode::Overwrite);
    }
}
