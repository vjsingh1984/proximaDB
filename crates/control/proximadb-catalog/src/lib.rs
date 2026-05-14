//! xCatalog contract types
//!
//! These serializable contracts are shared by catalog implementations, query planning, storage
//! selection, and protocol/API boundaries without requiring the root runtime crate.

use arrow_schema::DataType as ArrowDataType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Namespace metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogNamespace {
    /// Namespace hierarchy (e.g., ["database", "schema"])
    pub levels: Vec<String>,
    /// Namespace properties
    pub properties: HashMap<String, String>,
    /// Owner principal
    pub owner: Option<String>,
    /// Storage location
    pub location: Option<String>,
    /// Creation timestamp (millis since epoch)
    pub created_at_ms: i64,
    /// Last update timestamp (millis since epoch)
    pub updated_at_ms: i64,
}

impl CatalogNamespace {
    /// Create a new namespace
    pub fn new(levels: Vec<String>) -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        Self {
            levels,
            properties: HashMap::new(),
            owner: None,
            location: None,
            created_at_ms: now,
            updated_at_ms: now,
        }
    }

    /// Get fully qualified name
    pub fn fqn(&self) -> String {
        self.levels.join(".")
    }

    /// Add a property
    pub fn with_property(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.properties.insert(key.into(), value.into());
        self
    }

    /// Set owner
    pub fn with_owner(mut self, owner: impl Into<String>) -> Self {
        self.owner = Some(owner.into());
        self
    }

    /// Set location
    pub fn with_location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }
}

/// Column definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogColumn {
    /// Column ID (stable across renames)
    pub id: i32,
    /// Column name
    pub name: String,
    /// Data type
    pub data_type: CatalogDataType,
    /// Is nullable
    pub nullable: bool,
    /// Default value (SQL expression)
    pub default_value: Option<String>,
    /// Column comment
    pub comment: Option<String>,
    /// Column metadata/properties
    pub properties: HashMap<String, String>,
}

impl CatalogColumn {
    /// Create a new column
    pub fn new(id: i32, name: impl Into<String>, data_type: CatalogDataType) -> Self {
        Self {
            id,
            name: name.into(),
            data_type,
            nullable: true,
            default_value: None,
            comment: None,
            properties: HashMap::new(),
        }
    }

    /// Set nullable
    pub fn nullable(mut self, nullable: bool) -> Self {
        self.nullable = nullable;
        self
    }

    /// Set default value
    pub fn with_default(mut self, default: impl Into<String>) -> Self {
        self.default_value = Some(default.into());
        self
    }

    /// Set comment
    pub fn with_comment(mut self, comment: impl Into<String>) -> Self {
        self.comment = Some(comment.into());
        self
    }
}

/// Data types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CatalogDataType {
    /// Boolean true/false value
    Boolean,
    /// 8-bit signed integer
    Int8,
    /// 16-bit signed integer
    Int16,
    /// 32-bit signed integer
    Int32,
    /// 64-bit signed integer
    Int64,
    /// 32-bit IEEE 754 floating-point number
    Float32,
    /// 64-bit IEEE 754 floating-point number
    Float64,
    /// UTF-8 encoded string
    String,
    /// Arbitrary binary data
    Binary,
    /// Calendar date (no time component)
    Date,
    /// Time of day (no date component)
    Time,
    /// Timestamp without timezone
    Timestamp,
    /// Timestamp with timezone
    TimestampTz,
    /// Exact decimal numeric value
    Decimal,
    /// Universally unique identifier (UUID v4)
    Uuid,
    /// JSON document stored as text
    Json,
    /// Fixed-size array of floats for vector embeddings
    Vector,
    /// Sparse vector (map of index to value)
    SparseVector,
    /// Binary vector (packed bits)
    BinaryVector,
}

impl CatalogDataType {
    /// Convert to proto DataType value
    pub fn to_proto_i32(&self) -> i32 {
        match self {
            CatalogDataType::Boolean => 1,
            CatalogDataType::Int8 => 2,
            CatalogDataType::Int16 => 3,
            CatalogDataType::Int32 => 4,
            CatalogDataType::Int64 => 5,
            CatalogDataType::Float32 => 6,
            CatalogDataType::Float64 => 7,
            CatalogDataType::String => 8,
            CatalogDataType::Binary => 9,
            CatalogDataType::Date => 10,
            CatalogDataType::Time => 11,
            CatalogDataType::Timestamp => 12,
            CatalogDataType::TimestampTz => 13,
            CatalogDataType::Decimal => 14,
            CatalogDataType::Uuid => 15,
            CatalogDataType::Json => 16,
            CatalogDataType::Vector => 20,
            CatalogDataType::SparseVector => 21,
            CatalogDataType::BinaryVector => 22,
        }
    }

