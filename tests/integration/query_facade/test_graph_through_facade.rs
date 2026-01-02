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

//! # Graph Query Through Facade Integration Tests
//!
//! Tests that validate graph (Cypher-like) queries route correctly through the
//! `UnifiedQueryFacade` when the `unified-facade-routing` feature is enabled.
//!
//! ## Test Coverage
//!
//! - Graph query request conversion
//! - Strategy selection for graph queries
//! - Response format validation (nodes, edges, paths)
//! - Error handling for invalid graph queries
//! - Traversal pattern parsing

use std::sync::Arc;
use anyhow::Result;
use async_trait::async_trait;

use proximadb::query::facade::{
    ExecutionMetrics, FacadeConfig, GraphQueryResult, QueryContext, QueryFacadeAdapter,
    QueryRequest, QueryResult, QueryResultData, QueryStrategy, QueryType,
    UnifiedQueryFacade, QueryContent,
};

// ================================================================================
// MOCK STRATEGIES FOR TESTING
// ================================================================================

/// Mock graph strategy that returns predictable results
struct MockGraphStrategy {
    /// Nodes to return
    nodes: Vec<serde_json::Value>,
    /// Edges to return
    edges: Vec<serde_json::Value>,
    /// Paths to return
    paths: Vec<serde_json::Value>,
    /// Whether to simulate a parse error
    should_parse_error: bool,
    /// Whether to simulate a traversal error
    should_traversal_error: bool,
}

impl MockGraphStrategy {
    fn new() -> Self {
        Self {
            nodes: vec![
                serde_json::json!({
                    "id": "node_1",
                    "labels": ["Person"],
                    "properties": {
                        "name": "Alice",
                        "age": 30
                    }
                }),
                serde_json::json!({
                    "id": "node_2",
                    "labels": ["Person"],
                    "properties": {
                        "name": "Bob",
                        "age": 25
                    }
                }),
                serde_json::json!({
                    "id": "node_3",
                    "labels": ["Company"],
                    "properties": {
                        "name": "TechCorp",
                        "employees": 1000
                    }
                }),
            ],
            edges: vec![
                serde_json::json!({
                    "id": "edge_1",
                    "source": "node_1",
                    "target": "node_2",
                    "type": "KNOWS",
                    "weight": 0.9,
                    "properties": {
                        "since": 2020
                    }
                }),
                serde_json::json!({
                    "id": "edge_2",
                    "source": "node_1",
                    "target": "node_3",
                    "type": "WORKS_AT",
                    "weight": 1.0,
                    "properties": {
                        "role": "Engineer"
                    }
                }),
            ],
            paths: vec![
                serde_json::json!({
                    "entities": ["node_1", "node_2"],
                    "relations": [
                        {
                            "source": "node_1",
                            "target": "node_2",
                            "type": "KNOWS"
                        }
                    ]
                }),
            ],
            should_parse_error: false,
            should_traversal_error: false,
        }
    }

    fn with_nodes_edges(
        nodes: Vec<serde_json::Value>,
        edges: Vec<serde_json::Value>,
    ) -> Self {
        Self {
            nodes,
            edges,
            paths: vec![],
            should_parse_error: false,
            should_traversal_error: false,
        }
    }

    fn with_parse_error() -> Self {
        Self {
            nodes: vec![],
            edges: vec![],
            paths: vec![],
            should_parse_error: true,
            should_traversal_error: false,
        }
    }

    fn with_traversal_error() -> Self {
        Self {
            nodes: vec![],
            edges: vec![],
            paths: vec![],
            should_parse_error: false,
            should_traversal_error: true,
        }
    }
}

#[async_trait]
impl QueryStrategy for MockGraphStrategy {
    fn name(&self) -> &str {
        "mock_graph"
    }

    fn can_handle(&self, request: &QueryRequest) -> bool {
        request.query_type == QueryType::Graph
    }

    fn priority(&self) -> i32 {
        80
    }

