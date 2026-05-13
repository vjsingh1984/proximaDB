//! Multi-Model SQL Router
//!
//! Routes SQL statements to the correct service layer based on store type detection.
//! This is the central dispatch point for all SQL operations across all 5 data models.
//!
//! ## Store Type Detection
//!
//! Detection happens in priority order:
//! 1. **Explicit USING clause**: `CREATE TABLE t (...) USING DOCUMENT`
//! 2. **SQL operators**: `<->` (vector), `$.` (document JSON path)
//! 3. **Column types**: VECTOR → Vector, JSONB → Document
//! 4. **Table name prefix**: `graph_` → Graph, `log_`/`metric_` → Observability
//! 5. **Catalog lookup**: Check registered store type for existing tables
//! 6. **Default**: Relational (standard SQL tables)
//!
//! ## Execution Flow
//!
//! ```text
//! SQL Statement
//!     │
//!     ▼
//! detect_store_type()
//!     │
//!     ├── Vector       → VectorOperationsService    → OptimizedSearchRecord
//!     ├── Document     → DocumentService            → DocumentRecord
//!     ├── Graph        → GraphOperationsService      → Node/Edge/Path
//!     ├── Observability → ObservabilityService       → MetricSample/LogEntry/TraceData
//!     └── Relational   → RelationalService (SEQUOIA) → TypedRow
//! ```

pub use proximadb_data_model::DataModel;

/// Physical storage preference parsed from pgwire-compatible CREATE TABLE DDL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageOptions {
    pub engine: Option<String>,
    pub layout: Option<String>,
}

impl StorageOptions {
    pub fn is_columnar(&self) -> bool {
        self.layout.as_deref() == Some("columnar")
            || matches!(self.engine.as_deref(), Some("VIPER") | Some("RAPTOR"))
    }

    pub fn is_record_or_hybrid(&self) -> bool {
        self.layout
            .as_deref()
            .is_none_or(|layout| matches!(layout, "record" | "hybrid"))
    }
}

/// Detect the store type from a SQL CREATE TABLE statement.
///
/// Priority:
/// 1. Explicit USING clause
/// 2. Column type inference
/// 3. Default to Relational
pub fn detect_store_type_from_create(sql: &str) -> DataModel {
    let upper = sql.to_uppercase();

    // 1. Explicit USING clause
    if upper.contains("USING DOCUMENT") {
        return DataModel::Document;
    }
    if upper.contains("USING GRAPH") {
        return DataModel::Graph;
    }
    if upper.contains("USING OBSERVABILITY") || upper.contains("USING TIMESERIES") {
        return DataModel::Observability;
    }
    if upper.contains("USING VECTOR") {
        return DataModel::Vector;
    }

    // 2. Column type inference
    if upper.contains("VECTOR(") || upper.contains("EMBEDDING(") {
        return DataModel::Vector;
    }
    if upper.contains("JSONB") || upper.contains("JSON") {
        return DataModel::Document;
    }
    if upper.contains("SEVERITY") && upper.contains("MESSAGE") {
        return DataModel::Observability;
    }

    // 3. Default: standard relational table
    DataModel::Relational
}

/// Extract physical storage options from pgwire-compatible CREATE TABLE DDL.
///
/// Supported forms:
/// - `CREATE TABLE t (...) USING VIPER`
/// - `CREATE TABLE t (...) WITH (storage_engine='helix', layout='hybrid')`
/// - xcatalog comments emitted by SDK DDL helpers:
///   `COMMENT ON TABLE t IS 'xcatalog.namespace=...;engine=SST;layout=hybrid'`
pub fn detect_storage_options_from_create(sql: &str) -> StorageOptions {
    let engine = extract_using_engine(sql)
        .or_else(|| extract_option_value(sql, "storage_engine"))
        .or_else(|| extract_option_value(sql, "engine"))
        .map(|engine| engine.to_ascii_uppercase());
    let layout = extract_option_value(sql, "layout").map(|layout| layout.to_ascii_lowercase());

    StorageOptions { engine, layout }
}

fn extract_using_engine(sql: &str) -> Option<String> {
    let mut tokens = sql
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ';' | ',' | ')'))
        .filter(|token| !token.is_empty());
    while let Some(token) = tokens.next() {
        if token.eq_ignore_ascii_case("USING") {
            let candidate = tokens.next()?.trim_matches('"').trim_matches('\'');
            let upper = candidate.to_ascii_uppercase();
            if matches!(
                upper.as_str(),
                "SST"
                    | "VIPER"
                    | "SWIFT"
                    | "HELIX"
                    | "NOVA"
                    | "RAPTOR"
                    | "MMAP"
                    | "HYBRID"
                    | "TST"
                    | "CEDAR"
                    | "TITAN"
                    | "CHRONO"
            ) {
                return Some(upper);
            }
        }
    }
    None
}