    /// Create from proto DataType value
    pub fn from_proto_i32(value: i32) -> Self {
        match value {
            1 => CatalogDataType::Boolean,
            2 => CatalogDataType::Int8,
            3 => CatalogDataType::Int16,
            4 => CatalogDataType::Int32,
            5 => CatalogDataType::Int64,
            6 => CatalogDataType::Float32,
            7 => CatalogDataType::Float64,
            8 => CatalogDataType::String,
            9 => CatalogDataType::Binary,
            10 => CatalogDataType::Date,
            11 => CatalogDataType::Time,
            12 => CatalogDataType::Timestamp,
            13 => CatalogDataType::TimestampTz,
            14 => CatalogDataType::Decimal,
            15 => CatalogDataType::Uuid,
            16 => CatalogDataType::Json,
            20 => CatalogDataType::Vector,
            21 => CatalogDataType::SparseVector,
            22 => CatalogDataType::BinaryVector,
            _ => CatalogDataType::String,
        }
    }

    /// Convert to Arrow DataType
    pub fn to_arrow_datatype(&self) -> ArrowDataType {
        match self {
            CatalogDataType::Boolean => ArrowDataType::Boolean,
            CatalogDataType::Int8 => ArrowDataType::Int8,
            CatalogDataType::Int16 => ArrowDataType::Int16,
            CatalogDataType::Int32 => ArrowDataType::Int32,
            CatalogDataType::Int64 => ArrowDataType::Int64,
            CatalogDataType::Float32 => ArrowDataType::Float32,
            CatalogDataType::Float64 => ArrowDataType::Float64,
            CatalogDataType::String => ArrowDataType::Utf8,
            CatalogDataType::Binary => ArrowDataType::Binary,
            CatalogDataType::Date => ArrowDataType::Date32,
            CatalogDataType::Time => ArrowDataType::Time64(arrow_schema::TimeUnit::Nanosecond),
            CatalogDataType::Timestamp => {
                ArrowDataType::Timestamp(arrow_schema::TimeUnit::Nanosecond, None)
            }
            CatalogDataType::TimestampTz => {
                ArrowDataType::Timestamp(arrow_schema::TimeUnit::Nanosecond, Some("UTC".into()))
            }
            CatalogDataType::Decimal => ArrowDataType::Decimal128(38, 10),
            CatalogDataType::Uuid => ArrowDataType::Utf8, // UUID as string
            CatalogDataType::Json => ArrowDataType::Utf8, // JSON as string
            CatalogDataType::Vector => ArrowDataType::List(
                Box::new(arrow_schema::Field::new(
                    "item",
                    ArrowDataType::Float32,
                    true,
                ))
                .into(),
            ),
            CatalogDataType::SparseVector => ArrowDataType::Map(
                Box::new(arrow_schema::Field::new(
                    "entries",
                    ArrowDataType::Struct(
                        vec![
                            arrow_schema::Field::new("key", ArrowDataType::Int32, false),
                            arrow_schema::Field::new("value", ArrowDataType::Float32, false),
                        ]
                        .into(),
                    ),
                    false,
                ))
                .into(),
                false,
            ),
            CatalogDataType::BinaryVector => ArrowDataType::Binary, // Packed bits as binary
        }
    }
}

