//! DDL (Data Definition Language) Service
//!
//! Provides SQL DDL operations that integrate with the catalog system:
//! - CREATE TABLE / CREATE COLLECTION
//! - DROP TABLE / DROP COLLECTION
//! - ALTER TABLE (schema evolution)
//! - CREATE INDEX / DROP INDEX
//! - CREATE NAMESPACE / DROP NAMESPACE

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tracing::info;

use crate::catalog::CatalogManager;
use crate::catalog::types::{
    CatalogColumn, CatalogDataType, CatalogIndex, CatalogIndexType, CatalogSchemaEvolution,
    CatalogTableSchema, SchemaChange,
};

/// DDL Statement types
#[derive(Debug, Clone)]
pub enum DdlStatement {
    /// CREATE TABLE [IF NOT EXISTS] table_name (columns...)
    CreateTable {
        /// Name of the table to create.
        table_name: String,
        /// Column definitions for the new table.
        columns: Vec<ColumnDefinition>,
        /// When `true`, silently succeeds if the table already exists.
        if_not_exists: bool,
        /// Additional table-level properties (e.g., storage options).
        properties: HashMap<String, String>,
    },
    /// DROP TABLE [IF EXISTS] table_name
    DropTable {
        /// Name of the table to drop.
        table_name: String,
        /// When `true`, silently succeeds if the table does not exist.
        if_exists: bool,
        /// When `true`, also removes all underlying data files.
        purge: bool,
    },
    /// ALTER TABLE table_name ADD COLUMN / DROP COLUMN / etc.
    AlterTable {
        /// Name of the table to alter.
        table_name: String,
        /// Schema changes to apply.
        changes: Vec<AlterTableChange>,
    },
    /// CREATE INDEX [IF NOT EXISTS] index_name ON table_name (columns)
    CreateIndex {
        /// Name of the index to create.
        index_name: String,
        /// Table on which the index is created.
        table_name: String,
        /// Columns included in the index.
        columns: Vec<String>,
        /// Index algorithm and its configuration.
        index_type: IndexType,
        /// When `true`, silently succeeds if the index already exists.
        if_not_exists: bool,
    },
    /// DROP INDEX [IF EXISTS] index_name ON table_name
    DropIndex {
        /// Name of the index to drop.
        index_name: String,
        /// Table that owns the index.
        table_name: String,
        /// When `true`, silently succeeds if the index does not exist.
        if_exists: bool,
    },
    /// CREATE NAMESPACE [IF NOT EXISTS] namespace_name
    CreateNamespace {
        /// Hierarchical namespace path components.
        namespace: Vec<String>,
        /// When `true`, silently succeeds if the namespace already exists.
        if_not_exists: bool,
        /// Additional namespace-level properties.
        properties: HashMap<String, String>,
    },
    /// DROP NAMESPACE [IF EXISTS] namespace_name
    DropNamespace {
        /// Hierarchical namespace path components.
        namespace: Vec<String>,
        /// When `true`, silently succeeds if the namespace does not exist.
        if_exists: bool,
        /// When `true`, also drops all tables contained in the namespace.
        cascade: bool,
    },
    /// CREATE COLLECTION (ProximaDB-specific)
    CreateCollection {
        /// Name of the vector collection to create.
        collection_name: String,
        /// Vector dimensionality for this collection.
        dimension: u32,
        /// Optional storage engine override (e.g., "HELIX", "VIPER").
        engine: Option<String>,
        /// When `true`, silently succeeds if the collection already exists.
        if_not_exists: bool,
        /// Additional collection-level properties.
        properties: HashMap<String, String>,
    },
}

/// Column definition for CREATE TABLE
#[derive(Debug, Clone)]
pub struct ColumnDefinition {
    /// Column name.
    pub name: String,
    /// SQL data type of the column.
    pub data_type: SqlDataType,
    /// Whether the column accepts NULL values.
    pub nullable: bool,
    /// SQL expression used as the column's default value, if any.
    pub default_value: Option<String>,
    /// Optional human-readable description of the column.
    pub comment: Option<String>,
    /// Whether this column is part of the primary key.
    pub primary_key: bool,
}