fn extract_option_value(sql: &str, key: &str) -> Option<String> {
    let lower_sql = sql.to_ascii_lowercase();
    let key_lower = key.to_ascii_lowercase();
    let key_pos = lower_sql.find(&key_lower)?;
    let after_key = &sql[key_pos + key.len()..];
    let equals_pos = after_key.find('=')?;
    let value = after_key[equals_pos + 1..].trim_start();
    let trimmed = value.trim_start_matches(['"', '\'']);
    let end = trimmed
        .find(|ch: char| matches!(ch, '\'' | '"' | ';' | ',' | ')' | ' '))
        .unwrap_or(trimmed.len());
    let candidate = trimmed[..end].trim();
    if candidate.is_empty() {
        None
    } else {
        Some(candidate.to_string())
    }
}

/// Detect the store type from a SQL SELECT/INSERT/UPDATE/DELETE statement.
///
/// Priority:
/// 1. Vector operators (<->, <=>, <#>) and lowered vector distance calls
/// 2. JSON path expressions ($.) and lowered JSON helper calls
/// 3. Graph SQL helpers (`GRAPH_QUERY`)
/// 4. Table name prefix (graph_, log_, metric_, doc_)
/// 5. Catalog lookup (via callback)
/// 6. Default to Relational
pub fn detect_store_type_from_query(
    sql: &str,
    table_name: &str,
    catalog_lookup: Option<&dyn Fn(&str) -> Option<DataModel>>,
) -> DataModel {
    let upper = sql.to_uppercase();

    // 1. Vector operators and lowered pgwire/vector SQL helpers.
    if upper.contains("<->")
        || upper.contains("<=>")
        || upper.contains("<#>")
        || upper.contains("VECTOR_DISTANCE")
    {
        return DataModel::Vector;
    }

    // 2. JSON path expressions and lowered pgwire JSON helper calls.
    if sql.contains("$.")
        || upper.contains("JSON_EXTRACT")
        || upper.contains("JSON_CONTAINS")
        || upper.contains("JSON_EXISTS")
        || upper.contains("JSON_PATH_EXISTS")
    {
        return DataModel::Document;
    }

    // 3. Graph SQL helpers.
    if upper.contains("GRAPH_QUERY") {
        return DataModel::Graph;
    }

    // 4. Table name prefix
    let lower_table = table_name.to_lowercase();
    if lower_table.starts_with("graph_")
        || lower_table.starts_with("node_")
        || lower_table.starts_with("edge_")
    {
        return DataModel::Graph;
    }
    if lower_table.starts_with("doc_") || lower_table.starts_with("document_") {
        return DataModel::Document;
    }
    if lower_table.starts_with("log_")
        || lower_table.starts_with("metric_")
        || lower_table.starts_with("trace_")
    {
        return DataModel::Observability;
    }

    // 5. Catalog lookup
    if let Some(lookup) = catalog_lookup
        && let Some(store_type) = lookup(table_name)
    {
        return store_type;
    }

    // 6. Default to Relational
    DataModel::Relational
}

/// Result type envelope for multi-model query results.
///
/// Each variant wraps the native result type of its data model,
/// preserving type safety while allowing unified handling in
/// protocol layers (PG wire, Arrow Flight).
#[derive(Debug)]
pub enum MultiModelResult {
    /// Vector search results
    Vector {
        results: Vec<crate::core::search::results::OptimizedSearchRecord>,
        total_count: Option<u64>,
    },
    /// Document query results
    Document {
        records: Vec<crate::storage::document::DocumentRecord>,
        total_count: Option<u64>,
    },
    /// Graph query results
    Graph {
        nodes: Vec<crate::proto::proximadb_v1::Node>,
        edges: Vec<crate::proto::proximadb_v1::Edge>,
    },
    /// Observability query results
    Observability(ObservabilityResult),
    /// Relational query results
    Relational {
        rows: Vec<crate::storage::engines::sequoia::TypedRow>,
        column_names: Vec<String>,
        total_count: Option<u64>,
    },
    /// DDL result (CREATE/DROP/ALTER)
    Ddl { success: bool, message: String },
    /// DML result (INSERT/UPDATE/DELETE)
    Dml { rows_affected: u64 },
}