impl CatalogDataType {
    /// Convert to the canonical [`proximadb_data_model::ProximaType`].
    ///
    /// This is the single bridge between the catalog layer and the wire type
    /// system (spec §4). Call site example:
    /// ```ignore
    /// let pt = CatalogDataType::Decimal.to_proxima_type();
    /// let oid = pt.pgwire_oid(); // 1700
    /// ```
    pub fn to_proxima_type(&self) -> proximadb_data_model::ProximaType {
        use proximadb_data_model::{ProximaType, TimeUnit, VectorElement};
        match self {
            CatalogDataType::Boolean => ProximaType::Boolean,
            CatalogDataType::Int8 => ProximaType::Int8,
            CatalogDataType::Int16 => ProximaType::Int16,
            CatalogDataType::Int32 => ProximaType::Int32,
            CatalogDataType::Int64 => ProximaType::Int64,
            CatalogDataType::Float32 => ProximaType::Float32,
            CatalogDataType::Float64 => ProximaType::Float64,
            CatalogDataType::String => ProximaType::String,
            CatalogDataType::Binary => ProximaType::Binary,
            CatalogDataType::Date => ProximaType::Date,
            CatalogDataType::Time => ProximaType::Time(TimeUnit::Nanosecond),
            CatalogDataType::Timestamp => ProximaType::Timestamp(TimeUnit::Nanosecond),
            CatalogDataType::TimestampTz => ProximaType::TimestampTz(TimeUnit::Nanosecond),
            CatalogDataType::Decimal => ProximaType::Decimal {
                precision: 38,
                scale: 10,
            },
            CatalogDataType::Uuid => ProximaType::Uuid,
            CatalogDataType::Json => ProximaType::Json,
            CatalogDataType::Vector => ProximaType::DenseVector {
                element: VectorElement::Float32,
                dim: 0,
            },
            CatalogDataType::SparseVector => ProximaType::SparseVector {
                element: VectorElement::Float32,
            },
            CatalogDataType::BinaryVector => ProximaType::BinaryVector { dim: 0 },
        }
    }
}

/// Table schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogTableSchema {
    /// Table name
    pub name: String,
    /// Table columns
    pub columns: Vec<CatalogColumn>,
    /// Primary key columns (by name)
    pub primary_key: Vec<String>,
    /// Indexes
    pub indexes: Vec<CatalogIndex>,
    /// Schema version
    pub schema_version: i32,
    /// Table properties
    pub properties: HashMap<String, String>,
    /// Storage location
    pub location: Option<String>,
    /// Creation timestamp
    pub created_at_ms: i64,
    /// Last update timestamp
    pub updated_at_ms: i64,
}

impl Default for CatalogTableSchema {
    fn default() -> Self {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        Self {
            name: String::new(),
            columns: Vec::new(),
            primary_key: Vec::new(),
            indexes: Vec::new(),
            schema_version: 1,
            properties: HashMap::new(),
            location: None,
            created_at_ms: now,
            updated_at_ms: now,
        }
    }
}

impl CatalogTableSchema {
    /// Create a new table schema
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Add a column
    pub fn with_column(mut self, column: CatalogColumn) -> Self {
        self.columns.push(column);
        self
    }

    /// Set primary key
    pub fn with_primary_key(mut self, columns: Vec<String>) -> Self {
        self.primary_key = columns;
        self
    }

    /// Add an index
    pub fn with_index(mut self, index: CatalogIndex) -> Self {
        self.indexes.push(index);
        self
    }
}

/// Index definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogIndex {
    /// Index name
    pub name: String,
    /// Indexed columns
    pub columns: Vec<String>,
    /// Index type
    pub index_type: CatalogIndexType,
    /// Is unique index
    pub is_unique: bool,
    /// Index properties
    pub properties: HashMap<String, String>,
}

impl CatalogIndex {
    /// Create a new index
    pub fn new(
        name: impl Into<String>,
        columns: Vec<String>,
        index_type: CatalogIndexType,
    ) -> Self {
        Self {
            name: name.into(),
            columns,
            index_type,
            is_unique: false,
            properties: HashMap::new(),
        }
    }

    /// Set unique
    pub fn unique(mut self) -> Self {
        self.is_unique = true;
        self
    }
}

/// Index types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CatalogIndexType {
    /// B-tree index
    BTree,
    /// Hash index
    Hash,
    /// Full-text index
    FullText,
    /// PostgreSQL-compatible GIN index for JSONB/path-heavy document projections
    Gin,
    /// HNSW vector index
    Hnsw,
    /// IVF vector index
    Ivf,
    /// Product quantization
    Pq,
}

/// Source-of-truth mode for a cataloged table, stream, projection, or external format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CatalogAuthorityMode {
    /// ProximaRecord plus WAL/log/manifest own durable truth.
    #[default]
    InternalCanonical,
    /// An external table/source owns durable truth; ProximaDB maps and governs access.
    ExternalAuthoritative,
    /// Point-in-time import from an external source.
    ImportedSnapshot,
    /// Publication generated from canonical records.
    ExportedPublication,
    /// Rebuildable structure derived from canonical records or events.
    RebuildableProjection,
}

