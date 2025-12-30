//! # Hadoop InputFormat/OutputFormat Compatibility Shim
//!
//! Provides Hadoop InputFormat and OutputFormat interfaces for legacy integration
//! with Hive, EMR, and other Hadoop ecosystem tools.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                      Hadoop Ecosystem (Legacy)                          │
//! │  ┌─────────────────────┐    ┌─────────────────────┐                    │
//! │  │ Hive SerDe          │    │ MapReduce Job       │                    │
//! │  └─────────────────────┘    └─────────────────────┘                    │
//! │            │                          │                                 │
//! └────────────┼──────────────────────────┼─────────────────────────────────┘
//!              │ InputFormat               │ OutputFormat
//!              ▼                           ▼
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                     Hadoop Compatibility Shim                           │
//! │  ┌─────────────────────┐    ┌─────────────────────┐                    │
//! │  │ ProximaInputFormat  │    │ ProximaOutputFormat │                    │
//! │  │ (Arrow-to-Writable) │    │ (Writable-to-Arrow) │                    │
//! │  └─────────────────────┘    └─────────────────────┘                    │
//! └─────────────────────────────────────────────────────────────────────────┘
//!              │                           │
//!              ▼                           ▼
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │                    Arrow-Native TableProvider                           │
//! │              (SST, HELIX, SWIFT, NOVA, VIPER, RAPTOR)                  │
//! └─────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage with Hive
//!
//! ```sql
//! -- Create external table using ProximaDB InputFormat
//! CREATE EXTERNAL TABLE embeddings_ext (
//!     id STRING,
//!     embedding ARRAY<FLOAT>,
//!     metadata MAP<STRING, STRING>
//! )
//! STORED BY 'org.proximadb.hive.ProximaStorageHandler'
//! TBLPROPERTIES (
//!     'proximadb.collection' = 'embeddings',
//!     'proximadb.host' = 'localhost:5678'
//! );
//!
//! -- Query with vector search pushed down
//! SELECT * FROM embeddings_ext
//! WHERE proximadb_knn(embedding, array(0.1, 0.2, ...), 10) > 0.8;
//! ```
//!
//! ## Usage with MapReduce
//!
//! ```java
//! // Configure job with ProximaDB InputFormat
//! job.setInputFormatClass(ProximaInputFormat.class);
//! ProximaInputFormat.setCollection(job, "embeddings");
//! ProximaInputFormat.setHost(job, "localhost:5678");
//!
//! // Run MapReduce job
//! job.waitForCompletion(true);
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use arrow::array::RecordBatch;
use arrow::datatypes::Schema as ArrowSchema;
use serde::{Deserialize, Serialize};

use crate::storage::formats::{FileSplit, SplitStatistics, SplitType};

/// Configuration for Hadoop compatibility shim
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HadoopShimConfig {
    /// ProximaDB host
    pub host: String,
    /// ProximaDB port
    pub port: u16,
    /// Collection name
    pub collection: String,
    /// Authentication token
    pub auth_token: Option<String>,
    /// MapReduce split size hint (bytes)
    pub split_size_hint: u64,
    /// Enable speculative execution
    pub speculative_execution: bool,
    /// Maximum map tasks
    pub max_map_tasks: Option<usize>,
    /// SerDe class name
    pub serde_class: String,
}

impl Default for HadoopShimConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 5678,
            collection: String::new(),
            auth_token: None,
            split_size_hint: 128 * 1024 * 1024, // 128MB
            speculative_execution: true,
            max_map_tasks: None,
            serde_class: "org.proximadb.hive.ProximaSerDe".to_string(),
        }
    }
}

/// Hadoop InputSplit - represents a chunk of data for a single map task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HadoopInputSplit {
    /// Split ID
    pub split_id: String,
    /// Underlying ProximaDB file split
    pub file_split: FileSplit,
    /// Split length in bytes (for scheduling)
    pub length: u64,
    /// Preferred locations for this split
    pub locations: Vec<String>,
}

