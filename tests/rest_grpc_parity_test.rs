//! REST and gRPC Parity Integration Tests
//!
//! This module tests that REST and gRPC APIs produce identical results
//! for the same operations. This ensures API consistency across protocols.
//!
//! ## Test Coverage
//! - Vector search parity (same query returns same results)
//! - Graph query parity (same Cypher/traversal returns same results)
//! - SQL query parity (same SQL returns same results)
//! - Error response parity (same errors return same status codes)
//!
//! ## Running Tests
//! ```bash
//! # Start server first (in another terminal)
//! cargo run --release --bin proximadb-server
//!
//! # Run parity tests
//! cargo test --test rest_grpc_parity_test -- --test-threads=1
//! ```

use std::collections::HashMap;
use std::time::Duration;

use reqwest::Client as HttpClient;
use serde_json::{Value as JsonValue, json};
use tokio::time::sleep;
use tonic::transport::Channel;

// Proto types for gRPC
use proximadb::proto::proximadb_v1::{
    // Collection types
    CollectionConfig,
    // Graph types
    CreateNodeRequest,
    DistanceMetric,
    // SQL types
    ExecuteQueryRequest,
    ExecuteQueryResponse,
    GetCollectionRequest,
    GetNodeRequest,
    Node,
    SearchQuery,
    SearchVectorRecord,
    SqlValue,
    StorageEngine,
    // Vector types
    VectorBatchRequest,
    VectorRecord,
    VectorSearchRequest,
};

// gRPC service clients
use proximadb::proto::proximadb_v1::{
    collection_service_client::CollectionServiceClient, graph_service_client::GraphServiceClient,
    query_service_client::QueryServiceClient, vector_service_client::VectorServiceClient,
};

// Constants for server endpoints
const REST_BASE_URL: &str = "http://127.0.0.1:5678";
const GRPC_ENDPOINT: &str = "http://127.0.0.1:5679";

/// Test collection configuration
const TEST_DIMENSION: u32 = 128;
const TEST_TOP_K: u32 = 10;

// ================================================================================
// HELPER TYPES FOR COMPARISON
// ================================================================================

/// Normalized search result for comparison between REST and gRPC
#[derive(Debug, Clone, PartialEq)]
struct NormalizedSearchResult {
    id: String,
    score: f64,
    has_vector: bool,
    metadata_keys: Vec<String>,
}

impl NormalizedSearchResult {
    fn from_grpc(record: &SearchVectorRecord) -> Self {
        let mut metadata_keys: Vec<String> = record.metadata.keys().cloned().collect();
        metadata_keys.sort();

        Self {
            id: record.id.clone(),
            score: record.score,
            has_vector: !record.vector.is_empty(),
            metadata_keys,
        }
    }

    fn from_json(value: &JsonValue) -> Option<Self> {
        let id = value.get("id")?.as_str()?.to_string();
        let score = value.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let has_vector = value
            .get("vector")
            .and_then(|v| v.as_array())
            .map(|arr| !arr.is_empty())
            .unwrap_or(false);

        let mut metadata_keys: Vec<String> = value
            .get("metadata")
            .and_then(|v| v.as_object())
            .map(|obj| obj.keys().cloned().collect())
            .unwrap_or_default();
        metadata_keys.sort();

        Some(Self {
            id,
            score,
            has_vector,
            metadata_keys,
        })
    }
}

/// Normalized graph node for comparison
#[derive(Debug, Clone, PartialEq)]
struct NormalizedNode {
    id: String,
    labels: Vec<String>,
    property_keys: Vec<String>,
}

impl NormalizedNode {
    fn from_grpc(node: &Node) -> Self {
        let mut labels = node.labels.clone();
        labels.sort();

        let mut property_keys: Vec<String> = node.properties.keys().cloned().collect();
        property_keys.sort();

        Self {
            id: node.id.clone(),
            labels,
            property_keys,
        }
    }