    async fn execute(&self, request: QueryRequest, _ctx: &QueryContext) -> Result<QueryResult> {
        // Extract query from request
        let query = match &request.content {
            QueryContent::Graph(q) => q.clone(),
            _ => return Err(anyhow::anyhow!("Expected Graph content")),
        };

        // Simulate parse error
        if self.should_parse_error {
            return Err(anyhow::anyhow!("Graph query parse error: invalid syntax"));
        }

        // Simulate traversal error
        if self.should_traversal_error {
            return Err(anyhow::anyhow!("Graph traversal error: start node not found"));
        }

        // Apply LIMIT if present
        let limit = Self::extract_limit(&query).unwrap_or(100);
        let limited_nodes: Vec<serde_json::Value> = self.nodes
            .iter()
            .take(limit)
            .cloned()
            .collect();

        Ok(QueryResult {
            data: QueryResultData::Graph(GraphQueryResult {
                nodes: limited_nodes.clone(),
                edges: self.edges.clone(),
                paths: self.paths.clone(),
            }),
            metrics: Some(ExecutionMetrics {
                execution_path: "graph".to_string(),
                strategy_name: "mock_graph".to_string(),
                execution_time_ms: 8,
                planning_time_ms: 1,
                results_scanned: self.nodes.len() + self.edges.len(),
                results_returned: limited_nodes.len() + self.edges.len(),
                cache_hit: false,
                extra: serde_json::json!({
                    "mock": true,
                    "engine": "MockGraphEngine",
                    "query": query,
                    "nodes_returned": limited_nodes.len(),
                    "edges_returned": self.edges.len()
                }),
            }),
        })
    }
}

