//! # INFORMATION_SCHEMA Views
//!
//! Provides PostgreSQL-compatible introspection views:
//! - `information_schema.tables`
//! - `information_schema.columns`
//! - `information_schema.table_constraints`
//! - `information_schema.key_column_usage`
//! - `information_schema.referential_constraints`
//! - `information_schema.schemata`
//!
//! ## ProximaDB Extensions
//! - `information_schema.vector_collections`
//! - `information_schema.graphs`
//! - `information_schema.document_collections`
//! - `information_schema.observability_streams`
//!
//! ## PostgreSQL Compatibility
//!
//! Standard PostgreSQL introspection queries work:
//!
//! ```sql
//! -- List all tables
//! SELECT * FROM information_schema.tables
//! WHERE table_schema = 'public';
//!
//! -- List columns for a table
//! SELECT column_name, data_type, is_nullable
//! FROM information_schema.columns
//! WHERE table_name = 'users';
//!
//! -- List all constraints
//! SELECT constraint_name, constraint_type
//! FROM information_schema.table_constraints
//! WHERE table_name = 'orders';
//! ```
//!
//! ## ProximaDB Extension Queries
//!
//! ```sql
//! -- List all vector collections with dimensions
//! SELECT collection_name, dimension, distance_metric
//! FROM information_schema.vector_collections;
//!
//! -- List all graphs with node/edge counts
//! SELECT graph_name, node_count, edge_count
//! FROM information_schema.graphs;
//!
//! -- List document collections
//! SELECT collection_name, has_json_schema
//! FROM information_schema.document_collections;
//! ```
//!
//! ## Usage from Rust
//!
//! ```ignore
//! let info_schema = InformationSchema::new(registry);
//!
//! // Query tables view
//! let tables = info_schema.tables().await;
//!
//! // Query specific view
//! let result = info_schema.query(InformationSchemaView::VectorCollections).await;
//! ```

use std::sync::Arc;

use proximadb_catalog::{CatalogColumn, CatalogDataType};
use serde::{Deserialize, Serialize};

use super::{
    CatalogObject, ConstraintType, ForeignKeyReference, InternalSchemaRegistry, ModelProperties,
    ObjectType, ReferentialAction,
};

/// View types available in INFORMATION_SCHEMA
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InformationSchemaView {
    /// Standard: tables view
    Tables,
    /// Standard: columns view
    Columns,
    /// Standard: table_constraints view
    TableConstraints,
    /// Standard: key_column_usage view
    KeyColumnUsage,
    /// Standard: referential_constraints view
    ReferentialConstraints,
    /// Standard: schemata view
    Schemata,
    /// ProximaDB: vector_collections view
    VectorCollections,
    /// ProximaDB: graphs view
    Graphs,
    /// ProximaDB: document_collections view
    DocumentCollections,
    /// ProximaDB: observability_streams view
    ObservabilityStreams,
    /// ProximaDB: storage layout authority metadata
    StorageLayouts,
    /// ProximaDB: rebuildable projection metadata
    Projections,
    /// ProximaDB: optional relational capability metadata
    RelationalCapabilities,
}

impl InformationSchemaView {
    /// Get view name
    pub fn name(&self) -> &'static str {
        match self {
            InformationSchemaView::Tables => "tables",
            InformationSchemaView::Columns => "columns",
            InformationSchemaView::TableConstraints => "table_constraints",
            InformationSchemaView::KeyColumnUsage => "key_column_usage",
            InformationSchemaView::ReferentialConstraints => "referential_constraints",
            InformationSchemaView::Schemata => "schemata",
            InformationSchemaView::VectorCollections => "vector_collections",
            InformationSchemaView::Graphs => "graphs",
            InformationSchemaView::DocumentCollections => "document_collections",
            InformationSchemaView::ObservabilityStreams => "observability_streams",
            InformationSchemaView::StorageLayouts => "storage_layouts",
            InformationSchemaView::Projections => "projections",
            InformationSchemaView::RelationalCapabilities => "relational_capabilities",
        }
    }

    /// Check if this is a ProximaDB extension
    pub fn is_extension(&self) -> bool {
        matches!(
            self,
            InformationSchemaView::VectorCollections
                | InformationSchemaView::Graphs
                | InformationSchemaView::DocumentCollections
                | InformationSchemaView::ObservabilityStreams
                | InformationSchemaView::StorageLayouts
                | InformationSchemaView::Projections
                | InformationSchemaView::RelationalCapabilities
        )
    }
}

/// Row in information_schema.tables
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableRow {
    /// Name of the catalog that contains this table
    pub table_catalog: String,
    /// Name of the schema that contains this table
    pub table_schema: String,
    /// Name of the table
    pub table_name: String,
    /// Table type (e.g., `BASE TABLE`, `VIEW`, `VECTOR COLLECTION`)
    pub table_type: String,
    /// Name of the self-referencing column for typed tables (SQL standard)
    pub self_referencing_column_name: Option<String>,
    /// How the self-referencing column value is generated (SQL standard)
    pub reference_generation: Option<String>,
    /// Catalog of the user-defined type for typed tables
    pub user_defined_type_catalog: Option<String>,
    /// Schema of the user-defined type for typed tables
    pub user_defined_type_schema: Option<String>,
    /// Name of the user-defined type for typed tables
    pub user_defined_type_name: Option<String>,
    /// Whether rows can be inserted into this table (`YES`/`NO`)
    pub is_insertable_into: String,
    /// Whether this is a typed table (`YES`/`NO`)
    pub is_typed: String,
    /// Commit action for temporary tables (SQL standard)
    pub commit_action: Option<String>,
    // ProximaDB extensions
    /// Enforcement mode for multi-model objects (e.g., `STRICT`, `LOOSE`)
    pub enforcement_mode: Option<String>,
    /// Data model type label (e.g., `RDBMS`, `VECTOR`, `GRAPH`)
    pub model_type: Option<String>,
}