/// Observability-specific results (logs, metrics, or traces)
#[derive(Debug)]
pub enum ObservabilityResult {
    Logs(Vec<crate::proto::proximadb_v1::LogEntry>),
    Metrics(Vec<crate::proto::proximadb_v1::MetricSample>),
    Traces(Vec<crate::proto::proximadb_v1::TraceData>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_create_relational() {
        assert_eq!(
            detect_store_type_from_create("CREATE TABLE users (id INT, name VARCHAR(255))"),
            DataModel::Relational
        );
    }

    #[test]
    fn test_detect_create_document() {
        assert_eq!(
            detect_store_type_from_create("CREATE TABLE docs (id TEXT, data JSONB) USING DOCUMENT"),
            DataModel::Document
        );
    }

    #[test]
    fn test_detect_create_vector() {
        assert_eq!(
            detect_store_type_from_create("CREATE TABLE vecs (id TEXT, embedding VECTOR(384))"),
            DataModel::Vector
        );
    }

    #[test]
    fn test_detect_create_graph() {
        assert_eq!(
            detect_store_type_from_create("CREATE TABLE social (id TEXT) USING GRAPH"),
            DataModel::Graph
        );
    }

    #[test]
    fn test_detect_create_observability() {
        assert_eq!(
            detect_store_type_from_create(
                "CREATE TABLE app_logs (ts TIMESTAMP, severity TEXT) USING OBSERVABILITY"
            ),
            DataModel::Observability
        );
    }

    #[test]
    fn test_detect_create_storage_options_for_engines_and_layouts() {
        let viper = detect_storage_options_from_create(
            "CREATE TABLE events (id TEXT, payload JSONB) WITH (storage_engine='viper', layout='columnar')",
        );
        assert_eq!(viper.engine.as_deref(), Some("VIPER"));
        assert_eq!(viper.layout.as_deref(), Some("columnar"));
        assert!(viper.is_columnar());

        let helix =
            detect_storage_options_from_create("CREATE TABLE vectors (id TEXT) USING HELIX");
        assert_eq!(helix.engine.as_deref(), Some("HELIX"));
        assert!(helix.is_record_or_hybrid());

        let swift = detect_storage_options_from_create(
            "COMMENT ON TABLE agent_store IS 'xcatalog.namespace=agentic.demo;engine=SWIFT;layout=hybrid'",
        );
        assert_eq!(swift.engine.as_deref(), Some("SWIFT"));
        assert_eq!(swift.layout.as_deref(), Some("hybrid"));
        assert!(swift.is_record_or_hybrid());
    }

    #[test]
    fn test_detect_query_vector_operator() {
        assert_eq!(
            detect_store_type_from_query(
                "SELECT * FROM vecs ORDER BY embedding <-> '[0.1,0.2]' LIMIT 10",
                "vecs",
                None
            ),
            DataModel::Vector
        );
    }

    #[test]
    fn test_detect_query_lowered_vector_distance_function() {
        assert_eq!(
            detect_store_type_from_query(
                "SELECT * FROM vecs ORDER BY VECTOR_DISTANCE(embedding, [0.1,0.2], 'l2') LIMIT 10",
                "vecs",
                None
            ),
            DataModel::Vector
        );
    }

    #[test]
    fn test_detect_query_json_path() {
        assert_eq!(
            detect_store_type_from_query("SELECT * FROM docs WHERE $.name = 'Alice'", "docs", None),
            DataModel::Document
        );
    }

    #[test]
    fn test_detect_query_lowered_json_helpers() {
        for sql in [
            "SELECT * FROM docs WHERE JSON_EXTRACT_TEXT(metadata, 'tenant') = 'acme'",
            "SELECT * FROM docs WHERE JSON_CONTAINS(metadata, '{\"role\":\"planner\"}')",
            "SELECT * FROM docs WHERE JSON_EXISTS(metadata, 'skills')",
            "SELECT * FROM docs WHERE JSON_PATH_EXISTS(metadata, '$.skills[*]')",
        ] {
            assert_eq!(
                detect_store_type_from_query(sql, "docs", None),
                DataModel::Document,
                "{sql}"
            );
        }
    }

    #[test]
    fn test_detect_query_graph_prefix() {
        assert_eq!(
            detect_store_type_from_query(
                "SELECT * FROM graph_social WHERE label = 'Person'",
                "graph_social",
                None
            ),
            DataModel::Graph
        );
    }

    #[test]
    fn test_detect_query_graph_query_function() {
        assert_eq!(
            detect_store_type_from_query(
                "SELECT * FROM GRAPH_QUERY('MATCH (n:Agent)-[:CALLS]->(m) RETURN m')",
                "agent_queries",
                None
            ),
            DataModel::Graph
        );
    }

    #[test]
    fn test_detect_query_log_prefix() {
        assert_eq!(
            detect_store_type_from_query(
                "SELECT * FROM log_app WHERE severity = 'ERROR'",
                "log_app",
                None
            ),
            DataModel::Observability
        );
    }

    #[test]
    fn test_detect_query_relational_default() {
        assert_eq!(
            detect_store_type_from_query("SELECT * FROM users WHERE id > 5", "users", None),
            DataModel::Relational
        );
    }

    #[test]
    fn test_detect_query_catalog_override() {
        let catalog = |table: &str| -> Option<DataModel> {
            if table == "my_custom_docs" {
                Some(DataModel::Document)
            } else {
                None
            }
        };
        assert_eq!(
            detect_store_type_from_query(
                "SELECT * FROM my_custom_docs WHERE id = 1",
                "my_custom_docs",
                Some(&catalog)
            ),
            DataModel::Document
        );
    }

    #[test]
    fn test_store_type_display() {
        assert_eq!(DataModel::Vector.to_string(), "vector");
        assert_eq!(DataModel::Document.to_string(), "document");
        assert_eq!(DataModel::Graph.to_string(), "graph");
        assert_eq!(DataModel::Observability.to_string(), "observability");
        assert_eq!(DataModel::Relational.to_string(), "relational");
    }
}
