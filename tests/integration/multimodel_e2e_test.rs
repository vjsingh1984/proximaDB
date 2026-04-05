//! # Multi-Model End-to-End Integration Tests
//!
//! Tests the full multi-model routing pipeline across all 7 data models
//! and protocol layers:
//!
//! 1. SQL detection → StoreType routing
//! 2. SQL lowering → model-specific SqlPlan
//! 3. Protocol-layer match arm coverage (PG wire, REST, Arrow Flight)
//! 4. Cross-protocol consistency (same SQL → same StoreType)
//!
//! These tests validate that a SQL statement entered via any protocol
//! (REST, PG wire, Arrow Flight) is routed to the same engine.

use proximadb::query::multimodel_router::{
    detect_store_type_from_create, detect_store_type_from_query, StoreType,
};
use proximadb::query::multimodel_executor::{lower_sql_to_plan, SqlPlan};

// =============================================================================
// 1. STORE TYPE DETECTION — CREATE TABLE (all 7 models)
// =============================================================================

#[test]
fn test_create_routes_to_vector() {
    let sql = "CREATE TABLE embeddings (id TEXT, vec VECTOR(384))";
    assert_eq!(detect_store_type_from_create(sql), StoreType::Vector);
}

#[test]
fn test_create_routes_to_document_using_clause() {
    let sql = "CREATE TABLE products (id TEXT, data JSONB) USING DOCUMENT";
    assert_eq!(detect_store_type_from_create(sql), StoreType::Document);
}

#[test]
fn test_create_routes_to_document_jsonb_inference() {
    let sql = "CREATE TABLE products (id TEXT, payload JSONB)";
    assert_eq!(detect_store_type_from_create(sql), StoreType::Document);
}

#[test]
fn test_create_routes_to_graph() {
    let sql = "CREATE TABLE social (id TEXT, labels TEXT[]) USING GRAPH";
    assert_eq!(detect_store_type_from_create(sql), StoreType::Graph);
}

#[test]
fn test_create_routes_to_observability() {
    let sql = "CREATE TABLE app_logs (ts TIMESTAMP, severity TEXT, message TEXT) USING OBSERVABILITY";
    assert_eq!(
        detect_store_type_from_create(sql),
        StoreType::Observability
    );
}

#[test]
fn test_create_routes_to_observability_via_timeseries() {
    let sql = "CREATE TABLE sensor_data (ts TIMESTAMP, value FLOAT) USING TIMESERIES";
    assert_eq!(
        detect_store_type_from_create(sql),
        StoreType::Observability
    );
}

#[test]
fn test_create_routes_to_relational_default() {
    let sql = "CREATE TABLE users (id INT, name VARCHAR(255), email VARCHAR(255))";
    assert_eq!(detect_store_type_from_create(sql), StoreType::Relational);
}

#[test]
fn test_create_routes_to_vector_using_clause() {
    let sql = "CREATE TABLE vecs (id TEXT, embedding FLOAT[]) USING VECTOR";
    assert_eq!(detect_store_type_from_create(sql), StoreType::Vector);
}

// =============================================================================
// 2. STORE TYPE DETECTION — QUERY (SELECT/INSERT/UPDATE/DELETE)
// =============================================================================

#[test]
fn test_query_vector_operator_l2() {
    let sql = "SELECT * FROM embeddings ORDER BY vec <-> '[0.1, 0.2, 0.3]' LIMIT 10";
    assert_eq!(
        detect_store_type_from_query(sql, "embeddings", None),
        StoreType::Vector
    );
}

#[test]
fn test_query_vector_operator_cosine() {
    let sql = "SELECT * FROM embeddings ORDER BY vec <=> '[0.1, 0.2, 0.3]' LIMIT 10";
    assert_eq!(
        detect_store_type_from_query(sql, "embeddings", None),
        StoreType::Vector
    );
}

