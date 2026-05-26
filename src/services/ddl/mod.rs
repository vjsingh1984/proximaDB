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
use proximadb_catalog::{
    CatalogAuthorityMode, CatalogColumn, CatalogDataType, CatalogIndex, CatalogIndexType,
    CatalogPhysicalFormat, CatalogProjection, CatalogProjectionKind, CatalogSchemaEvolution,
    CatalogStorageLayout, CatalogStorageLayoutKind, CatalogStorageSpecialization,
    CatalogTableSchema, CatalogWorkloadProfile, ColumnConstraint, ProjectionFreshness,
    RelationalCapabilities, SchemaChange,
};
use tracing::info;

use crate::catalog::CatalogManager;

/// DDL Statement types
#[derive(Debug, Clone)]
pub enum DdlStatement {
    /// CREATE TABLE [IF NOT EXISTS] table_name (columns...)
    CreateTable {
        /// Name of the table to create.
        table_name: String,
        /// Column definitions for the new table.
        columns: Vec<DdlColumnDefinition>,
        /// Table-level relational constraints.
        constraints: Vec<TableConstraint>,
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

/// Backwards-compat alias for [`DdlColumnDefinition`].
pub type ColumnDefinition = DdlColumnDefinition;

/// Column definition for CREATE TABLE
#[derive(Debug, Clone)]
pub struct DdlColumnDefinition {
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
    AddColumn(DdlColumnDefinition),
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
    /// PROMOTE PROPS KEY key TYPE type — promote a high-frequency props key to a typed column.
    /// The column is named `props__<key>` and stored in the next available ID ≥ 100.
    PromotePropsKey {
        key: String,
        column_type: SqlDataType,
        comment: Option<String>,
    },
    /// SET (key = 'value') — table-level option, e.g. `props_auto_promotion = 'enabled'`.
    SetTableOption { key: String, value: String },
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
                constraints,
                if_not_exists,
                properties,
            } => {
                self.create_table(&table_name, columns, constraints, if_not_exists, properties)
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
        columns: Vec<DdlColumnDefinition>,
        constraints: Vec<TableConstraint>,
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
        let schema = self.build_catalog_schema(&table_id.name, columns, constraints, properties)?;

        // PostgreSQL clients routinely create unqualified tables without
        // explicitly creating a schema first. Keep the native catalog path
        // aligned with the OLTP catalog by materializing the resolved namespace.
        if !catalog.namespace_exists(&table_id.namespace).await? {
            catalog
                .create_namespace(&table_id.namespace, HashMap::new())
                .await?;
        }

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

        // Bump corpus_version: a schema change definitionally
        // invalidates cached plans — predicate types may shift,
        // new columns add predicate surface, dropped columns
        // invalidate predicates against them. Same tenant-extraction
        // convention as create_index/drop_index (namespace[0]).
        if let Some(tenant_id) = table_id.namespace.first() {
            let version = crate::catalog::CorpusVersionRegistry::global()
                .bump(tenant_id, &table_id.name)
                .await;
            tracing::debug!(
                table = %table_name,
                tenant = %tenant_id,
                version,
                "🔄 corpus_version bumped after evolve_schema"
            );
        }

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

        // Bump corpus_version: a new index changes which routes the
        // planner can pick (e.g. an HNSW index unlocks a different
        // route choice than the lexical-only path). Extract tenant
        // from namespace[0] per the catalog convention (tenant_id is
        // the first namespace segment in multi-tenant tables); skip
        // when the table isn't tenant-scoped.
        if let Some(tenant_id) = table_id.namespace.first() {
            let version = crate::catalog::CorpusVersionRegistry::global()
                .bump(tenant_id, &table_id.name)
                .await;
            tracing::debug!(
                index = %index_name,
                table = %table_name,
                tenant = %tenant_id,
                version,
                "🔄 corpus_version bumped after create_index"
            );
        }

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

        // Bump corpus_version — dropping an index changes the
        // available routes (FullPrecisionGraph → forced when the
        // quantized index is gone, etc.). Same tenant-extraction
        // convention as create_index.
        if let Some(tenant_id) = table_id.namespace.first() {
            let version = crate::catalog::CorpusVersionRegistry::global()
                .bump(tenant_id, &table_id.name)
                .await;
            tracing::debug!(
                index = %index_name,
                table = %table_name,
                tenant = %tenant_id,
                version,
                "🔄 corpus_version bumped after drop_index"
            );
        }

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
            DdlColumnDefinition {
                name: "id".to_string(),
                data_type: SqlDataType::Varchar {
                    max_length: Some(255),
                },
                nullable: false,
                default_value: None,
                comment: Some("Vector record ID".to_string()),
                primary_key: true,
            },
            DdlColumnDefinition {
                name: "vector".to_string(),
                data_type: SqlDataType::Vector { dimension },
                nullable: false,
                default_value: None,
                comment: Some("Vector embedding".to_string()),
                primary_key: false,
            },
            DdlColumnDefinition {
                name: "metadata".to_string(),
                data_type: SqlDataType::Json,
                nullable: true,
                default_value: None,
                comment: Some("JSON metadata".to_string()),
                primary_key: false,
            },
            DdlColumnDefinition {
                name: "timestamp".to_string(),
                data_type: SqlDataType::Timestamp,
                nullable: true,
                default_value: None,
                comment: Some("Creation timestamp".to_string()),
                primary_key: false,
            },
        ];

        let schema = self.build_catalog_schema(&table_id.name, columns, Vec::new(), properties)?;

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
        columns: Vec<DdlColumnDefinition>,
        constraints: Vec<TableConstraint>,
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

        let relational_capabilities =
            self.build_relational_capabilities(&schema.primary_key, constraints);
        if relational_capabilities.has_enforced_semantics() {
            schema = schema.with_relational_capabilities(relational_capabilities);
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

        // Honor `WITH (canonical_embedding_precision = '<fp32|fp16|bf16|int8|uint8>')`
        // on CREATE TABLE so pgwire SQL DDL clients can opt into a
        // non-fp32 collection. The property name matches what the
        // proto / REST handler expose. Same string-label dispatch as
        // `apply_proto_enum_workarounds` in
        // `crates/platform/proximadb-api/src/rest/v1/catalog.rs` so
        // mixed-protocol clients see consistent semantics. Unknown
        // values fall back to Fp32 with a warn-level trace rather
        // than failing the CREATE — the legacy fp32 path is the
        // safe default.
        if let Some(raw) = schema.properties.get("canonical_embedding_precision") {
            use proximadb_records::EmbeddingScalarType as P;
            let normalised = raw.trim().to_ascii_lowercase();
            let stripped = normalised
                .strip_prefix("embedding_precision_")
                .unwrap_or(&normalised);
            let target = match stripped {
                "fp32" | "f32" | "float32" | "" => Some(P::Fp32),
                "fp16" | "f16" | "float16" | "half" => Some(P::Fp16),
                "bf16" | "bfloat16" | "brain_float16" => Some(P::Bf16),
                "int8" | "i8" | "int8_scalar" => Some(P::Int8Scalar),
                "uint8" | "u8" | "uint8_scalar" => Some(P::UInt8Scalar),
                _ => {
                    tracing::warn!(
                        raw = %raw,
                        table = %table_name,
                        "CREATE TABLE WITH (canonical_embedding_precision=...): \
                         unrecognized value; defaulting to fp32"
                    );
                    None
                }
            };
            if let Some(t) = target {
                schema.canonical_embedding_precision = t;
            }
        }

        self.apply_storage_profile(&mut schema, has_json, has_vector)?;

        Ok(schema)
    }

    fn apply_storage_profile(
        &self,
        schema: &mut CatalogTableSchema,
        has_json: bool,
        has_vector: bool,
    ) -> Result<()> {
        let profile = schema
            .properties
            .get("workload")
            .or_else(|| schema.properties.get("workload_profile"))
            .or_else(|| schema.properties.get("profile"))
            .and_then(|value| CatalogWorkloadProfile::parse(value))
            .unwrap_or_else(|| infer_workload_profile(has_json, has_vector));

        let specialization = schema
            .properties
            .get("layout")
            .or_else(|| schema.properties.get("storage_layout"))
            .or_else(|| schema.properties.get("specialization"))
            .or_else(|| schema.properties.get("storage_engine"))
            .and_then(|value| CatalogStorageSpecialization::parse(value))
            .unwrap_or_else(|| default_specialization_for_workload(profile, has_json, has_vector));

        schema.workload_profile = profile;
        schema.storage_specialization = specialization;
        schema
            .properties
            .insert("workload_profile".to_string(), profile.as_str().to_string());
        schema.properties.insert(
            "storage_specialization".to_string(),
            specialization.as_str().to_string(),
        );
        normalize_route_knob_properties(schema);

        schema.storage_layouts = vec![primary_layout_for_specialization(specialization, profile)];

        add_specialty_projection_layouts(schema, has_json, has_vector);
        apply_projection_freshness_options(schema);
        add_open_table_layouts(schema)?;

        Ok(())
    }

    fn build_relational_capabilities(
        &self,
        primary_key: &[String],
        constraints: Vec<TableConstraint>,
    ) -> RelationalCapabilities {
        let mut capabilities = RelationalCapabilities {
            primary_key: primary_key.to_vec(),
            ..Default::default()
        };

        for constraint in constraints {
            match constraint {
                TableConstraint::Unique { columns } => {
                    let index_name = format!("unique_{}", columns.join("_"));
                    let index = CatalogIndex::new(index_name, columns, CatalogIndexType::BTree);
                    capabilities.unique_indexes.push(index);
                }
                TableConstraint::Check { expression } => {
                    capabilities
                        .constraints
                        .push(ColumnConstraint::Check { expression });
                }
                TableConstraint::ForeignKey {
                    columns,
                    references_table,
                    references_columns,
                } => {
                    capabilities.constraints.push(ColumnConstraint::ForeignKey {
                        columns,
                        references_table,
                        references_columns,
                        on_delete: None,
                        on_update: None,
                    });
                }
            }
        }

        capabilities
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
                AlterTableChange::PromotePropsKey {
                    key,
                    column_type,
                    comment,
                } => {
                    let (catalog_type, _) = self.sql_to_catalog_type(&column_type)?;
                    SchemaChange::PromotePropsKey {
                        key,
                        column_type: catalog_type,
                        comment,
                    }
                }
                AlterTableChange::SetTableOption { key, value } => {
                    SchemaChange::SetTableOption { key, value }
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

fn infer_workload_profile(has_json: bool, has_vector: bool) -> CatalogWorkloadProfile {
    match (has_json, has_vector) {
        (true, true) => CatalogWorkloadProfile::Mixed,
        (false, true) => CatalogWorkloadProfile::Vector,
        (true, false) => CatalogWorkloadProfile::Document,
        (false, false) => CatalogWorkloadProfile::Htap,
    }
}

fn default_specialization_for_workload(
    profile: CatalogWorkloadProfile,
    has_json: bool,
    has_vector: bool,
) -> CatalogStorageSpecialization {
    match profile {
        CatalogWorkloadProfile::Oltp => CatalogStorageSpecialization::PaxOltp,
        CatalogWorkloadProfile::Olap => CatalogStorageSpecialization::PaxOlap,
        CatalogWorkloadProfile::Htap | CatalogWorkloadProfile::Mixed => {
            CatalogStorageSpecialization::PaxRowFamily
        }
        CatalogWorkloadProfile::Vector if has_vector => CatalogStorageSpecialization::VectorAnn,
        CatalogWorkloadProfile::Document if has_json => CatalogStorageSpecialization::DocumentJson,
        CatalogWorkloadProfile::Graph => CatalogStorageSpecialization::GraphTopology,
        CatalogWorkloadProfile::Observability => {
            CatalogStorageSpecialization::ObservabilityTimeSeries
        }
        CatalogWorkloadProfile::Vector | CatalogWorkloadProfile::Document => {
            CatalogStorageSpecialization::PaxRowFamily
        }
    }
}

fn primary_layout_for_specialization(
    specialization: CatalogStorageSpecialization,
    profile: CatalogWorkloadProfile,
) -> CatalogStorageLayout {
    let mut layout = match specialization {
        CatalogStorageSpecialization::PaxOltp => {
            CatalogStorageLayout::proxima_authoritative_pax("primary")
        }
        CatalogStorageSpecialization::PaxOlap => {
            CatalogStorageLayout::proxima_authoritative_pax("primary")
        }
        CatalogStorageSpecialization::LsmWriteOptimized => {
            CatalogStorageLayout::internal("primary", CatalogStorageLayoutKind::LsmRecord)
        }
        CatalogStorageSpecialization::ColumnarAnalytics => {
            CatalogStorageLayout::internal("primary", CatalogStorageLayoutKind::Columnar)
        }
        CatalogStorageSpecialization::ExternalOpenTable => CatalogStorageLayout {
            name: "primary".to_string(),
            authority: CatalogAuthorityMode::FederatedRead,
            layout_kind: CatalogStorageLayoutKind::ExternalTable,
            physical_format: CatalogPhysicalFormat::External("unknown".to_string()),
            write_mode: proximadb_catalog::CatalogWriteMode::ReadOnly,
            snapshot_semantics: Some("external-snapshot".to_string()),
            policy_enforced_in_proxima: false,
            ..Default::default()
        },
        CatalogStorageSpecialization::GenericRelational
        | CatalogStorageSpecialization::PaxRowFamily
        | CatalogStorageSpecialization::VectorAnn
        | CatalogStorageSpecialization::DocumentJson
        | CatalogStorageSpecialization::GraphTopology
        | CatalogStorageSpecialization::ObservabilityTimeSeries => {
            CatalogStorageLayout::proxima_authoritative_pax("primary")
        }
    };

    match specialization {
        CatalogStorageSpecialization::PaxOltp => {
            layout.properties.insert("pax_mode".into(), "Oltp".into());
        }
        CatalogStorageSpecialization::PaxOlap => {
            layout.properties.insert("pax_mode".into(), "Olap".into());
        }
        CatalogStorageSpecialization::PaxRowFamily
        | CatalogStorageSpecialization::VectorAnn
        | CatalogStorageSpecialization::DocumentJson
        | CatalogStorageSpecialization::GraphTopology
        | CatalogStorageSpecialization::ObservabilityTimeSeries
        | CatalogStorageSpecialization::GenericRelational => {
            layout.properties.insert("pax_mode".into(), "Pax".into());
        }
        CatalogStorageSpecialization::LsmWriteOptimized => {
            layout.physical_format = CatalogPhysicalFormat::Sst;
            layout.snapshot_semantics = Some("mvcc-lsm".to_string());
        }
        CatalogStorageSpecialization::ColumnarAnalytics => {
            layout.physical_format = CatalogPhysicalFormat::ProximaBlock;
            layout.snapshot_semantics = Some("mvcc-columnar".to_string());
        }
        CatalogStorageSpecialization::ExternalOpenTable => {}
    }

    layout
        .properties
        .insert("workload_profile".into(), profile.as_str().into());
    layout.properties.insert(
        "storage_specialization".into(),
        specialization.as_str().into(),
    );
    layout
}

fn add_specialty_projection_layouts(
    schema: &mut CatalogTableSchema,
    has_json: bool,
    has_vector: bool,
) {
    if has_vector
        || matches!(
            schema.storage_specialization,
            CatalogStorageSpecialization::VectorAnn
        )
    {
        schema
            .storage_layouts
            .push(CatalogStorageLayout::specialty_projection(
                "vector_ann",
                CatalogStorageLayoutKind::VectorAnn,
                CatalogPhysicalFormat::ProximaBlock,
            ));
        schema.projections.push(CatalogProjection {
            name: "vector_ann".to_string(),
            kind: CatalogProjectionKind::VectorAnn,
            physical_format: CatalogPhysicalFormat::ProximaBlock,
            rebuild_source: "primary".to_string(),
            freshness: ProjectionFreshness::BoundedLag,
            freshness_state: Default::default(),
            max_lag_ms: Some(1_000),
            source_range: None,
            last_included_position: None,
            rebuild_rto: Some(proximadb_catalog::RebuildRtoSpec::hnsw_benchmarked()),
            rebuildable: true,
            invalidation_policy: Some("mark-stale".to_string()),
            policy_boundary: Some("engine-enforced".to_string()),
            lossy: true,
            benchmark_gate: Some("hybrid-vector-smoke".to_string()),
            support_status: "experimental".to_string(),
            properties: HashMap::new(),
        });
    }

    if has_json
        || matches!(
            schema.storage_specialization,
            CatalogStorageSpecialization::DocumentJson
        )
    {
        schema
            .storage_layouts
            .push(CatalogStorageLayout::specialty_projection(
                "json_path",
                CatalogStorageLayoutKind::Columnar,
                CatalogPhysicalFormat::ProximaBlock,
            ));
        schema.projections.push(CatalogProjection {
            name: "json_path".to_string(),
            kind: CatalogProjectionKind::JsonPath,
            physical_format: CatalogPhysicalFormat::ProximaBlock,
            rebuild_source: "primary".to_string(),
            freshness: ProjectionFreshness::Lazy,
            freshness_state: Default::default(),
            max_lag_ms: None,
            source_range: None,
            last_included_position: None,
            rebuild_rto: None,
            rebuildable: true,
            invalidation_policy: Some("mark-stale".to_string()),
            policy_boundary: Some("engine-enforced".to_string()),
            lossy: false,
            benchmark_gate: Some("json-path-smoke".to_string()),
            support_status: "experimental".to_string(),
            properties: HashMap::new(),
        });
    }

    match schema.storage_specialization {
        CatalogStorageSpecialization::GraphTopology => {
            schema
                .storage_layouts
                .push(CatalogStorageLayout::specialty_projection(
                    "graph_topology",
                    CatalogStorageLayoutKind::GraphTopology,
                    CatalogPhysicalFormat::GraphAr,
                ));
        }
        CatalogStorageSpecialization::ObservabilityTimeSeries => {
            schema
                .storage_layouts
                .push(CatalogStorageLayout::specialty_projection(
                    "time_series",
                    CatalogStorageLayoutKind::TimeSeriesBlock,
                    CatalogPhysicalFormat::ProximaBlock,
                ));
        }
        _ => {}
    }
}

fn normalize_route_knob_properties(schema: &mut CatalogTableSchema) {
    if let Some(route) = first_property(
        schema,
        &["compute_route", "preferred_compute_route", "compute"],
    ) {
        schema
            .properties
            .insert("compute_route".to_string(), canonical_option_value(&route));
    }

    if let Some(partitioning) = first_property(
        schema,
        &["partitioning", "partition_key", "distribution_key"],
    ) {
        schema
            .properties
            .insert("partitioning".to_string(), partitioning);
    }

    if let Some(isolation) = first_property(
        schema,
        &["isolation_profile", "isolation", "transaction_profile"],
    ) {
        schema
            .properties
            .insert("isolation_profile".to_string(), isolation.clone());
        if schema.relational_capabilities.transaction_profile.is_none() {
            schema.relational_capabilities.transaction_profile = Some(isolation);
        }
    }

    if let Some(freshness) = first_property(
        schema,
        &["freshness_sla", "projection_freshness", "freshness"],
    ) {
        schema
            .properties
            .insert("freshness_sla".to_string(), freshness);
    }

    if let Some(policy_boundary) =
        first_property(schema, &["policy_boundary", "rls_boundary", "policy"])
    {
        schema
            .properties
            .insert("policy_boundary".to_string(), policy_boundary);
    }

    if let Some(authority) = first_property(schema, &["authority_mode", "authority", "ownership"])
        .and_then(|value| parse_authority_mode(&value))
    {
        schema.properties.insert(
            "authority_mode".to_string(),
            authority.ownership_mode_name().to_string(),
        );
    }
}

fn apply_projection_freshness_options(schema: &mut CatalogTableSchema) {
    let Some(freshness) = first_property(schema, &["projection_freshness", "freshness"]) else {
        return;
    };
    let Some(freshness) = parse_projection_freshness(&freshness) else {
        return;
    };
    let max_lag_ms = first_property(schema, &["projection_max_lag_ms", "max_lag_ms"])
        .and_then(|value| value.parse::<i64>().ok());

    for projection in &mut schema.projections {
        projection.freshness = freshness;
        if max_lag_ms.is_some() {
            projection.max_lag_ms = max_lag_ms;
        }
    }
}

fn add_open_table_layouts(schema: &mut CatalogTableSchema) -> Result<()> {
    let Some(format) = schema
        .properties
        .get("open_table_format")
        .or_else(|| schema.properties.get("table_format"))
        .or_else(|| schema.properties.get("format"))
        .and_then(|value| parse_physical_format(value))
    else {
        return Ok(());
    };

    let location = schema
        .properties
        .get("location")
        .or_else(|| schema.properties.get("external_location"))
        .or_else(|| schema.properties.get("path"))
        .cloned()
        .unwrap_or_default();

    let ownership = schema
        .properties
        .get("ownership")
        .or_else(|| schema.properties.get("authority"))
        .or_else(|| schema.properties.get("authority_mode"))
        .and_then(|value| parse_authority_mode(value))
        .unwrap_or(CatalogAuthorityMode::ProjectionPublication);

    let mut layout = match ownership {
        CatalogAuthorityMode::ExternalAuthoritative => {
            if location.is_empty() {
                return Err(anyhow!(
                    "External authoritative table '{}' requires LOCATION or external_location",
                    schema.name
                ));
            }
            CatalogStorageLayout::external_authoritative("external", format, location)
        }
        CatalogAuthorityMode::ImportedSnapshot => {
            if location.is_empty() {
                return Err(anyhow!(
                    "Imported snapshot table '{}' requires LOCATION or external_location",
                    schema.name
                ));
            }
            CatalogStorageLayout::imported_snapshot("imported_snapshot", format, location)
        }
        CatalogAuthorityMode::FederatedRead => {
            if location.is_empty() {
                return Err(anyhow!(
                    "Federated table '{}' requires LOCATION or external_location",
                    schema.name
                ));
            }
            CatalogStorageLayout::federated_read("federated", format, location)
        }
        CatalogAuthorityMode::InternalCanonical
        | CatalogAuthorityMode::ProximaAuthoritative
        | CatalogAuthorityMode::ExportedPublication
        | CatalogAuthorityMode::ProjectionPublication
        | CatalogAuthorityMode::RebuildableProjection => {
            let publication_location = if location.is_empty() {
                format!("proximadb://{}/publications/open_table", schema.name)
            } else {
                location
            };
            CatalogStorageLayout::projection_publication("open_table", format, publication_location)
        }
    };

    if let Some(snapshot) = schema
        .properties
        .get("snapshot_semantics")
        .or_else(|| schema.properties.get("snapshot"))
    {
        layout.snapshot_semantics = Some(snapshot.clone());
    }
    if let Some(refresh) = schema
        .properties
        .get("refresh")
        .or_else(|| schema.properties.get("refresh_mode"))
    {
        layout.properties.insert("refresh".into(), refresh.clone());
    }
    if let Some(schema_mode) = schema.properties.get("schema_mode") {
        layout
            .properties
            .insert("schema_mode".into(), schema_mode.clone());
    }

    schema.storage_layouts.push(layout);
    Ok(())
}

fn parse_physical_format(value: &str) -> Option<CatalogPhysicalFormat> {
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "proximablock" | "proxima_block" | "pax" => CatalogPhysicalFormat::ProximaBlock,
        "sst" | "lsm" => CatalogPhysicalFormat::Sst,
        "arrow" | "arrow_ipc" => CatalogPhysicalFormat::Arrow,
        "csv" => CatalogPhysicalFormat::Csv,
        "json" | "jsonl" | "jsonlines" => CatalogPhysicalFormat::Json,
        "xml" => CatalogPhysicalFormat::Xml,
        "avro" => CatalogPhysicalFormat::Avro,
        "parquet" => CatalogPhysicalFormat::Parquet,
        "orc" => CatalogPhysicalFormat::Orc,
        "iceberg" => CatalogPhysicalFormat::Iceberg,
        "delta" | "deltalake" => CatalogPhysicalFormat::Delta,
        "hudi" => CatalogPhysicalFormat::Hudi,
        "graphar" => CatalogPhysicalFormat::GraphAr,
        other if !other.is_empty() => CatalogPhysicalFormat::External(other.to_string()),
        _ => return None,
    })
}

fn parse_authority_mode(value: &str) -> Option<CatalogAuthorityMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "proxima"
        | "proximadb"
        | "internal"
        | "canonical"
        | "proxima_authoritative"
        | "proximaauthoritative" => Some(CatalogAuthorityMode::ProximaAuthoritative),
        "projection"
        | "publication"
        | "projection_publication"
        | "projectionpublication"
        | "export" => Some(CatalogAuthorityMode::ProjectionPublication),
        "import" | "imported" | "imported_snapshot" | "importedsnapshot" => {
            Some(CatalogAuthorityMode::ImportedSnapshot)
        }
        "external" | "external_authoritative" | "externalauthoritative" => {
            Some(CatalogAuthorityMode::ExternalAuthoritative)
        }
        "federated" | "federated_read" | "federatedread" => {
            Some(CatalogAuthorityMode::FederatedRead)
        }
        _ => None,
    }
}

