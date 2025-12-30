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

use serde::{Deserialize, Serialize};

use super::{
    CatalogObject, ConstraintType, ForeignKeyReference, InternalSchemaRegistry, ModelProperties,
    ObjectType, ReferentialAction, SchemaEnforcementMode,
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
        )
    }
}

/// Row in information_schema.tables
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableRow {
    pub table_catalog: String,
    pub table_schema: String,
    pub table_name: String,
    pub table_type: String,
    pub self_referencing_column_name: Option<String>,
    pub reference_generation: Option<String>,
    pub user_defined_type_catalog: Option<String>,
    pub user_defined_type_schema: Option<String>,
    pub user_defined_type_name: Option<String>,
    pub is_insertable_into: String,
    pub is_typed: String,
    pub commit_action: Option<String>,
    // ProximaDB extensions
    pub enforcement_mode: Option<String>,
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
            ObjectType::LogStream | ObjectType::MetricStream | ObjectType::TraceStream => {
                "STREAM"
            }
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
    pub table_catalog: String,
    pub table_schema: String,
    pub table_name: String,
    pub column_name: String,
    pub ordinal_position: i32,
    pub column_default: Option<String>,
    pub is_nullable: String,
    pub data_type: String,
    pub character_maximum_length: Option<i32>,
    pub character_octet_length: Option<i32>,
    pub numeric_precision: Option<i32>,
    pub numeric_precision_radix: Option<i32>,
    pub numeric_scale: Option<i32>,
    pub datetime_precision: Option<i32>,
    pub interval_type: Option<String>,
    pub interval_precision: Option<i32>,
    pub character_set_catalog: Option<String>,
    pub character_set_schema: Option<String>,
    pub character_set_name: Option<String>,
    pub collation_catalog: Option<String>,
    pub collation_schema: Option<String>,
    pub collation_name: Option<String>,
    pub domain_catalog: Option<String>,
    pub domain_schema: Option<String>,
    pub domain_name: Option<String>,
    pub udt_catalog: Option<String>,
    pub udt_schema: Option<String>,
    pub udt_name: Option<String>,
    pub scope_catalog: Option<String>,
    pub scope_schema: Option<String>,
    pub scope_name: Option<String>,
    pub maximum_cardinality: Option<i32>,
    pub dtd_identifier: Option<String>,
    pub is_self_referencing: String,
    pub is_identity: String,
    pub identity_generation: Option<String>,
    pub identity_start: Option<String>,
    pub identity_increment: Option<String>,
    pub identity_maximum: Option<String>,
    pub identity_minimum: Option<String>,
    pub identity_cycle: String,
    pub is_generated: String,
    pub generation_expression: Option<String>,
    pub is_updatable: String,
}

impl ColumnRow {
    /// Create from a catalog object and column
    pub fn from_column(
        obj: &CatalogObject,
        col: &crate::catalog::types::CatalogColumn,
    ) -> Self {
        use crate::catalog::types::CatalogDataType;

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
    pub constraint_catalog: String,
    pub constraint_schema: String,
    pub constraint_name: String,
    pub table_catalog: String,
    pub table_schema: String,
    pub table_name: String,
    pub constraint_type: String,
    pub is_deferrable: String,
    pub initially_deferred: String,
    pub enforced: String,
}

impl TableConstraintRow {
    /// Create from a catalog object and constraint
    pub fn from_constraint(
        obj: &CatalogObject,
        constraint: &super::TableConstraint,
    ) -> Self {
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
            is_deferrable: if constraint.is_deferrable { "YES" } else { "NO" }.to_string(),
            initially_deferred: if constraint.is_deferred { "YES" } else { "NO" }.to_string(),
            enforced: "YES".to_string(),
        }
    }
}

/// Row in information_schema.key_column_usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyColumnUsageRow {
    pub constraint_catalog: String,
    pub constraint_schema: String,
    pub constraint_name: String,
    pub table_catalog: String,
    pub table_schema: String,
    pub table_name: String,
    pub column_name: String,
    pub ordinal_position: i32,
    pub position_in_unique_constraint: Option<i32>,
}

/// Row in information_schema.referential_constraints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferentialConstraintRow {
    pub constraint_catalog: String,
    pub constraint_schema: String,
    pub constraint_name: String,
    pub unique_constraint_catalog: Option<String>,
    pub unique_constraint_schema: Option<String>,
    pub unique_constraint_name: Option<String>,
    pub match_option: String,
    pub update_rule: String,
    pub delete_rule: String,
    // ProximaDB extensions
    pub reference_type: Option<String>,
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
    pub collection_catalog: String,
    pub collection_schema: String,
    pub collection_name: String,
    pub dimension: u32,
    pub distance_metric: String,
    pub quantization: Option<String>,
    pub index_type: Option<String>,
    pub hnsw_m: Option<u32>,
    pub hnsw_ef_construction: Option<u32>,
    pub vector_count: Option<u64>,
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
    pub graph_catalog: String,
    pub graph_schema: String,
    pub graph_name: String,
    pub graph_type: String,
    pub allow_self_loops: bool,
    pub allow_multi_edges: bool,
    pub node_labels: Vec<String>,
    pub edge_types: Vec<String>,
    pub node_count: Option<u64>,
    pub edge_count: Option<u64>,
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
    pub collection_catalog: String,
    pub collection_schema: String,
    pub collection_name: String,
    pub has_json_schema: bool,
    pub id_generation: String,
    pub enable_full_text: bool,
    pub indexed_paths: Vec<String>,
    pub document_count: Option<u64>,
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
    pub stream_catalog: String,
    pub stream_schema: String,
    pub stream_name: String,
    pub stream_type: String,
    pub retention_seconds: u64,
    pub rollup_intervals: Vec<String>,
    pub event_count: Option<u64>,
    pub oldest_timestamp: Option<i64>,
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

    /// Query a specific view
    pub async fn query(&self, view: InformationSchemaView) -> InformationSchemaResult {
        match view {
            InformationSchemaView::Tables => {
                InformationSchemaResult::Tables(self.tables().await)
            }
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
                InformationSchemaResult::ReferentialConstraints(self.referential_constraints().await)
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
            InformationSchemaView::Graphs => {
                InformationSchemaResult::Graphs(self.graphs().await)
            }
            InformationSchemaView::DocumentCollections => {
                InformationSchemaResult::DocumentCollections(self.document_collections().await)
            }
            InformationSchemaView::ObservabilityStreams => {
                InformationSchemaResult::ObservabilityStreams(self.observability_streams().await)
            }
        }
    }
}