impl MockGraphStrategy {
    /// Extract LIMIT value from Cypher query
    fn extract_limit(query: &str) -> Option<usize> {
        let upper = query.to_uppercase();
        if let Some(pos) = upper.find("LIMIT ") {
            let rest = &query[pos + 6..];
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

/// Create a test facade with mock graph strategy
fn create_graph_facade() -> UnifiedQueryFacade {
    let strategies: Vec<Arc<dyn QueryStrategy>> = vec![
        Arc::new(MockGraphStrategy::new()),
    ];
    UnifiedQueryFacade::new(strategies, FacadeConfig::default())
}

/// Create a test facade adapter
fn create_graph_adapter() -> QueryFacadeAdapter {
    let facade = Arc::new(create_graph_facade());
    QueryFacadeAdapter::new(facade)
}

/// Create a facade with custom nodes and edges
fn create_facade_with_graph(
    nodes: Vec<serde_json::Value>,
    edges: Vec<serde_json::Value>,
) -> QueryFacadeAdapter {
    let strategies: Vec<Arc<dyn QueryStrategy>> = vec![
        Arc::new(MockGraphStrategy::with_nodes_edges(nodes, edges)),
    ];
    let facade = Arc::new(UnifiedQueryFacade::new(strategies, FacadeConfig::default()));
    QueryFacadeAdapter::new(facade)
}

/// Create a facade that simulates parse errors
fn create_parse_error_facade() -> QueryFacadeAdapter {
    let strategies: Vec<Arc<dyn QueryStrategy>> = vec![
        Arc::new(MockGraphStrategy::with_parse_error()),
    ];
    let facade = Arc::new(UnifiedQueryFacade::new(strategies, FacadeConfig::default()));
    QueryFacadeAdapter::new(facade)
}

/// Create a facade that simulates traversal errors
fn create_traversal_error_facade() -> QueryFacadeAdapter {
    let strategies: Vec<Arc<dyn QueryStrategy>> = vec![
        Arc::new(MockGraphStrategy::with_traversal_error()),
    ];
    let facade = Arc::new(UnifiedQueryFacade::new(strategies, FacadeConfig::default()));
    QueryFacadeAdapter::new(facade)
}

// ================================================================================
// GRAPH QUERY ROUTING TESTS
// ================================================================================

/// Test that graph query routes through facade correctly
#[tokio::test]
async fn test_graph_query_routes_through_facade() {
    let adapter = create_graph_adapter();

    let result = adapter
        .graph_query("MATCH (n:Person) RETURN n", None)
        .await
        .unwrap();

    // Verify result structure
    assert!(matches!(result.data, QueryResultData::Graph(_)));

    if let QueryResultData::Graph(graph) = result.data {
        assert_eq!(graph.nodes.len(), 3, "Should return 3 mock nodes");
        assert_eq!(graph.edges.len(), 2, "Should return 2 mock edges");
    }
}

/// Test graph query with specific graph name
#[tokio::test]
async fn test_graph_query_with_graph_name() {
    let adapter = create_graph_adapter();

    let result = adapter
        .graph_query("MATCH (n) RETURN n", Some("social_graph"))
        .await
        .unwrap();

    assert!(matches!(result.data, QueryResultData::Graph(_)));
}

/// Test graph query with LIMIT clause
#[tokio::test]
async fn test_graph_query_respects_limit() {
    let adapter = create_graph_adapter();

    let result = adapter
        .graph_query("MATCH (n) RETURN n LIMIT 2", None)
        .await
        .unwrap();

    if let QueryResultData::Graph(graph) = result.data {
        assert_eq!(graph.nodes.len(), 2, "Should respect LIMIT 2");
    } else {
        panic!("Expected Graph result");
    }
}

/// Test graph query metrics
#[tokio::test]
async fn test_graph_query_includes_metrics() {
    let facade = create_graph_facade();

    let request = QueryRequest::graph("MATCH (n:Person)-[:KNOWS]->(m) RETURN n, m")
        .with_metrics();

    let result = facade.execute(request).await.unwrap();

    let metrics = result.metrics.expect("Should have metrics");
    assert_eq!(metrics.strategy_name, "mock_graph");
    assert_eq!(metrics.execution_path, "graph");

    // Verify extra metrics
    assert!(metrics.extra.get("nodes_returned").is_some());
    assert!(metrics.extra.get("edges_returned").is_some());
}

// ================================================================================
// CYPHER PATTERN TESTS
// ================================================================================

/// Test various Cypher query patterns
#[tokio::test]
async fn test_cypher_patterns() {
    let adapter = create_graph_adapter();

    // Simple node match
    let r1 = adapter.graph_query("MATCH (n) RETURN n", None).await;
    assert!(r1.is_ok());

    // Labeled node match
    let r2 = adapter.graph_query("MATCH (n:Person) RETURN n", None).await;
    assert!(r2.is_ok());

    // Relationship match
    let r3 = adapter.graph_query("MATCH (a)-[:KNOWS]->(b) RETURN a, b", None).await;
    assert!(r3.is_ok());

    // Multi-hop pattern
    let r4 = adapter
        .graph_query("MATCH (a)-[:KNOWS]->(b)-[:WORKS_AT]->(c) RETURN a, b, c", None)
        .await;
    assert!(r4.is_ok());

    // Property filter
    let r5 = adapter
        .graph_query(r#"MATCH (n:Person {name: "Alice"}) RETURN n"#, None)
        .await;
    assert!(r5.is_ok());
}

/// Test traversal with start node
#[tokio::test]
async fn test_graph_traversal_from_start_node() {
    let adapter = create_graph_adapter();

    // Query starting from specific node
    let result = adapter
        .graph_query(
            r#"MATCH (n:Person {id: "node_1"})-[r:KNOWS]->(m) RETURN n, r, m"#,
            None,
        )
        .await
        .unwrap();

    assert!(matches!(result.data, QueryResultData::Graph(_)));
}

// ================================================================================
// ERROR HANDLING TESTS
// ================================================================================

/// Test graph parse error propagation
#[tokio::test]
async fn test_graph_parse_error() {
    let adapter = create_parse_error_facade();

    let result = adapter
        .graph_query("MATC (n) RETURN n", None) // Typo in MATCH
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("parse error"),
        "Should contain parse error: {}",
        err
    );
}

/// Test graph traversal error propagation
#[tokio::test]
async fn test_graph_traversal_error() {
    let adapter = create_traversal_error_facade();

    let result = adapter
        .graph_query(
            r#"MATCH (n:Person {id: "unknown_node"}) RETURN n"#,
            None,
        )
        .await;

    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("not found"),
        "Should indicate node not found: {}",
        err
    );
}

/// Test empty graph result
#[tokio::test]
async fn test_graph_empty_results() {
    let adapter = create_facade_with_graph(vec![], vec![]);

    let result = adapter
        .graph_query("MATCH (n:NonExistent) RETURN n", None)
        .await
        .unwrap();

    if let QueryResultData::Graph(graph) = result.data {
        assert!(graph.nodes.is_empty());
        assert!(graph.edges.is_empty());
        assert!(graph.paths.is_empty());
    }
}

// ================================================================================
// RESPONSE FORMAT TESTS
// ================================================================================

/// Test graph response node format
#[tokio::test]
async fn test_graph_response_node_format() {
    let adapter = create_graph_adapter();

    let result = adapter
        .graph_query("MATCH (n) RETURN n", None)
        .await
        .unwrap();

    if let QueryResultData::Graph(graph) = result.data {
        for node in &graph.nodes {
            // Each node should have id, labels, and properties
            assert!(node.get("id").is_some(), "Node should have id");
            assert!(node.get("labels").is_some(), "Node should have labels");
            assert!(node.get("properties").is_some(), "Node should have properties");

            // Labels should be an array
            assert!(node.get("labels").unwrap().is_array());

            // Properties should be an object
            assert!(node.get("properties").unwrap().is_object());
        }
    }
}

/// Test graph response edge format
#[tokio::test]
async fn test_graph_response_edge_format() {
    let adapter = create_graph_adapter();

    let result = adapter
        .graph_query("MATCH ()-[r]->() RETURN r", None)
        .await
        .unwrap();

    if let QueryResultData::Graph(graph) = result.data {
        for edge in &graph.edges {
            // Each edge should have required fields
            assert!(edge.get("id").is_some(), "Edge should have id");
            assert!(edge.get("source").is_some(), "Edge should have source");
            assert!(edge.get("target").is_some(), "Edge should have target");
            assert!(edge.get("type").is_some(), "Edge should have type");
            assert!(edge.get("weight").is_some(), "Edge should have weight");
            assert!(edge.get("properties").is_some(), "Edge should have properties");
        }
    }
}

/// Test graph response path format
#[tokio::test]
async fn test_graph_response_path_format() {
    let adapter = create_graph_adapter();

    let result = adapter
        .graph_query("MATCH p = (a)-[*1..3]->(b) RETURN p", None)
        .await
        .unwrap();

    if let QueryResultData::Graph(graph) = result.data {
        for path in &graph.paths {
            // Each path should have entities and relations
            assert!(path.get("entities").is_some(), "Path should have entities");
            assert!(path.get("relations").is_some(), "Path should have relations");

            // Entities should be an array
            assert!(path.get("entities").unwrap().is_array());

            // Relations should be an array
            assert!(path.get("relations").unwrap().is_array());
        }
    }
}

// ================================================================================
// FACADE DIRECT TESTS
// ================================================================================

/// Test facade directly with graph QueryRequest
#[tokio::test]
async fn test_facade_graph_query_directly() {
    let facade = create_graph_facade();

    let request = QueryRequest::graph("MATCH (n:Company) RETURN n").with_metrics();

    let result = facade.execute(request).await.unwrap();

    assert!(matches!(result.data, QueryResultData::Graph(_)));

    let metrics = result.metrics.unwrap();
    assert_eq!(metrics.strategy_name, "mock_graph");
}

/// Test facade with target graph
#[tokio::test]
async fn test_facade_graph_with_target() {
    let facade = create_graph_facade();

    let request = QueryRequest::graph("MATCH (n) RETURN n")
        .with_target("knowledge_graph")
        .with_metrics();

    assert_eq!(request.target, Some("knowledge_graph".to_string()));

    let result = facade.execute(request).await.unwrap();
    assert!(matches!(result.data, QueryResultData::Graph(_)));
}

/// Test facade strategy selection for graph
#[tokio::test]
async fn test_facade_selects_graph_strategy() {
    let facade = create_graph_facade();

    let request = QueryRequest::graph("MATCH (n) RETURN n").with_metrics();
    let result = facade.execute(request).await.unwrap();

    let metrics = result.metrics.unwrap();
    assert_eq!(metrics.strategy_name, "mock_graph");
}

// ================================================================================
// CONCURRENT REQUEST TESTS
// ================================================================================

/// Test multiple concurrent graph queries
#[tokio::test]
async fn test_concurrent_graph_queries() {
    let adapter = create_graph_adapter();

    let mut handles = Vec::new();
    for i in 0..10 {
        let adapter_clone = adapter.clone();
        let handle = tokio::spawn(async move {
            let query = format!("MATCH (n:Type_{}) RETURN n", i);
            adapter_clone.graph_query(&query, None).await
        });
        handles.push(handle);
    }

    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok(), "Concurrent graph query should succeed");
    }
}