impl TableRow {
    /// Create from a catalog object
    pub fn from_object(obj: &CatalogObject) -> Self {
        let table_type = match obj.object_type {
            ObjectType::RdbmsTable => "BASE TABLE",
            ObjectType::View => "VIEW",
            ObjectType::MaterializedView => "MATERIALIZED VIEW",
            ObjectType::VectorCollection => "VECTOR COLLECTION",
            ObjectType::DocumentCollection => "DOCUMENT COLLECTION",
            ObjectType::Graph => "GRAPH",
            ObjectType::LogStream | ObjectType::MetricStream | ObjectType::TraceStream => "STREAM",
            _ => "OTHER",
        };

        Self {
            table_catalog: obj.catalog.clone(),
            table_schema: obj.namespace.join("."),
            table_name: obj.name.clone(),
            table_type: table_type.to_string(),
            self_referencing_column_name: None,
            reference_generation: None,
            user_defined_type_catalog: None,
            user_defined_type_schema: None,
            user_defined_type_name: None,
            is_insertable_into: "YES".to_string(),
            is_typed: "NO".to_string(),
            commit_action: None,
            enforcement_mode: Some(obj.enforcement_mode.as_str().to_string()),
            model_type: Some(obj.object_type.display_name().to_string()),
        }
    }
}

/// Row in information_schema.columns
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnRow {
    /// Name of the catalog that contains the table
    pub table_catalog: String,
    /// Name of the schema that contains the table
    pub table_schema: String,
    /// Name of the table that owns this column
    pub table_name: String,
    /// Name of the column
    pub column_name: String,
    /// 1-based position of the column within the table
    pub ordinal_position: i32,
    /// Default value expression for the column
    pub column_default: Option<String>,
    /// Whether the column accepts NULL values (`YES`/`NO`)
    pub is_nullable: String,
    /// SQL data type name
    pub data_type: String,
    /// Maximum character length for character types
    pub character_maximum_length: Option<i32>,
    /// Maximum octet (byte) length for character types
    pub character_octet_length: Option<i32>,
    /// Numeric precision for numeric types
    pub numeric_precision: Option<i32>,
    /// Radix of the numeric precision (2 for binary, 10 for decimal)
    pub numeric_precision_radix: Option<i32>,
    /// Scale (digits after decimal point) for numeric types
    pub numeric_scale: Option<i32>,
    /// Fractional seconds precision for datetime types
    pub datetime_precision: Option<i32>,
    /// Interval type qualifier (e.g., `YEAR`, `MONTH`)
    pub interval_type: Option<String>,
    /// Precision of the interval leading field
    pub interval_precision: Option<i32>,
    /// Catalog of the character set (SQL standard)
    pub character_set_catalog: Option<String>,
    /// Schema of the character set (SQL standard)
    pub character_set_schema: Option<String>,
    /// Name of the character set used for this column
    pub character_set_name: Option<String>,
    /// Catalog of the collation (SQL standard)
    pub collation_catalog: Option<String>,
    /// Schema of the collation (SQL standard)
    pub collation_schema: Option<String>,
    /// Name of the collation used for this column
    pub collation_name: Option<String>,
    /// Catalog of the domain, if the column is based on a domain
    pub domain_catalog: Option<String>,
    /// Schema of the domain, if the column is based on a domain
    pub domain_schema: Option<String>,
    /// Name of the domain, if the column is based on a domain
    pub domain_name: Option<String>,
    /// Catalog of the user-defined type (SQL standard)
    pub udt_catalog: Option<String>,
    /// Schema of the user-defined type (SQL standard)
    pub udt_schema: Option<String>,
    /// Name of the user-defined type (SQL standard)
    pub udt_name: Option<String>,
    /// Catalog of the scope table for REF columns
    pub scope_catalog: Option<String>,
    /// Schema of the scope table for REF columns
    pub scope_schema: Option<String>,
    /// Name of the scope table for REF columns
    pub scope_name: Option<String>,
    /// Maximum cardinality for array columns
    pub maximum_cardinality: Option<i32>,
    /// Data type descriptor identifier (SQL standard)
    pub dtd_identifier: Option<String>,
    /// Whether the column is self-referencing (`YES`/`NO`)
    pub is_self_referencing: String,
    /// Whether the column is an identity column (`YES`/`NO`)
    pub is_identity: String,
    /// How the identity value is generated (`ALWAYS` or `BY DEFAULT`)
    pub identity_generation: Option<String>,
    /// Starting value of the identity sequence
    pub identity_start: Option<String>,
    /// Increment value of the identity sequence
    pub identity_increment: Option<String>,
    /// Maximum value of the identity sequence
    pub identity_maximum: Option<String>,
    /// Minimum value of the identity sequence
    pub identity_minimum: Option<String>,
    /// Whether the identity sequence cycles (`YES`/`NO`)
    pub identity_cycle: String,
    /// Whether the column is generated (`ALWAYS`, `NEVER`)
    pub is_generated: String,
    /// Expression used to compute a generated column's value
    pub generation_expression: Option<String>,
    /// Whether the column can be updated (`YES`/`NO`)
    pub is_updatable: String,
}

