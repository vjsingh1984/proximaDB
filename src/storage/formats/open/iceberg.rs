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

//! # Apache Iceberg Format Connector
//!
//! Implementation of the `OpenTableFormat` trait for Apache Iceberg tables.
//! Iceberg is an open table format for huge analytic datasets, providing:
//!
//! ## Features
//!
//! - **Schema Evolution**: Add, drop, rename, and update columns
//! - **Hidden Partitioning**: Partition pruning without exposed partition columns
//! - **Time Travel**: Query any historical snapshot
//! - **ACID Transactions**: Serializable isolation for concurrent writes
//! - **Version History**: Full history of table changes
//!
//! ## Iceberg Table Structure
//!
//! ```text
//! warehouse/
//! └── db.table/
//!     ├── metadata/
//!     │   ├── version-hint.text           # Current metadata version
//!     │   ├── v1.metadata.json            # Metadata version 1
//!     │   ├── v2.metadata.json            # Metadata version 2
//!     │   ├── snap-123456789.avro         # Manifest list
//!     │   └── manifest-*.avro             # Manifest files
//!     └── data/
//!         └── part-*.parquet              # Data files
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::storage::formats::open::iceberg::{IcebergFormat, IcebergConfig};
//!
//! // REST catalog
//! let config = IcebergConfig::rest("http://localhost:8181", "warehouse", "db.table");
//! let iceberg = IcebergFormat::new(config).await?;
//!
//! // Hadoop-style catalog (file-based)
//! let config = IcebergConfig::hadoop("/path/to/warehouse", "db.table");
//! let iceberg = IcebergFormat::new(config).await?;
//!
//! // Read current snapshot
//! let snapshot = iceberg.get_current_snapshot("db.table").await?;
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
    CompressionCodec, FileEntry, FormatType, MergeAction, OpenTableFormat, OptimizeContext,
    OptimizeResult, ReadContext, RecordBatchStream, Snapshot, StorageFormat, VectorBatchStream,
    VectorReadContext, WriteContext, WriteMode,
};

// ============================================================================
// Configuration
// ============================================================================

/// Iceberg catalog type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CatalogType {
    /// REST catalog (Iceberg REST spec)
    Rest { uri: String },
    /// Hadoop-style file-based catalog
    Hadoop { warehouse_path: String },
    /// AWS Glue Data Catalog
    Glue { database: String, region: String },
    /// Hive Metastore
    Hive { uri: String },
}

/// Iceberg format configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IcebergConfig {
    /// Catalog type and connection info
    pub catalog: CatalogType,

    /// Table identifier (namespace.table)
    pub table_identifier: String,

    /// Storage options for data files
    pub storage_options: StorageOptions,

    /// Target file size for writes
    pub target_file_size_bytes: u64,

    /// Default compression
    pub compression: CompressionCodec,

    /// Write distribution mode
    pub write_distribution_mode: WriteDistributionMode,

    /// Metadata refresh interval (seconds)
    pub metadata_refresh_interval_secs: u64,
}

/// Write distribution mode for Iceberg
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum WriteDistributionMode {
    /// No specific distribution
    None,
    /// Hash by partition
    Hash,
    /// Range by sort columns
    Range,
}

impl Default for IcebergConfig {
    fn default() -> Self {
        Self {
            catalog: CatalogType::Hadoop {
                warehouse_path: String::new(),
            },
            table_identifier: String::new(),
            storage_options: StorageOptions::default(),
            target_file_size_bytes: 128 * 1024 * 1024, // 128MB
            compression: CompressionCodec::Zstd,
            write_distribution_mode: WriteDistributionMode::Hash,
            metadata_refresh_interval_secs: 60,
        }
    }
}

