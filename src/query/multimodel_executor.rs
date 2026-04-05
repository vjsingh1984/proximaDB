//! Multi-Model SQL Executor
//!
//! Dispatches parsed SQL statements to the correct service layer based on
//! store type detection. This bridges the SQL parser to the 5 data model
//! service layers.
//!
//! ## Execution Flow
//!
//! ```text
//! SQL text → SqlFrontendParser → Query AST
//!     │
//!     ▼
//! detect_store_type() → StoreType
//!     │
//!     ├── Vector       → VectorOperationsService.search()
//!     ├── Document     → DocumentService.query_documents()
//!     ├── Graph        → GraphService.query_nodes()
//!     ├── Observability → ObservabilityService.query_logs/metrics()
//!     └── Relational   → SequoiaEngine.query_rows()
//!     │
//!     ▼
//! MultiModelResult → format for protocol layer (PG wire, REST, Flight)
//! ```

use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, info};

use crate::query::multimodel_router::{MultiModelResult, ObservabilityResult, StoreType};

/// Multi-model SQL execution context.
///
/// Holds references to all service layers for dispatching SQL to the correct backend.
/// Created once at server startup and shared across all connections.
pub struct MultiModelExecutor {
    /// Store type for the current table/collection (resolved from catalog or SQL)
    _catalog_lookup: Option<Arc<dyn CatalogLookup>>,
}

/// Trait for looking up store type from the catalog.
pub trait CatalogLookup: Send + Sync {
    /// Look up the store type for a table/collection name.
    fn lookup_store_type(&self, table_name: &str) -> Option<StoreType>;
}

/// Result of executing a SQL statement across any data model.
#[derive(Debug)]
pub struct SqlExecutionResult {
    /// The model-specific result
    pub result: MultiModelResult,
    /// Execution time in milliseconds
    pub execution_time_ms: u64,
    /// Store type that was used
    pub store_type: StoreType,
}

/// Describes how a SQL statement was lowered for execution.
///
/// This is the intermediate representation between SQL AST and service layer calls.
/// Each variant maps to a specific service layer method.
#[derive(Debug)]
pub enum SqlPlan {
    // -- DDL --
    CreateTable {
        store_type: StoreType,
        table_name: String,
        columns: Vec<(String, String)>, // (name, type)
    },
    DropTable {
        table_name: String,
    },

    // -- Vector --
    VectorSearch {
        collection: String,
        query_vector: Vec<f32>,
        top_k: usize,
        filters: Option<String>,
    },

    // -- Document --
    DocumentQuery {
        collection: String,
        filter: Option<serde_json::Value>,
        projection: Vec<String>,
        limit: Option<u32>,
        offset: u32,
    },
    DocumentInsert {
        collection: String,
        document: serde_json::Value,
    },
    DocumentUpdate {
        collection: String,
        id: String,
        updates: serde_json::Value,
    },
    DocumentDelete {
        collection: String,
        id: String,
    },

    // -- Graph --
    GraphNodeQuery {
        graph: String,
        label_filter: Option<String>,
        property_filter: Option<serde_json::Value>,
        limit: Option<u32>,
    },
    GraphEdgeQuery {
        graph: String,
        edge_type: Option<String>,
        source_filter: Option<String>,
        limit: Option<u32>,
    },
    GraphInsertNode {
        graph: String,
        labels: Vec<String>,
        properties: serde_json::Value,
    },
    GraphInsertEdge {
        graph: String,
        source: String,
        target: String,
        edge_type: String,
        properties: serde_json::Value,
    },

    // -- Observability --
    LogQuery {
        namespace: String,
        start_ns: i64,
        end_ns: i64,
        severity: Option<i32>,
        text_filter: Option<String>,
        limit: Option<u32>,
    },
    MetricQuery {
        namespace: String,
        metric_name: String,
        labels: Vec<(String, String)>,
        start_ns: i64,
        end_ns: i64,
    },
    MetricAggregate {
        namespace: String,
        metric_name: String,
        aggregation: String, // avg, sum, min, max, count, rate
        start_ns: i64,
        end_ns: i64,
    },
    TraceQuery {
        namespace: String,
        trace_id: Option<String>,
        service: Option<String>,
        start_ns: i64,
        end_ns: i64,
    },