impl ColumnRow {
    /// Create from a catalog object and column
    pub fn from_column(obj: &CatalogObject, col: &CatalogColumn) -> Self {
        let data_type = match col.data_type {
            CatalogDataType::Boolean => "boolean",
            CatalogDataType::Int8 => "smallint",
            CatalogDataType::Int16 => "smallint",
            CatalogDataType::Int32 => "integer",
            CatalogDataType::Int64 => "bigint",
            CatalogDataType::Float32 => "real",
            CatalogDataType::Float64 => "double precision",
            CatalogDataType::String => "text",
            CatalogDataType::Binary => "bytea",
            CatalogDataType::Date => "date",
            CatalogDataType::Time => "time without time zone",
            CatalogDataType::Timestamp => "timestamp without time zone",
            CatalogDataType::TimestampTz => "timestamp with time zone",
            CatalogDataType::Decimal => "numeric",
            CatalogDataType::Uuid => "uuid",
            CatalogDataType::Json => "jsonb",
            CatalogDataType::Vector => "vector",
            CatalogDataType::SparseVector => "sparsevec",
            CatalogDataType::BinaryVector => "bit",
        };

        Self {
            table_catalog: obj.catalog.clone(),
            table_schema: obj.namespace.join("."),
            table_name: obj.name.clone(),
            column_name: col.name.clone(),
            ordinal_position: col.id,
            column_default: col.default_value.clone(),
            is_nullable: if col.nullable { "YES" } else { "NO" }.to_string(),
            data_type: data_type.to_string(),
            character_maximum_length: None,
            character_octet_length: None,
            numeric_precision: None,
            numeric_precision_radix: None,
            numeric_scale: None,
            datetime_precision: None,
            interval_type: None,
            interval_precision: None,
            character_set_catalog: None,
            character_set_schema: None,
            character_set_name: None,
            collation_catalog: None,
            collation_schema: None,
            collation_name: None,
            domain_catalog: None,
            domain_schema: None,
            domain_name: None,
            udt_catalog: None,
            udt_schema: None,
            udt_name: None,
            scope_catalog: None,
            scope_schema: None,
            scope_name: None,
            maximum_cardinality: None,
            dtd_identifier: None,
            is_self_referencing: "NO".to_string(),
            is_identity: "NO".to_string(),
            identity_generation: None,
            identity_start: None,
            identity_increment: None,
            identity_maximum: None,
            identity_minimum: None,
            identity_cycle: "NO".to_string(),
            is_generated: "NEVER".to_string(),
            generation_expression: None,
            is_updatable: "YES".to_string(),
        }
    }
}

/// Row in information_schema.table_constraints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableConstraintRow {
    /// Catalog that owns the constraint
    pub constraint_catalog: String,
    /// Schema that owns the constraint
    pub constraint_schema: String,
    /// Name of the constraint
    pub constraint_name: String,
    /// Catalog of the table the constraint belongs to
    pub table_catalog: String,
    /// Schema of the table the constraint belongs to
    pub table_schema: String,
    /// Name of the table the constraint belongs to
    pub table_name: String,
    /// Constraint type (e.g., `PRIMARY KEY`, `UNIQUE`, `FOREIGN KEY`, `CHECK`)
    pub constraint_type: String,
    /// Whether the constraint is deferrable (`YES`/`NO`)
    pub is_deferrable: String,
    /// Whether the constraint is initially deferred (`YES`/`NO`)
    pub initially_deferred: String,
    /// Whether the constraint is enforced (`YES`/`NO`)
    pub enforced: String,
}

impl TableConstraintRow {
    /// Create from a catalog object and constraint
    pub fn from_constraint(obj: &CatalogObject, constraint: &super::TableConstraint) -> Self {
        let constraint_type = match &constraint.constraint_type {
            ConstraintType::PrimaryKey { .. } => "PRIMARY KEY",
            ConstraintType::ForeignKey { .. } => "FOREIGN KEY",
            ConstraintType::Unique { .. } => "UNIQUE",
            ConstraintType::Check { .. } => "CHECK",
            ConstraintType::NotNull { .. } => "NOT NULL",
            ConstraintType::Exclusion { .. } => "EXCLUSION",
        };

        Self {
            constraint_catalog: obj.catalog.clone(),
            constraint_schema: obj.namespace.join("."),
            constraint_name: constraint.name.clone(),
            table_catalog: obj.catalog.clone(),
            table_schema: obj.namespace.join("."),
            table_name: obj.name.clone(),
            constraint_type: constraint_type.to_string(),
            is_deferrable: if constraint.is_deferrable {
                "YES"
            } else {
                "NO"
            }
            .to_string(),
            initially_deferred: if constraint.is_deferred { "YES" } else { "NO" }.to_string(),
            enforced: "YES".to_string(),
        }
    }
}

/// Row in information_schema.key_column_usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyColumnUsageRow {
    /// Catalog that owns the constraint
    pub constraint_catalog: String,
    /// Schema that owns the constraint
    pub constraint_schema: String,
    /// Name of the constraint
    pub constraint_name: String,
    /// Catalog of the table the constraint belongs to
    pub table_catalog: String,
    /// Schema of the table the constraint belongs to
    pub table_schema: String,
    /// Name of the table the constraint belongs to
    pub table_name: String,
    /// Name of the column participating in the key
    pub column_name: String,
    /// 1-based ordinal position of the column in the key
    pub ordinal_position: i32,
    /// Ordinal position of this column in the referenced unique constraint (for FKs)
    pub position_in_unique_constraint: Option<i32>,
}

/// Row in information_schema.referential_constraints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferentialConstraintRow {
    /// Catalog that owns the referential constraint
    pub constraint_catalog: String,
    /// Schema that owns the referential constraint
    pub constraint_schema: String,
    /// Name of the referential constraint
    pub constraint_name: String,
    /// Catalog of the unique constraint being referenced
    pub unique_constraint_catalog: Option<String>,
    /// Schema of the unique constraint being referenced
    pub unique_constraint_schema: Option<String>,
    /// Name of the unique constraint being referenced
    pub unique_constraint_name: Option<String>,
    /// Match option (`FULL`, `PARTIAL`, `NONE`)
    pub match_option: String,
    /// Referential update rule (e.g., `CASCADE`, `RESTRICT`)
    pub update_rule: String,
    /// Referential delete rule (e.g., `CASCADE`, `SET NULL`)
    pub delete_rule: String,
    // ProximaDB extensions
    /// Cross-model reference type (e.g., `TABLE`, `GRAPH_NODE`, `DOCUMENT`, `VECTOR`)
    pub reference_type: Option<String>,
    /// Name of the referenced cross-model object
    pub referenced_object: Option<String>,
}

