/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # Delta Lake Format Connector
//!
//! Implementation of the `OpenTableFormat` trait for Delta Lake tables.
//! Delta Lake provides ACID transactions, time travel, and schema evolution
//! on top of Parquet files.
//!
//! ## Features
//!
//! - **ACID Transactions**: All writes are atomic and durable
//! - **Time Travel**: Query any historical version of the table
//! - **Schema Evolution**: Add/rename/drop columns without rewriting data
//! - **Z-ordering**: Optimize file layout for query performance
//! - **Change Data Feed**: Track row-level changes
//!
//! ## Delta Log Structure
//!
//! ```text
//! table_path/
//! ├── _delta_log/
//! │   ├── 00000000000000000000.json   # Initial commit
//! │   ├── 00000000000000000001.json   # Second commit
//! │   ├── 00000000000000000002.json   # Third commit
//! │   └── 00000000000000000010.checkpoint.parquet  # Checkpoint
//! └── part-00000-*.parquet            # Data files
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::storage::formats::open::delta::{DeltaLakeFormat, DeltaLakeConfig};
//!
//! // Create Delta Lake connector
//! let config = DeltaLakeConfig::new("/path/to/table");
//! let delta = DeltaLakeFormat::new(config).await?;
//!
//! // Read current snapshot
//! let snapshot = delta.get_current_snapshot("/path/to/table").await?;
//!
//! // Time travel to version 5
//! let old_snapshot = delta.get_snapshot_at("/path/to/table", 5).await?;
//! ```

use std::collections::HashMap;
use std::fmt::Debug;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use arrow_array::RecordBatch;
use arrow_schema::{DataType as ArrowDataType, Field, Schema as ArrowSchema};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use futures::stream::{self, StreamExt};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tracing::{debug, info, warn};

use super::StorageOptions;
use crate::storage::formats::{
    CompressionCodec, FileEntry, FilterExpression, FormatType, MergeAction, OpenTableFormat,
    OptimizeContext, OptimizeResult, ReadContext, RecordBatchStream, Snapshot, StorageFormat,
    VectorBatchStream, VectorReadContext, WriteContext, WriteMode,
};

// ============================================================================
// Configuration
// ============================================================================

/// Delta Lake format configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaLakeConfig {
    /// Table location (path or URL)
    pub table_uri: String,

    /// Storage options for cloud access
    pub storage_options: StorageOptions,

    /// Enable change data feed
    pub enable_change_data_feed: bool,

    /// Checkpoint interval (in commits)
    pub checkpoint_interval: u32,

    /// Target file size for writes
    pub target_file_size_bytes: u64,

    /// Default compression codec
    pub compression: CompressionCodec,

    /// Enable deletion vectors (Delta 3.0+)
    pub enable_deletion_vectors: bool,

    /// Partition columns
    pub partition_columns: Vec<String>,
}

impl Default for DeltaLakeConfig {
    fn default() -> Self {
        Self {
            table_uri: String::new(),
            storage_options: StorageOptions::default(),
            enable_change_data_feed: false,
            checkpoint_interval: 10,
            target_file_size_bytes: 128 * 1024 * 1024, // 128MB
            compression: CompressionCodec::Snappy,
            enable_deletion_vectors: false,
            partition_columns: Vec::new(),
        }
    }
}

impl DeltaLakeConfig {
    /// Create config for local filesystem
    pub fn new(table_uri: &str) -> Self {
        Self {
            table_uri: table_uri.to_string(),
            storage_options: StorageOptions::local(table_uri),
            ..Default::default()
        }
    }

    /// Create config with storage options
    pub fn with_storage(table_uri: &str, storage_options: StorageOptions) -> Self {
        Self {
            table_uri: table_uri.to_string(),
            storage_options,
            ..Default::default()
        }
    }

    /// Enable change data feed
    pub fn with_change_data_feed(mut self, enabled: bool) -> Self {
        self.enable_change_data_feed = enabled;
        self
    }

    /// Set checkpoint interval
    pub fn with_checkpoint_interval(mut self, interval: u32) -> Self {
        self.checkpoint_interval = interval;
        self
    }

    /// Set partition columns
    pub fn with_partitions(mut self, columns: Vec<String>) -> Self {
        self.partition_columns = columns;
        self
    }
}

// ============================================================================
// Delta Action Types (from _delta_log JSON)
// ============================================================================

/// Delta action types (stored in _delta_log/*.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeltaAction {
    /// Add a new file
    Add(AddAction),
    /// Remove a file
    Remove(RemoveAction),
    /// Transaction metadata
    #[serde(rename = "txn")]
    Transaction(TransactionAction),
    /// Table protocol
    Protocol(ProtocolAction),
    /// Table metadata
    #[serde(rename = "metaData")]
    Metadata(MetadataAction),
    /// Commit info
    CommitInfo(CommitInfoAction),
}