impl HadoopInputSplit {
    /// Create from ProximaDB FileSplit
    pub fn from_file_split(file_split: FileSplit) -> Self {
        let locations = file_split.locality.preferred_hosts.clone();
        let length = file_split.length;
        let split_id = file_split.split_id.clone();

        Self {
            split_id,
            file_split,
            length,
            locations,
        }
    }

    /// Get split length (for Hadoop scheduling)
    pub fn get_length(&self) -> u64 {
        self.length
    }

    /// Get locations (for Hadoop locality)
    pub fn get_locations(&self) -> &[String] {
        &self.locations
    }

    /// Serialize to bytes for Hadoop
    pub fn write_fields(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Deserialize from bytes
    pub fn read_fields(data: &[u8]) -> Option<Self> {
        serde_json::from_slice(data).ok()
    }
}

/// ProximaDB InputFormat - Hadoop InputFormat implementation
///
/// This is a thin shim that wraps the Arrow-native TableProvider
/// and presents a Hadoop-compatible interface.
pub struct ProximaInputFormat {
    /// Configuration
    config: HadoopShimConfig,
}

impl ProximaInputFormat {
    /// Create new InputFormat
    pub fn new(config: HadoopShimConfig) -> Self {
        Self { config }
    }

    /// Get input splits for a job
    ///
    /// This maps to Arrow-native FileSplit generation and wraps them
    /// in HadoopInputSplit for compatibility.
    pub fn get_splits(&self, num_splits_hint: usize) -> Vec<HadoopInputSplit> {
        // TODO: Query ProximaDB for actual splits
        // For now, return a placeholder split
        let file_split = FileSplit {
            split_id: format!("{}:0", self.config.collection),
            file_path: String::new(),
            offset: 0,
            length: self.config.split_size_hint,
            split_type: SplitType::ByteRange { estimated_records: 0 },
            statistics: SplitStatistics::default(),
            locality: crate::storage::formats::SplitLocality::default(),
        };

        vec![HadoopInputSplit::from_file_split(file_split)]
    }

    /// Create a record reader for a split
    pub fn create_record_reader(&self, split: &HadoopInputSplit) -> ProximaRecordReader {
        ProximaRecordReader::new(split.clone(), self.config.clone())
    }
}

/// ProximaDB RecordReader - reads records from a split
///
/// Converts Arrow RecordBatches to Hadoop WritableComparable format.
pub struct ProximaRecordReader {
    /// Input split being read
    split: HadoopInputSplit,
    /// Configuration
    config: HadoopShimConfig,
    /// Current batch
    current_batch: Option<RecordBatch>,
    /// Current row in batch
    current_row: usize,
    /// Total rows read
    total_rows_read: u64,
    /// Whether reader is exhausted
    exhausted: bool,
}

impl ProximaRecordReader {
    /// Create new record reader
    pub fn new(split: HadoopInputSplit, config: HadoopShimConfig) -> Self {
        Self {
            split,
            config,
            current_batch: None,
            current_row: 0,
            total_rows_read: 0,
            exhausted: false,
        }
    }

    /// Initialize the reader
    pub fn initialize(&mut self) -> Result<(), HadoopError> {
        // TODO: Connect to ProximaDB and fetch first batch
        Ok(())
    }

    /// Read next key-value pair
    ///
    /// Returns false when no more records.
    pub fn next(&mut self) -> bool {
        if self.exhausted {
            return false;
        }

        // TODO: Implement actual reading from ProximaDB
        self.exhausted = true;
        false
    }

    /// Get current key (record ID)
    pub fn get_current_key(&self) -> HadoopWritable {
        HadoopWritable::Text(String::new())
    }

    /// Get current value (record data)
    pub fn get_current_value(&self) -> HadoopWritable {
        HadoopWritable::MapWritable(HashMap::new())
    }

    /// Get progress (0.0 to 1.0)
    pub fn get_progress(&self) -> f32 {
        if self.exhausted { 1.0 } else { 0.0 }
    }