/// Test concurrent graph queries with different targets
#[tokio::test]
async fn test_concurrent_graph_queries_different_targets() {
    let adapter = create_graph_adapter();

    let graphs = vec!["social", "knowledge", "product", "user", "transaction"];

    let mut handles = Vec::new();
    for graph in graphs {
        let adapter_clone = adapter.clone();
        let graph_name = graph.to_string();
        let handle = tokio::spawn(async move {
            adapter_clone
                .graph_query("MATCH (n) RETURN n", Some(&graph_name))
                .await
        });
        handles.push(handle);
    }

    for handle in handles {
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }
}

// ================================================================================
// MULTI-STRATEGY TESTS
// ================================================================================

/// Test facade with multiple strategies selects correct one
#[tokio::test]
async fn test_multi_strategy_facade_selects_graph() {
    // Create a mock vector strategy
    struct MockVectorStrategy;

    #[async_trait]
    impl QueryStrategy for MockVectorStrategy {
        fn name(&self) -> &str {
            "mock_vector"
        }
        fn can_handle(&self, request: &QueryRequest) -> bool {
            request.query_type == QueryType::VectorSearch
        }
        fn priority(&self) -> i32 {
            100
        }
        async fn execute(
            &self,
            _request: QueryRequest,
            _ctx: &QueryContext,
        ) -> Result<QueryResult> {
            Ok(QueryResult {
                data: QueryResultData::Empty,
                metrics: Some(ExecutionMetrics {
                    strategy_name: "mock_vector".to_string(),
                    ..Default::default()
                }),
            })
        }
    }

    // Create facade with both strategies
    let strategies: Vec<Arc<dyn QueryStrategy>> = vec![
        Arc::new(MockVectorStrategy),
        Arc::new(MockGraphStrategy::new()),
    ];
    let facade = UnifiedQueryFacade::new(strategies, FacadeConfig::default());

    // Graph query should use graph strategy
    let graph_request = QueryRequest::graph("MATCH (n) RETURN n").with_metrics();
    let result = facade.execute(graph_request).await.unwrap();
    assert_eq!(
        result.metrics.unwrap().strategy_name,
        "mock_graph",
        "Should select graph strategy"
    );

    // Vector query should use vector strategy
    let vector_request = QueryRequest::vector_search(vec![0.1], 10).with_metrics();
    let result = facade.execute(vector_request).await.unwrap();
    assert_eq!(
        result.metrics.unwrap().strategy_name,
        "mock_vector",
        "Should select vector strategy"
    );
}