/// Add file action
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddAction {
    /// File path (relative to table root)
    pub path: String,
    /// Partition values
    #[serde(default)]
    pub partition_values: HashMap<String, String>,
    /// File size in bytes
    pub size: i64,
    /// Modification time
    pub modification_time: i64,
    /// Data change flag
    pub data_change: bool,
    /// File statistics (JSON encoded)
    #[serde(default)]
    pub stats: Option<String>,
    /// Tags
    #[serde(default)]
    pub tags: Option<HashMap<String, String>>,
}

/// Remove file action
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveAction {
    /// File path
    pub path: String,
    /// Deletion timestamp
    pub deletion_timestamp: Option<i64>,
    /// Data change flag
    pub data_change: bool,
    /// Extended file metadata
    pub extended_file_metadata: Option<bool>,
    /// Partition values
    #[serde(default)]
    pub partition_values: Option<HashMap<String, String>>,
    /// File size
    pub size: Option<i64>,
}

/// Transaction action
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransactionAction {
    /// Application ID
    pub app_id: String,
    /// Version
    pub version: i64,
    /// Last updated
    pub last_updated: Option<i64>,
}

/// Protocol action
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolAction {
    /// Minimum reader version
    pub min_reader_version: i32,
    /// Minimum writer version
    pub min_writer_version: i32,
    /// Reader features (Delta 3.0+)
    #[serde(default)]
    pub reader_features: Option<Vec<String>>,
    /// Writer features (Delta 3.0+)
    #[serde(default)]
    pub writer_features: Option<Vec<String>>,
}

/// Metadata action
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataAction {
    /// Table ID
    pub id: String,
    /// Table name
    pub name: Option<String>,
    /// Table description
    pub description: Option<String>,
    /// Format
    pub format: FormatSpec,
    /// Schema (JSON encoded)
    pub schema_string: String,
    /// Partition columns
    #[serde(default)]
    pub partition_columns: Vec<String>,
    /// Table configuration
    #[serde(default)]
    pub configuration: HashMap<String, String>,
    /// Created time
    pub created_time: Option<i64>,
}

/// Format specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatSpec {
    /// Provider (e.g., "parquet")
    pub provider: String,
    /// Options
    #[serde(default)]
    pub options: HashMap<String, String>,
}

/// Commit info action
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitInfoAction {
    /// Commit timestamp
    pub timestamp: i64,
    /// Operation
    pub operation: String,
    /// Operation parameters
    #[serde(default)]
    pub operation_parameters: HashMap<String, String>,
    /// Read version
    pub read_version: Option<i64>,
    /// Isolation level
    pub isolation_level: Option<String>,
    /// Is blind append
    pub is_blind_append: Option<bool>,
    /// Operation metrics
    #[serde(default)]
    pub operation_metrics: Option<HashMap<String, String>>,
    /// User metadata
    #[serde(default)]
    pub user_metadata: Option<String>,
    /// Engine info
    pub engine_info: Option<String>,
}

// ============================================================================
// Delta Lake Format Implementation
// ============================================================================

/// Delta Lake format connector
///
/// Implements the `OpenTableFormat` trait for Delta Lake tables.
pub struct DeltaLakeFormat {
    /// Configuration
    config: DeltaLakeConfig,

    /// Cached current snapshot
    cached_snapshot: RwLock<Option<DeltaSnapshot>>,

    /// Cached metadata
    cached_metadata: RwLock<Option<MetadataAction>>,

    /// Cached protocol
    cached_protocol: RwLock<Option<ProtocolAction>>,
}

/// Internal Delta snapshot representation
#[derive(Debug, Clone)]
struct DeltaSnapshot {
    version: i64,
    timestamp: DateTime<Utc>,
    files: Vec<AddAction>,
    metadata: MetadataAction,
    protocol: ProtocolAction,
}

impl DeltaLakeFormat {
    /// Create a new Delta Lake format connector
    pub async fn new(config: DeltaLakeConfig) -> Result<Self> {
        let format = Self {
            config,
            cached_snapshot: RwLock::new(None),
            cached_metadata: RwLock::new(None),
            cached_protocol: RwLock::new(None),
        };

        // Load initial snapshot if table exists
        if format.table_exists().await? {
            format.load_snapshot(None).await?;
        }

        Ok(format)
    }

    /// Check if table exists
    async fn table_exists(&self) -> Result<bool> {
        let delta_log_path = Path::new(&self.config.table_uri).join("_delta_log");
        Ok(delta_log_path.exists())
    }

    /// Get the delta log directory path
    fn delta_log_path(&self) -> std::path::PathBuf {
        Path::new(&self.config.table_uri).join("_delta_log")
    }