impl ReferentialConstraintRow {
    /// Create from a catalog object and FK constraint
    pub fn from_fk(
        obj: &CatalogObject,
        constraint_name: &str,
        reference: &ForeignKeyReference,
        on_update: &ReferentialAction,
        on_delete: &ReferentialAction,
    ) -> Self {
        let (ref_type, ref_obj) = match reference {
            ForeignKeyReference::Table { table, .. } => ("TABLE", table.clone()),
            ForeignKeyReference::GraphNode { graph_id, .. } => ("GRAPH_NODE", graph_id.clone()),
            ForeignKeyReference::Document { collection, .. } => ("DOCUMENT", collection.clone()),
            ForeignKeyReference::Vector { collection, .. } => ("VECTOR", collection.clone()),
        };

        Self {
            constraint_catalog: obj.catalog.clone(),
            constraint_schema: obj.namespace.join("."),
            constraint_name: constraint_name.to_string(),
            unique_constraint_catalog: Some(obj.catalog.clone()),
            unique_constraint_schema: Some(obj.namespace.join(".")),
            unique_constraint_name: None,
            match_option: "NONE".to_string(),
            update_rule: on_update.as_sql().to_string(),
            delete_rule: on_delete.as_sql().to_string(),
            reference_type: Some(ref_type.to_string()),
            referenced_object: Some(ref_obj),
        }
    }
}

/// Row in information_schema.vector_collections (ProximaDB extension)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorCollectionRow {
    /// Catalog that contains the vector collection
    pub collection_catalog: String,
    /// Schema that contains the vector collection
    pub collection_schema: String,
    /// Name of the vector collection
    pub collection_name: String,
    /// Dimensionality of the vectors stored in this collection
    pub dimension: u32,
    /// Distance metric used for similarity search (e.g., `l2`, `cosine`, `dot_product`)
    pub distance_metric: String,
    /// Quantization scheme applied to stored vectors (e.g., `SQ8`, `PQ`)
    pub quantization: Option<String>,
    /// ANN index type (e.g., `HNSW`, `IVF`, `FLAT`)
    pub index_type: Option<String>,
    /// HNSW M parameter (max connections per layer)
    pub hnsw_m: Option<u32>,
    /// HNSW ef_construction parameter (build-time search width)
    pub hnsw_ef_construction: Option<u32>,
    /// Approximate number of vectors currently indexed
    pub vector_count: Option<u64>,
    /// Enforcement mode for multi-model constraints
    pub enforcement_mode: String,
}

impl VectorCollectionRow {
    /// Create from a catalog object
    pub fn from_object(obj: &CatalogObject) -> Option<Self> {
        if obj.object_type != ObjectType::VectorCollection {
            return None;
        }

        let props = match &obj.schema.model_properties {
            ModelProperties::Vector(v) => v,
            _ => return None,
        };

        Some(Self {
            collection_catalog: obj.catalog.clone(),
            collection_schema: obj.namespace.join("."),
            collection_name: obj.name.clone(),
            dimension: props.dimension,
            distance_metric: props.distance_metric.clone(),
            quantization: props.quantization.clone(),
            index_type: props.index_type.clone(),
            hnsw_m: props.hnsw_m,
            hnsw_ef_construction: props.hnsw_ef_construction,
            vector_count: None,
            enforcement_mode: obj.enforcement_mode.as_str().to_string(),
        })
    }
}

/// Row in information_schema.graphs (ProximaDB extension)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphRow {
    /// Catalog that contains the graph
    pub graph_catalog: String,
    /// Schema that contains the graph
    pub graph_schema: String,
    /// Name of the graph
    pub graph_name: String,
    /// Graph storage/processing type (e.g., `ORION`, `PULSAR`, `QUASAR`)
    pub graph_type: String,
    /// Whether the graph permits self-loop edges
    pub allow_self_loops: bool,
    /// Whether the graph permits multiple edges between the same node pair
    pub allow_multi_edges: bool,
    /// All registered node label names in the graph
    pub node_labels: Vec<String>,
    /// All registered edge type names in the graph
    pub edge_types: Vec<String>,
    /// Approximate number of nodes currently in the graph
    pub node_count: Option<u64>,
    /// Approximate number of edges currently in the graph
    pub edge_count: Option<u64>,
    /// Enforcement mode for multi-model constraints
    pub enforcement_mode: String,
}

impl GraphRow {
    /// Create from a catalog object
    pub fn from_object(obj: &CatalogObject) -> Option<Self> {
        if obj.object_type != ObjectType::Graph {
            return None;
        }

        let props = match &obj.schema.model_properties {
            ModelProperties::Graph(g) => g,
            _ => return None,
        };

        Some(Self {
            graph_catalog: obj.catalog.clone(),
            graph_schema: obj.namespace.join("."),
            graph_name: obj.name.clone(),
            graph_type: props.graph_type.clone(),
            allow_self_loops: props.allow_self_loops,
            allow_multi_edges: props.allow_multi_edges,
            node_labels: props.node_labels.clone(),
            edge_types: props.edge_types.clone(),
            node_count: None,
            edge_count: None,
            enforcement_mode: obj.enforcement_mode.as_str().to_string(),
        })
    }
}

/// Row in information_schema.document_collections (ProximaDB extension)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentCollectionRow {
    /// Catalog that contains the document collection
    pub collection_catalog: String,
    /// Schema that contains the document collection
    pub collection_schema: String,
    /// Name of the document collection
    pub collection_name: String,
    /// Whether the collection enforces a JSON Schema for its documents
    pub has_json_schema: bool,
    /// Strategy used to auto-generate document IDs (e.g., `UUID`, `SEQUENCE`)
    pub id_generation: String,
    /// Whether full-text search indexing is enabled for the collection
    pub enable_full_text: bool,
    /// JSONPath expressions of fields that have secondary indexes
    pub indexed_paths: Vec<String>,
    /// Approximate number of documents currently in the collection
    pub document_count: Option<u64>,
    /// Enforcement mode for multi-model constraints
    pub enforcement_mode: String,
}

impl DocumentCollectionRow {
    /// Create from a catalog object
    pub fn from_object(obj: &CatalogObject) -> Option<Self> {
        if obj.object_type != ObjectType::DocumentCollection {
            return None;
        }

        let props = match &obj.schema.model_properties {
            ModelProperties::Document(d) => d,
            _ => return None,
        };

        Some(Self {
            collection_catalog: obj.catalog.clone(),
            collection_schema: obj.namespace.join("."),
            collection_name: obj.name.clone(),
            has_json_schema: props.json_schema.is_some(),
            id_generation: props.id_generation.clone(),
            enable_full_text: props.enable_full_text,
            indexed_paths: props.indexed_paths.clone(),
            document_count: None,
            enforcement_mode: obj.enforcement_mode.as_str().to_string(),
        })
    }
}