/// Physical layout family selected for a cataloged object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CatalogStorageLayoutKind {
    /// Mutable record/row-oriented layout.
    #[default]
    RowRecord,
    /// Log-structured merge layout for write-heavy data.
    LsmRecord,
    /// Partition Attributes Across hybrid row/column layout.
    Pax,
    /// Columnar segment layout.
    Columnar,
    /// Append-only event stream layout.
    AppendOnlyEvent,
    /// Vector ANN projection or fragment layout.
    VectorAnn,
    /// Graph topology projection such as adjacency, CSR, or COO.
    GraphTopology,
    /// Time-series compression or partition block.
    TimeSeriesBlock,
    /// Externally owned or externally formatted table.
    ExternalTable,
}

/// Physical format used by a storage layout or projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CatalogPhysicalFormat {
    /// Native ProximaDB record/block format.
    #[default]
    ProximaBlock,
    /// Sorted string table or SST-family layout.
    Sst,
    /// Apache Arrow in-memory or IPC format.
    Arrow,
    /// Apache Parquet file format.
    Parquet,
    /// Apache ORC file format.
    Orc,
    /// Apache Iceberg table format.
    Iceberg,
    /// Delta Lake table format.
    Delta,
    /// Apache Hudi table format.
    Hudi,
    /// GraphAr-style graph lake format.
    GraphAr,
    /// Format not yet modeled as a first-class enum variant.
    External(String),
}

/// Write and refresh behavior for a cataloged layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CatalogWriteMode {
    /// In-place or logically mutable writes are accepted.
    #[default]
    Mutable,
    /// Writes append immutable records/events.
    AppendOnly,
    /// Writes create new snapshots or file sets.
    CopyOnWrite,
    /// Data is refreshed from an external authority.
    ExternalRefresh,
    /// Data arrives through a stream/CDC connector.
    StreamingIngest,
    /// Read-only mapping.
    ReadOnly,
}

/// Freshness semantics for a rebuildable projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ProjectionFreshness {
    /// Projection is updated in the same write transaction as canonical records.
    Synchronous,
    /// Projection may lag but must stay within a declared bound.
    BoundedLag,
    /// Projection is refreshed lazily on demand or by background work.
    #[default]
    Lazy,
    /// Projection is refreshed only by explicit command.
    Manual,
}

/// Projection or access-method family cataloged below xCatalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum CatalogProjectionKind {
    /// JSON path or generated-column projection.
    JsonPath,
    /// Full-text or lexical search projection.
    FullText,
    /// Dense/sparse/binary vector ANN projection.
    VectorAnn,
    /// Graph adjacency, CSR, COO, or graph algorithm projection.
    GraphTopology,
    /// Columnar materialization.
    Columnar,
    /// Time-series rollup or compression projection.
    TimeSeries,
    /// Trace assembly, service map, or observability correlation projection.
    Observability,
    /// Materialized SQL/logical view.
    MaterializedView,
    /// Generic projection until a more specific family is introduced.
    #[default]
    Other,
}

/// Catalog metadata shared by internal layouts and external table mappings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogStorageLayout {
    /// Stable layout identifier inside the table/profile/collection.
    pub name: String,
    /// Source-of-truth role for this layout.
    pub authority: CatalogAuthorityMode,
    /// Physical layout family.
    pub layout_kind: CatalogStorageLayoutKind,
    /// Concrete physical format.
    pub physical_format: CatalogPhysicalFormat,
    /// Write/refresh behavior.
    pub write_mode: CatalogWriteMode,
    /// Optional URI/path/catalog identifier for the physical data.
    pub location: Option<String>,
    /// Snapshot or isolation semantics, e.g. "mvcc", "iceberg-snapshot", "external-latest".
    pub snapshot_semantics: Option<String>,
    /// Whether policy/RLS enforcement happens inside ProximaDB before rows leave this layout.
    pub policy_enforced_in_proxima: bool,
    /// Names of ProximaType fields that require lossy conversion in this format, if any.
    pub lossy_type_mappings: Vec<String>,
    /// Additional implementation-specific metadata.
    pub properties: HashMap<String, String>,
}