    // -- Relational --
    RelationalQuery {
        table: String,
        columns: Vec<String>,
        filter: Option<String>,
        order_by: Vec<(String, bool)>,
        limit: Option<u32>,
        offset: u32,
    },
    RelationalInsert {
        table: String,
        columns: Vec<String>,
        values: Vec<Vec<serde_json::Value>>,
    },
    RelationalUpdate {
        table: String,
        updates: Vec<(String, serde_json::Value)>,
        filter: Option<String>,
    },
    RelationalDelete {
        table: String,
        filter: Option<String>,
    },

    // -- Aggregation (cross-model) --
    Aggregate {
        store_type: StoreType,
        table: String,
        group_by: Vec<String>,
        aggregations: Vec<(String, String, String)>, // (function, column, alias)
        filter: Option<String>,
    },
}

/// Lower a SQL statement to a model-specific execution plan.
///
/// This is the central dispatch point that bridges SQL parsing to service layer execution.
/// It uses `StoreType` detection to determine which service layer should handle the query.
pub fn lower_sql_to_plan(
    sql: &str,
    table_name: &str,
    store_type: StoreType,
) -> Result<SqlPlan> {
    let upper = sql.trim().to_uppercase();

    // DDL
    if upper.starts_with("CREATE TABLE") || upper.starts_with("CREATE TABLE IF NOT EXISTS") {
        return Ok(SqlPlan::CreateTable {
            store_type,
            table_name: table_name.to_string(),
            columns: vec![], // Parsed by PG wire protocol
        });
    }
    if upper.starts_with("DROP TABLE") {
        return Ok(SqlPlan::DropTable {
            table_name: table_name.to_string(),
        });
    }

    // Route by store type
    match store_type {
        StoreType::Vector => {
            if upper.starts_with("SELECT") {
                Ok(SqlPlan::VectorSearch {
                    collection: table_name.to_string(),
                    query_vector: vec![], // Parsed from WHERE clause
                    top_k: 10,
                    filters: None,
                })
            } else {
                Err(anyhow::anyhow!("Unsupported vector SQL: {}", &sql[..sql.len().min(50)]))
            }
        }
        StoreType::Document => {
            if upper.starts_with("SELECT") {
                Ok(SqlPlan::DocumentQuery {
                    collection: table_name.to_string(),
                    filter: None, // Parsed from WHERE clause
                    projection: vec![],
                    limit: None,
                    offset: 0,
                })
            } else if upper.starts_with("INSERT") {
                Ok(SqlPlan::DocumentInsert {
                    collection: table_name.to_string(),
                    document: serde_json::Value::Null,
                })
            } else if upper.starts_with("DELETE") {
                Ok(SqlPlan::DocumentDelete {
                    collection: table_name.to_string(),
                    id: String::new(),
                })
            } else {
                Err(anyhow::anyhow!("Unsupported document SQL: {}", &sql[..sql.len().min(50)]))
            }
        }
        StoreType::Graph => {
            if upper.starts_with("SELECT") {
                let is_edge = table_name.starts_with("edge_");
                if is_edge {
                    Ok(SqlPlan::GraphEdgeQuery {
                        graph: table_name.trim_start_matches("edge_").to_string(),
                        edge_type: None,
                        source_filter: None,
                        limit: None,
                    })
                } else {
                    let graph = table_name
                        .trim_start_matches("graph_")
                        .trim_start_matches("node_")
                        .to_string();
                    Ok(SqlPlan::GraphNodeQuery {
                        graph,
                        label_filter: None,
                        property_filter: None,
                        limit: None,
                    })
                }
            } else if upper.starts_with("INSERT") {
                let is_edge = table_name.starts_with("edge_");
                if is_edge {
                    Ok(SqlPlan::GraphInsertEdge {
                        graph: table_name.trim_start_matches("edge_").to_string(),
                        source: String::new(),
                        target: String::new(),
                        edge_type: String::new(),
                        properties: serde_json::json!({}),
                    })
                } else {
                    Ok(SqlPlan::GraphInsertNode {
                        graph: table_name
                            .trim_start_matches("graph_")
                            .trim_start_matches("node_")
                            .to_string(),
                        labels: vec![],
                        properties: serde_json::json!({}),
                    })
                }
            } else {
                Err(anyhow::anyhow!("Unsupported graph SQL: {}", &sql[..sql.len().min(50)]))
            }
        }
        StoreType::Observability => {
            let namespace = table_name
                .trim_start_matches("log_")
                .trim_start_matches("metric_")
                .trim_start_matches("trace_")
                .to_string();

            if table_name.starts_with("trace_") {
                Ok(SqlPlan::TraceQuery {
                    namespace,
                    trace_id: None,
                    service: None,
                    start_ns: 0,
                    end_ns: i64::MAX,
                })
            } else if table_name.starts_with("metric_") {
                // Check for aggregation functions
                if upper.contains("AVG(") || upper.contains("SUM(") || upper.contains("COUNT(")
                    || upper.contains("MIN(") || upper.contains("MAX(") || upper.contains("RATE(")
                {
                    Ok(SqlPlan::MetricAggregate {
                        namespace,
                        metric_name: String::new(),
                        aggregation: String::new(),
                        start_ns: 0,
                        end_ns: i64::MAX,
                    })
                } else {
                    Ok(SqlPlan::MetricQuery {
                        namespace,
                        metric_name: String::new(),
                        labels: vec![],
                        start_ns: 0,
                        end_ns: i64::MAX,
                    })
                }
            } else {
                Ok(SqlPlan::LogQuery {
                    namespace,
                    start_ns: 0,
                    end_ns: i64::MAX,
                    severity: None,
                    text_filter: None,
                    limit: None,
                })
            }
        }
        StoreType::Relational => {
            if upper.starts_with("SELECT") {
                // Check for aggregation
                if upper.contains("AVG(") || upper.contains("SUM(") || upper.contains("COUNT(")
                    || upper.contains("MIN(") || upper.contains("MAX(")
                {
                    Ok(SqlPlan::Aggregate {
                        store_type: StoreType::Relational,
                        table: table_name.to_string(),
                        group_by: vec![],
                        aggregations: vec![],
                        filter: None,
                    })
                } else {
                    Ok(SqlPlan::RelationalQuery {
                        table: table_name.to_string(),
                        columns: vec![],
                        filter: None,
                        order_by: vec![],
                        limit: None,
                        offset: 0,
                    })
                }
            } else if upper.starts_with("INSERT") {
                Ok(SqlPlan::RelationalInsert {
                    table: table_name.to_string(),
                    columns: vec![],
                    values: vec![],
                })
            } else if upper.starts_with("UPDATE") {
                Ok(SqlPlan::RelationalUpdate {
                    table: table_name.to_string(),
                    updates: vec![],
                    filter: None,
                })
            } else if upper.starts_with("DELETE") {
                Ok(SqlPlan::RelationalDelete {
                    table: table_name.to_string(),
                    filter: None,
                })
            } else {
                Err(anyhow::anyhow!("Unsupported relational SQL: {}", &sql[..sql.len().min(50)]))
            }
        }
        StoreType::TimeSeries => {
            // TST engine handles financial time-series via the vector path
            Ok(SqlPlan::VectorSearch {
                collection: table_name.to_string(),
                query_vector: vec![],
                top_k: 100,
                filters: None,
            })
        }
        StoreType::Event => {
            // EventLog engine handles append-only audit logs
            Ok(SqlPlan::RelationalQuery {
                table: table_name.to_string(),
                columns: vec![],
                filter: None,
                order_by: vec![],
                limit: None,
                offset: 0,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lower_create_table_relational() {
        let plan = lower_sql_to_plan(
            "CREATE TABLE users (id INT, name VARCHAR(255))",
            "users",
            StoreType::Relational,
        )
        .unwrap();
        assert!(matches!(plan, SqlPlan::CreateTable { store_type: StoreType::Relational, .. }));
    }

    #[test]
    fn test_lower_select_relational() {
        let plan = lower_sql_to_plan(
            "SELECT * FROM users WHERE id > 5",
            "users",
            StoreType::Relational,
        )
        .unwrap();
        assert!(matches!(plan, SqlPlan::RelationalQuery { .. }));
    }

    #[test]
    fn test_lower_insert_relational() {
        let plan = lower_sql_to_plan(
            "INSERT INTO users (id, name) VALUES (1, 'Alice')",
            "users",
            StoreType::Relational,
        )
        .unwrap();
        assert!(matches!(plan, SqlPlan::RelationalInsert { .. }));
    }

    #[test]
    fn test_lower_select_document() {
        let plan = lower_sql_to_plan(
            "SELECT * FROM doc_users WHERE $.age > 25",
            "doc_users",
            StoreType::Document,
        )
        .unwrap();
        assert!(matches!(plan, SqlPlan::DocumentQuery { .. }));
    }

    #[test]
    fn test_lower_select_graph_nodes() {
        let plan = lower_sql_to_plan(
            "SELECT * FROM graph_social WHERE label = 'Person'",
            "graph_social",
            StoreType::Graph,
        )
        .unwrap();
        assert!(matches!(plan, SqlPlan::GraphNodeQuery { .. }));
    }

    #[test]
    fn test_lower_select_graph_edges() {
        let plan = lower_sql_to_plan(
            "SELECT * FROM edge_social WHERE edge_type = 'KNOWS'",
            "edge_social",
            StoreType::Graph,
        )
        .unwrap();
        assert!(matches!(plan, SqlPlan::GraphEdgeQuery { .. }));
    }

    #[test]
    fn test_lower_select_logs() {
        let plan = lower_sql_to_plan(
            "SELECT * FROM log_app WHERE severity >= 4",
            "log_app",
            StoreType::Observability,
        )
        .unwrap();
        assert!(matches!(plan, SqlPlan::LogQuery { .. }));
    }

    #[test]
    fn test_lower_select_metrics() {
        let plan = lower_sql_to_plan(
            "SELECT * FROM metric_cpu WHERE name = 'cpu_usage'",
            "metric_cpu",
            StoreType::Observability,
        )
        .unwrap();
        assert!(matches!(plan, SqlPlan::MetricQuery { .. }));
    }

    #[test]
    fn test_lower_select_traces() {
        let plan = lower_sql_to_plan(
            "SELECT * FROM trace_app WHERE service = 'api'",
            "trace_app",
            StoreType::Observability,
        )
        .unwrap();
        assert!(matches!(plan, SqlPlan::TraceQuery { .. }));
    }

    #[test]
    fn test_lower_select_metric_aggregate() {
        let plan = lower_sql_to_plan(
            "SELECT AVG(value) FROM metric_cpu WHERE name = 'cpu'",
            "metric_cpu",
            StoreType::Observability,
        )
        .unwrap();
        assert!(matches!(plan, SqlPlan::MetricAggregate { .. }));
    }

    #[test]
    fn test_lower_select_relational_aggregate() {
        let plan = lower_sql_to_plan(
            "SELECT COUNT(*) FROM users",
            "users",
            StoreType::Relational,
        )
        .unwrap();
        assert!(matches!(plan, SqlPlan::Aggregate { store_type: StoreType::Relational, .. }));
    }

    #[test]
    fn test_lower_vector_search() {
        let plan = lower_sql_to_plan(
            "SELECT * FROM embeddings ORDER BY vec <-> '[0.1, 0.2]' LIMIT 10",
            "embeddings",
            StoreType::Vector,
        )
        .unwrap();
        assert!(matches!(plan, SqlPlan::VectorSearch { .. }));
    }

    #[test]
    fn test_lower_insert_graph_node() {
        let plan = lower_sql_to_plan(
            "INSERT INTO graph_social (id, label, name) VALUES ('n1', 'Person', 'Alice')",
            "graph_social",
            StoreType::Graph,
        )
        .unwrap();
        assert!(matches!(plan, SqlPlan::GraphInsertNode { .. }));
    }

    #[test]
    fn test_lower_insert_graph_edge() {
        let plan = lower_sql_to_plan(
            "INSERT INTO edge_social (from_id, to_id, edge_type) VALUES ('n1', 'n2', 'KNOWS')",
            "edge_social",
            StoreType::Graph,
        )
        .unwrap();
        assert!(matches!(plan, SqlPlan::GraphInsertEdge { .. }));
    }

    #[test]
    fn test_lower_drop_table() {
        let plan = lower_sql_to_plan(
            "DROP TABLE users",
            "users",
            StoreType::Relational,
        )
        .unwrap();
        assert!(matches!(plan, SqlPlan::DropTable { .. }));
    }

    #[test]
    fn test_lower_delete_relational() {
        let plan = lower_sql_to_plan(
            "DELETE FROM users WHERE id = 5",
            "users",
            StoreType::Relational,
        )
        .unwrap();
        assert!(matches!(plan, SqlPlan::RelationalDelete { .. }));
    }

    #[test]
    fn test_lower_update_relational() {
        let plan = lower_sql_to_plan(
            "UPDATE users SET name = 'Bob' WHERE id = 1",
            "users",
            StoreType::Relational,
        )
        .unwrap();
        assert!(matches!(plan, SqlPlan::RelationalUpdate { .. }));
    }
}