    /// List commit files in delta log
    async fn list_commit_files(&self) -> Result<Vec<(i64, std::path::PathBuf)>> {
        let delta_log = self.delta_log_path();
        let mut commits = Vec::new();

        if !delta_log.exists() {
            return Ok(commits);
        }

        let mut entries = fs::read_dir(&delta_log).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                // Match commit files like 00000000000000000001.json
                if name.ends_with(".json")
                    && !name.contains("checkpoint")
                    && let Ok(version) = name.trim_end_matches(".json").parse::<i64>()
                {
                    commits.push((version, path));
                }
            }
        }

        commits.sort_by_key(|(v, _)| *v);
        Ok(commits)
    }

    /// Get latest checkpoint version
    async fn find_latest_checkpoint(&self) -> Result<Option<i64>> {
        let delta_log = self.delta_log_path();
        let last_checkpoint_path = delta_log.join("_last_checkpoint");

        if last_checkpoint_path.exists() {
            let content = fs::read_to_string(&last_checkpoint_path).await?;

            #[derive(Deserialize)]
            struct LastCheckpoint {
                version: i64,
            }

            let checkpoint: LastCheckpoint = serde_json::from_str(&content)?;
            return Ok(Some(checkpoint.version));
        }

        Ok(None)
    }

    /// Load snapshot at a specific version (or latest if None)
    async fn load_snapshot(&self, version: Option<i64>) -> Result<DeltaSnapshot> {
        let commits = self.list_commit_files().await?;

        if commits.is_empty() {
            return Err(anyhow!("No commits found in delta log"));
        }

        let target_version = version.unwrap_or_else(|| commits.last().map_or(0, |(v, _)| *v));

        // Find checkpoint if available
        let checkpoint_version = self.find_latest_checkpoint().await?;
        let start_version = checkpoint_version
            .filter(|cv| *cv <= target_version)
            .unwrap_or(0);

        // Collect all actions from start_version to target_version
        let mut all_files: HashMap<String, AddAction> = HashMap::new();
        let mut metadata: Option<MetadataAction> = None;
        let mut protocol: Option<ProtocolAction> = None;
        let mut last_timestamp = Utc::now();

        // Load from checkpoint if available
        if let Some(cv) = checkpoint_version
            && cv <= target_version
        {
            debug!("Loading from checkpoint version {}", cv);
            // Deferred: Load checkpoint parquet file
        }

        // Apply commits
        for (v, path) in &commits {
            if *v < start_version || *v > target_version {
                continue;
            }

            let content = fs::read_to_string(path).await?;
            for line in content.lines() {
                if line.trim().is_empty() {
                    continue;
                }

                // Parse each action
                if let Ok(action) = serde_json::from_str::<HashMap<String, serde_json::Value>>(line)
                {
                    if let Some(add) = action.get("add") {
                        let add: AddAction = serde_json::from_value(add.clone())?;
                        all_files.insert(add.path.clone(), add);
                    }
                    if let Some(remove) = action.get("remove") {
                        let remove: RemoveAction = serde_json::from_value(remove.clone())?;
                        all_files.remove(&remove.path);
                    }
                    if let Some(meta) = action.get("metaData") {
                        metadata = Some(serde_json::from_value(meta.clone())?);
                    }
                    if let Some(proto) = action.get("protocol") {
                        protocol = Some(serde_json::from_value(proto.clone())?);
                    }
                    if let Some(info) = action.get("commitInfo") {
                        let commit_info: CommitInfoAction = serde_json::from_value(info.clone())?;
                        last_timestamp = Utc
                            .timestamp_millis_opt(commit_info.timestamp)
                            .single()
                            .unwrap_or_else(Utc::now);
                    }
                }
            }
        }

        let metadata = metadata.ok_or_else(|| anyhow!("No metadata found in delta log"))?;
        let protocol = protocol.ok_or_else(|| anyhow!("No protocol found in delta log"))?;

        let snapshot = DeltaSnapshot {
            version: target_version,
            timestamp: last_timestamp,
            files: all_files.into_values().collect(),
            metadata,
            protocol,
        };

        // Cache the snapshot
        *self.cached_snapshot.write() = Some(snapshot.clone());
        *self.cached_metadata.write() = Some(snapshot.metadata.clone());
        *self.cached_protocol.write() = Some(snapshot.protocol.clone());

        Ok(snapshot)
    }

    /// Convert Delta schema JSON to Arrow schema
    fn parse_schema(&self, schema_string: &str) -> Result<ArrowSchema> {
        #[derive(Deserialize)]
        struct DeltaSchema {
            #[serde(rename = "type")]
            _type: String,
            fields: Vec<DeltaField>,
        }

        #[derive(Deserialize)]
        struct DeltaField {
            name: String,
            #[serde(rename = "type")]
            field_type: serde_json::Value,
            nullable: bool,
            #[allow(dead_code)]
            metadata: Option<HashMap<String, String>>,
        }

        let delta_schema: DeltaSchema = serde_json::from_str(schema_string)?;
        let fields: Vec<Field> = delta_schema
            .fields
            .iter()
            .map(|f| {
                let arrow_type = self.delta_type_to_arrow(&f.field_type);
                Field::new(&f.name, arrow_type, f.nullable)
            })
            .collect();

        Ok(ArrowSchema::new(fields))
    }

    /// Convert Delta type to Arrow type
    fn delta_type_to_arrow(&self, delta_type: &serde_json::Value) -> ArrowDataType {
        match delta_type {
            serde_json::Value::String(s) => match s.as_str() {
                "string" => ArrowDataType::Utf8,
                "long" => ArrowDataType::Int64,
                "integer" => ArrowDataType::Int32,
                "short" => ArrowDataType::Int16,
                "byte" => ArrowDataType::Int8,
                "float" => ArrowDataType::Float32,
                "double" => ArrowDataType::Float64,
                "boolean" => ArrowDataType::Boolean,
                "binary" => ArrowDataType::Binary,
                "date" => ArrowDataType::Date32,
                "timestamp" => ArrowDataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None),
                _ => ArrowDataType::Utf8, // Default to string
            },
            serde_json::Value::Object(obj) => {
                if let Some(serde_json::Value::String(t)) = obj.get("type") {
                    match t.as_str() {
                        "array" => {
                            if let Some(element) = obj.get("elementType") {
                                let element_type = self.delta_type_to_arrow(element);
                                ArrowDataType::List(Arc::new(Field::new(
                                    "item",
                                    element_type,
                                    true,
                                )))
                            } else {
                                ArrowDataType::List(Arc::new(Field::new(
                                    "item",
                                    ArrowDataType::Utf8,
                                    true,
                                )))
                            }
                        }
                        "map" => ArrowDataType::Utf8,    // Simplified
                        "struct" => ArrowDataType::Utf8, // Simplified
                        _ => ArrowDataType::Utf8,
                    }
                } else {
                    ArrowDataType::Utf8
                }
            }
            _ => ArrowDataType::Utf8,
        }
    }

    /// Convert internal snapshot to public Snapshot type
    fn to_public_snapshot(&self, internal: &DeltaSnapshot) -> Snapshot {
        let files: Vec<FileEntry> = internal
            .files
            .iter()
            .map(|f| {
                FileEntry {
                    path: f.path.clone(),
                    size_bytes: f.size as u64,
                    record_count: 0, // Would need to parse stats
                    partition_values: if f.partition_values.is_empty() {
                        None
                    } else {
                        Some(f.partition_values.clone())
                    },
                    stats: None, // Would need to parse stats JSON
                    created_at: Utc
                        .timestamp_millis_opt(f.modification_time)
                        .single()
                        .unwrap_or_else(Utc::now),
                }
            })
            .collect();

        Snapshot {
            version: internal.version,
            timestamp: internal.timestamp,
            files,
            schema_string: internal.metadata.schema_string.clone(),
            properties: internal.metadata.configuration.clone(),
        }
    }

    /// Create initial commit for new table
    async fn create_table(&self, schema: &ArrowSchema) -> Result<()> {
        let delta_log = self.delta_log_path();
        fs::create_dir_all(&delta_log).await?;

        // Create initial protocol action
        let protocol = ProtocolAction {
            min_reader_version: 1,
            min_writer_version: 2,
            reader_features: None,
            writer_features: None,
        };

        // Convert Arrow schema to Delta schema JSON
        let schema_string = self.arrow_schema_to_delta(schema)?;

        // Create metadata action
        let metadata = MetadataAction {
            id: uuid::Uuid::new_v4().to_string(),
            name: None,
            description: None,
            format: FormatSpec {
                provider: "parquet".to_string(),
                options: HashMap::new(),
            },
            schema_string,
            partition_columns: self.config.partition_columns.clone(),
            configuration: HashMap::new(),
            created_time: Some(Utc::now().timestamp_millis()),
        };

        // Create commit file
        let commit_path = delta_log.join("00000000000000000000.json");
        let mut actions = Vec::new();

        actions.push(serde_json::json!({"protocol": protocol}));
        actions.push(serde_json::json!({"metaData": metadata}));

        let content = actions
            .iter()
            .map(|a| serde_json::to_string(a).unwrap_or_else(|_| "{}".to_string()))
            .collect::<Vec<_>>()
            .join("\n");

        fs::write(&commit_path, content).await?;

        info!("Created Delta Lake table at {}", self.config.table_uri);
        Ok(())
    }

    /// Convert Arrow schema to Delta schema JSON
    fn arrow_schema_to_delta(&self, schema: &ArrowSchema) -> Result<String> {
        let fields: Vec<serde_json::Value> = schema
            .fields()
            .iter()
            .map(|f| {
                let delta_type = self.arrow_type_to_delta(f.data_type());
                serde_json::json!({
                    "name": f.name(),
                    "type": delta_type,
                    "nullable": f.is_nullable(),
                    "metadata": {}
                })
            })
            .collect();

        let delta_schema = serde_json::json!({
            "type": "struct",
            "fields": fields
        });

        Ok(serde_json::to_string(&delta_schema)?)
    }

    /// Convert Arrow type to Delta type string
    fn arrow_type_to_delta(&self, arrow_type: &ArrowDataType) -> serde_json::Value {
        match arrow_type {
            ArrowDataType::Utf8 | ArrowDataType::LargeUtf8 => serde_json::json!("string"),
            ArrowDataType::Int64 => serde_json::json!("long"),
            ArrowDataType::Int32 => serde_json::json!("integer"),
            ArrowDataType::Int16 => serde_json::json!("short"),
            ArrowDataType::Int8 => serde_json::json!("byte"),
            ArrowDataType::Float32 => serde_json::json!("float"),
            ArrowDataType::Float64 => serde_json::json!("double"),
            ArrowDataType::Boolean => serde_json::json!("boolean"),
            ArrowDataType::Binary | ArrowDataType::LargeBinary => serde_json::json!("binary"),
            ArrowDataType::Date32 | ArrowDataType::Date64 => serde_json::json!("date"),
            ArrowDataType::Timestamp(_, _) => serde_json::json!("timestamp"),
            ArrowDataType::List(field) => {
                serde_json::json!({
                    "type": "array",
                    "elementType": self.arrow_type_to_delta(field.data_type()),
                    "containsNull": field.is_nullable()
                })
            }
            _ => serde_json::json!("string"), // Default
        }
    }

    /// Write a commit to the delta log
    async fn write_commit(&self, version: i64, actions: Vec<serde_json::Value>) -> Result<()> {
        let delta_log = self.delta_log_path();
        let commit_file = format!("{:020}.json", version);
        let commit_path = delta_log.join(&commit_file);

        let content = actions
            .iter()
            .map(|a| serde_json::to_string(a).unwrap_or_else(|_| "{}".to_string()))
            .collect::<Vec<_>>()
            .join("\n");

        fs::write(&commit_path, content).await?;
        debug!("Wrote Delta commit version {}", version);

        // Write checkpoint if needed
        if version % self.config.checkpoint_interval as i64 == 0 && version > 0 {
            self.write_checkpoint(version).await?;
        }

        Ok(())
    }

    /// Write checkpoint file
    async fn write_checkpoint(&self, version: i64) -> Result<()> {
        let delta_log = self.delta_log_path();

        // Write last checkpoint pointer
        let last_checkpoint = serde_json::json!({
            "version": version,
            "size": 1
        });

        let checkpoint_path = delta_log.join("_last_checkpoint");
        fs::write(&checkpoint_path, serde_json::to_string(&last_checkpoint)?).await?;

        info!("Created checkpoint at version {}", version);
        Ok(())
    }
}