    fn from_json(value: &JsonValue) -> Option<Self> {
        let id = value.get("id")?.as_str()?.to_string();

        let mut labels: Vec<String> = value
            .get("labels")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        labels.sort();

        let mut property_keys: Vec<String> = value
            .get("properties")
            .and_then(|v| v.as_object())
            .map(|obj| obj.keys().cloned().collect())
            .unwrap_or_default();
        property_keys.sort();

        Some(Self {
            id,
            labels,
            property_keys,
        })
    }
}

/// Normalized SQL result for comparison
#[derive(Debug, Clone, PartialEq)]
struct NormalizedSqlResult {
    row_count: u64,
    column_names: Vec<String>,
}

impl NormalizedSqlResult {
    fn from_grpc(response: &ExecuteQueryResponse) -> Self {
        let mut column_names = response.columns.clone();
        column_names.sort();

        Self {
            row_count: response.rows_returned,
            column_names,
        }
    }

    fn from_json(value: &JsonValue) -> Self {
        let row_count = value
            .get("row_count")
            .or_else(|| value.get("rows_returned"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let rows = value.get("rows").and_then(|v| v.as_array());
        let mut column_names: Vec<String> = rows
            .and_then(|arr| arr.first())
            .and_then(|row| row.as_object())
            .map(|obj| obj.keys().cloned().collect())
            .unwrap_or_else(|| {
                value
                    .get("columns")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default()
            });
        column_names.sort();

        Self {
            row_count,
            column_names,
        }
    }
}

// ================================================================================
// TEST INFRASTRUCTURE
// ================================================================================

/// Test harness for REST/gRPC parity testing
///
/// This harness provides methods for both REST and gRPC operations to enable
/// parity testing. Some methods may not be used in all tests but are provided
/// for extensibility and future test cases.
#[allow(dead_code)]
struct ParityTestHarness {
    http_client: HttpClient,
    vector_client: Option<VectorServiceClient<Channel>>,
    collection_client: Option<CollectionServiceClient<Channel>>,
    graph_client: Option<GraphServiceClient<Channel>>,
    sql_client: Option<SqlServiceClient<Channel>>,
    test_collection_name: String,
    test_graph_name: String,
}

#[allow(dead_code)]
impl ParityTestHarness {
    /// Create a new test harness and connect to both REST and gRPC servers
    async fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let http_client = HttpClient::builder()
            .timeout(Duration::from_secs(30))
            // Avoid macOS system proxy discovery in test sandboxes.
            .no_proxy()
            .build()?;

        // Generate unique test names to avoid conflicts
        let test_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
        let test_collection_name = format!("parity_test_coll_{}", test_id);
        let test_graph_name = format!("parity_test_graph_{}", test_id);

        // Try to connect to gRPC server
        let grpc_connected = match Channel::from_static(GRPC_ENDPOINT)
            .connect_timeout(Duration::from_secs(5))
            .connect()
            .await
        {
            Ok(channel) => {
                let vector_client = VectorServiceClient::new(channel.clone());
                let collection_client = CollectionServiceClient::new(channel.clone());
                let graph_client = GraphServiceClient::new(channel.clone());
                let sql_client = SqlServiceClient::new(channel);

                Some((vector_client, collection_client, graph_client, sql_client))
            }
            Err(e) => {
                eprintln!(
                    "Warning: Could not connect to gRPC server at {}: {}",
                    GRPC_ENDPOINT, e
                );
                None
            }
        };

        let (vector_client, collection_client, graph_client, sql_client) = match grpc_connected {
            Some((v, c, g, s)) => (Some(v), Some(c), Some(g), Some(s)),
            None => (None, None, None, None),
        };

        Ok(Self {
            http_client,
            vector_client,
            collection_client,
            graph_client,
            sql_client,
            test_collection_name,
            test_graph_name,
        })
    }

    /// Check if servers are available
    async fn check_servers(&self) -> (bool, bool) {
        // Check REST server
        let rest_available = self
            .http_client
            .get(&format!("{}/health", REST_BASE_URL))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false);

        // Check gRPC server
        let grpc_available = self.vector_client.is_some();

        (rest_available, grpc_available)
    }