/// Row in information_schema.observability_streams (ProximaDB extension)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilityStreamRow {
    /// Catalog that contains the observability stream
    pub stream_catalog: String,
    /// Schema that contains the observability stream
    pub stream_schema: String,
    /// Name of the observability stream
    pub stream_name: String,
    /// Stream type (e.g., `LOG`, `METRIC`, `TRACE`)
    pub stream_type: String,
    /// How long events are retained in the stream, in seconds
    pub retention_seconds: u64,
    /// Pre-aggregation rollup intervals configured for this stream (e.g., `1m`, `5m`, `1h`)
    pub rollup_intervals: Vec<String>,
    /// Approximate number of events currently stored in the stream
    pub event_count: Option<u64>,
    /// Unix timestamp (millis) of the oldest event in the stream
    pub oldest_timestamp: Option<i64>,
    /// Unix timestamp (millis) of the newest event in the stream
    pub newest_timestamp: Option<i64>,
}

impl ObservabilityStreamRow {
    /// Create from a catalog object
    pub fn from_object(obj: &CatalogObject) -> Option<Self> {
        if !obj.is_observability() {
            return None;
        }

        let props = match &obj.schema.model_properties {
            ModelProperties::Observability(o) => o,
            _ => return None,
        };

        Some(Self {
            stream_catalog: obj.catalog.clone(),
            stream_schema: obj.namespace.join("."),
            stream_name: obj.name.clone(),
            stream_type: props.stream_type.clone(),
            retention_seconds: props.retention_seconds,
            rollup_intervals: props.rollup_intervals.clone(),
            event_count: None,
            oldest_timestamp: None,
            newest_timestamp: None,
        })
    }
}

/// Row in information_schema.storage_layouts (ProximaDB extension)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageLayoutRow {
    /// Catalog that contains the object.
    pub table_catalog: String,
    /// Schema that contains the object.
    pub table_schema: String,
    /// Object/table/collection name.
    pub table_name: String,
    /// Layout name.
    pub layout_name: String,
    /// Authority mode for this layout.
    pub authority_mode: String,
    /// Physical layout family.
    pub layout_kind: String,
    /// Physical format.
    pub physical_format: String,
    /// Write/refresh mode.
    pub write_mode: String,
    /// Optional layout location.
    pub location: Option<String>,
    /// Snapshot/isolation semantics.
    pub snapshot_semantics: Option<String>,
    /// Whether ProximaDB enforces policy/RLS before rows leave this layout.
    pub policy_enforced_in_proxima: bool,
    /// Count of lossy type mappings declared for this format.
    pub lossy_type_mapping_count: usize,
}

impl StorageLayoutRow {
    /// Create rows from a catalog object.
    pub fn from_object(obj: &CatalogObject) -> Vec<Self> {
        obj.schema
            .storage_layouts
            .iter()
            .map(|layout| Self {
                table_catalog: obj.catalog.clone(),
                table_schema: obj.namespace.join("."),
                table_name: obj.name.clone(),
                layout_name: layout.name.clone(),
                authority_mode: format!("{:?}", layout.authority),
                layout_kind: format!("{:?}", layout.layout_kind),
                physical_format: format!("{:?}", layout.physical_format),
                write_mode: format!("{:?}", layout.write_mode),
                location: layout.location.clone(),
                snapshot_semantics: layout.snapshot_semantics.clone(),
                policy_enforced_in_proxima: layout.policy_enforced_in_proxima,
                lossy_type_mapping_count: layout.lossy_type_mappings.len(),
            })
            .collect()
    }
}

/// Row in information_schema.projections (ProximaDB extension)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectionRow {
    /// Catalog that contains the object.
    pub table_catalog: String,
    /// Schema that contains the object.
    pub table_schema: String,
    /// Object/table/collection name.
    pub table_name: String,
    /// Projection name.
    pub projection_name: String,
    /// Projection family.
    pub projection_kind: String,
    /// Physical format backing the projection.
    pub physical_format: String,
    /// Canonical rebuild source.
    pub rebuild_source: String,
    /// Freshness semantics.
    pub freshness: String,
    /// Optional bounded lag.
    pub max_lag_ms: Option<i64>,
    /// Whether the projection is rebuildable without data loss.
    pub rebuildable: bool,
    /// Whether the projection is lossy.
    pub lossy: bool,
    /// Support status label.
    pub support_status: String,
}

impl ProjectionRow {
    /// Create rows from a catalog object.
    pub fn from_object(obj: &CatalogObject) -> Vec<Self> {
        obj.schema
            .projections
            .iter()
            .map(|projection| Self {
                table_catalog: obj.catalog.clone(),
                table_schema: obj.namespace.join("."),
                table_name: obj.name.clone(),
                projection_name: projection.name.clone(),
                projection_kind: format!("{:?}", projection.kind),
                physical_format: format!("{:?}", projection.physical_format),
                rebuild_source: projection.rebuild_source.clone(),
                freshness: format!("{:?}", projection.freshness),
                max_lag_ms: projection.max_lag_ms,
                rebuildable: projection.rebuildable,
                lossy: projection.lossy,
                support_status: projection.support_status.clone(),
            })
            .collect()
    }
}

/// Row in information_schema.relational_capabilities (ProximaDB extension)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationalCapabilityRow {
    /// Catalog that contains the object.
    pub table_catalog: String,
    /// Schema that contains the object.
    pub table_schema: String,
    /// Object/table/collection name.
    pub table_name: String,
    /// Whether any relational semantics are enabled.
    pub has_enforced_semantics: bool,
    /// Primary-key columns.
    pub primary_key: Vec<String>,
    /// Number of unique indexes.
    pub unique_index_count: usize,
    /// Number of secondary indexes.
    pub secondary_index_count: usize,
    /// Number of constraints.
    pub constraint_count: usize,
    /// Number of materialized views.
    pub materialized_view_count: usize,
    /// Transaction profile name.
    pub transaction_profile: Option<String>,
    /// Schema evolution policy name.
    pub schema_evolution_policy: Option<String>,
}