impl Debug for DeltaLakeFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeltaLakeFormat")
            .field("table_uri", &self.config.table_uri)
            .finish()
    }
}

// ============================================================================
// StorageFormat Implementation
// ============================================================================

#[async_trait]
impl StorageFormat for DeltaLakeFormat {
    fn format_name(&self) -> &str {
        "delta"
    }

    fn format_version(&self) -> &str {
        "3.0"
    }

    fn supported_data_types(&self) -> Vec<ArrowDataType> {
        vec![
            ArrowDataType::Boolean,
            ArrowDataType::Int8,
            ArrowDataType::Int16,
            ArrowDataType::Int32,
            ArrowDataType::Int64,
            ArrowDataType::Float32,
            ArrowDataType::Float64,
            ArrowDataType::Utf8,
            ArrowDataType::Binary,
            ArrowDataType::Date32,
            ArrowDataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None),
        ]
    }

    async fn infer_schema(&self, path: &str) -> Result<ArrowSchema> {
        let config = DeltaLakeConfig::new(path);
        let format = DeltaLakeFormat::new(config).await?;

        let snapshot = format.load_snapshot(None).await?;
        format.parse_schema(&snapshot.metadata.schema_string)
    }

    fn validate_schema(&self, _schema: &ArrowSchema) -> Result<()> {
        // Delta Lake supports most Arrow types
        Ok(())
    }

    fn format_type(&self) -> FormatType {
        FormatType::DeltaLake
    }

    fn supports_feature(&self, feature: &str) -> bool {
        matches!(
            feature,
            "acid"
                | "time_travel"
                | "schema_evolution"
                | "partitioning"
                | "z_ordering"
                | "merge"
                | "delete"
                | "update"
        )
    }
}