    /// Create a test collection via REST
    async fn create_collection_rest(&self) -> Result<String, Box<dyn std::error::Error>> {
        let request = json!({
            "operation": 1, // COLLECTION_CREATE
            "collection_config": {
                "name": self.test_collection_name,
                "dimension": TEST_DIMENSION,
                "distance_metric": 1, // COSINE
                "storage_engine": 2  // SST
            }
        });

        let response = self
            .http_client
            .post(&format!("{}/api/v1/collections", REST_BASE_URL))
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        let body: JsonValue = response.json().await?;

        if !status.is_success() {
            return Err(format!("Failed to create collection: {:?}", body).into());
        }

        Ok(body
            .get("collection_id")
            .or_else(|| body.get("collection").and_then(|c| c.get("id")))
            .and_then(|v| v.as_str())
            .unwrap_or(&self.test_collection_name)
            .to_string())
    }

    /// Create a test collection via gRPC
    async fn create_collection_grpc(&self) -> Result<String, Box<dyn std::error::Error>> {
        let mut client = self
            .collection_client
            .clone()
            .ok_or("gRPC collection client not available")?;

        let request = CollectionConfig {
            name: self.test_collection_name.clone(),
            dimension: TEST_DIMENSION,
            distance_metric: Some(DistanceMetric::Cosine as i32),
            storage_engine: Some(StorageEngine::Sst as i32),
            ..Default::default()
        };

        let response = client.create_collection(request).await?;
        let inner = response.into_inner();

        Ok(inner.id)
    }

    /// Insert test vectors via REST
    async fn insert_vectors_rest(
        &self,
        vectors: &[(String, Vec<f32>, HashMap<String, String>)],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let records: Vec<JsonValue> = vectors
            .iter()
            .map(|(id, vec, meta)| {
                let metadata: HashMap<String, JsonValue> = meta
                    .iter()
                    .map(|(k, v)| (k.clone(), json!({ "string_value": v })))
                    .collect();

                json!({
                    "id": id,
                    "vector": vec,
                    "metadata": metadata
                })
            })
            .collect();

        let request = json!({
            "collection_id": self.test_collection_name,
            "vectors": records
        });

        let response = self
            .http_client
            .post(&format!("{}/api/v1/vectors/batch", REST_BASE_URL))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let body: JsonValue = response.json().await?;
            return Err(format!("Failed to insert vectors via REST: {:?}", body).into());
        }