    /// Close the reader
    pub fn close(&mut self) {
        self.exhausted = true;
    }
}

/// ProximaDB OutputFormat - Hadoop OutputFormat implementation
pub struct ProximaOutputFormat {
    /// Configuration
    config: HadoopShimConfig,
}

impl ProximaOutputFormat {
    /// Create new OutputFormat
    pub fn new(config: HadoopShimConfig) -> Self {
        Self { config }
    }

    /// Check if output is valid
    pub fn check_output_specs(&self) -> Result<(), HadoopError> {
        if self.config.collection.is_empty() {
            return Err(HadoopError {
                message: "Collection name not specified".to_string(),
                code: HadoopErrorCode::InvalidConfiguration,
            });
        }
        Ok(())
    }

    /// Create a record writer
    pub fn create_record_writer(&self, task_id: i32) -> ProximaRecordWriter {
        ProximaRecordWriter::new(self.config.clone(), task_id)
    }

    /// Get output committer
    pub fn get_output_committer(&self) -> ProximaOutputCommitter {
        ProximaOutputCommitter::new(self.config.clone())
    }
}

/// ProximaDB RecordWriter - writes records to ProximaDB
pub struct ProximaRecordWriter {
    /// Configuration
    config: HadoopShimConfig,
    /// Task ID
    task_id: i32,
    /// Records written
    records_written: u64,
    /// Bytes written
    bytes_written: u64,
    /// Batch buffer
    batch_buffer: Vec<HashMap<String, HadoopWritable>>,
    /// Batch size threshold
    batch_size: usize,
}

impl ProximaRecordWriter {
    /// Create new record writer
    pub fn new(config: HadoopShimConfig, task_id: i32) -> Self {
        Self {
            config,
            task_id,
            records_written: 0,
            bytes_written: 0,
            batch_buffer: Vec::new(),
            batch_size: 1000,
        }
    }

    /// Write a key-value pair
    pub fn write(&mut self, _key: &HadoopWritable, value: &HadoopWritable) -> Result<(), HadoopError> {
        // Convert Writable to record and buffer
        if let HadoopWritable::MapWritable(map) = value {
            self.batch_buffer.push(map.clone());

            if self.batch_buffer.len() >= self.batch_size {
                self.flush_batch()?;
            }
        }
        Ok(())
    }

    /// Flush buffered records to ProximaDB
    fn flush_batch(&mut self) -> Result<(), HadoopError> {
        if self.batch_buffer.is_empty() {
            return Ok(());
        }

        // TODO: Convert batch to Arrow and write to ProximaDB
        self.records_written += self.batch_buffer.len() as u64;
        self.batch_buffer.clear();

        Ok(())
    }

    /// Close the writer
    pub fn close(&mut self) -> Result<(), HadoopError> {
        self.flush_batch()
    }
}

/// ProximaDB Output Committer - handles job/task commit protocol
pub struct ProximaOutputCommitter {
    /// Configuration
    config: HadoopShimConfig,
}

impl ProximaOutputCommitter {
    /// Create new committer
    pub fn new(config: HadoopShimConfig) -> Self {
        Self { config }
    }

    /// Setup the job
    pub fn setup_job(&self) -> Result<(), HadoopError> {
        // TODO: Create temporary output directory if needed
        Ok(())
    }

    /// Setup a task
    pub fn setup_task(&self, _task_id: i32) -> Result<(), HadoopError> {
        Ok(())
    }

    /// Check if task needs commit
    pub fn needs_task_commit(&self, _task_id: i32) -> bool {
        true
    }

    /// Commit a task
    pub fn commit_task(&self, _task_id: i32) -> Result<(), HadoopError> {
        // TODO: Move task output from temp to final location
        Ok(())
    }

    /// Abort a task
    pub fn abort_task(&self, _task_id: i32) -> Result<(), HadoopError> {
        // TODO: Clean up task temp output
        Ok(())
    }

    /// Commit the job
    pub fn commit_job(&self) -> Result<(), HadoopError> {
        // TODO: Finalize all task outputs
        Ok(())
    }