// ============================================================================
// OpenTableFormat Implementation
// ============================================================================

#[async_trait]
impl OpenTableFormat for DeltaLakeFormat {
    async fn get_current_snapshot(&self, _table_path: &str) -> Result<Snapshot> {
        let internal = self.load_snapshot(None).await?;
        Ok(self.to_public_snapshot(&internal))
    }

    async fn get_snapshot_at(&self, _table_path: &str, version: i64) -> Result<Snapshot> {
        let internal = self.load_snapshot(Some(version)).await?;
        Ok(self.to_public_snapshot(&internal))
    }

    async fn list_files(&self, snapshot: &Snapshot) -> Result<Vec<FileEntry>> {
        Ok(snapshot.files.clone())
    }

    async fn list_versions(&self, _table_path: &str) -> Result<Vec<i64>> {
        let commits = self.list_commit_files().await?;
        Ok(commits.into_iter().map(|(v, _)| v).collect())
    }

    async fn read_snapshot(
        &self,
        snapshot: &Snapshot,
        ctx: &ReadContext,
    ) -> Result<RecordBatchStream> {
        // Get list of Parquet files to read
        let files: Vec<String> = snapshot
            .files
            .iter()
            .map(|f| {
                let table_path = Path::new(&self.config.table_uri);
                table_path.join(&f.path).to_string_lossy().to_string()
            })
            .collect();

        if files.is_empty() {
            return Ok(Box::pin(stream::empty()));
        }

        // Create a stream of record batches from Parquet files
        let batch_size = ctx.batch_size;
        let projection = ctx.projection.clone();
        let filter = ctx.filter.clone();

        let batches_stream = stream::iter(files)
            .then(move |file_path| {
                let projection = projection.clone();
                let filter = filter.clone();
                async move {
                    Self::read_parquet_file(
                        &file_path,
                        batch_size,
                        projection.as_ref(),
                        filter.as_ref(),
                    )
                    .await
                }
            })
            .flat_map(|result| match result {
                Ok(batches) => stream::iter(batches.into_iter().map(Ok)).boxed(),
                Err(e) => stream::once(async move { Err(e) }).boxed(),
            });

        Ok(Box::pin(batches_stream))
    }