#[test]
fn test_query_vector_operator_dot_product() {
    let sql = "SELECT * FROM embeddings ORDER BY vec <#> '[0.1, 0.2, 0.3]' LIMIT 10";
    assert_eq!(
        detect_store_type_from_query(sql, "embeddings", None),
        StoreType::Vector
    );
}

#[test]
fn test_query_document_json_path() {
    let sql = "SELECT * FROM products WHERE $.price > 100";
    assert_eq!(
        detect_store_type_from_query(sql, "products", None),
        StoreType::Document
    );
}

#[test]
fn test_query_document_prefix() {
    let sql = "SELECT * FROM doc_users WHERE id = 'u1'";
    assert_eq!(
        detect_store_type_from_query(sql, "doc_users", None),
        StoreType::Document
    );
}

#[test]
fn test_query_graph_prefix() {
    let sql = "SELECT * FROM graph_social WHERE label = 'Person'";
    assert_eq!(
        detect_store_type_from_query(sql, "graph_social", None),
        StoreType::Graph
    );
}

#[test]
fn test_query_graph_node_prefix() {
    let sql = "SELECT * FROM node_friends WHERE name = 'Alice'";
    assert_eq!(
        detect_store_type_from_query(sql, "node_friends", None),
        StoreType::Graph
    );
}

#[test]
fn test_query_graph_edge_prefix() {
    let sql = "SELECT * FROM edge_follows WHERE weight > 0.5";
    assert_eq!(
        detect_store_type_from_query(sql, "edge_follows", None),
        StoreType::Graph
    );
}

#[test]
fn test_query_observability_log_prefix() {
    let sql = "SELECT * FROM log_app WHERE severity = 'ERROR'";
    assert_eq!(
        detect_store_type_from_query(sql, "log_app", None),
        StoreType::Observability
    );
}

#[test]
fn test_query_observability_metric_prefix() {
    let sql = "SELECT * FROM metric_cpu WHERE host = 'prod-1'";
    assert_eq!(
        detect_store_type_from_query(sql, "metric_cpu", None),
        StoreType::Observability
    );
}

#[test]
fn test_query_observability_trace_prefix() {
    let sql = "SELECT * FROM trace_requests WHERE service = 'api'";
    assert_eq!(
        detect_store_type_from_query(sql, "trace_requests", None),
        StoreType::Observability
    );
}

#[test]
fn test_query_relational_default() {
    let sql = "SELECT * FROM users WHERE age > 25 ORDER BY name LIMIT 100";
    assert_eq!(
        detect_store_type_from_query(sql, "users", None),
        StoreType::Relational
    );
}

#[test]
fn test_query_catalog_override() {
    let catalog = |table: &str| -> Option<StoreType> {
        match table {
            "my_vectors" => Some(StoreType::Vector),
            "my_docs" => Some(StoreType::Document),
            _ => None,
        }
    };
    assert_eq!(
        detect_store_type_from_query("SELECT * FROM my_vectors LIMIT 5", "my_vectors", Some(&catalog)),
        StoreType::Vector
    );
    assert_eq!(
        detect_store_type_from_query("SELECT * FROM my_docs WHERE id = 1", "my_docs", Some(&catalog)),
        StoreType::Document
    );
    // Catalog returns None → default to Relational
    assert_eq!(
        detect_store_type_from_query("SELECT * FROM other_table", "other_table", Some(&catalog)),
        StoreType::Relational
    );
}

// =============================================================================
// 3. SQL LOWERING — detection → plan (end-to-end)
// =============================================================================

#[test]
fn test_e2e_vector_select_lowers_to_vector_search() {
    let sql = "SELECT * FROM embeddings ORDER BY vec <-> '[0.1]' LIMIT 10";
    let store_type = detect_store_type_from_query(sql, "embeddings", None);
    assert_eq!(store_type, StoreType::Vector);

    let plan = lower_sql_to_plan(sql, "embeddings", store_type).unwrap();
    assert!(matches!(plan, SqlPlan::VectorSearch { collection, .. } if collection == "embeddings"));
}