impl Default for CatalogStorageLayout {
    fn default() -> Self {
        Self {
            name: "primary".to_string(),
            authority: CatalogAuthorityMode::InternalCanonical,
            layout_kind: CatalogStorageLayoutKind::RowRecord,
            physical_format: CatalogPhysicalFormat::ProximaBlock,
            write_mode: CatalogWriteMode::Mutable,
            location: None,
            snapshot_semantics: None,
            policy_enforced_in_proxima: true,
            lossy_type_mappings: Vec::new(),
            properties: HashMap::new(),
        }
    }
}

impl CatalogStorageLayout {
    /// Create an internal canonical layout with default policy enforcement.
    pub fn internal(name: impl Into<String>, layout_kind: CatalogStorageLayoutKind) -> Self {
        Self {
            name: name.into(),
            layout_kind,
            ..Default::default()
        }
    }

    /// Create an external authoritative table mapping.
    pub fn external_authoritative(
        name: impl Into<String>,
        format: CatalogPhysicalFormat,
        location: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            authority: CatalogAuthorityMode::ExternalAuthoritative,
            layout_kind: CatalogStorageLayoutKind::ExternalTable,
            physical_format: format,
            write_mode: CatalogWriteMode::ExternalRefresh,
            location: Some(location.into()),
            policy_enforced_in_proxima: false,
            ..Default::default()
        }
    }
}

/// Catalog metadata for a rebuildable projection/access method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogProjection {
    /// Projection name.
    pub name: String,
    /// Projection family.
    pub kind: CatalogProjectionKind,
    /// Physical format backing the projection.
    pub physical_format: CatalogPhysicalFormat,
    /// Canonical layout/table/stream used to rebuild this projection.
    pub rebuild_source: String,
    /// Freshness semantics.
    pub freshness: ProjectionFreshness,
    /// Optional maximum accepted lag in milliseconds for bounded-lag projections.
    pub max_lag_ms: Option<i64>,
    /// Whether the projection can be rebuilt without data loss.
    pub rebuildable: bool,
    /// Whether using this projection can change recall or ranking quality.
    pub lossy: bool,
    /// Human-readable support status: experimental, beta, supported, deprecated.
    pub support_status: String,
    /// Additional implementation-specific metadata.
    pub properties: HashMap<String, String>,
}

impl CatalogProjection {
    /// Create a rebuildable projection with lazy freshness by default.
    pub fn rebuildable(
        name: impl Into<String>,
        kind: CatalogProjectionKind,
        rebuild_source: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            kind,
            physical_format: CatalogPhysicalFormat::ProximaBlock,
            rebuild_source: rebuild_source.into(),
            freshness: ProjectionFreshness::Lazy,
            max_lag_ms: None,
            rebuildable: true,
            lossy: false,
            support_status: "experimental".to_string(),
            properties: HashMap::new(),
        }
    }
}

/// Optional relational semantics for a table/profile.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RelationalCapabilities {
    /// Primary key columns, if enforced.
    pub primary_key: Vec<String>,
    /// Unique indexes or constraints.
    pub unique_indexes: Vec<CatalogIndex>,
    /// Non-unique secondary indexes.
    pub secondary_indexes: Vec<CatalogIndex>,
    /// Foreign-key and check constraints.
    pub constraints: Vec<ColumnConstraint>,
    /// Materialized view names derived from this table/profile.
    pub materialized_views: Vec<String>,
    /// Transaction/isolation profile name.
    pub transaction_profile: Option<String>,
    /// Schema evolution policy name.
    pub schema_evolution_policy: Option<String>,
}

impl RelationalCapabilities {
    /// Returns true when this table opts into any relational integrity semantics.
    pub fn has_enforced_semantics(&self) -> bool {
        !self.primary_key.is_empty()
            || !self.unique_indexes.is_empty()
            || !self.secondary_indexes.is_empty()
            || !self.constraints.is_empty()
            || !self.materialized_views.is_empty()
            || self.transaction_profile.is_some()
            || self.schema_evolution_policy.is_some()
    }
}

/// Table statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogTableStatistics {
    /// Row count
    pub row_count: u64,
    /// Size in bytes
    pub size_bytes: u64,
    /// Number of files
    pub file_count: u64,
    /// Last analyze timestamp
    pub last_analyzed_ms: Option<i64>,
    /// Column statistics
    pub column_stats: HashMap<String, CatalogColumnStatistics>,
}

