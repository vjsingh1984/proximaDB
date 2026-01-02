/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

//! # SQL Query Through Facade Integration Tests
//!
//! Tests that validate SQL and federated queries route correctly through the
//! `UnifiedQueryFacade` when the `unified-facade-routing` feature is enabled.
//!
//! ## Test Coverage
//!
//! - SQL query request conversion
//! - Federated query routing (SQL with multi-model extensions)
//! - Strategy selection for SQL queries
//! - Response format validation (JSON rows)
//! - Error handling for invalid SQL

use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;

use proximadb::query::facade::{
    ExecutionMetrics, FacadeConfig, QueryContext, QueryFacadeAdapter,
    QueryRequest, QueryResult, QueryResultData, QueryStrategy, QueryType,
    UnifiedQueryFacade, QueryContent,
};

// ================================================================================
// MOCK STRATEGIES FOR TESTING
// ================================================================================

/// Mock SQL strategy that returns predictable results
struct MockSqlStrategy {
    /// Rows to return
    rows: Vec<serde_json::Value>,
    /// Whether to simulate a parse error
    should_parse_error: bool,
    /// Whether to simulate an execution error
    should_exec_error: bool,
}

impl MockSqlStrategy {
    fn new() -> Self {
        Self {
            rows: vec![
                serde_json::json!({
                    "id": "product_1",
                    "name": "Widget A",
                    "price": 29.99,
                    "in_stock": true
                }),
                serde_json::json!({
                    "id": "product_2",
                    "name": "Widget B",
                    "price": 49.99,
                    "in_stock": false
                }),
                serde_json::json!({
                    "id": "product_3",
                    "name": "Gadget X",
                    "price": 99.99,
                    "in_stock": true
                }),
            ],
            should_parse_error: false,
            should_exec_error: false,
        }
    }

    fn with_rows(rows: Vec<serde_json::Value>) -> Self {
        Self {
            rows,
            should_parse_error: false,
            should_exec_error: false,
        }
    }

    fn with_parse_error() -> Self {
        Self {
            rows: vec![],
            should_parse_error: true,
            should_exec_error: false,
        }
    }

    fn with_exec_error() -> Self {
        Self {
            rows: vec![],
            should_parse_error: false,
            should_exec_error: true,
        }
    }
}

#[async_trait]
impl QueryStrategy for MockSqlStrategy {
    fn name(&self) -> &str {
        "mock_sql"
    }

    fn can_handle(&self, request: &QueryRequest) -> bool {
        matches!(request.query_type, QueryType::Sql | QueryType::Federated)
    }

    fn priority(&self) -> i32 {
        90
    }

    async fn execute(&self, request: QueryRequest, _ctx: &QueryContext) -> Result<QueryResult> {
        // Extract SQL from request
        let sql = match &request.content {
            QueryContent::Sql(query) => query.clone(),
            _ => return Err(anyhow::anyhow!("Expected SQL content")),
        };

        // Simulate parse error
        if self.should_parse_error {
            return Err(anyhow::anyhow!("SQL parse error: syntax error near 'SELEC'"));
        }

        // Simulate execution error
        if self.should_exec_error {
            return Err(anyhow::anyhow!("SQL execution error: table 'unknown' not found"));
        }

        // Simulate LIMIT clause
        let limit = Self::extract_limit(&sql).unwrap_or(self.rows.len());
        let limited_rows: Vec<serde_json::Value> = self.rows
            .iter()
            .take(limit)
            .cloned()
            .collect();

        Ok(QueryResult {
            data: QueryResultData::Rows(limited_rows.clone()),
            metrics: Some(ExecutionMetrics {
                execution_path: "unified".to_string(), // Unified facade path
                strategy_name: "mock_sql".to_string(),
                execution_time_ms: 10,
                planning_time_ms: 2,
                results_scanned: self.rows.len(),
                results_returned: limited_rows.len(),
                cache_hit: false,
                extra: serde_json::json!({
                    "mock": true,
                    "engine": "MockSqlEngine",
                    "sql": sql,
                    "is_federated": request.query_type == QueryType::Federated
                }),
            }),
        })
    }
}

impl MockSqlStrategy {
    /// Extract LIMIT value from SQL query
    fn extract_limit(sql: &str) -> Option<usize> {
        let upper = sql.to_uppercase();
        if let Some(pos) = upper.find("LIMIT ") {
            let rest = &sql[pos + 6..];
            let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            num_str.parse().ok()
        } else {
            None
        }
    }
}

// ================================================================================
// TEST UTILITIES
// ================================================================================