/// SQL data types
#[derive(Debug, Clone)]
pub enum SqlDataType {
    /// SQL BOOLEAN type.
    Boolean,
    /// 8-bit signed integer (TINYINT).
    TinyInt,
    /// 16-bit signed integer (SMALLINT).
    SmallInt,
    /// 32-bit signed integer (INT).
    Int,
    /// 64-bit signed integer (BIGINT).
    BigInt,
    /// 32-bit IEEE 754 floating-point (FLOAT).
    Float,
    /// 64-bit IEEE 754 floating-point (DOUBLE).
    Double,
    /// Fixed-precision decimal number.
    Decimal {
        /// Total number of significant digits.
        precision: u32,
        /// Number of digits after the decimal point.
        scale: u32,
    },
    /// Variable-length character string.
    Varchar {
        /// Optional maximum number of characters.
        max_length: Option<u32>,
    },
    /// Unbounded text string.
    Text,
    /// Fixed-size binary data.
    Binary,
    /// Variable-size binary large object.
    Blob,
    /// Calendar date (year, month, day).
    Date,
    /// Time of day (hours, minutes, seconds).
    Time,
    /// Timestamp without time zone.
    Timestamp,
    /// Timestamp with time zone.
    TimestampTz,
    /// Universally unique identifier (UUID).
    Uuid,
    /// JSON document.
    Json,
    /// Binary JSON document.
    Jsonb,
    /// Dense vector type: VECTOR(dimension)
    Vector {
        /// Number of dimensions in the vector.
        dimension: u32,
    },
    /// Sparse vector: SPARSE_VECTOR(dimension)
    SparseVector {
        /// Number of dimensions in the sparse vector.
        dimension: u32,
    },
    /// Binary vector: BINARY_VECTOR(dimension)
    BinaryVector {
        /// Number of dimensions in the binary vector.
        dimension: u32,
    },
}

/// ALTER TABLE change types
#[derive(Debug, Clone)]
pub enum AlterTableChange {
    /// ADD COLUMN column_def
    AddColumn(ColumnDefinition),
    /// DROP COLUMN column_name
    DropColumn(String),
    /// RENAME COLUMN old_name TO new_name
    RenameColumn {
        /// Current name of the column.
        old_name: String,
        /// New name for the column.
        new_name: String,
    },
    /// ALTER COLUMN column_name SET DATA TYPE type
    ChangeType {
        /// Name of the column whose type is being changed.
        column_name: String,
        /// New SQL data type for the column.
        new_type: SqlDataType,
    },
    /// ALTER COLUMN column_name SET NOT NULL / DROP NOT NULL
    SetNullable {
        /// Name of the column to modify.
        column_name: String,
        /// New nullability setting (`true` = allow NULL, `false` = NOT NULL).
        nullable: bool,
    },
    /// ALTER COLUMN column_name SET DEFAULT value / DROP DEFAULT
    SetDefault {
        /// Name of the column to modify.
        column_name: String,
        /// New default value expression, or `None` to drop the default.
        default_value: Option<String>,
    },
    /// COMMENT ON COLUMN column_name IS 'comment'
    SetComment {
        /// Name of the column to annotate.
        column_name: String,
        /// Human-readable description to attach to the column.
        comment: String,
    },
    /// Move column position: FIRST or AFTER another_column
    MoveColumn {
        /// Name of the column to reposition.
        column_name: String,
        /// Target position within the column list.
        position: ColumnPosition,
    },
    /// ADD CONSTRAINT
    AddConstraint {
        /// Optional name for the new constraint.
        constraint_name: Option<String>,
        /// Constraint definition to add.
        constraint: TableConstraint,
    },
    /// DROP CONSTRAINT
    DropConstraint {
        /// Name of the constraint to remove.
        constraint_name: String,
    },
}

/// Column position for ALTER TABLE ... MODIFY
#[derive(Debug, Clone)]
pub enum ColumnPosition {
    /// Move column to first position
    First,
    /// Move column after specified column
    After(String),
}

/// Table-level constraint for ALTER TABLE
#[derive(Debug, Clone)]
pub enum TableConstraint {
    /// UNIQUE constraint
    Unique {
        /// Columns that must be unique together.
        columns: Vec<String>,
    },
    /// CHECK constraint
    Check {
        /// SQL boolean expression that each row must satisfy.
        expression: String,
    },
    /// FOREIGN KEY constraint
    ForeignKey {
        /// Columns in this table that form the foreign key.
        columns: Vec<String>,
        /// Referenced table name.
        references_table: String,
        /// Columns in the referenced table.
        references_columns: Vec<String>,
    },
}