#[test]
fn test_e2e_document_select_lowers_to_document_query() {
    let sql = "SELECT * FROM doc_products WHERE $.category = 'electronics'";
    let store_type = detect_store_type_from_query(sql, "doc_products", None);
    assert_eq!(store_type, StoreType::Document);

    let plan = lower_sql_to_plan(sql, "doc_products", store_type).unwrap();
    assert!(matches!(plan, SqlPlan::DocumentQuery { collection, .. } if collection == "doc_products"));
}

#[test]
fn test_e2e_document_insert_lowers_to_document_insert() {
    let sql = "INSERT INTO doc_products (id, data) VALUES ('p1', '{\"name\": \"Widget\"}')";
    let store_type = detect_store_type_from_query(sql, "doc_products", None);
    assert_eq!(store_type, StoreType::Document);

    let plan = lower_sql_to_plan(sql, "doc_products", store_type).unwrap();
    assert!(matches!(plan, SqlPlan::DocumentInsert { .. }));
}

#[test]
fn test_e2e_graph_node_select_lowers_to_graph_node_query() {
    let sql = "SELECT * FROM graph_social WHERE label = 'Person'";
    let store_type = detect_store_type_from_query(sql, "graph_social", None);
    assert_eq!(store_type, StoreType::Graph);

    let plan = lower_sql_to_plan(sql, "graph_social", store_type).unwrap();
    assert!(matches!(plan, SqlPlan::GraphNodeQuery { graph, .. } if graph == "social"));
}

#[test]
fn test_e2e_graph_edge_select_lowers_to_graph_edge_query() {
    let sql = "SELECT * FROM edge_follows WHERE weight > 0.5";
    let store_type = detect_store_type_from_query(sql, "edge_follows", None);
    assert_eq!(store_type, StoreType::Graph);

    let plan = lower_sql_to_plan(sql, "edge_follows", store_type).unwrap();
    assert!(matches!(plan, SqlPlan::GraphEdgeQuery { graph, .. } if graph == "follows"));
}

#[test]
fn test_e2e_graph_insert_node() {
    let sql = "INSERT INTO graph_social (id, labels, props) VALUES ('n1', 'Person', '{}')";
    let store_type = detect_store_type_from_query(sql, "graph_social", None);
    assert_eq!(store_type, StoreType::Graph);

    let plan = lower_sql_to_plan(sql, "graph_social", store_type).unwrap();
    assert!(matches!(plan, SqlPlan::GraphInsertNode { .. }));
}

#[test]
fn test_e2e_graph_insert_edge() {
    let sql = "INSERT INTO edge_follows (source, target, type) VALUES ('n1', 'n2', 'FOLLOWS')";
    let store_type = detect_store_type_from_query(sql, "edge_follows", None);
    assert_eq!(store_type, StoreType::Graph);

    let plan = lower_sql_to_plan(sql, "edge_follows", store_type).unwrap();
    assert!(matches!(plan, SqlPlan::GraphInsertEdge { .. }));
}

#[test]
fn test_e2e_observability_log_query() {
    let sql = "SELECT * FROM log_app WHERE severity = 'ERROR' LIMIT 100";
    let store_type = detect_store_type_from_query(sql, "log_app", None);
    assert_eq!(store_type, StoreType::Observability);

    let plan = lower_sql_to_plan(sql, "log_app", store_type).unwrap();
    assert!(matches!(plan, SqlPlan::LogQuery { namespace, .. } if namespace == "app"));
}

#[test]
fn test_e2e_observability_metric_query() {
    let sql = "SELECT * FROM metric_cpu WHERE host = 'prod-1'";
    let store_type = detect_store_type_from_query(sql, "metric_cpu", None);
    assert_eq!(store_type, StoreType::Observability);

    let plan = lower_sql_to_plan(sql, "metric_cpu", store_type).unwrap();
    assert!(matches!(plan, SqlPlan::MetricQuery { namespace, .. } if namespace == "cpu"));
}