/// Column statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogColumnStatistics {
    /// Number of distinct values
    pub distinct_count: Option<u64>,
    /// Number of null values
    pub null_count: Option<u64>,
    /// Min value (as string)
    pub min_value: Option<String>,
    /// Max value (as string)
    pub max_value: Option<String>,
    /// Average size in bytes
    pub avg_size_bytes: Option<f64>,
}

/// Partition specification
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogPartitionSpec {
    /// Spec ID
    pub spec_id: i32,
    /// Partition fields
    pub fields: Vec<CatalogPartitionField>,
}

/// Partition field (Iceberg-compatible)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogPartitionField {
    /// Source column ID
    pub source_id: i32,
    /// Field ID (assigned by partition spec)
    pub field_id: i32,
    /// Field name
    pub name: String,
    /// Transform type
    pub transform: PartitionTransform,
}

/// Type alias for backwards compatibility
pub type PartitionField = CatalogPartitionField;

/// Partition transforms (Iceberg-compatible)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PartitionTransform {
    /// Identity transform (no transformation)
    #[default]
    Identity,
    /// Bucket transform (hash into N buckets)
    Bucket(u32),
    /// Truncate transform (truncate to width)
    Truncate(u32),
    /// Year transform (extract year from timestamp/date)
    Year,
    /// Month transform (extract month from timestamp/date)
    Month,
    /// Day transform (extract day from timestamp/date)
    Day,
    /// Hour transform (extract hour from timestamp)
    Hour,
    /// Void transform (always produces null)
    Void,
}

impl std::fmt::Display for PartitionTransform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PartitionTransform::Identity => write!(f, "identity"),
            PartitionTransform::Bucket(n) => write!(f, "bucket[{}]", n),
            PartitionTransform::Truncate(w) => write!(f, "truncate[{}]", w),
            PartitionTransform::Year => write!(f, "year"),
            PartitionTransform::Month => write!(f, "month"),
            PartitionTransform::Day => write!(f, "day"),
            PartitionTransform::Hour => write!(f, "hour"),
            PartitionTransform::Void => write!(f, "void"),
        }
    }
}

impl PartitionTransform {
    /// Parse from string (Iceberg format)
    pub fn parse_from_iceberg_format(s: &str) -> Self {
        let lower = s.to_lowercase();
        if lower == "identity" {
            PartitionTransform::Identity
        } else if lower.starts_with("bucket[") {
            let n: u32 = lower
                .trim_start_matches("bucket[")
                .trim_end_matches(']')
                .parse()
                .unwrap_or(16);
            PartitionTransform::Bucket(n)
        } else if lower.starts_with("truncate[") {
            let w: u32 = lower
                .trim_start_matches("truncate[")
                .trim_end_matches(']')
                .parse()
                .unwrap_or(16);
            PartitionTransform::Truncate(w)
        } else if lower == "year" {
            PartitionTransform::Year
        } else if lower == "month" {
            PartitionTransform::Month
        } else if lower == "day" {
            PartitionTransform::Day
        } else if lower == "hour" {
            PartitionTransform::Hour
        } else if lower == "void" {
            PartitionTransform::Void
        } else {
            PartitionTransform::Identity
        }
    }
}

/// Sort order
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CatalogSortOrder {
    /// Order ID
    pub order_id: i32,
    /// Sort fields
    pub fields: Vec<CatalogSortField>,
}

/// Sort field (Iceberg-compatible)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSortField {
    /// Source column ID
    pub source_id: i32,
    /// Transform to apply before sorting
    pub transform: PartitionTransform,
    /// Sort direction
    pub direction: SortDirection,
    /// Null ordering
    pub null_order: NullOrder,
}

/// Type alias for backwards compatibility
pub type SortField = CatalogSortField;

/// Sort direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortDirection {
    /// Sort values from smallest to largest (ASC)
    Ascending,
    /// Sort values from largest to smallest (DESC)
    Descending,
}

/// Null ordering
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NullOrder {
    /// NULL values sort before non-NULL values
    NullsFirst,
    /// NULL values sort after non-NULL values
    NullsLast,
}

/// Schema evolution request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSchemaEvolution {
    /// Changes to apply
    pub changes: Vec<SchemaChange>,
}