/// Index types for CREATE INDEX
#[derive(Debug, Clone)]
pub enum IndexType {
    /// B-tree index (default for scalar columns)
    BTree,
    /// Hash index
    Hash,
    /// Full-text search index
    FullText,
    /// GIN index for JSONB/document projection columns
    Gin,
    /// HNSW vector index (default for vector columns)
    Hnsw {
        /// Maximum number of bi-directional links per node (`M` parameter).
        m: Option<u32>,
        /// Size of the dynamic candidate list during construction.
        ef_construction: Option<u32>,
    },
    /// IVF vector index
    Ivf {
        /// Number of Voronoi cells (inverted lists).
        nlist: Option<u32>,
    },
    /// Product quantization
    Pq {
        /// Number of sub-quantizers.
        m: Option<u32>,
        /// Bits per sub-quantizer code.
        nbits: Option<u32>,
    },
}

/// Result of a DDL operation
#[derive(Debug, Clone)]
pub struct DdlResult {
    /// Was the operation successful?
    pub success: bool,
    /// Number of affected objects (tables, indexes, etc.)
    pub affected_count: u32,
    /// Message describing the result
    pub message: String,
    /// Warnings (if any)
    pub warnings: Vec<String>,
}

impl DdlResult {
    /// Create a successful DDL result
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            success: true,
            affected_count: 1,
            message: message.into(),
            warnings: Vec::new(),
        }
    }

    /// Create a result indicating an object already exists
    pub fn already_exists(object_type: &str, name: &str) -> Self {
        Self {
            success: true,
            affected_count: 0,
            message: format!("{} '{}' already exists", object_type, name),
            warnings: Vec::new(),
        }
    }

    /// Create a result indicating an object was not found
    pub fn not_found(object_type: &str, name: &str) -> Self {
        Self {
            success: true,
            affected_count: 0,
            message: format!("{} '{}' does not exist", object_type, name),
            warnings: Vec::new(),
        }
    }

    /// Add a warning to this result
    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }
}

/// DDL Service for executing DDL statements
pub struct DdlService {
    /// Catalog manager for metadata operations
    catalog_manager: Arc<CatalogManager>,
}

impl DdlService {
    /// Create a new DDL service
    pub fn new(catalog_manager: Arc<CatalogManager>) -> Self {
        Self { catalog_manager }
    }

    /// Execute a DDL statement
    ///
    /// This is the main entry point for DDL operations. It dispatches to the appropriate
    /// handler based on the statement type.
    pub async fn execute(&self, statement: DdlStatement) -> Result<DdlResult> {
        match statement {
            DdlStatement::CreateTable {
                table_name,
                columns,
                if_not_exists,
                properties,
            } => {
                self.create_table(&table_name, columns, if_not_exists, properties)
                    .await
            }
            DdlStatement::DropTable {
                table_name,
                if_exists,
                purge,
            } => self.drop_table(&table_name, if_exists, purge).await,
            DdlStatement::AlterTable {
                table_name,
                changes,
            } => self.alter_table(&table_name, changes).await,
            DdlStatement::CreateIndex {
                index_name,
                table_name,
                columns,
                index_type,
                if_not_exists,
            } => {
                self.create_index(&index_name, &table_name, columns, index_type, if_not_exists)
                    .await
            }
            DdlStatement::DropIndex {
                index_name,
                table_name,
                if_exists,
            } => self.drop_index(&index_name, &table_name, if_exists).await,
            DdlStatement::CreateNamespace {
                namespace,
                if_not_exists,
                properties,
            } => {
                self.create_namespace(&namespace, if_not_exists, properties)
                    .await
            }
            DdlStatement::DropNamespace {
                namespace,
                if_exists,
                cascade,
            } => self.drop_namespace(&namespace, if_exists, cascade).await,
            DdlStatement::CreateCollection {
                collection_name,
                dimension,
                engine,
                if_not_exists,
                properties,
            } => {
                self.create_collection(
                    &collection_name,
                    dimension,
                    engine,
                    if_not_exists,
                    properties,
                )
                .await
            }
        }
    }

    // ========================
    // Table Operations
    // ========================

    /// Create a new table with the given columns and properties in the catalog.
    async fn create_table(
        &self,
        table_name: &str,
        columns: Vec<ColumnDefinition>,
        if_not_exists: bool,
        properties: HashMap<String, String>,
    ) -> Result<DdlResult> {
        let (catalog, table_id) = self.catalog_manager.resolve_table(table_name).await?;

        // Check if table exists
        if catalog.table_exists(&table_id).await? {
            if if_not_exists {
                return Ok(DdlResult::already_exists("Table", table_name));
            } else {
                return Err(anyhow!("Table '{}' already exists", table_name));
            }
        }

        // Build catalog schema
        let schema = self.build_catalog_schema(&table_id.name, columns, properties)?;

        // Create the table
        catalog.create_table(&table_id, schema).await?;

        info!(table = %table_name, "Created table");
        Ok(DdlResult::success(format!(
            "Created table '{}'",
            table_name
        )))
    }