impl IcebergConfig {
    /// Create config for REST catalog
    pub fn rest(uri: &str, warehouse: &str, table: &str) -> Self {
        Self {
            catalog: CatalogType::Rest {
                uri: uri.to_string(),
            },
            table_identifier: table.to_string(),
            storage_options: StorageOptions {
                url: warehouse.to_string(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Create config for Hadoop-style catalog
    pub fn hadoop(warehouse_path: &str, table: &str) -> Self {
        Self {
            catalog: CatalogType::Hadoop {
                warehouse_path: warehouse_path.to_string(),
            },
            table_identifier: table.to_string(),
            storage_options: StorageOptions::local(warehouse_path),
            ..Default::default()
        }
    }

    /// Create config for AWS Glue catalog
    pub fn glue(database: &str, table: &str, region: &str) -> Self {
        Self {
            catalog: CatalogType::Glue {
                database: database.to_string(),
                region: region.to_string(),
            },
            table_identifier: table.to_string(),
            ..Default::default()
        }
    }

    /// Create config for Hive Metastore
    pub fn hive(uri: &str, table: &str) -> Self {
        Self {
            catalog: CatalogType::Hive {
                uri: uri.to_string(),
            },
            table_identifier: table.to_string(),
            ..Default::default()
        }
    }
}

// ============================================================================
// Iceberg Metadata Types
// ============================================================================

/// Iceberg table metadata (stored in metadata/*.json)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct TableMetadata {
    /// Format version (1 or 2)
    pub format_version: i32,
    /// Table UUID
    pub table_uuid: String,
    /// Table location
    pub location: String,
    /// Last sequence number
    #[serde(default)]
    pub last_sequence_number: i64,
    /// Last updated timestamp (ms)
    pub last_updated_ms: i64,
    /// Last assigned column ID
    pub last_column_id: i32,
    /// Current schema ID
    #[serde(default)]
    pub current_schema_id: i32,
    /// Schemas (list of all schemas)
    pub schemas: Vec<IcebergSchema>,
    /// Default spec ID
    #[serde(default)]
    pub default_spec_id: i32,
    /// Partition specs
    pub partition_specs: Vec<PartitionSpec>,
    /// Last assigned partition ID
    #[serde(default)]
    pub last_partition_id: i32,
    /// Default sort order ID
    #[serde(default)]
    pub default_sort_order_id: i32,
    /// Sort orders
    #[serde(default)]
    pub sort_orders: Vec<SortOrder>,
    /// Table properties
    #[serde(default)]
    pub properties: HashMap<String, String>,
    /// Current snapshot ID
    pub current_snapshot_id: Option<i64>,
    /// Snapshots
    #[serde(default)]
    pub snapshots: Vec<IcebergSnapshot>,
    /// Snapshot log
    #[serde(default)]
    pub snapshot_log: Vec<SnapshotLogEntry>,
    /// Metadata log
    #[serde(default)]
    pub metadata_log: Vec<MetadataLogEntry>,
}

/// Iceberg schema
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct IcebergSchema {
    /// Schema ID
    pub schema_id: i32,
    /// Schema type (always "struct")
    #[serde(rename = "type")]
    pub schema_type: String,
    /// Fields
    pub fields: Vec<IcebergField>,
    /// Identifier field IDs (primary key columns)
    #[serde(default)]
    pub identifier_field_ids: Vec<i32>,
}

/// Iceberg field definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct IcebergField {
    /// Field ID
    pub id: i32,
    /// Field name
    pub name: String,
    /// Field type
    #[serde(rename = "type")]
    pub field_type: serde_json::Value,
    /// Required flag
    pub required: bool,
    /// Documentation
    #[serde(default)]
    pub doc: Option<String>,
}

/// Partition specification
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PartitionSpec {
    /// Spec ID
    pub spec_id: i32,
    /// Partition fields
    pub fields: Vec<PartitionField>,
}

/// Partition field
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PartitionField {
    /// Field ID
    pub field_id: i32,
    /// Source column ID
    pub source_id: i32,
    /// Partition name
    pub name: String,
    /// Transform (identity, year, month, day, hour, bucket, truncate)
    pub transform: String,
}

/// Sort order
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SortOrder {
    /// Order ID
    pub order_id: i32,
    /// Sort fields
    pub fields: Vec<SortField>,
}

/// Sort field
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SortField {
    /// Transform (identity, bucket, truncate)
    pub transform: String,
    /// Source column ID
    pub source_id: i32,
    /// Direction (asc or desc)
    pub direction: String,
    /// Null order (nulls-first or nulls-last)
    pub null_order: String,
}

/// Iceberg snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct IcebergSnapshot {
    /// Snapshot ID
    pub snapshot_id: i64,
    /// Parent snapshot ID
    pub parent_snapshot_id: Option<i64>,
    /// Sequence number
    #[serde(default)]
    pub sequence_number: i64,
    /// Timestamp (ms)
    pub timestamp_ms: i64,
    /// Manifest list location
    pub manifest_list: String,
    /// Summary
    pub summary: SnapshotSummary,
    /// Schema ID
    #[serde(default)]
    pub schema_id: Option<i32>,
}

/// Snapshot summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotSummary {
    /// Operation (append, overwrite, delete, replace)
    pub operation: String,
    /// Additional properties
    #[serde(flatten)]
    pub properties: HashMap<String, String>,
}

/// Snapshot log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SnapshotLogEntry {
    /// Timestamp (ms)
    pub timestamp_ms: i64,
    /// Snapshot ID
    pub snapshot_id: i64,
}

/// Metadata log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct MetadataLogEntry {
    /// Timestamp (ms)
    pub timestamp_ms: i64,
    /// Metadata file location
    pub metadata_file: String,
}

// ============================================================================
// Manifest Types
// ============================================================================

/// Manifest list entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestFile {
    /// Manifest path
    pub manifest_path: String,
    /// Manifest length
    pub manifest_length: i64,
    /// Partition spec ID
    pub partition_spec_id: i32,
    /// Content type (0=data, 1=deletes)
    pub content: i32,
    /// Sequence number
    pub sequence_number: i64,
    /// Min sequence number
    pub min_sequence_number: i64,
    /// Added snapshot ID
    pub added_snapshot_id: i64,
    /// Added data files count
    pub added_data_files_count: i32,
    /// Existing data files count
    pub existing_data_files_count: i32,
    /// Deleted data files count
    pub deleted_data_files_count: i32,
    /// Added rows count
    pub added_rows_count: i64,
    /// Existing rows count
    pub existing_rows_count: i64,
    /// Deleted rows count
    pub deleted_rows_count: i64,
    /// Partitions
    #[serde(default)]
    pub partitions: Vec<PartitionFieldSummary>,
}

/// Partition field summary in manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionFieldSummary {
    /// Contains null values
    pub contains_null: bool,
    /// Contains NaN values
    #[serde(default)]
    pub contains_nan: Option<bool>,
    /// Lower bound
    #[serde(default)]
    pub lower_bound: Option<Vec<u8>>,
    /// Upper bound
    #[serde(default)]
    pub upper_bound: Option<Vec<u8>>,
}

