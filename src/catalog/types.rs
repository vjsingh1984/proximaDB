//! Internal Catalog Types
//!
//! These types are used internally by catalog implementations and
//! converted to/from proto types at API boundaries.

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
    /// HNSW vector index
    Hnsw,
    /// IVF vector index
    Ivf,
    /// Product quantization
    Pq,
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
}