    /// Drop a table from the catalog, optionally purging its data.
    async fn drop_table(
        &self,
        table_name: &str,
        if_exists: bool,
        purge: bool,
    ) -> Result<DdlResult> {
        let (catalog, table_id) = self.catalog_manager.resolve_table(table_name).await?;

        // Check if table exists
        if !catalog.table_exists(&table_id).await? {
            if if_exists {
                return Ok(DdlResult::not_found("Table", table_name));
            } else {
                return Err(anyhow!("Table '{}' does not exist", table_name));
            }
        }

        // Drop the table
        catalog.drop_table(&table_id, purge).await?;

        info!(table = %table_name, purge = purge, "Dropped table");
        Ok(DdlResult::success(format!(
            "Dropped table '{}'",
            table_name
        )))
    }

    /// Apply schema evolution changes (add/drop columns, rename, etc.) to an existing table.
    async fn alter_table(
        &self,
        table_name: &str,
        changes: Vec<AlterTableChange>,
    ) -> Result<DdlResult> {
        let (catalog, table_id) = self.catalog_manager.resolve_table(table_name).await?;

        // Check if table exists
        if !catalog.table_exists(&table_id).await? {
            return Err(anyhow!("Table '{}' does not exist", table_name));
        }

        // Convert changes to catalog schema evolution
        let evolution = self.build_schema_evolution(changes)?;

        // Apply evolution
        catalog.evolve_schema(&table_id, evolution).await?;

        info!(table = %table_name, "Altered table");
        Ok(DdlResult::success(format!(
            "Altered table '{}'",
            table_name
        )))
    }

    // ========================
    // Index Operations
    // ========================

    /// Create a new index on the specified table columns with the given type.
    async fn create_index(
        &self,
        index_name: &str,
        table_name: &str,
        columns: Vec<String>,
        index_type: IndexType,
        if_not_exists: bool,
    ) -> Result<DdlResult> {
        let (catalog, table_id) = self.catalog_manager.resolve_table(table_name).await?;

        // Check if table exists
        if !catalog.table_exists(&table_id).await? {
            return Err(anyhow!("Table '{}' does not exist", table_name));
        }

        // Check if index already exists
        let existing_indexes = catalog.list_indexes(&table_id).await?;
        if existing_indexes.iter().any(|idx| idx.name == index_name) {
            if if_not_exists {
                return Ok(DdlResult::already_exists("Index", index_name));
            } else {
                return Err(anyhow!(
                    "Index '{}' already exists on table '{}'",
                    index_name,
                    table_name
                ));
            }
        }

        // Convert index type
        let catalog_index_type = self.convert_index_type(&index_type);

        // Build index with properties
        let mut index = CatalogIndex::new(index_name.to_string(), columns, catalog_index_type);
        index.properties = self.get_index_properties(&index_type);

        // Create the index
        catalog.create_index(&table_id, index).await?;

        info!(index = %index_name, table = %table_name, "Created index");
        Ok(DdlResult::success(format!(
            "Created index '{}' on table '{}'",
            index_name, table_name
        )))
    }

    /// Remove an existing index from the specified table.
    async fn drop_index(
        &self,
        index_name: &str,
        table_name: &str,
        if_exists: bool,
    ) -> Result<DdlResult> {
        let (catalog, table_id) = self.catalog_manager.resolve_table(table_name).await?;

        // Check if table exists
        if !catalog.table_exists(&table_id).await? {
            return Err(anyhow!("Table '{}' does not exist", table_name));
        }

        // Check if index exists
        let existing_indexes = catalog.list_indexes(&table_id).await?;
        if !existing_indexes.iter().any(|idx| idx.name == index_name) {
            if if_exists {
                return Ok(DdlResult::not_found("Index", index_name));
            } else {
                return Err(anyhow!(
                    "Index '{}' does not exist on table '{}'",
                    index_name,
                    table_name
                ));
            }
        }

        // Drop the index
        catalog.drop_index(&table_id, index_name).await?;

        info!(index = %index_name, table = %table_name, "Dropped index");
        Ok(DdlResult::success(format!(
            "Dropped index '{}' from table '{}'",
            index_name, table_name
        )))
    }

    // ========================
    // Namespace Operations
    // ========================