/// Schema change types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SchemaChange {
    /// Add a new column
    AddColumn {
        /// Name of the new column
        name: String,
        /// Data type of the new column
        data_type: CatalogDataType,
        /// Whether the column accepts NULL values
        nullable: bool,
        /// Optional SQL expression used as the column default
        default_value: Option<String>,
        /// Optional human-readable description of the column
        comment: Option<String>,
        /// Position: None = end, Some(col) = after column
        after: Option<String>,
    },
    /// Drop a column
    DropColumn {
        /// Name of the column to drop
        name: String,
    },
    /// Rename a column
    RenameColumn {
        /// Current column name
        old_name: String,
        /// Desired new column name
        new_name: String,
    },
    /// Change column type (must be compatible)
    ChangeType {
        /// Name of the column whose type should change
        name: String,
        /// New data type (must be compatible with the existing type)
        new_type: CatalogDataType,
    },
    /// Update column comment
    UpdateComment {
        /// Name of the column to update
        name: String,
        /// New comment text
        comment: String,
    },
    /// Make column nullable (DROP NOT NULL)
    MakeNullable {
        /// Name of the column to make nullable
        name: String,
    },
    /// Make column NOT NULL (SET NOT NULL)
    MakeNotNullable {
        /// Name of the column to make non-nullable
        name: String,
    },
    /// Add default value
    SetDefault {
        /// Name of the column to update
        name: String,
        /// SQL expression to use as the default value
        default_value: String,
    },
    /// Remove default value
    DropDefault {
        /// Name of the column whose default should be removed
        name: String,
    },
    /// Move column position (FIRST or AFTER another column)
    MoveColumn {
        /// Name of the column to move
        name: String,
        /// Position: None = FIRST, Some(col) = AFTER column
        after: Option<String>,
    },
    /// Add a constraint to a column or table
    AddConstraint {
        /// Constraint name (optional for some DBs)
        constraint_name: Option<String>,
        /// Constraint type
        constraint: ColumnConstraint,
    },
    /// Drop a constraint
    DropConstraint {
        /// Name of the constraint to drop
        constraint_name: String,
    },
}

/// Column constraint types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ColumnConstraint {
    /// UNIQUE constraint on one or more columns
    Unique {
        /// Column names included in the uniqueness constraint
        columns: Vec<String>,
    },
    /// CHECK constraint with SQL expression
    Check {
        /// SQL boolean expression that rows must satisfy
        expression: String,
    },
    /// FOREIGN KEY constraint
    ForeignKey {
        /// Local column names that form the foreign key
        columns: Vec<String>,
        /// Name of the referenced table
        references_table: String,
        /// Column names in the referenced table
        references_columns: Vec<String>,
        /// Action to take when the referenced row is deleted
        on_delete: Option<ReferentialAction>,
        /// Action to take when the referenced row is updated
        on_update: Option<ReferentialAction>,
    },
}