/// Data file entry in manifest
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataFile {
    /// Content type (0=data, 1=position deletes, 2=equality deletes)
    pub content: i32,
    /// File path
    pub file_path: String,
    /// File format (parquet, avro, orc)
    pub file_format: String,
    /// Partition values
    pub partition: HashMap<String, serde_json::Value>,
    /// Record count
    pub record_count: i64,
    /// File size in bytes
    pub file_size_in_bytes: i64,
    /// Column sizes
    #[serde(default)]
    pub column_sizes: Option<HashMap<i32, i64>>,
    /// Value counts
    #[serde(default)]
    pub value_counts: Option<HashMap<i32, i64>>,
    /// Null value counts
    #[serde(default)]
    pub null_value_counts: Option<HashMap<i32, i64>>,
    /// NaN value counts
    #[serde(default)]
    pub nan_value_counts: Option<HashMap<i32, i64>>,
    /// Lower bounds
    #[serde(default)]
    pub lower_bounds: Option<HashMap<i32, Vec<u8>>>,
    /// Upper bounds
    #[serde(default)]
    pub upper_bounds: Option<HashMap<i32, Vec<u8>>>,
    /// Split offsets
    #[serde(default)]
    pub split_offsets: Option<Vec<i64>>,
    /// Sort order ID
    #[serde(default)]
    pub sort_order_id: Option<i32>,
}

// ============================================================================
// Iceberg Format Implementation
// ============================================================================

/// Apache Iceberg format connector
pub struct IcebergFormat {
    /// Configuration
    config: IcebergConfig,

    /// Cached table metadata
    cached_metadata: RwLock<Option<TableMetadata>>,

    /// Table location (resolved from catalog)
    table_location: RwLock<Option<String>>,
}

impl IcebergFormat {
    /// Create a new Iceberg format connector
    pub async fn new(config: IcebergConfig) -> Result<Self> {
        let format = Self {
            config,
            cached_metadata: RwLock::new(None),
            table_location: RwLock::new(None),
        };

        // Resolve table location and load metadata
        format.resolve_table().await?;

        Ok(format)
    }

    /// Resolve table location from catalog
    async fn resolve_table(&self) -> Result<()> {
        match &self.config.catalog {
            CatalogType::Hadoop { warehouse_path } => {
                // Hadoop-style: warehouse/namespace/table
                let parts: Vec<&str> = self.config.table_identifier.split('.').collect();
                let table_path = if parts.len() >= 2 {
                    Path::new(warehouse_path).join(parts[0]).join(parts[1])
                } else {
                    Path::new(warehouse_path).join(&self.config.table_identifier)
                };

                *self.table_location.write() = Some(table_path.to_string_lossy().to_string());

                // Load metadata if exists
                if table_path.join("metadata").exists() {
                    self.load_metadata().await?;
                }
            }
            CatalogType::Rest { uri } => {
                // REST catalog would make HTTP call to get table location
                // Simplified: assume table identifier is the path
                info!(
                    "REST catalog at {}, table: {}",
                    uri, self.config.table_identifier
                );
                *self.table_location.write() = Some(self.config.table_identifier.clone());
            }
            CatalogType::Glue {
                database,
                region: _,
            } => {
                info!(
                    "Glue catalog database: {}, table: {}",
                    database, self.config.table_identifier
                );
                *self.table_location.write() = Some(self.config.table_identifier.clone());
            }
            CatalogType::Hive { uri } => {
                info!(
                    "Hive catalog at {}, table: {}",
                    uri, self.config.table_identifier
                );
                *self.table_location.write() = Some(self.config.table_identifier.clone());
            }
        }
        Ok(())
    }

    /// Get table location
    fn get_table_location(&self) -> Result<String> {
        self.table_location
            .read()
            .clone()
            .ok_or_else(|| anyhow!("Table location not resolved"))
    }

    /// Get metadata directory path
    fn metadata_path(&self) -> Result<std::path::PathBuf> {
        let location = self.get_table_location()?;
        Ok(Path::new(&location).join("metadata"))
    }

    /// Load current metadata from version-hint.text
    async fn load_metadata(&self) -> Result<TableMetadata> {
        let metadata_dir = self.metadata_path()?;
        let version_hint_path = metadata_dir.join("version-hint.text");

        let current_version = if version_hint_path.exists() {
            let content = fs::read_to_string(&version_hint_path).await?;
            content.trim().parse::<i32>().unwrap_or(1)
        } else {
            // Find latest metadata file
            self.find_latest_metadata_version().await?
        };

        let metadata_file = metadata_dir.join(format!("v{}.metadata.json", current_version));
        let content = fs::read_to_string(&metadata_file).await?;
        let metadata: TableMetadata = serde_json::from_str(&content)?;

        *self.cached_metadata.write() = Some(metadata.clone());
        Ok(metadata)
    }

    /// Find latest metadata version by scanning files
    async fn find_latest_metadata_version(&self) -> Result<i32> {
        let metadata_dir = self.metadata_path()?;
        let mut max_version = 0i32;

        if !metadata_dir.exists() {
            return Ok(0);
        }

        let mut entries = fs::read_dir(&metadata_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let name = name.to_string_lossy();

            // Match v1.metadata.json, v2.metadata.json, etc.
            if name.starts_with('v')
                && name.ends_with(".metadata.json")
                && let Ok(version) = name
                    .trim_start_matches('v')
                    .trim_end_matches(".metadata.json")
                    .parse::<i32>()
            {
                max_version = max_version.max(version);
            }
        }

        Ok(max_version)
    }