    /// Create a new namespace (schema grouping) in the default catalog.
    async fn create_namespace(
        &self,
        namespace: &[String],
        if_not_exists: bool,
        properties: HashMap<String, String>,
    ) -> Result<DdlResult> {
        let catalog = self.catalog_manager.default_catalog().await?;
        let ns_name = namespace.join(".");

        // Check if namespace exists
        if catalog.namespace_exists(namespace).await? {
            if if_not_exists {
                return Ok(DdlResult::already_exists("Namespace", &ns_name));
            } else {
                return Err(anyhow!("Namespace '{}' already exists", ns_name));
            }
        }

        // Create the namespace
        catalog.create_namespace(namespace, properties).await?;

        info!(namespace = %ns_name, "Created namespace");
        Ok(DdlResult::success(format!(
            "Created namespace '{}'",
            ns_name
        )))
    }

    /// Drop a namespace, optionally cascading to all contained tables.
    async fn drop_namespace(
        &self,
        namespace: &[String],
        if_exists: bool,
        cascade: bool,
    ) -> Result<DdlResult> {
        let catalog = self.catalog_manager.default_catalog().await?;
        let ns_name = namespace.join(".");

        // Check if namespace exists
        if !catalog.namespace_exists(namespace).await? {
            if if_exists {
                return Ok(DdlResult::not_found("Namespace", &ns_name));
            } else {
                return Err(anyhow!("Namespace '{}' does not exist", ns_name));
            }
        }

        // Drop the namespace
        catalog.drop_namespace(namespace, cascade).await?;

        info!(namespace = %ns_name, cascade = cascade, "Dropped namespace");
        Ok(DdlResult::success(format!(
            "Dropped namespace '{}'",
            ns_name
        )))
    }

    // ========================
    // Collection Operations (ProximaDB-specific)
    // ========================

    /// Create a ProximaDB vector collection with auto-generated HNSW index.
    async fn create_collection(
        &self,
        collection_name: &str,
        dimension: u32,
        engine: Option<String>,
        if_not_exists: bool,
        mut properties: HashMap<String, String>,
    ) -> Result<DdlResult> {
        let (catalog, table_id) = self.catalog_manager.resolve_table(collection_name).await?;

        // Check if collection exists
        if catalog.table_exists(&table_id).await? {
            if if_not_exists {
                return Ok(DdlResult::already_exists("Collection", collection_name));
            } else {
                return Err(anyhow!("Collection '{}' already exists", collection_name));
            }
        }

        // Add ProximaDB-specific properties
        properties.insert("type".to_string(), "vector_collection".to_string());
        properties.insert("dimension".to_string(), dimension.to_string());
        if let Some(eng) = engine {
            properties.insert("engine".to_string(), eng);
        }

        // Build schema with standard vector collection columns
        let columns = vec![
            ColumnDefinition {
                name: "id".to_string(),
                data_type: SqlDataType::Varchar {
                    max_length: Some(255),
                },
                nullable: false,
                default_value: None,
                comment: Some("Vector record ID".to_string()),
                primary_key: true,
            },
            ColumnDefinition {
                name: "vector".to_string(),
                data_type: SqlDataType::Vector { dimension },
                nullable: false,
                default_value: None,
                comment: Some("Vector embedding".to_string()),
                primary_key: false,
            },
            ColumnDefinition {
                name: "metadata".to_string(),
                data_type: SqlDataType::Json,
                nullable: true,
                default_value: None,
                comment: Some("JSON metadata".to_string()),
                primary_key: false,
            },
            ColumnDefinition {
                name: "timestamp".to_string(),
                data_type: SqlDataType::Timestamp,
                nullable: true,
                default_value: None,
                comment: Some("Creation timestamp".to_string()),
                primary_key: false,
            },
        ];

        let schema = self.build_catalog_schema(&table_id.name, columns, properties)?;

        // Create the collection/table
        catalog.create_table(&table_id, schema).await?;

        // Auto-create HNSW index on vector column
        let index = CatalogIndex::new(
            format!("{}_vector_hnsw", table_id.name),
            vec!["vector".to_string()],
            CatalogIndexType::Hnsw,
        );
        catalog.create_index(&table_id, index).await?;

        info!(collection = %collection_name, dimension = dimension, "Created collection");
        Ok(DdlResult::success(format!(
            "Created collection '{}' with dimension {}",
            collection_name, dimension
        )))
    }

    // ========================
    // Helper Methods
    // ========================