impl RelationalCapabilityRow {
    /// Create from a catalog object.
    pub fn from_object(obj: &CatalogObject) -> Self {
        let capabilities = &obj.schema.relational_capabilities;
        Self {
            table_catalog: obj.catalog.clone(),
            table_schema: obj.namespace.join("."),
            table_name: obj.name.clone(),
            has_enforced_semantics: capabilities.has_enforced_semantics(),
            primary_key: capabilities.primary_key.clone(),
            unique_index_count: capabilities.unique_indexes.len(),
            secondary_index_count: capabilities.secondary_indexes.len(),
            constraint_count: capabilities.constraints.len(),
            materialized_view_count: capabilities.materialized_views.len(),
            transaction_profile: capabilities.transaction_profile.clone(),
            schema_evolution_policy: capabilities.schema_evolution_policy.clone(),
        }
    }
}

/// INFORMATION_SCHEMA query interface
pub struct InformationSchema {
    registry: Arc<InternalSchemaRegistry>,
}

impl InformationSchema {
    /// Create a new INFORMATION_SCHEMA interface
    pub fn new(registry: Arc<InternalSchemaRegistry>) -> Self {
        Self { registry }
    }

    /// Get all tables
    pub async fn tables(&self) -> Vec<TableRow> {
        let objects = self.registry.list_all().await;
        objects.iter().map(|o| TableRow::from_object(o)).collect()
    }

    /// Get all columns
    pub async fn columns(&self) -> Vec<ColumnRow> {
        let objects = self.registry.list_all().await;
        let mut rows = Vec::new();

        for obj in objects {
            for col in &obj.schema.columns {
                rows.push(ColumnRow::from_column(&obj, col));
            }
        }

        rows
    }

    /// Get all table constraints
    pub async fn table_constraints(&self) -> Vec<TableConstraintRow> {
        let objects = self.registry.list_all().await;
        let mut rows = Vec::new();

        for obj in objects {
            for constraint in &obj.schema.constraints {
                rows.push(TableConstraintRow::from_constraint(&obj, constraint));
            }
        }

        rows
    }

    /// Get all key column usages
    pub async fn key_column_usage(&self) -> Vec<KeyColumnUsageRow> {
        let objects = self.registry.list_all().await;
        let mut rows = Vec::new();

        for obj in objects {
            for constraint in &obj.schema.constraints {
                let columns = match &constraint.constraint_type {
                    ConstraintType::PrimaryKey { columns } => columns,
                    ConstraintType::Unique { columns } => columns,
                    ConstraintType::ForeignKey { columns, .. } => columns,
                    _ => continue,
                };

                for (pos, col) in columns.iter().enumerate() {
                    rows.push(KeyColumnUsageRow {
                        constraint_catalog: obj.catalog.clone(),
                        constraint_schema: obj.namespace.join("."),
                        constraint_name: constraint.name.clone(),
                        table_catalog: obj.catalog.clone(),
                        table_schema: obj.namespace.join("."),
                        table_name: obj.name.clone(),
                        column_name: col.clone(),
                        ordinal_position: (pos + 1) as i32,
                        position_in_unique_constraint: None,
                    });
                }
            }
        }

        rows
    }

    /// Get all referential constraints (foreign keys)
    pub async fn referential_constraints(&self) -> Vec<ReferentialConstraintRow> {
        let objects = self.registry.list_all().await;
        let mut rows = Vec::new();

        for obj in objects {
            for constraint in &obj.schema.constraints {
                if let ConstraintType::ForeignKey {
                    reference,
                    on_update,
                    on_delete,
                    ..
                } = &constraint.constraint_type
                {
                    rows.push(ReferentialConstraintRow::from_fk(
                        &obj,
                        &constraint.name,
                        reference,
                        on_update,
                        on_delete,
                    ));
                }
            }
        }

        rows
    }

    /// Get all vector collections
    pub async fn vector_collections(&self) -> Vec<VectorCollectionRow> {
        let objects = self.registry.list_vector_collections().await;
        objects
            .iter()
            .filter_map(|o| VectorCollectionRow::from_object(o))
            .collect()
    }

    /// Get all graphs
    pub async fn graphs(&self) -> Vec<GraphRow> {
        let objects = self.registry.list_graphs().await;
        objects
            .iter()
            .filter_map(|o| GraphRow::from_object(o))
            .collect()
    }

    /// Get all document collections
    pub async fn document_collections(&self) -> Vec<DocumentCollectionRow> {
        let objects = self.registry.list_document_collections().await;
        objects
            .iter()
            .filter_map(|o| DocumentCollectionRow::from_object(o))
            .collect()
    }

    /// Get all observability streams
    pub async fn observability_streams(&self) -> Vec<ObservabilityStreamRow> {
        let objects = self.registry.list_all().await;
        objects
            .iter()
            .filter(|o| o.is_observability())
            .filter_map(|o| ObservabilityStreamRow::from_object(o))
            .collect()
    }

    /// Get all storage layout descriptors.
    pub async fn storage_layouts(&self) -> Vec<StorageLayoutRow> {
        let objects = self.registry.list_all().await;
        objects
            .iter()
            .flat_map(|o| StorageLayoutRow::from_object(o))
            .collect()
    }

    /// Get all projection descriptors.
    pub async fn projections(&self) -> Vec<ProjectionRow> {
        let objects = self.registry.list_all().await;
        objects
            .iter()
            .flat_map(|o| ProjectionRow::from_object(o))
            .collect()
    }

    /// Get relational capability descriptors for all objects.
    pub async fn relational_capabilities(&self) -> Vec<RelationalCapabilityRow> {
        let objects = self.registry.list_all().await;
        objects
            .iter()
            .map(|o| RelationalCapabilityRow::from_object(o))
            .collect()
    }