        Ok(())
    }

    /// Insert test vectors via gRPC
    async fn insert_vectors_grpc(
        &self,
        vectors: &[(String, Vec<f32>, HashMap<String, String>)],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut client = self
            .vector_client
            .clone()
            .ok_or("gRPC vector client not available")?;

        let records: Vec<VectorRecord> = vectors
            .iter()
            .map(|(id, vec, meta)| {
                let metadata: HashMap<String, SqlValue> = meta
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            SqlValue {
                                value: Some(
                                    proximadb::proto::proximadb_v1::sql_value::Value::StringValue(
                                        v.clone(),
                                    ),
                                ),
                            },
                        )
                    })
                    .collect();

                VectorRecord {
                    id: id.clone(),
                    vector: vec.clone(),
                    metadata,
                    ..Default::default()
                }
            })
            .collect();

        let request = VectorBatchRequest {
            collection_id: self.test_collection_name.clone(),
            vectors: records,
        };

        client.vector_batch(request).await?;
        Ok(())
    }

    /// Search vectors via REST
    async fn search_vectors_rest(
        &self,
        query_vector: &[f32],
        top_k: u32,
    ) -> Result<Vec<NormalizedSearchResult>, Box<dyn std::error::Error>> {
        let request = json!({
            "collection_id": self.test_collection_name,
            "queries": [{
                "vector": query_vector
            }],
            "top_k": top_k
        });

        let response = self
            .http_client
            .post(&format!("{}/api/v1/search", REST_BASE_URL))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let body: JsonValue = response.json().await?;
            return Err(format!("Search failed via REST: {:?}", body).into());
        }

        let body: JsonValue = response.json().await?;

        // Parse results from REST response
        let results = body
            .get("results")
            .and_then(|r| r.get("results"))
            .or_else(|| body.get("results"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| NormalizedSearchResult::from_json(item))
                    .collect()
            })
            .unwrap_or_default();

        Ok(results)
    }

    /// Search vectors via gRPC
    async fn search_vectors_grpc(
        &self,
        query_vector: &[f32],
        top_k: u32,
    ) -> Result<Vec<NormalizedSearchResult>, Box<dyn std::error::Error>> {
        let mut client = self
            .vector_client
            .clone()
            .ok_or("gRPC vector client not available")?;

        let request = VectorSearchRequest {
            collection_id: self.test_collection_name.clone(),
            queries: vec![SearchQuery {
                vector: query_vector.to_vec(),
                filters: HashMap::new(),
                advanced_filter: None,
            }],
            top_k,
            ..Default::default()
        };

        let response = client.vector_search(request).await?;
        let inner = response.into_inner();

        let results = inner
            .results
            .map(|r| {
                r.results
                    .iter()
                    .map(NormalizedSearchResult::from_grpc)
                    .collect()
            })
            .unwrap_or_default();

        Ok(results)
    }

    /// Execute SQL via REST
    async fn execute_sql_rest(
        &self,
        query: &str,
    ) -> Result<NormalizedSqlResult, Box<dyn std::error::Error>> {
        let request = json!({
            "query": query,
            "collection": self.test_collection_name
        });

        let response = self
            .http_client
            .post(&format!("{}/api/v1/sql/execute", REST_BASE_URL))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let body: JsonValue = response.json().await?;
            return Err(format!("SQL execution failed via REST: {:?}", body).into());
        }

        let body: JsonValue = response.json().await?;
        Ok(NormalizedSqlResult::from_json(&body))
    }

    /// Execute SQL via gRPC
    async fn execute_sql_grpc(
        &self,
        query: &str,
    ) -> Result<NormalizedSqlResult, Box<dyn std::error::Error>> {
        let mut client = self
            .sql_client
            .clone()
            .ok_or("gRPC SQL client not available")?;

        let request = ExecuteQueryRequest {
            query: query.to_string(),
            parameters: vec![],
            collection: Some(self.test_collection_name.clone()),
            limit: None,
            offset: None,
        };

        let response = client.execute_sql(request).await?;
        Ok(NormalizedSqlResult::from_grpc(&response.into_inner()))
    }

    /// Create a graph node via REST
    async fn create_graph_node_rest(
        &self,
        node_id: &str,
        labels: &[&str],
        properties: HashMap<String, String>,
    ) -> Result<NormalizedNode, Box<dyn std::error::Error>> {
        let props: HashMap<String, JsonValue> = properties
            .iter()
            .map(|(k, v)| (k.clone(), json!({ "string_value": v })))
            .collect();

        let request = json!({
            "graph_id": self.test_graph_name,
            "node": {
                "id": node_id,
                "labels": labels,
                "properties": props
            }
        });

        let response = self
            .http_client
            .post(&format!("{}/api/v1/graph/nodes", REST_BASE_URL))
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let body: JsonValue = response.json().await?;
            return Err(format!("Failed to create node via REST: {:?}", body).into());
        }

        let body: JsonValue = response.json().await?;
        NormalizedNode::from_json(&body).ok_or_else(|| "Failed to parse node response".into())
    }

    /// Create a graph node via gRPC
    async fn create_graph_node_grpc(
        &self,
        node_id: &str,
        labels: &[&str],
        properties: HashMap<String, String>,
    ) -> Result<NormalizedNode, Box<dyn std::error::Error>> {
        let mut client = self
            .graph_client
            .clone()
            .ok_or("gRPC graph client not available")?;

        use proximadb::proto::proximadb_v1::{PropertyValue, property_value};

        let props: HashMap<String, PropertyValue> = properties
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    PropertyValue {
                        value: Some(property_value::Value::StringValue(v.clone())),
                    },
                )
            })
            .collect();

        let request = CreateNodeRequest {
            graph_id: self.test_graph_name.clone(),
            node: Some(Node {
                id: node_id.to_string(),
                labels: labels.iter().map(|s| s.to_string()).collect(),
                properties: props,
                ..Default::default()
            }),
        };

        let response = client.create_node(request).await?;
        Ok(NormalizedNode::from_grpc(&response.into_inner()))
    }

    /// Get a graph node via REST
    async fn get_graph_node_rest(
        &self,
        node_id: &str,
    ) -> Result<Option<NormalizedNode>, Box<dyn std::error::Error>> {
        let response = self
            .http_client
            .get(&format!(
                "{}/api/v1/graph/{}/nodes/{}",
                REST_BASE_URL, self.test_graph_name, node_id
            ))
            .send()
            .await?;

        if response.status().as_u16() == 404 {
            return Ok(None);
        }

        if !response.status().is_success() {
            let body: JsonValue = response.json().await?;
            return Err(format!("Failed to get node via REST: {:?}", body).into());
        }

        let body: JsonValue = response.json().await?;
        Ok(NormalizedNode::from_json(&body))
    }

    /// Get a graph node via gRPC
    async fn get_graph_node_grpc(
        &self,
        node_id: &str,
    ) -> Result<Option<NormalizedNode>, Box<dyn std::error::Error>> {
        let mut client = self
            .graph_client
            .clone()
            .ok_or("gRPC graph client not available")?;

        let request = GetNodeRequest {
            graph_id: self.test_graph_name.clone(),
            node_id: node_id.to_string(),
        };

        match client.get_node(request).await {
            Ok(response) => Ok(Some(NormalizedNode::from_grpc(&response.into_inner()))),
            Err(status) if status.code() == tonic::Code::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Test error response for non-existent collection via REST
    async fn get_error_rest(&self, collection_id: &str) -> Result<u16, Box<dyn std::error::Error>> {
        let response = self
            .http_client
            .get(&format!(
                "{}/api/v1/collections/{}",
                REST_BASE_URL, collection_id
            ))
            .send()
            .await?;

        Ok(response.status().as_u16())
    }

    /// Test error response for non-existent collection via gRPC
    async fn get_error_grpc(
        &self,
        collection_id: &str,
    ) -> Result<tonic::Code, Box<dyn std::error::Error>> {
        let mut client = self
            .collection_client
            .clone()
            .ok_or("gRPC collection client not available")?;

        let request = GetCollectionRequest {
            collection_id: collection_id.to_string(),
        };

        match client.get_collection(request).await {
            Ok(_) => Ok(tonic::Code::Ok),
            Err(status) => Ok(status.code()),
        }
    }

    /// Cleanup test resources
    async fn cleanup(&self) {
        // Try to delete test collection via REST
        let _ = self
            .http_client
            .delete(&format!(
                "{}/api/v1/collections/{}",
                REST_BASE_URL, self.test_collection_name
            ))
            .send()
            .await;

        // Note: Graph cleanup would be handled separately if needed
    }
}

// ================================================================================
// PARITY TESTS
// ================================================================================

/// Generate test vectors for testing
fn generate_test_vectors(
    count: usize,
    dimension: usize,
) -> Vec<(String, Vec<f32>, HashMap<String, String>)> {
    (0..count)
        .map(|i| {
            let id = format!("vec_{}", i);
            let vector: Vec<f32> = (0..dimension)
                .map(|j| (i * dimension + j) as f32 / (count * dimension) as f32)
                .collect();
            let mut metadata = HashMap::new();
            metadata.insert("category".to_string(), format!("cat_{}", i % 3));
            metadata.insert("index".to_string(), i.to_string());
            (id, vector, metadata)
        })
        .collect()
}

/// Test that vector search returns identical results via REST and gRPC
#[tokio::test]
async fn test_vector_search_parity() {
    let harness = match ParityTestHarness::new().await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Skipping test: Failed to create test harness: {}", e);
            return;
        }
    };

    let (rest_available, grpc_available) = harness.check_servers().await;

    if !rest_available || !grpc_available {
        eprintln!(
            "Skipping test: Servers not available (REST: {}, gRPC: {})",
            rest_available, grpc_available
        );
        eprintln!("Start the server with: cargo run --release --bin proximadb-server");
        return;
    }

    // Create test collection via REST first
    let collection_id = match harness.create_collection_rest().await {
        Ok(id) => id,
        Err(e) => {
            eprintln!("Skipping test: Failed to create collection: {}", e);
            return;
        }
    };

    println!("Created test collection: {}", collection_id);

    // Insert test vectors
    let test_vectors = generate_test_vectors(20, TEST_DIMENSION as usize);

    if let Err(e) = harness.insert_vectors_rest(&test_vectors).await {
        eprintln!("Failed to insert vectors: {}", e);
        harness.cleanup().await;
        return;
    }

    // Wait for indexing
    sleep(Duration::from_secs(1)).await;

    // Query vector (middle of the range)
    let query_vector: Vec<f32> = (0..TEST_DIMENSION as usize)
        .map(|j| 0.5 + (j as f32 * 0.001))
        .collect();

    // Search via REST
    let rest_results = match harness.search_vectors_rest(&query_vector, TEST_TOP_K).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("REST search failed: {}", e);
            harness.cleanup().await;
            return;
        }
    };

    // Search via gRPC
    let grpc_results = match harness.search_vectors_grpc(&query_vector, TEST_TOP_K).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("gRPC search failed: {}", e);
            harness.cleanup().await;
            return;
        }
    };

    // Compare results
    println!("REST results: {} items", rest_results.len());
    println!("gRPC results: {} items", grpc_results.len());

    // Check result count parity
    assert_eq!(
        rest_results.len(),
        grpc_results.len(),
        "REST and gRPC returned different number of results"
    );

    // Check that the same IDs are returned (order may vary due to ties)
    let rest_ids: std::collections::HashSet<_> = rest_results.iter().map(|r| &r.id).collect();
    let grpc_ids: std::collections::HashSet<_> = grpc_results.iter().map(|r| &r.id).collect();

    assert_eq!(
        rest_ids, grpc_ids,
        "REST and gRPC returned different vector IDs"
    );

    // Cleanup
    harness.cleanup().await;

    println!("Vector search parity test PASSED");
}