    /// Abort the job
    pub fn abort_job(&self) -> Result<(), HadoopError> {
        // TODO: Clean up all temp outputs
        Ok(())
    }

    /// Check if recovery is supported
    pub fn is_recovery_supported(&self) -> bool {
        true
    }

    /// Recover a task
    pub fn recover_task(&self, _task_id: i32) -> Result<(), HadoopError> {
        Ok(())
    }
}

/// Hadoop Writable types for data exchange
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HadoopWritable {
    /// Null value
    NullWritable,
    /// Boolean
    BooleanWritable(bool),
    /// Byte (i8)
    ByteWritable(i8),
    /// Short (i16)
    ShortWritable(i16),
    /// Int (i32)
    IntWritable(i32),
    /// Long (i64)
    LongWritable(i64),
    /// Float
    FloatWritable(f32),
    /// Double
    DoubleWritable(f64),
    /// Text (String)
    Text(String),
    /// Bytes
    BytesWritable(Vec<u8>),
    /// Array of Writables
    ArrayWritable(Vec<HadoopWritable>),
    /// Map of Writables
    MapWritable(HashMap<String, HadoopWritable>),
}

impl HadoopWritable {
    /// Convert to serde_json::Value
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::NullWritable => serde_json::Value::Null,
            Self::BooleanWritable(b) => serde_json::json!(b),
            Self::ByteWritable(b) => serde_json::json!(b),
            Self::ShortWritable(s) => serde_json::json!(s),
            Self::IntWritable(i) => serde_json::json!(i),
            Self::LongWritable(l) => serde_json::json!(l),
            Self::FloatWritable(f) => serde_json::json!(f),
            Self::DoubleWritable(d) => serde_json::json!(d),
            Self::Text(t) => serde_json::json!(t),
            Self::BytesWritable(b) => serde_json::json!(base64::encode(b)),
            Self::ArrayWritable(arr) => {
                serde_json::json!(arr.iter().map(|w| w.to_json()).collect::<Vec<_>>())
            }
            Self::MapWritable(map) => {
                serde_json::json!(map.iter().map(|(k, v)| (k.clone(), v.to_json())).collect::<HashMap<_, _>>())
            }
        }
    }

    /// Create from serde_json::Value
    pub fn from_json(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::NullWritable,
            serde_json::Value::Bool(b) => Self::BooleanWritable(*b),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    if i >= i32::MIN as i64 && i <= i32::MAX as i64 {
                        Self::IntWritable(i as i32)
                    } else {
                        Self::LongWritable(i)
                    }
                } else if let Some(f) = n.as_f64() {
                    Self::DoubleWritable(f)
                } else {
                    Self::NullWritable
                }
            }
            serde_json::Value::String(s) => Self::Text(s.clone()),
            serde_json::Value::Array(arr) => {
                Self::ArrayWritable(arr.iter().map(Self::from_json).collect())
            }
            serde_json::Value::Object(obj) => {
                Self::MapWritable(obj.iter().map(|(k, v)| (k.clone(), Self::from_json(v))).collect())
            }
        }
    }
}

/// Hive SerDe - serializer/deserializer for Hive integration
pub struct ProximaSerDe {
    /// Schema
    schema: Option<Arc<ArrowSchema>>,
    /// Column names
    column_names: Vec<String>,
    /// Column types
    column_types: Vec<HiveType>,
}

/// Hive types for SerDe
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HiveType {
    /// String/varchar
    String,
    /// Boolean
    Boolean,
    /// Tinyint (i8)
    Tinyint,
    /// Smallint (i16)
    Smallint,
    /// Int (i32)
    Int,
    /// Bigint (i64)
    Bigint,
    /// Float
    Float,
    /// Double
    Double,
    /// Binary
    Binary,
    /// Array
    Array(Box<HiveType>),
    /// Map
    Map(Box<HiveType>, Box<HiveType>),
    /// Struct
    Struct(Vec<(String, HiveType)>),
}

impl ProximaSerDe {
    /// Create new SerDe
    pub fn new() -> Self {
        Self {
            schema: None,
            column_names: Vec::new(),
            column_types: Vec::new(),
        }
    }