    /// Build a `CatalogTableSchema` from column definitions and table properties.
    fn build_catalog_schema(
        &self,
        table_name: &str,
        columns: Vec<ColumnDefinition>,
        properties: HashMap<String, String>,
    ) -> Result<CatalogTableSchema> {
        let mut schema = CatalogTableSchema::new(table_name);
        schema.properties = properties;

        let mut primary_key_cols = Vec::new();
        let mut has_json = false;
        let mut has_vector = false;

        for (idx, col) in columns.into_iter().enumerate() {
            let (data_type, col_properties) = self.sql_to_catalog_type(&col.data_type)?;
            has_json |= matches!(data_type, CatalogDataType::Json);
            has_vector |= matches!(
                data_type,
                CatalogDataType::Vector
                    | CatalogDataType::SparseVector
                    | CatalogDataType::BinaryVector
            );

            let catalog_col = CatalogColumn {
                id: idx as i32 + 1,
                name: col.name.clone(),
                data_type,
                nullable: col.nullable,
                default_value: col.default_value,
                comment: col.comment,
                properties: col_properties,
            };

            if col.primary_key {
                primary_key_cols.push(col.name);
            }

            schema = schema.with_column(catalog_col);
        }

        if !primary_key_cols.is_empty() {
            schema = schema.with_primary_key(primary_key_cols);
        }

        schema
            .properties
            .entry("schema_kind".to_string())
            .or_insert_with(|| match (has_json, has_vector) {
                (true, true) => "mixed_relational_document_vector".to_string(),
                (true, false) => "relational_document".to_string(),
                (false, true) => "relational_vector".to_string(),
                (false, false) => "relational".to_string(),
            });

        Ok(schema)
    }

    /// Convert a SQL data type to its catalog equivalent and extract type properties.
    fn sql_to_catalog_type(
        &self,
        sql_type: &SqlDataType,
    ) -> Result<(CatalogDataType, HashMap<String, String>)> {
        let mut properties = HashMap::new();

        let catalog_type = match sql_type {
            SqlDataType::Boolean => CatalogDataType::Boolean,
            SqlDataType::TinyInt => CatalogDataType::Int8,
            SqlDataType::SmallInt => CatalogDataType::Int16,
            SqlDataType::Int => CatalogDataType::Int32,
            SqlDataType::BigInt => CatalogDataType::Int64,
            SqlDataType::Float => CatalogDataType::Float32,
            SqlDataType::Double => CatalogDataType::Float64,
            SqlDataType::Decimal { precision, scale } => {
                properties.insert("precision".to_string(), precision.to_string());
                properties.insert("scale".to_string(), scale.to_string());
                CatalogDataType::Decimal
            }
            SqlDataType::Varchar { max_length } => {
                if let Some(len) = max_length {
                    properties.insert("max_length".to_string(), len.to_string());
                }
                CatalogDataType::String
            }
            SqlDataType::Text => CatalogDataType::String,
            SqlDataType::Binary | SqlDataType::Blob => CatalogDataType::Binary,
            SqlDataType::Date => CatalogDataType::Date,
            SqlDataType::Time => CatalogDataType::Time,
            SqlDataType::Timestamp => CatalogDataType::Timestamp,
            SqlDataType::TimestampTz => CatalogDataType::TimestampTz,
            SqlDataType::Uuid => CatalogDataType::Uuid,
            SqlDataType::Json => {
                properties.insert("json_encoding".to_string(), "json".to_string());
                CatalogDataType::Json
            }
            SqlDataType::Jsonb => {
                properties.insert("json_encoding".to_string(), "jsonb".to_string());
                CatalogDataType::Json
            }
            SqlDataType::Vector { dimension } => {
                properties.insert("dimension".to_string(), dimension.to_string());
                CatalogDataType::Vector
            }
            SqlDataType::SparseVector { dimension } => {
                properties.insert("dimension".to_string(), dimension.to_string());
                CatalogDataType::SparseVector
            }
            SqlDataType::BinaryVector { dimension } => {
                properties.insert("dimension".to_string(), dimension.to_string());
                CatalogDataType::BinaryVector
            }
        };

        Ok((catalog_type, properties))
    }