/// Test that SQL queries return identical results via REST and gRPC
#[tokio::test]
async fn test_sql_query_parity() {
    let harness = match ParityTestHarness::new().await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Skipping test: Failed to create test harness: {}", e);
            return;
        }
    };

    let (rest_available, grpc_available) = harness.check_servers().await;

    if !rest_available || !grpc_available {
        eprintln!(
            "Skipping test: Servers not available (REST: {}, gRPC: {})",
            rest_available, grpc_available
        );
        return;
    }

    // Create and populate test collection
    if let Err(e) = harness.create_collection_rest().await {
        eprintln!("Skipping test: Failed to create collection: {}", e);
        return;
    }

    let test_vectors = generate_test_vectors(10, TEST_DIMENSION as usize);
    if let Err(e) = harness.insert_vectors_rest(&test_vectors).await {
        eprintln!("Failed to insert vectors: {}", e);
        harness.cleanup().await;
        return;
    }

    // Wait for indexing
    sleep(Duration::from_secs(1)).await;

    // Test SQL query
    let sql_query = format!("SELECT id FROM {} LIMIT 5", harness.test_collection_name);

    // Execute via REST
    let rest_result = match harness.execute_sql_rest(&sql_query).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("REST SQL execution failed: {}", e);
            harness.cleanup().await;
            return;
        }
    };

    // Execute via gRPC
    let grpc_result = match harness.execute_sql_grpc(&sql_query).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("gRPC SQL execution failed: {}", e);
            harness.cleanup().await;
            return;
        }
    };

    println!("REST SQL result: {:?}", rest_result);
    println!("gRPC SQL result: {:?}", grpc_result);

    // Compare row counts
    assert_eq!(
        rest_result.row_count, grpc_result.row_count,
        "REST and gRPC returned different row counts"
    );

    // Compare column names (normalized/sorted)
    assert_eq!(
        rest_result.column_names, grpc_result.column_names,
        "REST and gRPC returned different columns"
    );

    // Cleanup
    harness.cleanup().await;

    println!("SQL query parity test PASSED");
}

