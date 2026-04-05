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

use serde::{Deserialize, Serialize};

/// Store type for multi-model routing.
///
/// Each variant maps to a dedicated service layer and storage engine
/// with its own native result type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum StoreType {
    /// Vector similarity search (SST/HELIX/VIPER/NOVA engines)
    /// Returns: OptimizedSearchRecord
    Vector,

    /// JSON document store (CEDAR engine)
    /// Returns: DocumentRecord
    Document,

    /// Property graph (ORION/PULSAR engines)
    /// Returns: Node/Edge/Path
    Graph,

    /// Time-series observability: metrics, logs, traces (CHRONO engine)
    /// Returns: MetricSample/LogEntry/TraceData
    Observability,

    /// Standard relational tables with typed columns (SEQUOIA engine)
    /// Returns: TypedRow
    Relational,

    /// Financial time-series data: OHLC, tick data, IoT sensors (TST engine)
    /// Returns: TimeSeriesRecord
    TimeSeries,

    /// Append-only event/audit log (EventLog engine)
    /// Returns: EventRecord
    Event,
}

impl std::fmt::Display for StoreType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreType::Vector => write!(f, "vector"),
            StoreType::Document => write!(f, "document"),
            StoreType::Graph => write!(f, "graph"),
            StoreType::Observability => write!(f, "observability"),
            StoreType::Relational => write!(f, "relational"),
            StoreType::TimeSeries => write!(f, "timeseries"),
            StoreType::Event => write!(f, "event"),
        }
    }
}

/// Detect the store type from a SQL CREATE TABLE statement.
///
/// Priority:
/// 1. Explicit USING clause
/// 2. Column type inference
/// 3. Default to Relational
pub fn detect_store_type_from_create(sql: &str) -> StoreType {
    let upper = sql.to_uppercase();

    // 1. Explicit USING clause
    if upper.contains("USING DOCUMENT") {
        return StoreType::Document;
    }
    if upper.contains("USING GRAPH") {
        return StoreType::Graph;
    }
    if upper.contains("USING OBSERVABILITY") || upper.contains("USING TIMESERIES") {
        return StoreType::Observability;
    }
    if upper.contains("USING VECTOR") {
        return StoreType::Vector;
    }

    // 2. Column type inference
    if upper.contains("VECTOR(") || upper.contains("EMBEDDING(") {
        return StoreType::Vector;
    }
    if upper.contains("JSONB") || upper.contains("JSON") {
        return StoreType::Document;
    }
    if upper.contains("SEVERITY") && upper.contains("MESSAGE") {
        return StoreType::Observability;
    }

    // 3. Default: standard relational table
    StoreType::Relational
}

/// Detect the store type from a SQL SELECT/INSERT/UPDATE/DELETE statement.
///
/// Priority:
/// 1. Vector operators (<->, <=>, <#>)
/// 2. JSON path expressions ($.)
/// 3. Table name prefix (graph_, log_, metric_, doc_)
/// 4. Catalog lookup (via callback)
/// 5. Default to Relational
pub fn detect_store_type_from_query(
    sql: &str,
    table_name: &str,
    catalog_lookup: Option<&dyn Fn(&str) -> Option<StoreType>>,
) -> StoreType {
    let upper = sql.to_uppercase();

    // 1. Vector operators
    if upper.contains("<->") || upper.contains("<=>") || upper.contains("<#>") {
        return StoreType::Vector;
    }

    // 2. JSON path expressions
    if sql.contains("$.") {
        return StoreType::Document;
    }

    // 3. Table name prefix
    let lower_table = table_name.to_lowercase();
    if lower_table.starts_with("graph_")
        || lower_table.starts_with("node_")
        || lower_table.starts_with("edge_")
    {
        return StoreType::Graph;
    }
    if lower_table.starts_with("doc_") || lower_table.starts_with("document_") {
        return StoreType::Document;
    }
    if lower_table.starts_with("log_")
        || lower_table.starts_with("metric_")
        || lower_table.starts_with("trace_")
    {
        return StoreType::Observability;
    }

    // 4. Catalog lookup
    if let Some(lookup) = catalog_lookup {
        if let Some(store_type) = lookup(table_name) {
            return store_type;
        }
    }

    // 5. Default to Relational
    StoreType::Relational
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
        rows: Vec<crate::storage::engines::impls::sequoia::TypedRow>,
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
            StoreType::Relational
        );
    }

    #[test]
    fn test_detect_create_document() {
        assert_eq!(
            detect_store_type_from_create("CREATE TABLE docs (id TEXT, data JSONB) USING DOCUMENT"),
            StoreType::Document
        );
    }

    #[test]
    fn test_detect_create_vector() {
        assert_eq!(
            detect_store_type_from_create("CREATE TABLE vecs (id TEXT, embedding VECTOR(384))"),
            StoreType::Vector
        );
    }

    #[test]
    fn test_detect_create_graph() {
        assert_eq!(
            detect_store_type_from_create("CREATE TABLE social (id TEXT) USING GRAPH"),
            StoreType::Graph
        );
    }

    #[test]
    fn test_detect_create_observability() {
        assert_eq!(
            detect_store_type_from_create(
                "CREATE TABLE app_logs (ts TIMESTAMP, severity TEXT) USING OBSERVABILITY"
            ),
            StoreType::Observability
        );
    }

    #[test]
    fn test_detect_query_vector_operator() {
        assert_eq!(
            detect_store_type_from_query(
                "SELECT * FROM vecs ORDER BY embedding <-> '[0.1,0.2]' LIMIT 10",
                "vecs",
                None
            ),
            StoreType::Vector
        );
    }

    #[test]
    fn test_detect_query_json_path() {
        assert_eq!(
            detect_store_type_from_query(
                "SELECT * FROM docs WHERE $.name = 'Alice'",
                "docs",
                None
            ),
            StoreType::Document
        );
    }

    #[test]
    fn test_detect_query_graph_prefix() {
        assert_eq!(
            detect_store_type_from_query("SELECT * FROM graph_social WHERE label = 'Person'", "graph_social", None),
            StoreType::Graph
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
            StoreType::Observability
        );
    }

    #[test]
    fn test_detect_query_relational_default() {
        assert_eq!(
            detect_store_type_from_query("SELECT * FROM users WHERE id > 5", "users", None),
            StoreType::Relational
        );
    }

    #[test]
    fn test_detect_query_catalog_override() {
        let catalog = |table: &str| -> Option<StoreType> {
            if table == "my_custom_docs" {
                Some(StoreType::Document)
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
            StoreType::Document
        );
    }

    #[test]
    fn test_store_type_display() {
        assert_eq!(StoreType::Vector.to_string(), "vector");
        assert_eq!(StoreType::Document.to_string(), "document");
        assert_eq!(StoreType::Graph.to_string(), "graph");
        assert_eq!(StoreType::Observability.to_string(), "observability");
        assert_eq!(StoreType::Relational.to_string(), "relational");
    }
}