// ================================================================================
// FEATURE FLAG CONCEPT VALIDATION
// ================================================================================

/// Test that validates the unified-facade-routing concept for graph queries
#[tokio::test]
async fn test_unified_facade_routing_graph_concept() {
    // When unified-facade-routing is enabled:
    // 1. Graph requests route through QueryFacadeAdapter
    // 2. Adapter creates QueryRequest with Graph type
    // 3. Facade selects graph strategy
    // 4. Strategy executes traversal and returns GraphQueryResult
    // 5. Adapter returns QueryResult with Graph data

    let adapter = create_graph_adapter();

    // Simulate handler receiving graph query
    let cypher = r#"
        MATCH (a:Person)-[:KNOWS]->(b:Person)
        WHERE a.name = "Alice"
        RETURN a, b
    "#;

    // Handler routes through adapter
    let result = adapter.graph_query(cypher, Some("social_graph")).await.unwrap();

    // Verify result structure
    if let QueryResultData::Graph(graph) = result.data {
        assert!(!graph.nodes.is_empty(), "Should have nodes");
        assert!(!graph.edges.is_empty(), "Should have edges");
    } else {
        panic!("Expected Graph result");
    }

    // Verify metrics are available
    let metrics = result.metrics.expect("Should have metrics");
    assert_eq!(metrics.execution_path, "graph");
}

/// Test graph query with properties in response
#[tokio::test]
async fn test_graph_response_with_properties() {
    let nodes = vec![
        serde_json::json!({
            "id": "person_1",
            "labels": ["Person", "Employee"],
            "properties": {
                "name": "John Doe",
                "age": 35,
                "email": "john@example.com",
                "skills": ["rust", "python", "go"],
                "active": true,
                "metadata": {
                    "created_at": "2024-01-01",
                    "department": "Engineering"
                }
            }
        }),
    ];

    let edges = vec![
        serde_json::json!({
            "id": "rel_1",
            "source": "person_1",
            "target": "company_1",
            "type": "WORKS_AT",
            "weight": 1.0,
            "properties": {
                "since": "2020-01-15",
                "title": "Senior Engineer",
                "full_time": true
            }
        }),
    ];

    let adapter = create_facade_with_graph(nodes, edges);

    let result = adapter
        .graph_query("MATCH (p:Person)-[r:WORKS_AT]->(c:Company) RETURN p, r, c", None)
        .await
        .unwrap();

    if let QueryResultData::Graph(graph) = result.data {
        let node = &graph.nodes[0];
        let props = node.get("properties").unwrap();

        // Verify all property types
        assert!(props.get("name").unwrap().is_string());
        assert!(props.get("age").unwrap().is_number());
        assert!(props.get("skills").unwrap().is_array());
        assert!(props.get("active").unwrap().is_boolean());
        assert!(props.get("metadata").unwrap().is_object());
    }
}