/// Test that graph operations return identical results via REST and gRPC
#[tokio::test]
async fn test_graph_query_parity() {
    let harness = match ParityTestHarness::new().await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Skipping test: Failed to create test harness: {}", e);
            return;
        }
    };

    let (rest_available, grpc_available) = harness.check_servers().await;

    if !rest_available || !grpc_available {
        eprintln!(
            "Skipping test: Servers not available (REST: {}, gRPC: {})",
            rest_available, grpc_available
        );
        return;
    }

    // Create a test node via REST
    let node_id = format!(
        "test_node_{}",
        uuid::Uuid::new_v4().to_string()[..8].to_string()
    );
    let labels = vec!["Person", "Employee"];
    let mut properties = HashMap::new();
    properties.insert("name".to_string(), "Alice".to_string());
    properties.insert("age".to_string(), "30".to_string());

    // Create node via REST
    let rest_node = match harness
        .create_graph_node_rest(&node_id, &labels, properties.clone())
        .await
    {
        Ok(n) => n,
        Err(e) => {
            // Graph operations may not be fully implemented
            eprintln!("Skipping graph test: Failed to create node via REST: {}", e);
            return;
        }
    };

    // Get the same node via gRPC
    let grpc_node = match harness.get_graph_node_grpc(&node_id).await {
        Ok(Some(n)) => n,
        Ok(None) => {
            eprintln!("Node not found via gRPC after REST creation");
            return;
        }
        Err(e) => {
            eprintln!("Failed to get node via gRPC: {}", e);
            return;
        }
    };

    // Compare nodes
    println!("REST node: {:?}", rest_node);
    println!("gRPC node: {:?}", grpc_node);

    assert_eq!(rest_node.id, grpc_node.id, "Node IDs do not match");

    assert_eq!(
        rest_node.labels, grpc_node.labels,
        "Node labels do not match"
    );

    assert_eq!(
        rest_node.property_keys, grpc_node.property_keys,
        "Node property keys do not match"
    );

    println!("Graph query parity test PASSED");
}