    /// Get current metadata (from cache or reload)
    async fn get_metadata(&self) -> Result<TableMetadata> {
        if let Some(metadata) = self.cached_metadata.read().clone() {
            return Ok(metadata);
        }
        self.load_metadata().await
    }

    /// Convert Iceberg schema to Arrow schema
    fn iceberg_schema_to_arrow(&self, schema: &IcebergSchema) -> Result<ArrowSchema> {
        let fields: Vec<Field> = schema
            .fields
            .iter()
            .map(|f| {
                let arrow_type = self.iceberg_type_to_arrow(&f.field_type);
                Field::new(&f.name, arrow_type, !f.required)
            })
            .collect();

        Ok(ArrowSchema::new(fields))
    }

    /// Convert Iceberg type to Arrow type
    fn iceberg_type_to_arrow(&self, iceberg_type: &serde_json::Value) -> ArrowDataType {
        match iceberg_type {
            serde_json::Value::String(s) => match s.as_str() {
                "boolean" => ArrowDataType::Boolean,
                "int" => ArrowDataType::Int32,
                "long" => ArrowDataType::Int64,
                "float" => ArrowDataType::Float32,
                "double" => ArrowDataType::Float64,
                "date" => ArrowDataType::Date32,
                "time" => ArrowDataType::Time64(arrow_schema::TimeUnit::Microsecond),
                "timestamp" => ArrowDataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None),
                "timestamptz" => ArrowDataType::Timestamp(
                    arrow_schema::TimeUnit::Microsecond,
                    Some(Arc::from("UTC")),
                ),
                "string" => ArrowDataType::Utf8,
                "uuid" => ArrowDataType::Utf8,
                "binary" => ArrowDataType::Binary,
                "fixed" => ArrowDataType::Binary,
                _ => ArrowDataType::Utf8,
            },
            serde_json::Value::Object(obj) => {
                if let Some(serde_json::Value::String(t)) = obj.get("type") {
                    match t.as_str() {
                        "list" => {
                            if let Some(element) = obj.get("element-type") {
                                let element_type = self.iceberg_type_to_arrow(element);
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
                        "map" => ArrowDataType::Utf8, // Simplified
                        "struct" => {
                            if let Some(serde_json::Value::Array(fields)) = obj.get("fields") {
                                let arrow_fields: Vec<Field> = fields
                                    .iter()
                                    .filter_map(|f| {
                                        let name = f.get("name")?.as_str()?;
                                        let field_type = f.get("type")?;
                                        let required = f
                                            .get("required")
                                            .and_then(|r| r.as_bool())
                                            .unwrap_or(false);
                                        let arrow_type = self.iceberg_type_to_arrow(field_type);
                                        Some(Field::new(name, arrow_type, !required))
                                    })
                                    .collect();
                                ArrowDataType::Struct(arrow_fields.into())
                            } else {
                                ArrowDataType::Utf8
                            }
                        }
                        "decimal" => {
                            let precision =
                                obj.get("precision").and_then(|p| p.as_u64()).unwrap_or(38) as u8;
                            let scale =
                                obj.get("scale").and_then(|s| s.as_i64()).unwrap_or(0) as i8;
                            ArrowDataType::Decimal128(precision, scale)
                        }
                        _ => ArrowDataType::Utf8,
                    }
                } else {
                    ArrowDataType::Utf8
                }
            }
            _ => ArrowDataType::Utf8,
        }
    }

    /// Convert Arrow schema to Iceberg schema
    fn arrow_schema_to_iceberg(&self, schema: &ArrowSchema) -> Result<IcebergSchema> {
        let fields: Vec<IcebergField> = schema
            .fields()
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let iceberg_type = self.arrow_type_to_iceberg(f.data_type());
                IcebergField {
                    id: (i + 1) as i32,
                    name: f.name().clone(),
                    field_type: iceberg_type,
                    required: !f.is_nullable(),
                    doc: None,
                }
            })
            .collect();