fn parse_projection_freshness(value: &str) -> Option<ProjectionFreshness> {
    match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "sync" | "synchronous" => Some(ProjectionFreshness::Synchronous),
        "bounded" | "bounded_lag" | "boundedlag" => Some(ProjectionFreshness::BoundedLag),
        "lazy" | "async" | "asynchronous" => Some(ProjectionFreshness::Lazy),
        "manual" | "explicit" => Some(ProjectionFreshness::Manual),
        _ => None,
    }
}

fn first_property(schema: &CatalogTableSchema, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| schema.properties.get(*key).cloned())
}

fn canonical_option_value(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace('_', "-")
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
        let col = DdlColumnDefinition {
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

    #[tokio::test]
    async fn test_pgwire_table_options_shape_oltp_olap_htap_and_open_table_layouts() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");

        let service = DdlService::new(manager.clone());
        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let create_table = parser
            .parse_ddl(
                "CREATE TABLE facts (
                    id BIGINT PRIMARY KEY,
                    tenant_id TEXT NOT NULL,
                    payload JSONB,
                    embedding VECTOR(4)
                ) WITH (
                    workload = 'htap',
                    layout = 'pax',
                    compute_route = 'datafusion-local',
                    partitioning = 'tenant_id,bucket',
                    isolation = 'snapshot-isolation',
                    freshness_sla = '5s',
                    projection_freshness = 'bounded_lag',
                    projection_max_lag_ms = 250,
                    policy_boundary = 'engine-enforced',
                    open_table_format = 'iceberg',
                    ownership = 'projection',
                    location = 's3://warehouse/facts',
                    refresh = 'manual'
                );",
            )
            .expect("parse create table")
            .expect("ddl statement");

        service
            .execute(create_table)
            .await
            .expect("execute create table");

        let (catalog, table_id) = manager.resolve_table("facts").await.expect("resolve table");
        let schema = catalog.get_table(&table_id).await.expect("get schema");

        assert_eq!(schema.workload_profile, CatalogWorkloadProfile::Htap);
        assert_eq!(
            schema.storage_specialization,
            CatalogStorageSpecialization::PaxRowFamily
        );
        assert_eq!(
            schema.properties.get("compute_route").map(String::as_str),
            Some("datafusion-local")
        );
        assert_eq!(
            schema.properties.get("partitioning").map(String::as_str),
            Some("tenant_id,bucket")
        );
        assert_eq!(
            schema
                .relational_capabilities
                .transaction_profile
                .as_deref(),
            Some("snapshot-isolation")
        );
        assert_eq!(
            schema.properties.get("freshness_sla").map(String::as_str),
            Some("5s")
        );
        assert_eq!(
            schema.properties.get("policy_boundary").map(String::as_str),
            Some("engine-enforced")
        );
        assert!(
            schema
                .storage_layouts
                .iter()
                .any(|layout| layout.name == "primary"
                    && layout.layout_kind == CatalogStorageLayoutKind::Pax)
        );
        assert!(
            schema
                .storage_layouts
                .iter()
                .any(|layout| layout.name == "vector_ann"
                    && layout.layout_kind == CatalogStorageLayoutKind::VectorAnn)
        );
        assert!(
            schema
                .storage_layouts
                .iter()
                .any(|layout| layout.name == "json_path"
                    && layout.layout_kind == CatalogStorageLayoutKind::Columnar)
        );
        assert!(schema.storage_layouts.iter().any(|layout| {
            layout.name == "open_table"
                && layout.authority == CatalogAuthorityMode::ProjectionPublication
                && layout.physical_format == CatalogPhysicalFormat::Iceberg
                && layout.location.as_deref() == Some("s3://warehouse/facts")
        }));
        assert!(schema.projections.iter().any(|projection| {
            projection.name == "vector_ann"
                && projection.freshness == ProjectionFreshness::BoundedLag
                && projection.max_lag_ms == Some(250)
        }));
    }

    #[tokio::test]
    async fn test_pgwire_options_can_select_sst_for_legacy_vector_specialty() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");

        let service = DdlService::new(manager.clone());
        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let create_table = parser
            .parse_ddl(
                "CREATE TABLE vec_sst (
                    id TEXT PRIMARY KEY,
                    embedding VECTOR(8)
                ) WITH (
                    workload = 'vector',
                    storage_engine = 'sst'
                );",
            )
            .expect("parse create table")
            .expect("ddl statement");

        service
            .execute(create_table)
            .await
            .expect("execute create table");

        let (catalog, table_id) = manager
            .resolve_table("vec_sst")
            .await
            .expect("resolve table");
        let schema = catalog.get_table(&table_id).await.expect("get schema");

        assert_eq!(schema.workload_profile, CatalogWorkloadProfile::Vector);
        assert_eq!(
            schema.storage_specialization,
            CatalogStorageSpecialization::LsmWriteOptimized
        );
        assert!(schema.storage_layouts.iter().any(|layout| {
            layout.name == "primary"
                && layout.layout_kind == CatalogStorageLayoutKind::LsmRecord
                && layout.physical_format == CatalogPhysicalFormat::Sst
        }));
        assert!(
            schema
                .storage_layouts
                .iter()
                .any(|layout| layout.name == "vector_ann")
        );
    }

    // ── canonical_embedding_precision via WITH (...) properties ───────────────
    //
    // pgwire SQL clients write
    //   CREATE TABLE t (...) WITH (canonical_embedding_precision = 'fp16')
    // which lands on DdlStatement::CreateTable.properties. The handler
    // path tested below is what `apply_proto_enum_workarounds` is for
    // the REST/gRPC route, and what
    // `services::collection::manager.rs::catalog_schema_from_collection`
    // is for the proto CollectionConfig path.

    async fn create_with_precision_property(
        value: &str,
    ) -> proximadb_records::EmbeddingScalarType {
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("default", "file:///tmp/proximadb-ddl-precision-test")
            .await
            .expect("create catalog");
        let service = DdlService::new(manager.clone());

        let mut properties = HashMap::new();
        properties.insert(
            "canonical_embedding_precision".to_string(),
            value.to_string(),
        );

        let stmt = DdlStatement::CreateTable {
            table_name: format!(
                "ddl_precision_{}_{}",
                value.replace('-', "_"),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ),
            columns: vec![DdlColumnDefinition {
                name: "id".to_string(),
                data_type: SqlDataType::BigInt,
                nullable: false,
                default_value: None,
                comment: None,
                primary_key: true,
            }],
            constraints: vec![],
            if_not_exists: false,
            properties,
        };

        let table_name = match &stmt {
            DdlStatement::CreateTable { table_name, .. } => table_name.clone(),
            _ => unreachable!(),
        };

        service.execute(stmt).await.expect("execute create table");

        let (catalog, table_id) = manager
            .resolve_table(&table_name)
            .await
            .expect("resolve table");
        catalog
            .get_table(&table_id)
            .await
            .expect("get schema")
            .canonical_embedding_precision
    }

    #[tokio::test]
    async fn create_table_with_canonical_embedding_precision_fp16() {
        assert_eq!(
            create_with_precision_property("fp16").await,
            proximadb_records::EmbeddingScalarType::Fp16
        );
    }

    #[tokio::test]
    async fn create_table_with_canonical_embedding_precision_bf16() {
        assert_eq!(
            create_with_precision_property("bf16").await,
            proximadb_records::EmbeddingScalarType::Bf16
        );
    }

    #[tokio::test]
    async fn create_table_with_canonical_embedding_precision_int8() {
        assert_eq!(
            create_with_precision_property("int8").await,
            proximadb_records::EmbeddingScalarType::Int8Scalar
        );
    }

    #[tokio::test]
    async fn create_table_with_canonical_embedding_precision_uint8() {
        assert_eq!(
            create_with_precision_property("uint8").await,
            proximadb_records::EmbeddingScalarType::UInt8Scalar
        );
    }

    #[tokio::test]
    async fn create_table_with_canonical_embedding_precision_screaming_label() {
        // The proto-generated SCREAMING form ("EMBEDDING_PRECISION_FP16")
        // should normalize the same way as "fp16".
        assert_eq!(
            create_with_precision_property("EMBEDDING_PRECISION_FP16").await,
            proximadb_records::EmbeddingScalarType::Fp16
        );
    }

    #[tokio::test]
    async fn create_table_with_canonical_embedding_precision_aliases() {
        // SDKs and users send a variety of shorthand — accept all.
        assert_eq!(
            create_with_precision_property("f16").await,
            proximadb_records::EmbeddingScalarType::Fp16
        );
        assert_eq!(
            create_with_precision_property("half").await,
            proximadb_records::EmbeddingScalarType::Fp16
        );
        assert_eq!(
            create_with_precision_property("float16").await,
            proximadb_records::EmbeddingScalarType::Fp16
        );
    }

    #[tokio::test]
    async fn create_table_unknown_precision_value_falls_back_to_fp32_silently() {
        // Don't fail CREATE TABLE on a typo — log a warn and use the
        // legacy fp32 path. This matches the proto enum behavior
        // (Unspecified → Fp32 default).
        assert_eq!(
            create_with_precision_property("bogus_precision").await,
            proximadb_records::EmbeddingScalarType::Fp32
        );
    }

    #[tokio::test]
    async fn create_table_without_precision_property_defaults_to_fp32() {
        // No WITH option set → legacy fp32 (no behavior change for
        // existing CREATE TABLE statements).
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("default", "file:///tmp/proximadb-ddl-precision-test-default")
            .await
            .expect("create catalog");
        let service = DdlService::new(manager.clone());

        let stmt = DdlStatement::CreateTable {
            table_name: format!(
                "ddl_precision_default_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ),
            columns: vec![DdlColumnDefinition {
                name: "id".to_string(),
                data_type: SqlDataType::BigInt,
                nullable: false,
                default_value: None,
                comment: None,
                primary_key: true,
            }],
            constraints: vec![],
            if_not_exists: false,
            properties: HashMap::new(),
        };

        let table_name = match &stmt {
            DdlStatement::CreateTable { table_name, .. } => table_name.clone(),
            _ => unreachable!(),
        };

        service.execute(stmt).await.expect("execute create table");

        let (catalog, table_id) = manager
            .resolve_table(&table_name)
            .await
            .expect("resolve");
        let schema = catalog.get_table(&table_id).await.expect("get schema");
        assert_eq!(
            schema.canonical_embedding_precision,
            proximadb_records::EmbeddingScalarType::Fp32
        );
    }
}