    /// Query a specific view
    pub async fn query(&self, view: InformationSchemaView) -> InformationSchemaResult {
        match view {
            InformationSchemaView::Tables => InformationSchemaResult::Tables(self.tables().await),
            InformationSchemaView::Columns => {
                InformationSchemaResult::Columns(self.columns().await)
            }
            InformationSchemaView::TableConstraints => {
                InformationSchemaResult::TableConstraints(self.table_constraints().await)
            }
            InformationSchemaView::KeyColumnUsage => {
                InformationSchemaResult::KeyColumnUsage(self.key_column_usage().await)
            }
            InformationSchemaView::ReferentialConstraints => {
                InformationSchemaResult::ReferentialConstraints(
                    self.referential_constraints().await,
                )
            }
            InformationSchemaView::Schemata => {
                // Return unique schemas from all objects
                let objects = self.registry.list_all().await;
                let mut schemas: std::collections::HashSet<(String, String)> =
                    std::collections::HashSet::new();
                for obj in objects {
                    schemas.insert((obj.catalog.clone(), obj.namespace.join(".")));
                }
                InformationSchemaResult::Schemata(
                    schemas
                        .into_iter()
                        .map(|(catalog, schema)| SchemaRow {
                            catalog_name: catalog,
                            schema_name: schema,
                            schema_owner: None,
                            default_character_set_catalog: None,
                            default_character_set_schema: None,
                            default_character_set_name: None,
                            sql_path: None,
                        })
                        .collect(),
                )
            }
            InformationSchemaView::VectorCollections => {
                InformationSchemaResult::VectorCollections(self.vector_collections().await)
            }
            InformationSchemaView::Graphs => InformationSchemaResult::Graphs(self.graphs().await),
            InformationSchemaView::DocumentCollections => {
                InformationSchemaResult::DocumentCollections(self.document_collections().await)
            }
            InformationSchemaView::ObservabilityStreams => {
                InformationSchemaResult::ObservabilityStreams(self.observability_streams().await)
            }
            InformationSchemaView::StorageLayouts => {
                InformationSchemaResult::StorageLayouts(self.storage_layouts().await)
            }
            InformationSchemaView::Projections => {
                InformationSchemaResult::Projections(self.projections().await)
            }
            InformationSchemaView::RelationalCapabilities => {
                InformationSchemaResult::RelationalCapabilities(
                    self.relational_capabilities().await,
                )
            }
        }
    }
}

/// Row in information_schema.schemata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaRow {
    /// Name of the catalog that contains this schema
    pub catalog_name: String,
    /// Name of the schema
    pub schema_name: String,
    /// Principal that owns the schema
    pub schema_owner: Option<String>,
    /// Catalog of the default character set for new columns
    pub default_character_set_catalog: Option<String>,
    /// Schema of the default character set for new columns
    pub default_character_set_schema: Option<String>,
    /// Name of the default character set for new columns
    pub default_character_set_name: Option<String>,
    /// SQL-path for schema name resolution
    pub sql_path: Option<String>,
}

/// Result of an INFORMATION_SCHEMA query
#[derive(Debug, Clone)]
pub enum InformationSchemaResult {
    /// Rows from `information_schema.tables`
    Tables(Vec<TableRow>),
    /// Rows from `information_schema.columns`
    Columns(Vec<ColumnRow>),
    /// Rows from `information_schema.table_constraints`
    TableConstraints(Vec<TableConstraintRow>),
    /// Rows from `information_schema.key_column_usage`
    KeyColumnUsage(Vec<KeyColumnUsageRow>),
    /// Rows from `information_schema.referential_constraints`
    ReferentialConstraints(Vec<ReferentialConstraintRow>),
    /// Rows from `information_schema.schemata`
    Schemata(Vec<SchemaRow>),
    /// Rows from `information_schema.vector_collections` (ProximaDB extension)
    VectorCollections(Vec<VectorCollectionRow>),
    /// Rows from `information_schema.graphs` (ProximaDB extension)
    Graphs(Vec<GraphRow>),
    /// Rows from `information_schema.document_collections` (ProximaDB extension)
    DocumentCollections(Vec<DocumentCollectionRow>),
    /// Rows from `information_schema.observability_streams` (ProximaDB extension)
    ObservabilityStreams(Vec<ObservabilityStreamRow>),
    /// Rows from `information_schema.storage_layouts` (ProximaDB extension)
    StorageLayouts(Vec<StorageLayoutRow>),
    /// Rows from `information_schema.projections` (ProximaDB extension)
    Projections(Vec<ProjectionRow>),
    /// Rows from `information_schema.relational_capabilities` (ProximaDB extension)
    RelationalCapabilities(Vec<RelationalCapabilityRow>),
}