/// Referential actions for foreign key constraints
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReferentialAction {
    /// CASCADE - propagate changes to referencing rows
    Cascade,
    /// SET NULL - set referencing columns to NULL
    SetNull,
    /// SET DEFAULT - set referencing columns to their default values
    SetDefault,
    /// RESTRICT - prevent the action if there are referencing rows
    Restrict,
    /// NO ACTION - similar to RESTRICT but deferred
    NoAction,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_namespace_fqn() {
        let ns = CatalogNamespace::new(vec!["catalog".into(), "database".into()]);
        assert_eq!(ns.fqn(), "catalog.database");
    }

    #[test]
    fn test_column_builder() {
        let col = CatalogColumn::new(1, "id", CatalogDataType::Int64)
            .nullable(false)
            .with_comment("Primary key");

        assert_eq!(col.name, "id");
        assert!(!col.nullable);
        assert_eq!(col.comment, Some("Primary key".to_string()));
    }

    #[test]
    fn test_table_schema_builder() {
        let schema = CatalogTableSchema::new("users")
            .with_column(CatalogColumn::new(1, "id", CatalogDataType::Int64).nullable(false))
            .with_column(CatalogColumn::new(2, "name", CatalogDataType::String))
            .with_primary_key(vec!["id".into()]);

        assert_eq!(schema.name, "users");
        assert_eq!(schema.columns.len(), 2);
        assert_eq!(schema.primary_key, vec!["id"]);
    }

    #[test]
    fn test_data_type_roundtrip() {
        let types = vec![
            CatalogDataType::Boolean,
            CatalogDataType::Int64,
            CatalogDataType::Float64,
            CatalogDataType::String,
            CatalogDataType::Vector,
        ];

        for dt in types {
            let proto = dt.to_proto_i32();
            let back = CatalogDataType::from_proto_i32(proto);
            assert_eq!(dt, back);
        }
    }

    // ----- Phase A: catalog → ProximaType bridge tests -----

    #[test]
    fn test_catalog_to_proxima_type_lossless() {
        let all = [
            CatalogDataType::Boolean,
            CatalogDataType::Int8,
            CatalogDataType::Int16,
            CatalogDataType::Int32,
            CatalogDataType::Int64,
            CatalogDataType::Float32,
            CatalogDataType::Float64,
            CatalogDataType::String,
            CatalogDataType::Binary,
            CatalogDataType::Date,
            CatalogDataType::Time,
            CatalogDataType::Timestamp,
            CatalogDataType::TimestampTz,
            CatalogDataType::Decimal,
            CatalogDataType::Uuid,
            CatalogDataType::Json,
            CatalogDataType::Vector,
            CatalogDataType::SparseVector,
            CatalogDataType::BinaryVector,
        ];
        for dt in &all {
            let pt = dt.to_proxima_type();
            // Every variant must map — no panics, no unhandled cases
            let _ = format!("{:?}", pt);
        }
    }

    #[test]
    fn test_catalog_decimal_maps_to_proxima_decimal() {
        use proximadb_data_model::ProximaType;
        let pt = CatalogDataType::Decimal.to_proxima_type();
        assert!(matches!(
            pt,
            ProximaType::Decimal {
                precision: 38,
                scale: 10
            }
        ));
    }

    #[test]
    fn test_catalog_timestamptz_pgwire_oid() {
        // Through the bridge: CatalogDataType → ProximaType → pgwire OID
        let oid = CatalogDataType::TimestampTz.to_proxima_type().pgwire_oid();
        assert_eq!(oid, 1184, "TimestampTz OID must be 1184");
    }

    #[test]
    fn test_catalog_uuid_pgwire_oid() {
        let oid = CatalogDataType::Uuid.to_proxima_type().pgwire_oid();
        assert_eq!(oid, 2950, "UUID OID must be 2950");
    }

    #[test]
    fn test_storage_layout_defaults_to_internal_authority() {
        let layout = CatalogStorageLayout::default();

        assert_eq!(layout.authority, CatalogAuthorityMode::InternalCanonical);
        assert_eq!(layout.layout_kind, CatalogStorageLayoutKind::RowRecord);
        assert_eq!(layout.physical_format, CatalogPhysicalFormat::ProximaBlock);
        assert_eq!(layout.write_mode, CatalogWriteMode::Mutable);
        assert!(layout.policy_enforced_in_proxima);
        assert!(layout.lossy_type_mappings.is_empty());
    }

    #[test]
    fn test_external_authoritative_layout_declares_boundary() {
        let layout = CatalogStorageLayout::external_authoritative(
            "iceberg_orders",
            CatalogPhysicalFormat::Iceberg,
            "s3://warehouse/orders",
        );

        assert_eq!(
            layout.authority,
            CatalogAuthorityMode::ExternalAuthoritative
        );
        assert_eq!(layout.layout_kind, CatalogStorageLayoutKind::ExternalTable);
        assert_eq!(layout.physical_format, CatalogPhysicalFormat::Iceberg);
        assert_eq!(layout.write_mode, CatalogWriteMode::ExternalRefresh);
        assert_eq!(layout.location.as_deref(), Some("s3://warehouse/orders"));
        assert!(!layout.policy_enforced_in_proxima);
    }

    #[test]
    fn test_projection_records_rebuild_source_and_freshness() {
        let projection = CatalogProjection::rebuildable(
            "orders_hnsw",
            CatalogProjectionKind::VectorAnn,
            "orders.primary",
        );

        assert_eq!(projection.kind, CatalogProjectionKind::VectorAnn);
        assert_eq!(projection.rebuild_source, "orders.primary");
        assert_eq!(projection.freshness, ProjectionFreshness::Lazy);
        assert!(projection.rebuildable);
        assert!(!projection.lossy);
    }

    #[test]
    fn test_relational_capabilities_are_optional() {
        let empty = RelationalCapabilities::default();
        assert!(!empty.has_enforced_semantics());

        let with_pk = RelationalCapabilities {
            primary_key: vec!["id".to_string()],
            ..Default::default()
        };
        assert!(with_pk.has_enforced_semantics());
    }
}