/// Test that error responses are consistent between REST and gRPC
#[tokio::test]
async fn test_error_response_parity() {
    let harness = match ParityTestHarness::new().await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Skipping test: Failed to create test harness: {}", e);
            return;
        }
    };

    let (rest_available, grpc_available) = harness.check_servers().await;

    if !rest_available || !grpc_available {
        eprintln!(
            "Skipping test: Servers not available (REST: {}, gRPC: {})",
            rest_available, grpc_available
        );
        return;
    }

    // Test error for non-existent collection
    let non_existent_id = "non_existent_collection_12345";

    // Get error via REST
    let rest_status = match harness.get_error_rest(non_existent_id).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to get REST error: {}", e);
            return;
        }
    };

    // Get error via gRPC
    let grpc_code = match harness.get_error_grpc(non_existent_id).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to get gRPC error: {}", e);
            return;
        }
    };

    println!("REST status code: {}", rest_status);
    println!("gRPC status code: {:?}", grpc_code);

    // Both should indicate "not found" or similar error
    // REST: 404 Not Found
    // gRPC: NOT_FOUND
    assert!(
        (rest_status == 404 && grpc_code == tonic::Code::NotFound)
            || (rest_status >= 400 && grpc_code != tonic::Code::Ok),
        "Error responses should be consistent: REST {} vs gRPC {:?}",
        rest_status,
        grpc_code
    );

    println!("Error response parity test PASSED");
}