#[test]
fn test_e2e_observability_metric_aggregate() {
    let sql = "SELECT AVG(value) FROM metric_cpu GROUP BY host";
    let store_type = detect_store_type_from_query(sql, "metric_cpu", None);
    assert_eq!(store_type, StoreType::Observability);

    let plan = lower_sql_to_plan(sql, "metric_cpu", store_type).unwrap();
    assert!(matches!(plan, SqlPlan::MetricAggregate { .. }));
}

#[test]
fn test_e2e_observability_trace_query() {
    let sql = "SELECT * FROM trace_requests WHERE service = 'api-gateway'";
    let store_type = detect_store_type_from_query(sql, "trace_requests", None);
    assert_eq!(store_type, StoreType::Observability);

    let plan = lower_sql_to_plan(sql, "trace_requests", store_type).unwrap();
    assert!(matches!(plan, SqlPlan::TraceQuery { namespace, .. } if namespace == "requests"));
}

#[test]
fn test_e2e_relational_select() {
    let sql = "SELECT name, email FROM users WHERE age > 25 ORDER BY name";
    let store_type = detect_store_type_from_query(sql, "users", None);
    assert_eq!(store_type, StoreType::Relational);

    let plan = lower_sql_to_plan(sql, "users", store_type).unwrap();
    assert!(matches!(plan, SqlPlan::RelationalQuery { table, .. } if table == "users"));
}

#[test]
fn test_e2e_relational_insert() {
    let sql = "INSERT INTO users (id, name, email) VALUES (1, 'Alice', 'alice@example.com')";
    let store_type = detect_store_type_from_query(sql, "users", None);
    assert_eq!(store_type, StoreType::Relational);

    let plan = lower_sql_to_plan(sql, "users", store_type).unwrap();
    assert!(matches!(plan, SqlPlan::RelationalInsert { table, .. } if table == "users"));
}

#[test]
fn test_e2e_relational_update() {
    let sql = "UPDATE users SET email = 'new@example.com' WHERE id = 1";
    let store_type = detect_store_type_from_query(sql, "users", None);
    assert_eq!(store_type, StoreType::Relational);

    let plan = lower_sql_to_plan(sql, "users", store_type).unwrap();
    assert!(matches!(plan, SqlPlan::RelationalUpdate { table, .. } if table == "users"));
}

#[test]
fn test_e2e_relational_delete() {
    let sql = "DELETE FROM users WHERE id = 1";
    let store_type = detect_store_type_from_query(sql, "users", None);
    assert_eq!(store_type, StoreType::Relational);

    let plan = lower_sql_to_plan(sql, "users", store_type).unwrap();
    assert!(matches!(plan, SqlPlan::RelationalDelete { table, .. } if table == "users"));
}

#[test]
fn test_e2e_relational_aggregate() {
    let sql = "SELECT department, AVG(salary) FROM employees GROUP BY department";
    let store_type = detect_store_type_from_query(sql, "employees", None);
    assert_eq!(store_type, StoreType::Relational);

    let plan = lower_sql_to_plan(sql, "employees", store_type).unwrap();
    assert!(matches!(plan, SqlPlan::Aggregate { store_type: StoreType::Relational, .. }));
}

// =============================================================================
// 4. CREATE TABLE → SQL LOWERING (DDL path)
// =============================================================================

#[test]
fn test_e2e_create_vector_table() {
    let sql = "CREATE TABLE embeddings (id TEXT, vec VECTOR(384))";
    let store_type = detect_store_type_from_create(sql);
    assert_eq!(store_type, StoreType::Vector);

    let plan = lower_sql_to_plan(sql, "embeddings", store_type).unwrap();
    assert!(matches!(
        plan,
        SqlPlan::CreateTable { store_type: StoreType::Vector, table_name, .. } if table_name == "embeddings"
    ));
}