/// Create a test facade with mock SQL strategy
fn create_sql_facade() -> UnifiedQueryFacade {
    let strategies: Vec<Arc<dyn QueryStrategy>> = vec![
        Arc::new(MockSqlStrategy::new()),
    ];
    UnifiedQueryFacade::new(strategies, FacadeConfig::default())
}

/// Create a test facade adapter
fn create_sql_adapter() -> QueryFacadeAdapter {
    let facade = Arc::new(create_sql_facade());
    QueryFacadeAdapter::new(facade)
}

/// Create a facade with custom rows
fn create_facade_with_rows(rows: Vec<serde_json::Value>) -> QueryFacadeAdapter {
    let strategies: Vec<Arc<dyn QueryStrategy>> = vec![
        Arc::new(MockSqlStrategy::with_rows(rows)),
    ];
    let facade = Arc::new(UnifiedQueryFacade::new(strategies, FacadeConfig::default()));
    QueryFacadeAdapter::new(facade)
}

/// Create a facade that simulates parse errors
fn create_parse_error_facade() -> QueryFacadeAdapter {
    let strategies: Vec<Arc<dyn QueryStrategy>> = vec![
        Arc::new(MockSqlStrategy::with_parse_error()),
    ];
    let facade = Arc::new(UnifiedQueryFacade::new(strategies, FacadeConfig::default()));
    QueryFacadeAdapter::new(facade)
}

/// Create a facade that simulates execution errors
fn create_exec_error_facade() -> QueryFacadeAdapter {
    let strategies: Vec<Arc<dyn QueryStrategy>> = vec![
        Arc::new(MockSqlStrategy::with_exec_error()),
    ];
    let facade = Arc::new(UnifiedQueryFacade::new(strategies, FacadeConfig::default()));
    QueryFacadeAdapter::new(facade)
}

// ================================================================================
// SQL QUERY ROUTING TESTS
// ================================================================================

/// Test that SQL query routes through facade correctly
#[tokio::test]
async fn test_sql_query_routes_through_facade() {
    let adapter = create_sql_adapter();

    let result = adapter.sql_query("SELECT * FROM products").await.unwrap();

    // Verify result structure
    assert!(matches!(result.data, QueryResultData::Rows(_)));

    if let QueryResultData::Rows(rows) = result.data {
        assert_eq!(rows.len(), 3, "Should return 3 mock rows");

        // Verify row structure
        let first_row = &rows[0];
        assert!(first_row.get("id").is_some());
        assert!(first_row.get("name").is_some());
        assert!(first_row.get("price").is_some());
    }
}

/// Test SQL query with LIMIT clause
#[tokio::test]
async fn test_sql_query_respects_limit() {
    let adapter = create_sql_adapter();

    let result = adapter.sql_query("SELECT * FROM products LIMIT 2").await.unwrap();

    if let QueryResultData::Rows(rows) = result.data {
        assert_eq!(rows.len(), 2, "Should respect LIMIT 2");
    } else {
        panic!("Expected Rows result");
    }
}

/// Test SQL query metrics
#[tokio::test]
async fn test_sql_query_includes_metrics() {
    let facade = create_sql_facade();

    let request = QueryRequest::sql("SELECT * FROM products").with_metrics();
    let result = facade.execute(request).await.unwrap();

    let metrics = result.metrics.expect("Should have metrics");
    assert_eq!(metrics.strategy_name, "mock_sql");
    assert_eq!(metrics.execution_path, "unified"); // All queries route through unified facade
    assert!(metrics.extra.get("sql").is_some());
}

// ================================================================================
// FEDERATED QUERY TESTS
// ================================================================================

/// Test federated query (SQL with VECTOR_SEARCH extension)
#[tokio::test]
async fn test_federated_query_routes_through_facade() {
    let adapter = create_sql_adapter();

    let result = adapter
        .federated_query("SELECT * FROM VECTOR_SEARCH('products', '[0.1,0.2]', 10)")
        .await
        .unwrap();

    assert!(matches!(result.data, QueryResultData::Rows(_)));
}

/// Test federated query with GRAPH_QUERY extension
#[tokio::test]
async fn test_federated_query_with_graph() {
    let adapter = create_sql_adapter();

    let result = adapter
        .federated_query("SELECT * FROM GRAPH_QUERY('MATCH (n) RETURN n LIMIT 5')")
        .await
        .unwrap();

    assert!(matches!(result.data, QueryResultData::Rows(_)));
}