/// Row in information_schema.schemata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaRow {
    pub catalog_name: String,
    pub schema_name: String,
    pub schema_owner: Option<String>,
    pub default_character_set_catalog: Option<String>,
    pub default_character_set_schema: Option<String>,
    pub default_character_set_name: Option<String>,
    pub sql_path: Option<String>,
}

/// Result of an INFORMATION_SCHEMA query
#[derive(Debug, Clone)]
pub enum InformationSchemaResult {
    Tables(Vec<TableRow>),
    Columns(Vec<ColumnRow>),
    TableConstraints(Vec<TableConstraintRow>),
    KeyColumnUsage(Vec<KeyColumnUsageRow>),
    ReferentialConstraints(Vec<ReferentialConstraintRow>),
    Schemata(Vec<SchemaRow>),
    VectorCollections(Vec<VectorCollectionRow>),
    Graphs(Vec<GraphRow>),
    DocumentCollections(Vec<DocumentCollectionRow>),
    ObservabilityStreams(Vec<ObservabilityStreamRow>),
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::internal::InternalSchemaRegistry;

    #[tokio::test]
    async fn test_information_schema_tables() {
        let registry = Arc::new(InternalSchemaRegistry::new());

        registry
            .create_vector_collection("embeddings", 768, "cosine")
            .await
            .unwrap();
        registry.create_graph("social", true).await.unwrap();
        registry
            .create_document_collection("products", None)
            .await
            .unwrap();

        let info_schema = InformationSchema::new(registry);
        let tables = info_schema.tables().await;

        assert_eq!(tables.len(), 3);

        let vec_table = tables.iter().find(|t| t.table_name == "embeddings").unwrap();
        assert_eq!(vec_table.table_type, "VECTOR COLLECTION");
        assert_eq!(vec_table.model_type, Some("VECTOR COLLECTION".to_string()));

        let graph_table = tables.iter().find(|t| t.table_name == "social").unwrap();
        assert_eq!(graph_table.table_type, "GRAPH");
    }

    #[tokio::test]
    async fn test_information_schema_columns() {
        let registry = Arc::new(InternalSchemaRegistry::new());

        registry
            .create_vector_collection("embeddings", 768, "cosine")
            .await
            .unwrap();

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
            .unwrap();
        registry
            .create_vector_collection("images", 512, "l2")
            .await
            .unwrap();

        let info_schema = InformationSchema::new(registry);
        let vec_collections = info_schema.vector_collections().await;

        assert_eq!(vec_collections.len(), 2);

        let embeddings = vec_collections
            .iter()
            .find(|v| v.collection_name == "embeddings")
            .unwrap();
        assert_eq!(embeddings.dimension, 768);
        assert_eq!(embeddings.distance_metric, "cosine");

        let images = vec_collections
            .iter()
            .find(|v| v.collection_name == "images")
            .unwrap();
        assert_eq!(images.dimension, 512);
        assert_eq!(images.distance_metric, "l2");
    }

    #[tokio::test]
    async fn test_information_schema_graphs() {
        let registry = Arc::new(InternalSchemaRegistry::new());

        registry.create_graph("social", true).await.unwrap();
        registry.create_graph("knowledge", false).await.unwrap();

        let info_schema = InformationSchema::new(registry);
        let graphs = info_schema.graphs().await;

        assert_eq!(graphs.len(), 2);

        let social = graphs.iter().find(|g| g.graph_name == "social").unwrap();
        assert_eq!(social.graph_type, "directed");

        let knowledge = graphs.iter().find(|g| g.graph_name == "knowledge").unwrap();
        assert_eq!(knowledge.graph_type, "undirected");
    }

    #[tokio::test]
    async fn test_information_schema_query() {
        let registry = Arc::new(InternalSchemaRegistry::new());

        registry
            .create_vector_collection("embeddings", 768, "cosine")
            .await
            .unwrap();

        let info_schema = InformationSchema::new(registry);

        let result = info_schema.query(InformationSchemaView::Tables).await;
        assert_eq!(result.row_count(), 1);

        let result = info_schema
            .query(InformationSchemaView::VectorCollections)
            .await;
        assert_eq!(result.row_count(), 1);
    }

    #[test]
    fn test_view_names() {
        assert_eq!(InformationSchemaView::Tables.name(), "tables");
        assert_eq!(InformationSchemaView::VectorCollections.name(), "vector_collections");
        assert!(InformationSchemaView::VectorCollections.is_extension());
        assert!(!InformationSchemaView::Tables.is_extension());
    }
}