    /// Initialize with table properties
    pub fn initialize(&mut self, properties: &HashMap<String, String>) -> Result<(), HadoopError> {
        // Parse column names
        if let Some(cols) = properties.get("columns") {
            self.column_names = cols.split(',').map(|s| s.trim().to_string()).collect();
        }

        // Parse column types
        if let Some(types) = properties.get("columns.types") {
            self.column_types = types
                .split(':')
                .map(|t| parse_hive_type(t.trim()))
                .collect();
        }

        Ok(())
    }

    /// Deserialize a row
    pub fn deserialize(&self, _data: &[u8]) -> Result<HadoopWritable, HadoopError> {
        // TODO: Implement actual deserialization
        Ok(HadoopWritable::MapWritable(HashMap::new()))
    }

    /// Serialize a row
    pub fn serialize(&self, _row: &HadoopWritable) -> Result<Vec<u8>, HadoopError> {
        // TODO: Implement actual serialization
        Ok(Vec::new())
    }
}

impl Default for ProximaSerDe {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse Hive type string to HiveType
fn parse_hive_type(type_str: &str) -> HiveType {
    let type_str = type_str.to_lowercase();

    if type_str == "string" || type_str.starts_with("varchar") {
        HiveType::String
    } else if type_str == "boolean" {
        HiveType::Boolean
    } else if type_str == "tinyint" {
        HiveType::Tinyint
    } else if type_str == "smallint" {
        HiveType::Smallint
    } else if type_str == "int" {
        HiveType::Int
    } else if type_str == "bigint" {
        HiveType::Bigint
    } else if type_str == "float" {
        HiveType::Float
    } else if type_str == "double" {
        HiveType::Double
    } else if type_str == "binary" {
        HiveType::Binary
    } else if type_str.starts_with("array<") {
        let inner = &type_str[6..type_str.len() - 1];
        HiveType::Array(Box::new(parse_hive_type(inner)))
    } else if type_str.starts_with("map<") {
        let inner = &type_str[4..type_str.len() - 1];
        let parts: Vec<&str> = inner.splitn(2, ',').collect();
        if parts.len() == 2 {
            HiveType::Map(
                Box::new(parse_hive_type(parts[0].trim())),
                Box::new(parse_hive_type(parts[1].trim())),
            )
        } else {
            HiveType::String
        }
    } else {
        HiveType::String
    }
}

/// Hadoop error
#[derive(Debug)]
pub struct HadoopError {
    /// Error message
    pub message: String,
    /// Error code
    pub code: HadoopErrorCode,
}

impl std::fmt::Display for HadoopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HadoopError({:?}): {}", self.code, self.message)
    }
}

impl std::error::Error for HadoopError {}

/// Hadoop error codes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HadoopErrorCode {
    /// Invalid configuration
    InvalidConfiguration,
    /// IO error
    IO,
    /// Connection error
    Connection,
    /// Authentication error
    Authentication,
    /// Not found
    NotFound,
    /// Internal error
    Internal,
}

/// Base64 encoding helper (minimal implementation)
mod base64 {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(data: &[u8]) -> String {
        let mut result = String::new();
        let mut i = 0;

        while i < data.len() {
            let a = data[i];
            let b = data.get(i + 1).copied().unwrap_or(0);
            let c = data.get(i + 2).copied().unwrap_or(0);

            result.push(ALPHABET[(a >> 2) as usize] as char);
            result.push(ALPHABET[(((a & 0x03) << 4) | (b >> 4)) as usize] as char);

            if i + 1 < data.len() {
                result.push(ALPHABET[(((b & 0x0f) << 2) | (c >> 6)) as usize] as char);
            } else {
                result.push('=');
            }

            if i + 2 < data.len() {
                result.push(ALPHABET[(c & 0x3f) as usize] as char);
            } else {
                result.push('=');
            }

            i += 3;
        }