/// Test federated query with multiple extensions
#[tokio::test]
async fn test_federated_query_multi_model() {
    let adapter = create_sql_adapter();

    // Complex federated query joining vector and graph results
    let sql = r#"
        SELECT v.id, v.score, g.name
        FROM VECTOR_SEARCH('products', '[0.1,0.2]', 10) v
        JOIN LATERAL GRAPH_QUERY('MATCH (n:Category)-[:CONTAINS]->(p) RETURN p.name') g ON true
    "#;

    let result = adapter.federated_query(sql).await.unwrap();
    assert!(matches!(result.data, QueryResultData::Rows(_)));
}

/// Test federated query metrics include is_federated flag
#[tokio::test]
async fn test_federated_query_metrics() {
    let facade = create_sql_facade();

    let request = QueryRequest::federated(
        "SELECT * FROM VECTOR_SEARCH('products', '[0.1]', 5)"
    ).with_metrics();

    let result = facade.execute(request).await.unwrap();

    let metrics = result.metrics.expect("Should have metrics");

    // Verify federated flag in extra
    let is_federated = metrics.extra.get("is_federated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(is_federated, "Should mark as federated query");
}

// ================================================================================
// ERROR HANDLING TESTS
// ================================================================================

/// Test SQL parse error propagation
#[tokio::test]
async fn test_sql_parse_error() {
    let adapter = create_parse_error_facade();

    let result = adapter.sql_query("SELEC * FROM products").await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("parse error"),
        "Should contain parse error: {}",
        err
    );
}

/// Test SQL execution error propagation
#[tokio::test]
async fn test_sql_execution_error() {
    let adapter = create_exec_error_facade();

    let result = adapter.sql_query("SELECT * FROM unknown_table").await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("not found"),
        "Should indicate table not found: {}",
        err
    );
}

/// Test federated query error propagation
#[tokio::test]
async fn test_federated_query_error() {
    let adapter = create_exec_error_facade();

    let result = adapter.federated_query("SELECT * FROM unknown").await;

    assert!(result.is_err());
}

// ================================================================================
// RESPONSE FORMAT TESTS
// ================================================================================

/// Test SQL response with various data types
#[tokio::test]
async fn test_sql_response_data_types() {
    let rows = vec![
        serde_json::json!({
            "string_col": "hello",
            "int_col": 42,
            "float_col": 3.14,
            "bool_col": true,
            "null_col": null,
            "array_col": [1, 2, 3],
            "object_col": {"nested": "value"}
        }),
    ];

    let adapter = create_facade_with_rows(rows);
    let result = adapter.sql_query("SELECT * FROM test").await.unwrap();

    if let QueryResultData::Rows(rows) = result.data {
        let row = &rows[0];

        // Verify all data types are preserved
        assert_eq!(row.get("string_col"), Some(&serde_json::json!("hello")));
        assert_eq!(row.get("int_col"), Some(&serde_json::json!(42)));
        assert_eq!(row.get("bool_col"), Some(&serde_json::json!(true)));
        assert!(row.get("null_col").unwrap().is_null());
        assert!(row.get("array_col").unwrap().is_array());
        assert!(row.get("object_col").unwrap().is_object());
    }
}

/// Test SQL response with empty results
#[tokio::test]
async fn test_sql_empty_results() {
    let adapter = create_facade_with_rows(vec![]);

    let result = adapter.sql_query("SELECT * FROM empty_table").await.unwrap();

    if let QueryResultData::Rows(rows) = result.data {
        assert!(rows.is_empty());
    }

    // Metrics should still be present
    let metrics = result.metrics.unwrap();
    assert_eq!(metrics.results_returned, 0);
}

// ================================================================================
// FACADE DIRECT TESTS
// ================================================================================

/// Test facade directly with SQL QueryRequest
#[tokio::test]
async fn test_facade_sql_query_directly() {
    let facade = create_sql_facade();

    let request = QueryRequest::sql("SELECT name, price FROM products WHERE in_stock = true")
        .with_metrics();

    let result = facade.execute(request).await.unwrap();

    assert!(matches!(result.data, QueryResultData::Rows(_)));

    let metrics = result.metrics.unwrap();
    assert_eq!(metrics.strategy_name, "mock_sql");
}

/// Test facade strategy selection for SQL vs Federated
#[tokio::test]
async fn test_facade_distinguishes_sql_and_federated() {
    let facade = create_sql_facade();

    // Regular SQL query
    let sql_request = QueryRequest::sql("SELECT * FROM products");
    assert_eq!(sql_request.query_type, QueryType::Sql);

    // Federated query
    let fed_request = QueryRequest::federated("SELECT * FROM VECTOR_SEARCH('x', '[0.1]', 10)");
    assert_eq!(fed_request.query_type, QueryType::Federated);

    // Both should be handled by SQL strategy
    let sql_result = facade.execute(sql_request).await;
    let fed_result = facade.execute(fed_request).await;

    assert!(sql_result.is_ok());
    assert!(fed_result.is_ok());
}