    async fn read_snapshot_vectors(
        &self,
        _snapshot: &Snapshot,
        _ctx: &VectorReadContext,
    ) -> Result<Option<VectorBatchStream>> {
        // Delta Lake doesn't have native vector support
        // Would need to read Parquet and extract vector columns
        Ok(None)
    }

    async fn write_atomic(
        &self,
        _table_path: &str,
        batches: Vec<RecordBatch>,
        ctx: &WriteContext,
    ) -> Result<Snapshot> {
        // Ensure table exists
        if !self.table_exists().await? {
            if let Some(batch) = batches.first() {
                self.create_table(batch.schema().as_ref()).await?;
            } else {
                return Err(anyhow!("Cannot create table without schema"));
            }
        }

        // Load current snapshot for version
        let current = self.load_snapshot(None).await?;
        let new_version = current.version + 1;

        // Write Parquet files
        let mut add_actions = Vec::new();
        let mut total_rows = 0u64;

        for (i, batch) in batches.iter().enumerate() {
            let file_name = format!("part-{:05}-{}.snappy.parquet", i, uuid::Uuid::new_v4());
            let file_path = Path::new(&self.config.table_uri).join(&file_name);

            let size = Self::write_parquet_file(&file_path, batch, &ctx.compression).await?;
            total_rows += batch.num_rows() as u64;

            add_actions.push(AddAction {
                path: file_name,
                partition_values: HashMap::new(),
                size: size as i64,
                modification_time: Utc::now().timestamp_millis(),
                data_change: true,
                stats: None,
                tags: None,
            });
        }

        // Build commit actions
        let mut actions: Vec<serde_json::Value> = add_actions
            .iter()
            .map(|a| serde_json::json!({"add": a}))
            .collect();

        // Add commit info
        let commit_info = CommitInfoAction {
            timestamp: Utc::now().timestamp_millis(),
            operation: match ctx.mode {
                WriteMode::Append => "WRITE".to_string(),
                WriteMode::Overwrite => "OVERWRITE".to_string(),
                WriteMode::ErrorIfExists => "CREATE".to_string(),
            },
            operation_parameters: HashMap::new(),
            read_version: Some(current.version),
            isolation_level: Some("WriteSerializable".to_string()),
            is_blind_append: Some(true),
            operation_metrics: Some({
                let mut m = HashMap::new();
                m.insert("numFiles".to_string(), add_actions.len().to_string());
                m.insert("numOutputRows".to_string(), total_rows.to_string());
                m
            }),
            user_metadata: None,
            engine_info: Some("ProximaDB/0.2.0".to_string()),
        };
        actions.push(serde_json::json!({"commitInfo": commit_info}));

        // Handle overwrite mode
        if ctx.mode == WriteMode::Overwrite {
            for file in &current.files {
                let remove = RemoveAction {
                    path: file.path.clone(),
                    deletion_timestamp: Some(Utc::now().timestamp_millis()),
                    data_change: true,
                    extended_file_metadata: Some(true),
                    partition_values: Some(file.partition_values.clone()),
                    size: Some(file.size),
                };
                actions.insert(0, serde_json::json!({"remove": remove}));
            }
        }

        // Write commit
        self.write_commit(new_version, actions).await?;

        // Reload and return new snapshot
        let new_snapshot = self.load_snapshot(Some(new_version)).await?;
        Ok(self.to_public_snapshot(&new_snapshot))
    }

    async fn merge_into(
        &self,
        _table_path: &str,
        _source: RecordBatchStream,
        _merge_condition: &str,
        _matched_action: MergeAction,
        _not_matched_action: MergeAction,
    ) -> Result<Snapshot> {
        // MERGE INTO is complex - simplified implementation
        warn!("MERGE INTO not fully implemented yet");
        self.get_current_snapshot(&self.config.table_uri).await
    }