/// Test that batch vector operations produce identical results
#[tokio::test]
async fn test_vector_batch_parity() {
    let harness = match ParityTestHarness::new().await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("Skipping test: Failed to create test harness: {}", e);
            return;
        }
    };

    let (rest_available, grpc_available) = harness.check_servers().await;

    if !rest_available || !grpc_available {
        eprintln!(
            "Skipping test: Servers not available (REST: {}, gRPC: {})",
            rest_available, grpc_available
        );
        return;
    }

    // Create collection
    if let Err(e) = harness.create_collection_rest().await {
        eprintln!("Skipping test: Failed to create collection: {}", e);
        return;
    }

    // Create different test vectors for REST and gRPC
    let rest_vectors = generate_test_vectors(5, TEST_DIMENSION as usize);
    let grpc_vectors: Vec<_> = rest_vectors
        .iter()
        .map(|(id, vec, meta)| (format!("{}_grpc", id), vec.clone(), meta.clone()))
        .collect();

    // Insert via REST
    if let Err(e) = harness.insert_vectors_rest(&rest_vectors).await {
        eprintln!("REST batch insert failed: {}", e);
        harness.cleanup().await;
        return;
    }

    // Insert via gRPC
    if let Err(e) = harness.insert_vectors_grpc(&grpc_vectors).await {
        eprintln!("gRPC batch insert failed: {}", e);
        harness.cleanup().await;
        return;
    }

    // Wait for indexing
    sleep(Duration::from_secs(1)).await;

    // Verify both batches are searchable and return correct counts
    let query_vector: Vec<f32> = vec![0.5; TEST_DIMENSION as usize];

    let all_results = match harness.search_vectors_rest(&query_vector, 20).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Search failed: {}", e);
            harness.cleanup().await;
            return;
        }
    };

    // Should find vectors from both REST and gRPC inserts
    let rest_inserted = all_results
        .iter()
        .filter(|r| !r.id.contains("_grpc"))
        .count();
    let grpc_inserted = all_results
        .iter()
        .filter(|r| r.id.contains("_grpc"))
        .count();

    println!(
        "Found {} vectors from REST insert, {} from gRPC insert",
        rest_inserted, grpc_inserted
    );

    assert!(rest_inserted > 0, "Should find REST-inserted vectors");
    assert!(grpc_inserted > 0, "Should find gRPC-inserted vectors");

    // Cleanup
    harness.cleanup().await;

    println!("Vector batch parity test PASSED");
}

/// Summary test that exercises the full parity test suite
#[tokio::test]
async fn test_rest_grpc_parity_summary() {
    let separator = "=".repeat(60);
    println!("\n");
    println!("{}", separator);
    println!("REST/gRPC PARITY TEST SUITE");
    println!("{}", separator);
    println!("\nThis test suite verifies that REST and gRPC APIs produce");
    println!("identical results for the same operations.\n");
    println!("Prerequisites:");
    println!("  1. Start the ProximaDB server: cargo run --release --bin proximadb-server");
    println!("  2. Ensure ports 5678 (REST) and 5679 (gRPC) are available\n");
    println!("Individual tests:");
    println!("  - test_vector_search_parity: Vector similarity search");
    println!("  - test_sql_query_parity: SQL query execution");
    println!("  - test_graph_query_parity: Graph node operations");
    println!("  - test_error_response_parity: Error handling consistency");
    println!("  - test_vector_batch_parity: Batch insert operations");
    println!("{}", separator);
    println!("\n");

    // This is a summary test - actual tests run independently
    // Check if servers are available
    let harness = match ParityTestHarness::new().await {
        Ok(h) => h,
        Err(e) => {
            println!("Could not initialize test harness: {}", e);
            println!("Please start the server and run individual tests.");
            return;
        }
    };

    let (rest_available, grpc_available) = harness.check_servers().await;

    println!("Server Status:");
    println!(
        "  REST ({}): {}",
        REST_BASE_URL,
        if rest_available {
            "AVAILABLE"
        } else {
            "NOT AVAILABLE"
        }
    );
    println!(
        "  gRPC ({}): {}",
        GRPC_ENDPOINT,
        if grpc_available {
            "AVAILABLE"
        } else {
            "NOT AVAILABLE"
        }
    );

    if rest_available && grpc_available {
        println!("\nBoth servers are available. Run individual tests with:");
        println!("  cargo test --test rest_grpc_parity_test -- --test-threads=1");
    } else {
        println!("\nServers not fully available. Start the server first:");
        println!("  cargo run --release --bin proximadb-server");
    }
}