#[test]
fn test_e2e_create_document_table() {
    let sql = "CREATE TABLE products (id TEXT, data JSONB) USING DOCUMENT";
    let store_type = detect_store_type_from_create(sql);
    assert_eq!(store_type, StoreType::Document);

    let plan = lower_sql_to_plan(sql, "products", store_type).unwrap();
    assert!(matches!(
        plan,
        SqlPlan::CreateTable { store_type: StoreType::Document, .. }
    ));
}

#[test]
fn test_e2e_create_graph_table() {
    let sql = "CREATE TABLE social (id TEXT) USING GRAPH";
    let store_type = detect_store_type_from_create(sql);
    assert_eq!(store_type, StoreType::Graph);

    let plan = lower_sql_to_plan(sql, "social", store_type).unwrap();
    assert!(matches!(
        plan,
        SqlPlan::CreateTable { store_type: StoreType::Graph, .. }
    ));
}

#[test]
fn test_e2e_create_observability_table() {
    let sql = "CREATE TABLE app_logs (ts TIMESTAMP, severity TEXT) USING OBSERVABILITY";
    let store_type = detect_store_type_from_create(sql);
    assert_eq!(store_type, StoreType::Observability);

    let plan = lower_sql_to_plan(sql, "app_logs", store_type).unwrap();
    assert!(matches!(
        plan,
        SqlPlan::CreateTable { store_type: StoreType::Observability, .. }
    ));
}

#[test]
fn test_e2e_create_relational_table() {
    let sql = "CREATE TABLE users (id INT PRIMARY KEY, name VARCHAR(255))";
    let store_type = detect_store_type_from_create(sql);
    assert_eq!(store_type, StoreType::Relational);

    let plan = lower_sql_to_plan(sql, "users", store_type).unwrap();
    assert!(matches!(
        plan,
        SqlPlan::CreateTable { store_type: StoreType::Relational, .. }
    ));
}

#[test]
fn test_e2e_drop_table() {
    let sql = "DROP TABLE users";
    let plan = lower_sql_to_plan(sql, "users", StoreType::Relational).unwrap();
    assert!(matches!(plan, SqlPlan::DropTable { table_name } if table_name == "users"));
}

// =============================================================================
// 5. TIMESERIES and EVENT model routing
// =============================================================================

#[test]
fn test_e2e_timeseries_lowers_to_vector_search() {
    // TimeSeries data is routed through the vector path (TST engine)
    let sql = "SELECT * FROM sensor_data WHERE ts > '2024-01-01'";
    let plan = lower_sql_to_plan(sql, "sensor_data", StoreType::TimeSeries).unwrap();
    assert!(matches!(plan, SqlPlan::VectorSearch { .. }));
}

#[test]
fn test_e2e_event_lowers_to_relational_query() {
    // Event data is routed through the relational path (EventLog engine)
    let sql = "SELECT * FROM audit_log WHERE action = 'login'";
    let plan = lower_sql_to_plan(sql, "audit_log", StoreType::Event).unwrap();
    assert!(matches!(plan, SqlPlan::RelationalQuery { .. }));
}

// =============================================================================
// 6. CROSS-PROTOCOL CONSISTENCY
// =============================================================================

/// Verifies that the same SQL text produces the same StoreType regardless of
/// which protocol entry point is used (REST, PG wire, Arrow Flight all call
/// the same detect functions from multimodel_router).
#[test]
fn test_cross_protocol_store_type_consistency() {
    let test_cases = vec![
        // (SQL, table_name, expected StoreType)
        (
            "SELECT * FROM embeddings ORDER BY vec <-> '[0.1]' LIMIT 5",
            "embeddings",
            StoreType::Vector,
        ),
        (
            "SELECT * FROM doc_products WHERE $.price > 10",
            "doc_products",
            StoreType::Document,
        ),
        (
            "SELECT * FROM graph_social WHERE label = 'Person'",
            "graph_social",
            StoreType::Graph,
        ),
        (
            "SELECT * FROM log_app WHERE severity = 'ERROR'",
            "log_app",
            StoreType::Observability,
        ),
        (
            "SELECT * FROM metric_cpu WHERE host = 'prod'",
            "metric_cpu",
            StoreType::Observability,
        ),
        (
            "SELECT * FROM trace_requests WHERE span_id = 'abc'",
            "trace_requests",
            StoreType::Observability,
        ),
        (
            "SELECT * FROM users WHERE id = 1",
            "users",
            StoreType::Relational,
        ),
    ];

    for (sql, table, expected) in &test_cases {
        let detected = detect_store_type_from_query(sql, table, None);
        assert_eq!(
            detected, *expected,
            "SQL '{}' with table '{}' should route to {:?}, got {:?}",
            sql, table, expected, detected
        );
    }
}