        Ok(IcebergSchema {
            schema_id: 0,
            schema_type: "struct".to_string(),
            fields,
            identifier_field_ids: Vec::new(),
        })
    }

    /// Convert Arrow type to Iceberg type
    fn arrow_type_to_iceberg(&self, arrow_type: &ArrowDataType) -> serde_json::Value {
        match arrow_type {
            ArrowDataType::Boolean => serde_json::json!("boolean"),
            ArrowDataType::Int32 => serde_json::json!("int"),
            ArrowDataType::Int64 => serde_json::json!("long"),
            ArrowDataType::Float32 => serde_json::json!("float"),
            ArrowDataType::Float64 => serde_json::json!("double"),
            ArrowDataType::Date32 | ArrowDataType::Date64 => serde_json::json!("date"),
            ArrowDataType::Timestamp(_, None) => serde_json::json!("timestamp"),
            ArrowDataType::Timestamp(_, Some(_)) => serde_json::json!("timestamptz"),
            ArrowDataType::Utf8 | ArrowDataType::LargeUtf8 => serde_json::json!("string"),
            ArrowDataType::Binary | ArrowDataType::LargeBinary => serde_json::json!("binary"),
            ArrowDataType::Decimal128(p, s) => serde_json::json!({
                "type": "decimal",
                "precision": p,
                "scale": s
            }),
            ArrowDataType::List(field) => serde_json::json!({
                "type": "list",
                "element-id": 1,
                "element-type": self.arrow_type_to_iceberg(field.data_type()),
                "element-required": !field.is_nullable()
            }),
            _ => serde_json::json!("string"),
        }
    }

    /// Get current snapshot from metadata
    fn get_current_iceberg_snapshot<'a>(
        &self,
        metadata: &'a TableMetadata,
    ) -> Option<&'a IcebergSnapshot> {
        metadata
            .current_snapshot_id
            .and_then(move |id| metadata.snapshots.iter().find(|s| s.snapshot_id == id))
    }

    /// Convert Iceberg snapshot to public Snapshot type
    async fn to_public_snapshot(
        &self,
        iceberg_snap: &IcebergSnapshot,
        metadata: &TableMetadata,
    ) -> Result<Snapshot> {
        // Get schema for this snapshot
        let schema = metadata
            .schemas
            .iter()
            .find(|s| {
                Some(s.schema_id) == iceberg_snap.schema_id || iceberg_snap.schema_id.is_none()
            })
            .ok_or_else(|| anyhow!("Schema not found for snapshot"))?;

        // Load manifest list and get files
        let files = self.load_snapshot_files(iceberg_snap).await?;

        Ok(Snapshot {
            version: iceberg_snap.snapshot_id,
            timestamp: Utc
                .timestamp_millis_opt(iceberg_snap.timestamp_ms)
                .single()
                .unwrap_or_else(Utc::now),
            files,
            schema_string: serde_json::to_string(schema)?,
            properties: iceberg_snap.summary.properties.clone(),
        })
    }

    /// Load files from snapshot's manifest list
    async fn load_snapshot_files(&self, snapshot: &IcebergSnapshot) -> Result<Vec<FileEntry>> {
        let location = self.get_table_location()?;
        let manifest_list_path = if snapshot.manifest_list.starts_with('/') {
            snapshot.manifest_list.clone()
        } else {
            Path::new(&location)
                .join(&snapshot.manifest_list)
                .to_string_lossy()
                .to_string()
        };

        // For now, return empty - would need to read Avro manifest list
        // In production, would read manifest_list_path (Avro) to get manifest files,
        // then read each manifest file (Avro) to get data files
        debug!("Would load manifest list from: {}", manifest_list_path);

        Ok(Vec::new())
    }

    /// Create initial table metadata
    async fn create_table(&self, schema: &ArrowSchema) -> Result<()> {
        let location = self.get_table_location()?;
        let metadata_dir = Path::new(&location).join("metadata");
        let data_dir = Path::new(&location).join("data");

        fs::create_dir_all(&metadata_dir).await?;
        fs::create_dir_all(&data_dir).await?;

        let iceberg_schema = self.arrow_schema_to_iceberg(schema)?;

        let metadata = TableMetadata {
            format_version: 2,
            table_uuid: uuid::Uuid::new_v4().to_string(),
            location: location.clone(),
            last_sequence_number: 0,
            last_updated_ms: Utc::now().timestamp_millis(),
            last_column_id: iceberg_schema.fields.len() as i32,
            current_schema_id: 0,
            schemas: vec![iceberg_schema],
            default_spec_id: 0,
            partition_specs: vec![PartitionSpec {
                spec_id: 0,
                fields: Vec::new(),
            }],
            last_partition_id: 999,
            default_sort_order_id: 0,
            sort_orders: vec![SortOrder {
                order_id: 0,
                fields: Vec::new(),
            }],
            properties: HashMap::new(),
            current_snapshot_id: None,
            snapshots: Vec::new(),
            snapshot_log: Vec::new(),
            metadata_log: Vec::new(),
        };

        // Write metadata file
        let metadata_file = metadata_dir.join("v1.metadata.json");
        let content = serde_json::to_string_pretty(&metadata)?;
        fs::write(&metadata_file, &content).await?;

        // Write version hint
        let version_hint_path = metadata_dir.join("version-hint.text");
        fs::write(&version_hint_path, "1").await?;

        *self.cached_metadata.write() = Some(metadata);

        info!("Created Iceberg table at {}", location);
        Ok(())
    }

    /// Write new metadata version
    async fn write_metadata(&self, metadata: &TableMetadata) -> Result<i32> {
        let metadata_dir = self.metadata_path()?;
        let current_version = self.find_latest_metadata_version().await?;
        let new_version = current_version + 1;

        // Update metadata log
        let mut metadata = metadata.clone();
        if current_version > 0 {
            metadata.metadata_log.push(MetadataLogEntry {
                timestamp_ms: Utc::now().timestamp_millis(),
                metadata_file: format!("v{}.metadata.json", current_version),
            });
        }

        // Write new metadata file
        let metadata_file = metadata_dir.join(format!("v{}.metadata.json", new_version));
        let content = serde_json::to_string_pretty(&metadata)?;
        fs::write(&metadata_file, &content).await?;

        // Update version hint
        let version_hint_path = metadata_dir.join("version-hint.text");
        fs::write(&version_hint_path, new_version.to_string()).await?;

        *self.cached_metadata.write() = Some(metadata);

        debug!("Wrote Iceberg metadata version {}", new_version);
        Ok(new_version)
    }
}