impl InformationSchemaResult {
    /// Get the number of rows
    pub fn row_count(&self) -> usize {
        match self {
            InformationSchemaResult::Tables(rows) => rows.len(),
            InformationSchemaResult::Columns(rows) => rows.len(),
            InformationSchemaResult::TableConstraints(rows) => rows.len(),
            InformationSchemaResult::KeyColumnUsage(rows) => rows.len(),
            InformationSchemaResult::ReferentialConstraints(rows) => rows.len(),
            InformationSchemaResult::Schemata(rows) => rows.len(),
            InformationSchemaResult::VectorCollections(rows) => rows.len(),
            InformationSchemaResult::Graphs(rows) => rows.len(),
            InformationSchemaResult::DocumentCollections(rows) => rows.len(),
            InformationSchemaResult::ObservabilityStreams(rows) => rows.len(),
            InformationSchemaResult::StorageLayouts(rows) => rows.len(),
            InformationSchemaResult::Projections(rows) => rows.len(),
            InformationSchemaResult::RelationalCapabilities(rows) => rows.len(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::TableIdentifier;
    use crate::catalog::internal::InternalSchemaRegistry;
    use proximadb_catalog::{
        CatalogProjection, CatalogProjectionKind, CatalogStorageLayout, CatalogStorageLayoutKind,
        CatalogTableSchema, RelationalCapabilities,
    };

    #[tokio::test]
    async fn test_information_schema_tables() {
        let registry = Arc::new(InternalSchemaRegistry::new());

        registry
            .create_vector_collection("embeddings", 768, "cosine")
            .await
            .expect("failed to create vector collection");
        registry
            .create_graph("social", true)
            .await
            .expect("failed to create graph");
        registry
            .create_document_collection("products", None)
            .await
            .expect("failed to create document collection");

        let info_schema = InformationSchema::new(registry);
        let tables = info_schema.tables().await;

        assert_eq!(tables.len(), 3);

        let vec_table = tables
            .iter()
            .find(|t| t.table_name == "embeddings")
            .expect("embeddings table should exist");
        assert_eq!(vec_table.table_type, "VECTOR COLLECTION");
        assert_eq!(vec_table.model_type, Some("VECTOR COLLECTION".to_string()));

        let graph_table = tables
            .iter()
            .find(|t| t.table_name == "social")
            .expect("social table should exist");
        assert_eq!(graph_table.table_type, "GRAPH");
    }

    #[tokio::test]
    async fn test_information_schema_columns() {
        let registry = Arc::new(InternalSchemaRegistry::new());

        registry
            .create_vector_collection("embeddings", 768, "cosine")
            .await
            .expect("failed to create vector collection");

        let info_schema = InformationSchema::new(registry);
        let columns = info_schema.columns().await;

        // Vector collection has id, vector, metadata columns
        let vec_columns: Vec<_> = columns
            .iter()
            .filter(|c| c.table_name == "embeddings")
            .collect();

        assert!(vec_columns.len() >= 3);
        assert!(vec_columns.iter().any(|c| c.column_name == "id"));
        assert!(vec_columns.iter().any(|c| c.column_name == "vector"));
    }

    #[tokio::test]
    async fn test_information_schema_vector_collections() {
        let registry = Arc::new(InternalSchemaRegistry::new());

        registry
            .create_vector_collection("embeddings", 768, "cosine")
            .await
            .expect("failed to create vector collection");
        registry
            .create_vector_collection("images", 512, "l2")
            .await
            .expect("failed to create vector collection");

        let info_schema = InformationSchema::new(registry);
        let vec_collections = info_schema.vector_collections().await;

        assert_eq!(vec_collections.len(), 2);

        let embeddings = vec_collections
            .iter()
            .find(|v| v.collection_name == "embeddings")
            .expect("embeddings collection should exist");
        assert_eq!(embeddings.dimension, 768);
        assert_eq!(embeddings.distance_metric, "cosine");

        let images = vec_collections
            .iter()
            .find(|v| v.collection_name == "images")
            .expect("images collection should exist");
        assert_eq!(images.dimension, 512);
        assert_eq!(images.distance_metric, "l2");
    }

    #[tokio::test]
    async fn test_information_schema_graphs() {
        let registry = Arc::new(InternalSchemaRegistry::new());

        registry
            .create_graph("social", true)
            .await
            .expect("failed to create graph");
        registry
            .create_graph("knowledge", false)
            .await
            .expect("failed to create graph");

        let info_schema = InformationSchema::new(registry);
        let graphs = info_schema.graphs().await;

        assert_eq!(graphs.len(), 2);

        let social = graphs
            .iter()
            .find(|g| g.graph_name == "social")
            .expect("social graph should exist");
        assert_eq!(social.graph_type, "directed");

        let knowledge = graphs
            .iter()
            .find(|g| g.graph_name == "knowledge")
            .expect("knowledge graph should exist");
        assert_eq!(knowledge.graph_type, "undirected");
    }

    #[tokio::test]
    async fn test_information_schema_query() {
        let registry = Arc::new(InternalSchemaRegistry::new());

        registry
            .create_vector_collection("embeddings", 768, "cosine")
            .await
            .expect("failed to create vector collection");

        let info_schema = InformationSchema::new(registry);

        let result = info_schema.query(InformationSchemaView::Tables).await;
        assert_eq!(result.row_count(), 1);

        let result = info_schema
            .query(InformationSchemaView::VectorCollections)
            .await;
        assert_eq!(result.row_count(), 1);
    }

    #[tokio::test]
    async fn test_information_schema_storage_authority_views() {
        let registry = Arc::new(InternalSchemaRegistry::new());
        let table = TableIdentifier::new(vec![], "docs".to_string());
        let schema = CatalogTableSchema::new("docs")
            .with_storage_layout(CatalogStorageLayout::internal(
                "pax_hot",
                CatalogStorageLayoutKind::Pax,
            ))
            .with_projection(CatalogProjection::rebuildable(
                "docs_text",
                CatalogProjectionKind::FullText,
                "primary",
            ))
            .with_relational_capabilities(RelationalCapabilities {
                primary_key: vec!["id".to_string()],
                ..Default::default()
            });

        registry
            .create_table(&table, schema)
            .await
            .expect("table creation should preserve catalog metadata");

        let info_schema = InformationSchema::new(registry);

        let layouts = info_schema.storage_layouts().await;
        assert_eq!(layouts.len(), 2);
        assert!(layouts.iter().any(|row| row.layout_name == "primary"));
        assert!(
            layouts
                .iter()
                .any(|row| { row.layout_name == "pax_hot" && row.layout_kind == "Pax" })
        );

        let projections = info_schema.projections().await;
        assert_eq!(projections.len(), 1);
        assert_eq!(projections[0].projection_name, "docs_text");
        assert_eq!(projections[0].projection_kind, "FullText");
        assert_eq!(projections[0].rebuild_source, "primary");

        let capabilities = info_schema.relational_capabilities().await;
        assert_eq!(capabilities.len(), 1);
        assert!(capabilities[0].has_enforced_semantics);
        assert_eq!(capabilities[0].primary_key, vec!["id"]);

        let result = info_schema
            .query(InformationSchemaView::StorageLayouts)
            .await;
        assert_eq!(result.row_count(), 2);
        assert!(InformationSchemaView::StorageLayouts.is_extension());
    }

    #[test]
    fn test_view_names() {
        assert_eq!(InformationSchemaView::Tables.name(), "tables");
        assert_eq!(
            InformationSchemaView::VectorCollections.name(),
            "vector_collections"
        );
        assert_eq!(
            InformationSchemaView::StorageLayouts.name(),
            "storage_layouts"
        );
        assert!(InformationSchemaView::VectorCollections.is_extension());
        assert!(!InformationSchemaView::Tables.is_extension());
    }
}