// ================================================================================
// CONCURRENT REQUEST TESTS
// ================================================================================

/// Test multiple concurrent SQL queries
#[tokio::test]
async fn test_concurrent_sql_queries() {
    let adapter = create_sql_adapter();

    let mut handles = Vec::new();
    for i in 0..10 {
        let adapter_clone = adapter.clone();
        let handle = tokio::spawn(async move {
            let sql = format!("SELECT * FROM table_{}", i);
            adapter_clone.sql_query(&sql).await
        });
        handles.push(handle);
    }

    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok(), "Concurrent SQL query should succeed");
    }
}

/// Test concurrent mix of SQL and federated queries
#[tokio::test]
async fn test_concurrent_mixed_queries() {
    let adapter = create_sql_adapter();

    let mut handles = Vec::new();

    // SQL queries
    for i in 0..5 {
        let adapter_clone = adapter.clone();
        let handle = tokio::spawn(async move {
            let sql = format!("SELECT * FROM products LIMIT {}", i + 1);
            adapter_clone.sql_query(&sql).await
        });
        handles.push(handle);
    }

    // Federated queries
    for i in 0..5 {
        let adapter_clone = adapter.clone();
        let handle = tokio::spawn(async move {
            let sql = format!(
                "SELECT * FROM VECTOR_SEARCH('collection_{}', '[0.1,0.2]', 10)",
                i
            );
            adapter_clone.federated_query(&sql).await
        });
        handles.push(handle);
    }

    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok(), "Concurrent query should succeed");
    }
}

// ================================================================================
// ADAPTER SQL INTERFACE TESTS
// ================================================================================

/// Test adapter sql_query method
#[tokio::test]
async fn test_adapter_sql_query_method() {
    let adapter = create_sql_adapter();

    // The adapter returns QueryResult directly for SQL queries
    let result = adapter.sql_query("SELECT * FROM test").await.unwrap();

    // Should return QueryResult with Rows data
    assert!(matches!(result.data, QueryResultData::Rows(_)));
}

/// Test adapter federated_query method
#[tokio::test]
async fn test_adapter_federated_query_method() {
    let adapter = create_sql_adapter();

    let result = adapter
        .federated_query("SELECT * FROM VECTOR_SEARCH('products', '[0.1]', 5)")
        .await
        .unwrap();

    assert!(matches!(result.data, QueryResultData::Rows(_)));
}

// ================================================================================
// FEATURE FLAG CONCEPT VALIDATION
// ================================================================================

/// Test that validates the unified-facade-routing concept for SQL
#[tokio::test]
async fn test_unified_facade_routing_sql_concept() {
    // When unified-facade-routing is enabled:
    // 1. SQL/federated requests route through QueryFacadeAdapter
    // 2. Adapter creates QueryRequest with appropriate type
    // 3. Facade selects SQL strategy
    // 4. Strategy executes and returns QueryResult with Rows
    // 5. Adapter returns QueryResult directly (not converted to proto)

    let adapter = create_sql_adapter();

    // Simulate REST handler receiving SQL request
    let sql = "SELECT id, name, price FROM products WHERE price > 50 ORDER BY price DESC";

    // Handler routes through adapter
    let result = adapter.sql_query(sql).await.unwrap();

    // Verify result is properly structured
    if let QueryResultData::Rows(rows) = result.data {
        // Mock returns 3 rows by default
        assert!(!rows.is_empty());
    } else {
        panic!("Expected Rows result");
    }

    // Verify metrics are available
    assert!(result.metrics.is_some());
}

/// Test federated query full round-trip
#[tokio::test]
async fn test_federated_query_full_roundtrip() {
    let adapter = create_sql_adapter();

    // Complex federated query that would use multiple data sources
    let sql = r#"
        SELECT u.name, v.product_id, v.score, d.review_text
        FROM users u
        JOIN LATERAL VECTOR_SEARCH('products', u.preference_vector, 10) v ON true
        JOIN LATERAL DOCUMENT_QUERY('reviews', 'product_id = "' || v.product_id || '"') d ON true
    "#;

    let result = adapter.federated_query(sql).await.unwrap();

    // Even though this is a complex query, the mock strategy handles it
    assert!(matches!(result.data, QueryResultData::Rows(_)));

    // Metrics should indicate federated execution
    let metrics = result.metrics.unwrap();
    assert_eq!(metrics.execution_path, "unified"); // All queries route through unified facade
}