    /// Translate ALTER TABLE changes into a `CatalogSchemaEvolution` for the catalog layer.
    fn build_schema_evolution(
        &self,
        changes: Vec<AlterTableChange>,
    ) -> Result<CatalogSchemaEvolution> {
        let mut schema_changes = Vec::new();

        for change in changes {
            let catalog_change = match change {
                AlterTableChange::AddColumn(col) => {
                    let (data_type, _) = self.sql_to_catalog_type(&col.data_type)?;
                    SchemaChange::AddColumn {
                        name: col.name,
                        data_type,
                        nullable: col.nullable,
                        default_value: col.default_value,
                        comment: col.comment,
                        after: None,
                    }
                }
                AlterTableChange::DropColumn(name) => SchemaChange::DropColumn { name },
                AlterTableChange::RenameColumn { old_name, new_name } => {
                    SchemaChange::RenameColumn { old_name, new_name }
                }
                AlterTableChange::ChangeType {
                    column_name,
                    new_type,
                } => {
                    let (data_type, _) = self.sql_to_catalog_type(&new_type)?;
                    SchemaChange::ChangeType {
                        name: column_name,
                        new_type: data_type,
                    }
                }
                AlterTableChange::SetNullable {
                    column_name,
                    nullable,
                } => {
                    if nullable {
                        SchemaChange::MakeNullable { name: column_name }
                    } else {
                        // Now we support SET NOT NULL via MakeNotNullable
                        SchemaChange::MakeNotNullable { name: column_name }
                    }
                }
                AlterTableChange::SetDefault {
                    column_name,
                    default_value,
                } => {
                    if let Some(val) = default_value {
                        SchemaChange::SetDefault {
                            name: column_name,
                            default_value: val,
                        }
                    } else {
                        SchemaChange::DropDefault { name: column_name }
                    }
                }
                AlterTableChange::SetComment {
                    column_name,
                    comment,
                } => SchemaChange::UpdateComment {
                    name: column_name,
                    comment,
                },
                AlterTableChange::MoveColumn {
                    column_name,
                    position,
                } => SchemaChange::MoveColumn {
                    name: column_name,
                    after: match position {
                        ColumnPosition::First => None,
                        ColumnPosition::After(col) => Some(col),
                    },
                },
                AlterTableChange::AddConstraint {
                    constraint_name,
                    constraint,
                } => {
                    use crate::catalog::types::ColumnConstraint;
                    let catalog_constraint = match constraint {
                        TableConstraint::Unique { columns } => ColumnConstraint::Unique { columns },
                        TableConstraint::Check { expression } => {
                            ColumnConstraint::Check { expression }
                        }
                        TableConstraint::ForeignKey {
                            columns,
                            references_table,
                            references_columns,
                        } => ColumnConstraint::ForeignKey {
                            columns,
                            references_table,
                            references_columns,
                            on_delete: None,
                            on_update: None,
                        },
                    };
                    SchemaChange::AddConstraint {
                        constraint_name,
                        constraint: catalog_constraint,
                    }
                }
                AlterTableChange::DropConstraint { constraint_name } => {
                    SchemaChange::DropConstraint { constraint_name }
                }
            };
            schema_changes.push(catalog_change);
        }

        Ok(CatalogSchemaEvolution {
            changes: schema_changes,
        })
    }

    /// Map a SQL-level index type to its catalog enum variant.
    fn convert_index_type(&self, index_type: &IndexType) -> CatalogIndexType {
        match index_type {
            IndexType::BTree => CatalogIndexType::BTree,
            IndexType::Hash => CatalogIndexType::Hash,
            IndexType::FullText => CatalogIndexType::FullText,
            IndexType::Gin => CatalogIndexType::Gin,
            IndexType::Hnsw { .. } => CatalogIndexType::Hnsw,
            IndexType::Ivf { .. } => CatalogIndexType::Ivf,
            IndexType::Pq { .. } => CatalogIndexType::Pq,
        }
    }