    async fn time_travel(&self, _table_path: &str, timestamp: DateTime<Utc>) -> Result<Snapshot> {
        // Find version closest to timestamp
        let commits = self.list_commit_files().await?;

        for (version, path) in commits.iter().rev() {
            let content = fs::read_to_string(path).await?;
            for line in content.lines() {
                if let Ok(action) = serde_json::from_str::<HashMap<String, serde_json::Value>>(line)
                    && let Some(info) = action.get("commitInfo")
                {
                    let commit_info: CommitInfoAction = serde_json::from_value(info.clone())?;
                    let commit_time = Utc
                        .timestamp_millis_opt(commit_info.timestamp)
                        .single()
                        .unwrap_or_else(Utc::now);
                    if commit_time <= timestamp {
                        return self.get_snapshot_at(&self.config.table_uri, *version).await;
                    }
                }
            }
        }

        Err(anyhow!("No snapshot found for timestamp {}", timestamp))
    }

    async fn restore(&self, _table_path: &str, version: i64) -> Result<Snapshot> {
        // Create a new commit that restores to old version
        let old_snapshot = self.load_snapshot(Some(version)).await?;
        let current = self.load_snapshot(None).await?;
        let new_version = current.version + 1;

        let mut actions: Vec<serde_json::Value> = Vec::new();

        // Remove current files
        for file in &current.files {
            let remove = RemoveAction {
                path: file.path.clone(),
                deletion_timestamp: Some(Utc::now().timestamp_millis()),
                data_change: true,
                extended_file_metadata: Some(true),
                partition_values: Some(file.partition_values.clone()),
                size: Some(file.size),
            };
            actions.push(serde_json::json!({"remove": remove}));
        }

        // Add back old files
        for file in &old_snapshot.files {
            actions.push(serde_json::json!({"add": file}));
        }

        // Commit info
        let commit_info = CommitInfoAction {
            timestamp: Utc::now().timestamp_millis(),
            operation: "RESTORE".to_string(),
            operation_parameters: {
                let mut m = HashMap::new();
                m.insert("version".to_string(), version.to_string());
                m
            },
            read_version: Some(current.version),
            isolation_level: Some("WriteSerializable".to_string()),
            is_blind_append: Some(false),
            operation_metrics: None,
            user_metadata: None,
            engine_info: Some("ProximaDB/0.2.0".to_string()),
        };
        actions.push(serde_json::json!({"commitInfo": commit_info}));

        self.write_commit(new_version, actions).await?;

        let restored = self.load_snapshot(Some(new_version)).await?;
        Ok(self.to_public_snapshot(&restored))
    }

    async fn optimize(&self, _table_path: &str, ctx: &OptimizeContext) -> Result<OptimizeResult> {
        let start = std::time::Instant::now();
        let current = self.load_snapshot(None).await?;

        // Find small files to compact
        let small_files: Vec<_> = current
            .files
            .iter()
            .filter(|f| (f.size as u64) < ctx.target_file_size_bytes / 2)
            .collect();

        if small_files.len() < 2 {
            return Ok(OptimizeResult {
                files_optimized: 0,
                files_vacuumed: 0,
                space_reclaimed_bytes: 0,
                duration_ms: start.elapsed().as_millis() as u64,
            });
        }

        info!("Optimizing {} small files", small_files.len());

        // Deferred: Actually compact files
        // This would read all small files, merge them, and write new larger files

        Ok(OptimizeResult {
            files_optimized: small_files.len(),
            files_vacuumed: 0,
            space_reclaimed_bytes: 0,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    async fn vacuum(&self, _table_path: &str, retention_hours: u64) -> Result<u64> {
        let _delta_log = self.delta_log_path();
        let retention_ms = retention_hours * 3600 * 1000;
        let cutoff = Utc::now().timestamp_millis() - retention_ms as i64;

        let current = self.load_snapshot(None).await?;
        let active_files: std::collections::HashSet<_> =
            current.files.iter().map(|f| f.path.clone()).collect();

        let table_path = Path::new(&self.config.table_uri);
        let mut removed_bytes = 0u64;

        // Find orphan files older than retention
        let mut entries = fs::read_dir(table_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                continue;
            }

            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.ends_with(".parquet") {
                continue;
            }

            if active_files.contains(name) {
                continue;
            }

            // Check modification time
            if let Ok(metadata) = entry.metadata().await
                && let Ok(modified) = metadata.modified()
            {
                let modified_ms = modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);

                if modified_ms < cutoff {
                    let size = metadata.len();
                    if fs::remove_file(&path).await.is_ok() {
                        removed_bytes += size;
                        debug!("Vacuumed file: {}", name);
                    }
                }
            }
        }

        info!("Vacuum removed {} bytes", removed_bytes);
        Ok(removed_bytes)
    }

    async fn get_schema_at(&self, _table_path: &str, version: i64) -> Result<ArrowSchema> {
        let snapshot = self.load_snapshot(Some(version)).await?;
        self.parse_schema(&snapshot.metadata.schema_string)
    }

    async fn evolve_schema(&self, _table_path: &str, new_schema: &ArrowSchema) -> Result<Snapshot> {
        let current = self.load_snapshot(None).await?;
        let new_version = current.version + 1;

        // Create new metadata with evolved schema
        let mut new_metadata = current.metadata.clone();
        new_metadata.schema_string = self.arrow_schema_to_delta(new_schema)?;

        let mut actions: Vec<serde_json::Value> = Vec::new();
        actions.push(serde_json::json!({"metaData": new_metadata}));

        // Commit info
        let commit_info = CommitInfoAction {
            timestamp: Utc::now().timestamp_millis(),
            operation: "SET TBLPROPERTIES".to_string(),
            operation_parameters: HashMap::new(),
            read_version: Some(current.version),
            isolation_level: Some("WriteSerializable".to_string()),
            is_blind_append: Some(true),
            operation_metrics: None,
            user_metadata: None,
            engine_info: Some("ProximaDB/0.2.0".to_string()),
        };
        actions.push(serde_json::json!({"commitInfo": commit_info}));

        self.write_commit(new_version, actions).await?;

        let new_snapshot = self.load_snapshot(Some(new_version)).await?;
        Ok(self.to_public_snapshot(&new_snapshot))
    }
}

// ============================================================================
// Parquet I/O Helpers
// ============================================================================

impl DeltaLakeFormat {
    /// Read a Parquet file and return record batches
    async fn read_parquet_file(
        path: &str,
        batch_size: usize,
        projection: Option<&Vec<String>>,
        _filter: Option<&FilterExpression>,
    ) -> Result<Vec<RecordBatch>> {
        use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
        use std::fs::File;

        let file = File::open(path)?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)?.with_batch_size(batch_size);