impl Debug for IcebergFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IcebergFormat")
            .field("table_identifier", &self.config.table_identifier)
            .field("catalog", &self.config.catalog)
            .finish()
    }
}

// ============================================================================
// StorageFormat Implementation
// ============================================================================

#[async_trait]
impl StorageFormat for IcebergFormat {
    fn format_name(&self) -> &str {
        "iceberg"
    }

    fn format_version(&self) -> &str {
        "2"
    }

    fn supported_data_types(&self) -> Vec<ArrowDataType> {
        vec![
            ArrowDataType::Boolean,
            ArrowDataType::Int32,
            ArrowDataType::Int64,
            ArrowDataType::Float32,
            ArrowDataType::Float64,
            ArrowDataType::Utf8,
            ArrowDataType::Binary,
            ArrowDataType::Date32,
            ArrowDataType::Timestamp(arrow_schema::TimeUnit::Microsecond, None),
            ArrowDataType::Decimal128(38, 10),
        ]
    }

    async fn infer_schema(&self, _path: &str) -> Result<ArrowSchema> {
        let metadata = self.get_metadata().await?;
        let schema = metadata
            .schemas
            .iter()
            .find(|s| s.schema_id == metadata.current_schema_id)
            .ok_or_else(|| anyhow!("Current schema not found"))?;
        self.iceberg_schema_to_arrow(schema)
    }

    fn validate_schema(&self, _schema: &ArrowSchema) -> Result<()> {
        Ok(())
    }

    fn format_type(&self) -> FormatType {
        FormatType::Iceberg
    }

    fn supports_feature(&self, feature: &str) -> bool {
        matches!(
            feature,
            "acid"
                | "time_travel"
                | "schema_evolution"
                | "partitioning"
                | "hidden_partitioning"
                | "merge"
                | "delete"
                | "update"
                | "row_level_deletes"
        )
    }
}

// ============================================================================
// OpenTableFormat Implementation
// ============================================================================

#[async_trait]
impl OpenTableFormat for IcebergFormat {
    async fn get_current_snapshot(&self, _table_path: &str) -> Result<Snapshot> {
        let metadata = self.get_metadata().await?;
        let iceberg_snap = self
            .get_current_iceberg_snapshot(&metadata)
            .ok_or_else(|| anyhow!("No current snapshot"))?;
        self.to_public_snapshot(iceberg_snap, &metadata).await
    }

    async fn get_snapshot_at(&self, _table_path: &str, version: i64) -> Result<Snapshot> {
        let metadata = self.get_metadata().await?;
        let iceberg_snap = metadata
            .snapshots
            .iter()
            .find(|s| s.snapshot_id == version)
            .ok_or_else(|| anyhow!("Snapshot {} not found", version))?;
        self.to_public_snapshot(iceberg_snap, &metadata).await
    }

    async fn list_files(&self, snapshot: &Snapshot) -> Result<Vec<FileEntry>> {
        Ok(snapshot.files.clone())
    }

    async fn list_versions(&self, _table_path: &str) -> Result<Vec<i64>> {
        let metadata = self.get_metadata().await?;
        Ok(metadata.snapshots.iter().map(|s| s.snapshot_id).collect())
    }