    /// Extract type-specific properties (e.g., m, ef_construction, nlist) from an index type.
    fn get_index_properties(&self, index_type: &IndexType) -> HashMap<String, String> {
        let mut props = HashMap::new();

        match index_type {
            IndexType::Hnsw { m, ef_construction } => {
                if let Some(m_val) = m {
                    props.insert("m".to_string(), m_val.to_string());
                }
                if let Some(ef) = ef_construction {
                    props.insert("ef_construction".to_string(), ef.to_string());
                }
            }
            IndexType::Ivf { nlist: Some(n) } => {
                props.insert("nlist".to_string(), n.to_string());
            }
            IndexType::Ivf { .. } => {}
            IndexType::Pq { m, nbits } => {
                if let Some(m_val) = m {
                    props.insert("m".to_string(), m_val.to_string());
                }
                if let Some(nb) = nbits {
                    props.insert("nbits".to_string(), nb.to_string());
                }
            }
            _ => {}
        }

        props
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ddl_result_success() {
        let result = DdlResult::success("Operation completed");
        assert!(result.success);
        assert_eq!(result.affected_count, 1);
    }

    #[test]
    fn test_ddl_result_already_exists() {
        let result = DdlResult::already_exists("Table", "users");
        assert!(result.success);
        assert_eq!(result.affected_count, 0);
        assert!(result.message.contains("already exists"));
    }

    #[test]
    fn test_column_definition() {
        let col = ColumnDefinition {
            name: "id".to_string(),
            data_type: SqlDataType::BigInt,
            nullable: false,
            default_value: None,
            comment: Some("Primary key".to_string()),
            primary_key: true,
        };

        assert_eq!(col.name, "id");
        assert!(!col.nullable);
        assert!(col.primary_key);
    }

    #[test]
    fn test_sql_data_types() {
        let vector_type = SqlDataType::Vector { dimension: 768 };
        match vector_type {
            SqlDataType::Vector { dimension } => assert_eq!(dimension, 768),
            _ => panic!("Expected Vector type"),
        }
    }

    #[test]
    fn test_sql_to_catalog_type_maps_json_and_jsonb_to_json_catalog_type() {
        let service = DdlService::new(Arc::new(CatalogManager::new()));

        let mapping_inputs = [SqlDataType::Json, SqlDataType::Jsonb];

        for sql_type in mapping_inputs {
            let (catalog_type, props) = service
                .sql_to_catalog_type(&sql_type)
                .expect("mapping json/jsonb should succeed");
            assert_eq!(catalog_type, CatalogDataType::Json);
            assert_eq!(props.len(), 1, "json/jsonb should preserve encoding");
        }
    }

    #[tokio::test]
    async fn test_agentic_pgwire_ddl_executes_into_catalog_schema() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");

        let service = DdlService::new(manager.clone());
        service
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("create default namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let create_table = parser
            .parse_ddl(
                "CREATE TABLE IF NOT EXISTS \"agent_store\" (
                    \"record_id\" TEXT NOT NULL,
                    \"tenant_id\" TEXT NOT NULL,
                    \"payload\" JSONB NOT NULL DEFAULT '{}'::jsonb,
                    \"metadata\" JSONB NOT NULL DEFAULT '{}'::jsonb,
                    \"embedding\" VECTOR(384),
                    PRIMARY KEY (\"record_id\")
                ) WITH (
                    storage_engine = 'SST',
                    layout = 'hybrid',
                    xcatalog_namespace = 'agentic.demo',
                    schema_kind = 'agentic_mixed'
                );",
            )
            .expect("parse create table")
            .expect("ddl statement");
        service
            .execute(create_table)
            .await
            .expect("execute create table");

        for index_sql in [
            "CREATE INDEX idx_agent_store_payload_gin ON agent_store USING GIN (payload);",
            "CREATE INDEX idx_agent_store_embedding_hnsw ON agent_store USING HNSW (embedding);",
        ] {
            let statement = parser
                .parse_ddl(index_sql)
                .expect("parse create index")
                .expect("index ddl statement");
            service
                .execute(statement)
                .await
                .expect("execute create index");
        }

        let (catalog, table_id) = manager
            .resolve_table("agent_store")
            .await
            .expect("resolve table");
        let schema = catalog.get_table(&table_id).await.expect("get schema");

        assert_eq!(schema.primary_key, vec!["record_id".to_string()]);
        assert_eq!(
            schema.properties.get("storage_engine").map(String::as_str),
            Some("SST")
        );
        assert_eq!(
            schema.properties.get("layout").map(String::as_str),
            Some("hybrid")
        );
        assert_eq!(
            schema
                .properties
                .get("xcatalog_namespace")
                .map(String::as_str),
            Some("agentic.demo")
        );
        assert_eq!(
            schema.properties.get("schema_kind").map(String::as_str),
            Some("agentic_mixed")
        );

        let payload = schema
            .columns
            .iter()
            .find(|column| column.name == "payload")
            .expect("payload column");
        assert_eq!(payload.data_type, CatalogDataType::Json);
        assert_eq!(
            payload.properties.get("json_encoding").map(String::as_str),
            Some("jsonb")
        );

        let embedding = schema
            .columns
            .iter()
            .find(|column| column.name == "embedding")
            .expect("embedding column");
        assert_eq!(embedding.data_type, CatalogDataType::Vector);
        assert_eq!(
            embedding.properties.get("dimension").map(String::as_str),
            Some("384")
        );

        let indexes = catalog.list_indexes(&table_id).await.expect("list indexes");
        assert!(
            indexes
                .iter()
                .any(|index| index.index_type == CatalogIndexType::Gin)
        );
        assert!(
            indexes
                .iter()
                .any(|index| index.index_type == CatalogIndexType::Hnsw)
        );
    }
}