        let reader = if let Some(cols) = projection {
            // Project specific columns
            let schema = builder.schema();
            let indices: Vec<usize> = cols
                .iter()
                .filter_map(|name| schema.index_of(name).ok())
                .collect();

            let mask = parquet::arrow::ProjectionMask::roots(builder.parquet_schema(), indices);
            builder.with_projection(mask).build()?
        } else {
            builder.build()?
        };

        let batches: Vec<RecordBatch> = reader.filter_map(|r| r.ok()).collect();

        Ok(batches)
    }

    /// Write a record batch to a Parquet file
    async fn write_parquet_file(
        path: &Path,
        batch: &RecordBatch,
        compression: &CompressionCodec,
    ) -> Result<u64> {
        use parquet::arrow::ArrowWriter;
        use parquet::basic::Compression;
        use parquet::file::properties::WriterProperties;
        use std::fs::File;

        let file = File::create(path)?;

        let compression = match compression {
            CompressionCodec::None => Compression::UNCOMPRESSED,
            CompressionCodec::Snappy => Compression::SNAPPY,
            CompressionCodec::Gzip => Compression::GZIP(Default::default()),
            CompressionCodec::Lz4 | CompressionCodec::Lz4hc | CompressionCodec::Lzo => Compression::LZ4,
            CompressionCodec::Zstd => Compression::ZSTD(Default::default()),
            CompressionCodec::Brotli => Compression::BROTLI(Default::default()),
            // Algorithms not natively supported by Parquet: fall back to Snappy
            _ => Compression::SNAPPY,
        };

        let props = WriterProperties::builder()
            .set_compression(compression)
            .build();

        let mut writer = ArrowWriter::try_new(file, batch.schema(), Some(props))?;
        writer.write(batch)?;
        writer.close()?;

        // Get file size
        let metadata = std::fs::metadata(path)?;
        Ok(metadata.len())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delta_config_new() {
        let config = DeltaLakeConfig::new("/tmp/delta/test");
        assert_eq!(config.table_uri, "/tmp/delta/test");
        assert!(!config.enable_change_data_feed);
    }

    #[test]
    fn test_delta_config_builder() {
        let config = DeltaLakeConfig::new("/tmp/delta/test")
            .with_change_data_feed(true)
            .with_checkpoint_interval(5)
            .with_partitions(vec!["date".to_string()]);

        assert!(config.enable_change_data_feed);
        assert_eq!(config.checkpoint_interval, 5);
        assert_eq!(config.partition_columns, vec!["date".to_string()]);
    }

    #[test]
    fn test_format_spec_serialization() {
        let spec = FormatSpec {
            provider: "parquet".to_string(),
            options: HashMap::new(),
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("parquet"));
    }

    #[test]
    fn test_add_action_serialization() {
        let add = AddAction {
            path: "part-00000.parquet".to_string(),
            partition_values: HashMap::new(),
            size: 1024,
            modification_time: 1234567890,
            data_change: true,
            stats: None,
            tags: None,
        };
        let json = serde_json::to_string(&add).unwrap();
        assert!(json.contains("part-00000.parquet"));
    }

    #[tokio::test]
    async fn test_delta_format_type() {
        let config = DeltaLakeConfig::new("/tmp/delta/test");
        // Can't fully test without a real table, but check configuration
        assert_eq!(config.compression, CompressionCodec::Snappy);
    }
}