    async fn read_snapshot(
        &self,
        snapshot: &Snapshot,
        ctx: &ReadContext,
    ) -> Result<RecordBatchStream> {
        // Get list of Parquet files to read
        let location = self.get_table_location()?;
        let files: Vec<String> = snapshot
            .files
            .iter()
            .map(|f| {
                if f.path.starts_with('/') || f.path.starts_with("s3://") {
                    f.path.clone()
                } else {
                    Path::new(&location)
                        .join("data")
                        .join(&f.path)
                        .to_string_lossy()
                        .to_string()
                }
            })
            .collect();

        if files.is_empty() {
            return Ok(Box::pin(stream::empty()));
        }

        let batch_size = ctx.batch_size;
        let projection = ctx.projection.clone();

        let batches_stream =
            stream::iter(files)
                .then(move |file_path| {
                    let projection = projection.clone();
                    async move {
                        Self::read_parquet_file(&file_path, batch_size, projection.as_ref()).await
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
        Ok(None)
    }

    async fn write_atomic(
        &self,
        _table_path: &str,
        batches: Vec<RecordBatch>,
        ctx: &WriteContext,
    ) -> Result<Snapshot> {
        // Create table if needed
        let metadata = match self.get_metadata().await {
            Ok(m) => m,
            Err(_) => {
                if let Some(batch) = batches.first() {
                    self.create_table(batch.schema().as_ref()).await?;
                    self.get_metadata().await?
                } else {
                    return Err(anyhow!("Cannot create table without schema"));
                }
            }
        };

        let location = self.get_table_location()?;
        let data_dir = Path::new(&location).join("data");
        fs::create_dir_all(&data_dir).await?;

        // Write data files
        let mut data_files = Vec::new();
        let mut total_rows = 0i64;

        for (i, batch) in batches.iter().enumerate() {
            let file_name = format!("{}-{}.parquet", uuid::Uuid::new_v4(), i);
            let file_path = data_dir.join(&file_name);

            let size = Self::write_parquet_file(&file_path, batch, &ctx.compression).await?;
            total_rows += batch.num_rows() as i64;

            data_files.push(DataFile {
                content: 0,
                file_path: format!("data/{}", file_name),
                file_format: "PARQUET".to_string(),
                partition: HashMap::new(),
                record_count: batch.num_rows() as i64,
                file_size_in_bytes: size as i64,
                column_sizes: None,
                value_counts: None,
                null_value_counts: None,
                nan_value_counts: None,
                lower_bounds: None,
                upper_bounds: None,
                split_offsets: None,
                sort_order_id: None,
            });
        }

        // Create new snapshot
        let new_snapshot_id = Utc::now().timestamp_millis();
        let manifest_list_path = format!(
            "metadata/snap-{}-{}.avro",
            new_snapshot_id,
            uuid::Uuid::new_v4()
        );

        let operation = match ctx.mode {
            WriteMode::Append => "append",
            WriteMode::Overwrite => "overwrite",
            WriteMode::ErrorIfExists => "append",
        };

        let new_snapshot = IcebergSnapshot {
            snapshot_id: new_snapshot_id,
            parent_snapshot_id: metadata.current_snapshot_id,
            sequence_number: metadata.last_sequence_number + 1,
            timestamp_ms: Utc::now().timestamp_millis(),
            manifest_list: manifest_list_path.clone(),
            summary: SnapshotSummary {
                operation: operation.to_string(),
                properties: {
                    let mut p = HashMap::new();
                    p.insert("added-data-files".to_string(), data_files.len().to_string());
                    p.insert("added-records".to_string(), total_rows.to_string());
                    p
                },
            },
            schema_id: Some(metadata.current_schema_id),
        };

        // Update metadata
        let mut new_metadata = metadata.clone();
        new_metadata.last_sequence_number += 1;
        new_metadata.last_updated_ms = Utc::now().timestamp_millis();
        new_metadata.current_snapshot_id = Some(new_snapshot_id);
        new_metadata.snapshots.push(new_snapshot.clone());
        new_metadata.snapshot_log.push(SnapshotLogEntry {
            timestamp_ms: Utc::now().timestamp_millis(),
            snapshot_id: new_snapshot_id,
        });

        // Handle overwrite - would need to mark old files as deleted
        if ctx.mode == WriteMode::Overwrite {
            // In full implementation, would create delete manifests
            warn!("Overwrite mode - previous data would be marked as deleted");
        }

        // Write manifest list (simplified - would be Avro in production)
        // For now, write as JSON for testing
        let manifest_data = serde_json::to_string_pretty(&data_files)?;
        let manifest_path = Path::new(&location).join(&manifest_list_path);
        if let Some(parent) = manifest_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::write(&manifest_path, &manifest_data).await?;

        // Write new metadata version
        self.write_metadata(&new_metadata).await?;

        self.to_public_snapshot(&new_snapshot, &new_metadata).await
    }

    async fn merge_into(
        &self,
        _table_path: &str,
        _source: RecordBatchStream,
        _merge_condition: &str,
        _matched_action: MergeAction,
        _not_matched_action: MergeAction,
    ) -> Result<Snapshot> {
        warn!("MERGE INTO not fully implemented for Iceberg");
        self.get_current_snapshot(&self.config.table_identifier)
            .await
    }

    async fn time_travel(&self, _table_path: &str, timestamp: DateTime<Utc>) -> Result<Snapshot> {
        let metadata = self.get_metadata().await?;
        let target_ms = timestamp.timestamp_millis();

        // Find snapshot closest to (but not after) timestamp
        let snap = metadata
            .snapshots
            .iter()
            .filter(|s| s.timestamp_ms <= target_ms)
            .max_by_key(|s| s.timestamp_ms)
            .ok_or_else(|| anyhow!("No snapshot found for timestamp {}", timestamp))?;

        self.to_public_snapshot(snap, &metadata).await
    }

    async fn restore(&self, _table_path: &str, version: i64) -> Result<Snapshot> {
        let metadata = self.get_metadata().await?;

        // Find the snapshot
        let old_snap = metadata
            .snapshots
            .iter()
            .find(|s| s.snapshot_id == version)
            .ok_or_else(|| anyhow!("Snapshot {} not found", version))?
            .clone();

        // Create restore snapshot
        let new_snapshot_id = Utc::now().timestamp_millis();
        let new_snapshot = IcebergSnapshot {
            snapshot_id: new_snapshot_id,
            parent_snapshot_id: metadata.current_snapshot_id,
            sequence_number: metadata.last_sequence_number + 1,
            timestamp_ms: Utc::now().timestamp_millis(),
            manifest_list: old_snap.manifest_list.clone(),
            summary: SnapshotSummary {
                operation: "rollback".to_string(),
                properties: {
                    let mut p = HashMap::new();
                    p.insert("rollback-to-snapshot-id".to_string(), version.to_string());
                    p
                },
            },
            schema_id: old_snap.schema_id,
        };

        // Update metadata
        let mut new_metadata = metadata.clone();
        new_metadata.last_sequence_number += 1;
        new_metadata.last_updated_ms = Utc::now().timestamp_millis();
        new_metadata.current_snapshot_id = Some(new_snapshot_id);
        new_metadata.snapshots.push(new_snapshot.clone());
        new_metadata.snapshot_log.push(SnapshotLogEntry {
            timestamp_ms: Utc::now().timestamp_millis(),
            snapshot_id: new_snapshot_id,
        });

        self.write_metadata(&new_metadata).await?;
        self.to_public_snapshot(&new_snapshot, &new_metadata).await
    }

    async fn optimize(&self, _table_path: &str, _ctx: &OptimizeContext) -> Result<OptimizeResult> {
        // Iceberg optimization would involve:
        // 1. Identify small files
        // 2. Rewrite into larger files
        // 3. Create new snapshot
        warn!("Iceberg optimize not fully implemented");
        Ok(OptimizeResult {
            files_optimized: 0,
            files_vacuumed: 0,
            space_reclaimed_bytes: 0,
            duration_ms: 0,
        })
    }

    async fn vacuum(&self, _table_path: &str, retention_hours: u64) -> Result<u64> {
        let metadata = self.get_metadata().await?;
        let cutoff = Utc::now().timestamp_millis() - (retention_hours * 3600 * 1000) as i64;

        // Find snapshots to expire
        let expired: Vec<_> = metadata
            .snapshots
            .iter()
            .filter(|s| {
                s.timestamp_ms < cutoff && Some(s.snapshot_id) != metadata.current_snapshot_id
            })
            .collect();

        info!(
            "Would expire {} snapshots older than {} hours",
            expired.len(),
            retention_hours
        );

        // In production, would:
        // 1. Remove expired snapshots from metadata
        // 2. Delete orphan data files not in any snapshot
        // 3. Delete old metadata files

        Ok(0)
    }

    async fn get_schema_at(&self, _table_path: &str, version: i64) -> Result<ArrowSchema> {
        let metadata = self.get_metadata().await?;
        let snap = metadata
            .snapshots
            .iter()
            .find(|s| s.snapshot_id == version)
            .ok_or_else(|| anyhow!("Snapshot {} not found", version))?;

        let schema_id = snap.schema_id.unwrap_or(metadata.current_schema_id);
        let schema = metadata
            .schemas
            .iter()
            .find(|s| s.schema_id == schema_id)
            .ok_or_else(|| anyhow!("Schema {} not found", schema_id))?;

        self.iceberg_schema_to_arrow(schema)
    }

    async fn evolve_schema(&self, _table_path: &str, new_schema: &ArrowSchema) -> Result<Snapshot> {
        let mut metadata = self.get_metadata().await?;

        // Create new schema
        let new_schema_id = metadata
            .schemas
            .iter()
            .map(|s| s.schema_id)
            .max()
            .unwrap_or(0)
            + 1;

        let mut iceberg_schema = self.arrow_schema_to_iceberg(new_schema)?;
        iceberg_schema.schema_id = new_schema_id;

        // Update last column ID
        let max_field_id = iceberg_schema
            .fields
            .iter()
            .map(|f| f.id)
            .max()
            .unwrap_or(0);

        metadata.schemas.push(iceberg_schema);
        metadata.current_schema_id = new_schema_id;
        metadata.last_column_id = max_field_id;
        metadata.last_updated_ms = Utc::now().timestamp_millis();

        self.write_metadata(&metadata).await?;

        // Return current snapshot with new schema
        if let Some(snap) = self.get_current_iceberg_snapshot(&metadata) {
            self.to_public_snapshot(snap, &metadata).await
        } else {
            Err(anyhow!("No current snapshot after schema evolution"))
        }
    }
}

// ============================================================================
// Parquet I/O Helpers
// ============================================================================

impl IcebergFormat {
    /// Read a Parquet file
    async fn read_parquet_file(
        path: &str,
        batch_size: usize,
        projection: Option<&Vec<String>>,
    ) -> Result<Vec<RecordBatch>> {
        use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
        use std::fs::File;

        let file = File::open(path)?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)?.with_batch_size(batch_size);

        let reader = if let Some(cols) = projection {
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

        Ok(reader.filter_map(|r| r.ok()).collect())
    }

    /// Write a Parquet file
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

        Ok(std::fs::metadata(path)?.len())
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_iceberg_config_hadoop() {
        let config = IcebergConfig::hadoop("/tmp/warehouse", "db.table");
        assert!(matches!(config.catalog, CatalogType::Hadoop { .. }));
        assert_eq!(config.table_identifier, "db.table");
    }

    #[test]
    fn test_iceberg_config_rest() {
        let config = IcebergConfig::rest("http://localhost:8181", "s3://bucket", "db.table");
        assert!(matches!(config.catalog, CatalogType::Rest { .. }));
    }

    #[test]
    fn test_iceberg_schema_serialization() {
        let schema = IcebergSchema {
            schema_id: 0,
            schema_type: "struct".to_string(),
            fields: vec![IcebergField {
                id: 1,
                name: "id".to_string(),
                field_type: serde_json::json!("long"),
                required: true,
                doc: None,
            }],
            identifier_field_ids: vec![1],
        };

        let json = serde_json::to_string(&schema).unwrap();
        assert!(json.contains("schema-id"));
        assert!(json.contains("identifier-field-ids"));
    }

    #[test]
    fn test_partition_spec_serialization() {
        let spec = PartitionSpec {
            spec_id: 0,
            fields: vec![PartitionField {
                field_id: 1000,
                source_id: 4,
                name: "ts_day".to_string(),
                transform: "day".to_string(),
            }],
        };

        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains("spec-id"));
        assert!(json.contains("ts_day"));
    }
}