        result
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hadoop_config_default() {
        let config = HadoopShimConfig::default();
        assert_eq!(config.host, "localhost");
        assert_eq!(config.port, 5678);
        assert!(config.speculative_execution);
    }

    #[test]
    fn test_hadoop_input_split() {
        let file_split = FileSplit {
            split_id: "test:0".to_string(),
            file_path: "/data/file.sst".to_string(),
            offset: 0,
            length: 1024,
            split_type: SplitType::Block { block_id: 0, record_count: 100 },
            statistics: SplitStatistics::default(),
            locality: crate::storage::formats::SplitLocality {
                preferred_hosts: vec!["host1".to_string()],
                ..Default::default()
            },
        };

        let split = HadoopInputSplit::from_file_split(file_split);
        assert_eq!(split.split_id, "test:0");
        assert_eq!(split.get_length(), 1024);
        assert_eq!(split.get_locations(), &["host1"]);
    }

    #[test]
    fn test_hadoop_writable_conversion() {
        let writable = HadoopWritable::IntWritable(42);
        let json = writable.to_json();
        assert_eq!(json, serde_json::json!(42));

        let back = HadoopWritable::from_json(&json);
        match back {
            HadoopWritable::IntWritable(i) => assert_eq!(i, 42),
            _ => panic!("Expected IntWritable"),
        }
    }

    #[test]
    fn test_hadoop_writable_map() {
        let mut map = HashMap::new();
        map.insert("name".to_string(), HadoopWritable::Text("test".to_string()));
        map.insert("count".to_string(), HadoopWritable::IntWritable(5));

        let writable = HadoopWritable::MapWritable(map);
        let json = writable.to_json();

        assert!(json.get("name").is_some());
        assert_eq!(json.get("name").unwrap(), &serde_json::json!("test"));
    }

    #[test]
    fn test_hive_type_parsing() {
        assert!(matches!(parse_hive_type("string"), HiveType::String));
        assert!(matches!(parse_hive_type("int"), HiveType::Int));
        assert!(matches!(parse_hive_type("bigint"), HiveType::Bigint));
        assert!(matches!(parse_hive_type("BOOLEAN"), HiveType::Boolean));

        match parse_hive_type("array<float>") {
            HiveType::Array(inner) => assert!(matches!(*inner, HiveType::Float)),
            _ => panic!("Expected Array"),
        }

        match parse_hive_type("map<string,int>") {
            HiveType::Map(k, v) => {
                assert!(matches!(*k, HiveType::String));
                assert!(matches!(*v, HiveType::Int));
            }
            _ => panic!("Expected Map"),
        }
    }

    #[test]
    fn test_input_format() {
        let config = HadoopShimConfig {
            collection: "test".to_string(),
            ..Default::default()
        };

        let input_format = ProximaInputFormat::new(config);
        let splits = input_format.get_splits(4);
        assert!(!splits.is_empty());
    }

    #[test]
    fn test_output_format() {
        let config = HadoopShimConfig {
            collection: "test".to_string(),
            ..Default::default()
        };

        let output_format = ProximaOutputFormat::new(config);
        assert!(output_format.check_output_specs().is_ok());

        let empty_config = HadoopShimConfig::default();
        let output_format_empty = ProximaOutputFormat::new(empty_config);
        assert!(output_format_empty.check_output_specs().is_err());
    }

    #[test]
    fn test_proxima_serde() {
        let mut serde = ProximaSerDe::new();
        let mut props = HashMap::new();
        props.insert("columns".to_string(), "id,name,value".to_string());
        props.insert("columns.types".to_string(), "string:string:int".to_string());

        assert!(serde.initialize(&props).is_ok());
        assert_eq!(serde.column_names.len(), 3);
        assert_eq!(serde.column_types.len(), 3);
    }

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64::encode(b""), "");
        assert_eq!(base64::encode(b"f"), "Zg==");
        assert_eq!(base64::encode(b"fo"), "Zm8=");
        assert_eq!(base64::encode(b"foo"), "Zm9v");
        assert_eq!(base64::encode(b"foob"), "Zm9vYg==");
    }
}
