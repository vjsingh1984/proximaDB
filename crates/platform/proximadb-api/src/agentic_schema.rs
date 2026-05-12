//! Agentic store schema and physical layout contracts.
//!
//! This is the schema-first bridge for agent backing stores. Transport APIs
//! should accept or reference this contract, while runtime bindings lower it to
//! ProximaRecord fields, JSONB document payloads, graph projections, vector
//! indexes, and append-only event logs.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStoreSchema {
    pub store: String,
    pub version: u32,
    pub catalog_namespace: String,
    pub fields: Vec<AgentField>,
    pub graph_edges: Vec<GraphEdgeSpec>,
    pub vector_indexes: Vec<VectorIndexSpec>,
    pub event_streams: Vec<EventStreamSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentField {
    pub name: String,
    pub field_type: AgentFieldType,
    pub required: bool,
    pub indexed: bool,
    pub storage: FieldStorage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentFieldType {
    Text,
    Integer,
    Float,
    Boolean,
    TimestampMs,
    Json,
    Jsonb,
    Vector { dimension: u32 },
    Symbol,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FieldStorage {
    RelationalColumn { column: String },
    DocumentJsonb { column: String, path: String },
    GraphProperty { label: String, property: String },
    EventPayload { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphEdgeSpec {
    pub edge_type: String,
    pub from_field: String,
    pub to_field: String,
    pub properties_jsonb_column: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VectorIndexSpec {
    pub name: String,
    pub field: String,
    pub dimension: u32,
    pub metadata_jsonb_column: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventStreamSpec {
    pub stream_prefix: String,
    pub partition_fields: Vec<String>,
    pub payload_jsonb_column: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentPhysicalLayout {
    pub catalog_namespace: String,
    pub relational_table: RelationalTableLayout,
    pub document_store: DocumentStoreLayout,
    pub graph_store: GraphStoreLayout,
    pub vector_indexes: Vec<VectorIndexSpec>,
    pub event_log: EventLogLayout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationalTableLayout {
    pub table: String,
    pub columns: Vec<RelationalColumn>,
    pub jsonb_columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationalColumn {
    pub name: String,
    pub sql_type: String,
    pub required: bool,
    pub indexed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentStoreLayout {
    pub collection: String,
    pub jsonb_column: String,
    pub indexed_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphStoreLayout {
    pub graph: String,
    pub node_labels: Vec<String>,
    pub edge_types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventLogLayout {
    pub table: String,
    pub stream_prefixes: Vec<String>,
    pub payload_jsonb_column: String,
}

impl AgentStoreSchema {
    pub fn agent_memory_default(store: impl Into<String>) -> Self {
        let store = store.into();
        Self {
            catalog_namespace: format!("agentic.{store}"),
            store,
            version: 1,
            fields: vec![
                AgentField::relational("tenant_id", AgentFieldType::Text, true, true),
                AgentField::relational("thread_id", AgentFieldType::Text, true, true),
                AgentField::relational("namespace", AgentFieldType::Text, true, true),
                AgentField::relational("key", AgentFieldType::Text, true, true),
                AgentField::document_jsonb("value", AgentFieldType::Jsonb, "payload", "$", true),
                AgentField::document_jsonb(
                    "metadata",
                    AgentFieldType::Jsonb,
                    "metadata",
                    "$",
                    true,
                ),
                AgentField::vector("embedding", 1536, "agent_memory_embedding", true),
                AgentField::graph_property(
                    "symbol_id",
                    AgentFieldType::Symbol,
                    "Symbol",
                    "id",
                    true,
                ),
            ],
            graph_edges: vec![GraphEdgeSpec {
                edge_type: "REFERENCES".to_string(),
                from_field: "symbol_id".to_string(),
                to_field: "related_symbol_id".to_string(),
                properties_jsonb_column: "edge_properties".to_string(),
            }],
            vector_indexes: vec![VectorIndexSpec {
                name: "agent_memory_embedding".to_string(),
                field: "embedding".to_string(),
                dimension: 1536,
                metadata_jsonb_column: "metadata".to_string(),
            }],
            event_streams: vec![EventStreamSpec {
                stream_prefix: "agent".to_string(),
                partition_fields: vec!["tenant_id".to_string(), "thread_id".to_string()],
                payload_jsonb_column: "payload".to_string(),
            }],
        }
    }

    pub fn physical_layout(&self) -> AgentPhysicalLayout {
        let mut columns = Vec::new();
        let mut jsonb_columns = Vec::new();
        let mut indexed_paths = Vec::new();
        let mut node_labels = Vec::new();

        for field in &self.fields {
            match &field.storage {
                FieldStorage::RelationalColumn { column } => {
                    columns.push(RelationalColumn {
                        name: column.clone(),
                        sql_type: field.field_type.sql_type().to_string(),
                        required: field.required,
                        indexed: field.indexed,
                    });
                }
                FieldStorage::DocumentJsonb { column, path } => {
                    push_unique(&mut jsonb_columns, column.clone());
                    if field.indexed {
                        indexed_paths.push(path.clone());
                    }
                }
                FieldStorage::GraphProperty { label, .. } => {
                    push_unique(&mut node_labels, label.clone());
                }
                FieldStorage::EventPayload { .. } => {}
            }
        }

        let payload_jsonb_column = self
            .event_streams
            .first()
            .map(|stream| stream.payload_jsonb_column.clone())
            .unwrap_or_else(|| "payload".to_string());
        push_unique(&mut jsonb_columns, payload_jsonb_column.clone());

        AgentPhysicalLayout {
            catalog_namespace: self.catalog_namespace.clone(),
            relational_table: RelationalTableLayout {
                table: format!("{}_agent_store", self.store),
                columns,
                jsonb_columns,
            },
            document_store: DocumentStoreLayout {
                collection: format!("{}_documents", self.store),
                jsonb_column: "payload".to_string(),
                indexed_paths,
            },
            graph_store: GraphStoreLayout {
                graph: format!("{}_graph", self.store),
                node_labels,
                edge_types: self
                    .graph_edges
                    .iter()
                    .map(|edge| edge.edge_type.clone())
                    .collect(),
            },
            vector_indexes: self.vector_indexes.clone(),
            event_log: EventLogLayout {
                table: format!("{}_events", self.store),
                stream_prefixes: self
                    .event_streams
                    .iter()
                    .map(|stream| stream.stream_prefix.clone())
                    .collect(),
                payload_jsonb_column,
            },
        }
    }
}

impl AgentPhysicalLayout {
    pub fn relational_schema_sql(&self) -> Vec<String> {
        let mut statements = vec![self.relational_table.create_table_sql()];
        statements.extend(self.relational_table.index_sql());
        statements
    }
}

impl RelationalTableLayout {
    pub fn create_table_sql(&self) -> String {
        let mut definitions = vec!["record_id TEXT PRIMARY KEY".to_string()];
        definitions.extend(self.columns.iter().map(|column| {
            format!(
                "{} {}{}",
                column.name,
                column.sql_type,
                if column.required { " NOT NULL" } else { "" }
            )
        }));
        definitions.extend(
            self.jsonb_columns
                .iter()
                .map(|column| format!("{column} JSONB NOT NULL DEFAULT '{{}}'::jsonb")),
        );

        format!(
            "CREATE TABLE IF NOT EXISTS {} ({});",
            self.table,
            definitions.join(", ")
        )
    }

    pub fn index_sql(&self) -> Vec<String> {
        self.columns
            .iter()
            .filter(|column| column.indexed)
            .map(|column| {
                format!(
                    "CREATE INDEX IF NOT EXISTS idx_{}_{} ON {} ({});",
                    self.table, column.name, self.table, column.name
                )
            })
            .collect()
    }
}

impl AgentField {
    pub fn relational(
        name: impl Into<String>,
        field_type: AgentFieldType,
        required: bool,
        indexed: bool,
    ) -> Self {
        let name = name.into();
        Self {
            storage: FieldStorage::RelationalColumn {
                column: name.clone(),
            },
            name,
            field_type,
            required,
            indexed,
        }
    }

    pub fn document_jsonb(
        name: impl Into<String>,
        field_type: AgentFieldType,
        column: impl Into<String>,
        path: impl Into<String>,
        indexed: bool,
    ) -> Self {
        Self {
            name: name.into(),
            field_type,
            required: false,
            indexed,
            storage: FieldStorage::DocumentJsonb {
                column: column.into(),
                path: path.into(),
            },
        }
    }

    pub fn vector(
        name: impl Into<String>,
        dimension: u32,
        _index: impl Into<String>,
        indexed: bool,
    ) -> Self {
        let name = name.into();
        Self {
            storage: FieldStorage::DocumentJsonb {
                column: "payload".to_string(),
                path: format!("$.{name}"),
            },
            name,
            field_type: AgentFieldType::Vector { dimension },
            required: false,
            indexed,
        }
    }

    pub fn graph_property(
        name: impl Into<String>,
        field_type: AgentFieldType,
        label: impl Into<String>,
        property: impl Into<String>,
        indexed: bool,
    ) -> Self {
        Self {
            name: name.into(),
            field_type,
            required: false,
            indexed,
            storage: FieldStorage::GraphProperty {
                label: label.into(),
                property: property.into(),
            },
        }
    }
}

impl AgentFieldType {
    fn sql_type(&self) -> &'static str {
        match self {
            Self::Text | Self::Symbol => "TEXT",
            Self::Integer | Self::TimestampMs => "BIGINT",
            Self::Float => "DOUBLE PRECISION",
            Self::Boolean => "BOOLEAN",
            Self::Json | Self::Jsonb => "JSONB",
            Self::Vector { .. } => "VECTOR",
        }
    }
}

fn push_unique<T: PartialEq>(values: &mut Vec<T>, value: T) {
    if !values.contains(&value) {
        values.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_store_schema_generates_mixed_physical_layout() {
        let schema = AgentStoreSchema::agent_memory_default("victor");
        let layout = schema.physical_layout();

        assert_eq!(layout.catalog_namespace, "agentic.victor");
        assert_eq!(layout.relational_table.table, "victor_agent_store");
        assert!(layout.relational_table.columns.iter().any(|column| {
            column.name == "thread_id" && column.sql_type == "TEXT" && column.indexed
        }));
        assert!(
            layout
                .relational_table
                .jsonb_columns
                .contains(&"payload".to_string())
        );
        assert_eq!(layout.document_store.collection, "victor_documents");
        assert!(
            layout
                .document_store
                .indexed_paths
                .contains(&"$".to_string())
        );
        assert!(
            layout
                .document_store
                .indexed_paths
                .contains(&"$.embedding".to_string())
        );
        assert_eq!(layout.graph_store.graph, "victor_graph");
        assert_eq!(layout.graph_store.node_labels, vec!["Symbol".to_string()]);
        assert_eq!(
            layout.graph_store.edge_types,
            vec!["REFERENCES".to_string()]
        );
        assert_eq!(layout.vector_indexes[0].field, "embedding");
        assert_eq!(layout.vector_indexes[0].dimension, 1536);
        assert_eq!(layout.event_log.table, "victor_events");
    }

    #[test]
    fn agent_store_schema_emits_relational_ddl_for_stable_fields_and_jsonb() {
        let ddl = AgentStoreSchema::agent_memory_default("victor")
            .physical_layout()
            .relational_schema_sql();

        assert!(ddl[0].starts_with("CREATE TABLE IF NOT EXISTS victor_agent_store"));
        assert!(ddl[0].contains("thread_id TEXT NOT NULL"));
        assert!(ddl[0].contains("namespace TEXT NOT NULL"));
        assert!(ddl[0].contains("payload JSONB NOT NULL DEFAULT '{}'::jsonb"));
        assert!(
            ddl.iter()
                .any(|sql| sql == "CREATE INDEX IF NOT EXISTS idx_victor_agent_store_thread_id ON victor_agent_store (thread_id);")
        );
    }
}