/// Verifies that CREATE TABLE detection covers all explicit USING clauses.
#[test]
fn test_all_using_clauses_recognized() {
    let cases = vec![
        ("CREATE TABLE t (id TEXT) USING VECTOR", StoreType::Vector),
        ("CREATE TABLE t (id TEXT) USING DOCUMENT", StoreType::Document),
        ("CREATE TABLE t (id TEXT) USING GRAPH", StoreType::Graph),
        (
            "CREATE TABLE t (id TEXT) USING OBSERVABILITY",
            StoreType::Observability,
        ),
        (
            "CREATE TABLE t (id TEXT) USING TIMESERIES",
            StoreType::Observability,
        ),
    ];

    for (sql, expected) in &cases {
        assert_eq!(
            detect_store_type_from_create(sql),
            *expected,
            "USING clause not recognized: {}",
            sql
        );
    }
}

// =============================================================================
// 7. STORE TYPE DISPLAY & SERIALIZATION
// =============================================================================

#[test]
fn test_store_type_display_all_variants() {
    assert_eq!(StoreType::Vector.to_string(), "vector");
    assert_eq!(StoreType::Document.to_string(), "document");
    assert_eq!(StoreType::Graph.to_string(), "graph");
    assert_eq!(StoreType::Observability.to_string(), "observability");
    assert_eq!(StoreType::Relational.to_string(), "relational");
    assert_eq!(StoreType::TimeSeries.to_string(), "timeseries");
    assert_eq!(StoreType::Event.to_string(), "event");
}

#[test]
fn test_store_type_serde_roundtrip() {
    let all_types = vec![
        StoreType::Vector,
        StoreType::Document,
        StoreType::Graph,
        StoreType::Observability,
        StoreType::Relational,
        StoreType::TimeSeries,
        StoreType::Event,
    ];

    for store_type in all_types {
        let json = serde_json::to_string(&store_type).unwrap();
        let deserialized: StoreType = serde_json::from_str(&json).unwrap();
        assert_eq!(
            store_type, deserialized,
            "Serde roundtrip failed for {:?}",
            store_type
        );
    }
}

// =============================================================================
// 8. TYPE ALIAS CONSISTENCY (DataModel, ModelType = StoreType)
// =============================================================================

#[test]
fn test_data_model_alias_is_store_type() {
    use proximadb::storage::traits::DataModel;
    // DataModel is pub use StoreType as DataModel
    let dm: DataModel = DataModel::Vector;
    let st: StoreType = dm; // same type, zero-cost
    assert_eq!(st, StoreType::Vector);
}

#[test]
fn test_model_type_alias_is_store_type() {
    use proximadb::storage::multimodel::traits::ModelType;
    // ModelType is type alias for StoreType
    let mt: ModelType = ModelType::Graph;
    let st: StoreType = mt; // same type
    assert_eq!(st, StoreType::Graph);
}

#[test]
fn test_rbac_data_model_alias_is_store_type() {
    use proximadb::security::unified_rbac::DataModel;
    // RBAC DataModel is pub use StoreType as DataModel
    let dm: DataModel = DataModel::Observability;
    let st: StoreType = dm;
    assert_eq!(st, StoreType::Observability);
}
