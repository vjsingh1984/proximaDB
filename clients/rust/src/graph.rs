//! Graph operations for ProximaDB
//!
//! Provides a fluent API for graph database operations including
//! node and edge management, traversal queries, and graph analytics.
//!
//! # Examples
//!
//! ```rust,ignore
//! use proximadb_sdk::{ProximaClient, GraphBuilder};
//!
//! let client = ProximaClient::connect("http://localhost:5678")?;
//!
//! // Create a graph
//! client.create_graph("knowledge")
//!     .execute()
//!     .await?;
//!
//! // Add nodes with fluent API
//! client.graph("knowledge")
//!     .add_node()
//!     .id("person_1")
//!     .label("Person")
//!     .property("name", "Alice")
//!     .property("age", 30)
//!     .execute()
//!     .await?;
//!
//! // Add edges
//! client.graph("knowledge")
//!     .add_edge()
//!     .from("person_1")
//!     .to("person_2")
//!     .relationship("KNOWS")
//!     .property("since", "2020")
//!     .execute()
//!     .await?;
//!
//! // Traverse the graph
//! let results = client.graph("knowledge")
//!     .traverse()
//!     .start("person_1")
//!     .relationship("KNOWS")
//!     .max_depth(3)
//!     .execute()
//!     .await?;
//! ```

use crate::error::{ProximaError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A graph node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    /// Unique node identifier
    pub id: String,
    /// Node label (type)
    #[serde(default)]
    pub label: Option<String>,
    /// Node properties
    #[serde(default)]
    pub properties: HashMap<String, serde_json::Value>,
    /// Optional embedding vector for semantic operations
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vector: Option<Vec<f32>>,
}

impl GraphNode {
    /// Create a new graph node
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: None,
            properties: HashMap::new(),
            vector: None,
        }
    }

    /// Set the node label
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Add a property
    pub fn with_property(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.properties.insert(key.into(), value.into());
        self
    }

    /// Set the embedding vector
    pub fn with_vector(mut self, vector: Vec<f32>) -> Self {
        self.vector = Some(vector);
        self
    }
}

/// A graph edge
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    /// Source node ID
    pub source: String,
    /// Target node ID
    pub target: String,
    /// Relationship type
    pub relationship: String,
    /// Edge properties
    #[serde(default)]
    pub properties: HashMap<String, serde_json::Value>,
    /// Optional weight
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
}

impl GraphEdge {
    /// Create a new graph edge
    pub fn new(
        source: impl Into<String>,
        target: impl Into<String>,
        relationship: impl Into<String>,
    ) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            relationship: relationship.into(),
            properties: HashMap::new(),
            weight: None,
        }
    }

    /// Add a property
    pub fn with_property(
        mut self,
        key: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.properties.insert(key.into(), value.into());
        self
    }

    /// Set the edge weight
    pub fn with_weight(mut self, weight: f64) -> Self {
        self.weight = Some(weight);
        self
    }
}

/// Traversal direction
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum TraversalDirection {
    /// Traverse outgoing edges
    #[default]
    Outgoing,
    /// Traverse incoming edges
    Incoming,
    /// Traverse both directions
    Both,
}

/// Graph traversal result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraversalResult {
    /// Nodes in the traversal path
    pub nodes: Vec<GraphNode>,
    /// Edges in the traversal path
    pub edges: Vec<GraphEdge>,
    /// Path from start to each node
    #[serde(default)]
    pub paths: Vec<Vec<String>>,
}

/// Handle to a graph for fluent operations
pub struct GraphHandle<'a> {
    #[cfg(feature = "client")]
    client: &'a crate::client::ProximaClient,
    name: String,
}

impl<'a> GraphHandle<'a> {
    /// Create a new graph handle
    #[cfg(feature = "client")]
    pub fn new(client: &'a crate::client::ProximaClient, name: &str) -> Self {
        Self {
            client,
            name: name.to_string(),
        }
    }

    /// Get the graph name
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Start building a node addition
    #[cfg(feature = "client")]
    pub fn add_node(&'a self) -> NodeBuilder<'a> {
        NodeBuilder::new(self)
    }

    /// Start building an edge addition
    #[cfg(feature = "client")]
    pub fn add_edge(&'a self) -> EdgeBuilder<'a> {
        EdgeBuilder::new(self)
    }

    /// Start building a traversal query
    #[cfg(feature = "client")]
    pub fn traverse(&'a self) -> TraversalBuilder<'a> {
        TraversalBuilder::new(self)
    }

    /// Add a batch of nodes
    #[cfg(feature = "client")]
    pub async fn add_nodes(&self, nodes: Vec<GraphNode>) -> Result<usize> {
        let request = AddNodesRequest {
            graph: self.name.clone(),
            nodes,
        };
        let url = format!("{}/api/v1/graphs/{}/nodes", self.client.url(), self.name);
        let response: AddNodesResponse = self.client.post(&url, &request).await?;
        Ok(response.added_count)
    }

    /// Add a batch of edges
    #[cfg(feature = "client")]
    pub async fn add_edges(&self, edges: Vec<GraphEdge>) -> Result<usize> {
        let request = AddEdgesRequest {
            graph: self.name.clone(),
            edges,
        };
        let url = format!("{}/api/v1/graphs/{}/edges", self.client.url(), self.name);
        let response: AddEdgesResponse = self.client.post(&url, &request).await?;
        Ok(response.added_count)
    }

    /// Get a node by ID
    #[cfg(feature = "client")]
    pub async fn get_node(&self, id: &str) -> Result<Option<GraphNode>> {
        let url = format!(
            "{}/api/v1/graphs/{}/nodes/{}",
            self.client.url(),
            self.name,
            id
        );
        match self.client.get::<GraphNode>(&url).await {
            Ok(node) => Ok(Some(node)),
            Err(ProximaError::Network(crate::error::NetworkError::HttpError {
                status: 404,
                ..
            })) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Delete a node by ID
    #[cfg(feature = "client")]
    pub async fn delete_node(&self, id: &str) -> Result<()> {
        let url = format!(
            "{}/api/v1/graphs/{}/nodes/{}",
            self.client.url(),
            self.name,
            id
        );
        self.client.delete::<serde_json::Value>(&url).await?;
        Ok(())
    }

    /// Delete an edge
    #[cfg(feature = "client")]
    pub async fn delete_edge(&self, source: &str, target: &str, relationship: &str) -> Result<()> {
        let url = format!(
            "{}/api/v1/graphs/{}/edges/{}/{}/{}",
            self.client.url(),
            self.name,
            source,
            target,
            relationship
        );
        self.client.delete::<serde_json::Value>(&url).await?;
        Ok(())
    }
}

/// Builder for adding nodes
pub struct NodeBuilder<'a> {
    handle: &'a GraphHandle<'a>,
    id: Option<String>,
    label: Option<String>,
    properties: HashMap<String, serde_json::Value>,
    vector: Option<Vec<f32>>,
}

impl<'a> NodeBuilder<'a> {
    fn new(handle: &'a GraphHandle<'a>) -> Self {
        Self {
            handle,
            id: None,
            label: None,
            properties: HashMap::new(),
            vector: None,
        }
    }

    /// Set the node ID
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the node label
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Add a property
    pub fn property(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.properties.insert(key.into(), value.into());
        self
    }

    /// Set the embedding vector
    pub fn vector(mut self, vector: &[f32]) -> Self {
        self.vector = Some(vector.to_vec());
        self
    }

    /// Execute the node addition
    #[cfg(feature = "client")]
    pub async fn execute(self) -> Result<()> {
        let id = self
            .id
            .ok_or_else(|| ProximaError::Internal("Node ID is required".to_string()))?;

        let node = GraphNode {
            id,
            label: self.label,
            properties: self.properties,
            vector: self.vector,
        };

        let request = AddNodesRequest {
            graph: self.handle.name.clone(),
            nodes: vec![node],
        };

        let url = format!(
            "{}/api/v1/graphs/{}/nodes",
            self.handle.client.url(),
            self.handle.name
        );
        let _response: AddNodesResponse = self.handle.client.post(&url, &request).await?;
        Ok(())
    }
}

/// Builder for adding edges
pub struct EdgeBuilder<'a> {
    handle: &'a GraphHandle<'a>,
    source: Option<String>,
    target: Option<String>,
    relationship: Option<String>,
    properties: HashMap<String, serde_json::Value>,
    weight: Option<f64>,
}

impl<'a> EdgeBuilder<'a> {
    fn new(handle: &'a GraphHandle<'a>) -> Self {
        Self {
            handle,
            source: None,
            target: None,
            relationship: None,
            properties: HashMap::new(),
            weight: None,
        }
    }

    /// Set the source node ID
    pub fn from(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Set the target node ID
    pub fn to(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Set the relationship type
    pub fn relationship(mut self, rel: impl Into<String>) -> Self {
        self.relationship = Some(rel.into());
        self
    }

    /// Add a property
    pub fn property(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
        self.properties.insert(key.into(), value.into());
        self
    }

    /// Set the edge weight
    pub fn weight(mut self, weight: f64) -> Self {
        self.weight = Some(weight);
        self
    }

    /// Execute the edge addition
    #[cfg(feature = "client")]
    pub async fn execute(self) -> Result<()> {
        let source = self
            .source
            .ok_or_else(|| ProximaError::Internal("Source node ID is required".to_string()))?;
        let target = self
            .target
            .ok_or_else(|| ProximaError::Internal("Target node ID is required".to_string()))?;
        let relationship = self
            .relationship
            .ok_or_else(|| ProximaError::Internal("Relationship type is required".to_string()))?;

        let edge = GraphEdge {
            source,
            target,
            relationship,
            properties: self.properties,
            weight: self.weight,
        };

        let request = AddEdgesRequest {
            graph: self.handle.name.clone(),
            edges: vec![edge],
        };

        let url = format!(
            "{}/api/v1/graphs/{}/edges",
            self.handle.client.url(),
            self.handle.name
        );
        let _response: AddEdgesResponse = self.handle.client.post(&url, &request).await?;
        Ok(())
    }
}

/// Builder for graph traversal queries
pub struct TraversalBuilder<'a> {
    handle: &'a GraphHandle<'a>,
    start_node: Option<String>,
    relationships: Vec<String>,
    direction: TraversalDirection,
    max_depth: usize,
    limit: usize,
    filter: Option<String>,
}

impl<'a> TraversalBuilder<'a> {
    fn new(handle: &'a GraphHandle<'a>) -> Self {
        Self {
            handle,
            start_node: None,
            relationships: Vec::new(),
            direction: TraversalDirection::default(),
            max_depth: 3,
            limit: 100,
            filter: None,
        }
    }

    /// Set the starting node
    pub fn start(mut self, node_id: impl Into<String>) -> Self {
        self.start_node = Some(node_id.into());
        self
    }

    /// Add a relationship type to follow
    pub fn relationship(mut self, rel: impl Into<String>) -> Self {
        self.relationships.push(rel.into());
        self
    }

    /// Add multiple relationship types
    pub fn relationships(mut self, rels: Vec<String>) -> Self {
        self.relationships.extend(rels);
        self
    }

    /// Set the traversal direction
    pub fn direction(mut self, dir: TraversalDirection) -> Self {
        self.direction = dir;
        self
    }

    /// Traverse outgoing edges only
    pub fn outgoing(mut self) -> Self {
        self.direction = TraversalDirection::Outgoing;
        self
    }

    /// Traverse incoming edges only
    pub fn incoming(mut self) -> Self {
        self.direction = TraversalDirection::Incoming;
        self
    }

    /// Traverse both directions
    pub fn both(mut self) -> Self {
        self.direction = TraversalDirection::Both;
        self
    }

    /// Set the maximum traversal depth
    pub fn max_depth(mut self, depth: usize) -> Self {
        self.max_depth = depth;
        self
    }

    /// Set the maximum number of results
    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Add a filter expression for nodes
    pub fn filter(mut self, filter: impl Into<String>) -> Self {
        self.filter = Some(filter.into());
        self
    }

    /// Execute the traversal
    #[cfg(feature = "client")]
    pub async fn execute(self) -> Result<TraversalResult> {
        let start_node = self.start_node.ok_or_else(|| {
            ProximaError::Internal("Start node is required for traversal".to_string())
        })?;

        let request = TraversalRequest {
            graph: self.handle.name.clone(),
            start_node,
            relationships: if self.relationships.is_empty() {
                None
            } else {
                Some(self.relationships)
            },
            direction: self.direction,
            max_depth: self.max_depth,
            limit: self.limit,
            filter: self.filter,
        };

        let url = format!(
            "{}/api/v1/graphs/{}/traverse",
            self.handle.client.url(),
            self.handle.name
        );
        self.handle.client.post(&url, &request).await
    }
}

/// Builder for creating graphs
pub struct GraphBuilder<'a> {
    #[cfg(feature = "client")]
    client: &'a crate::client::ProximaClient,
    name: String,
    description: Option<String>,
}

impl<'a> GraphBuilder<'a> {
    /// Create a new graph builder
    #[cfg(feature = "client")]
    pub fn new(client: &'a crate::client::ProximaClient, name: &str) -> Self {
        Self {
            client,
            name: name.to_string(),
            description: None,
        }
    }

    /// Set the graph description
    pub fn description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Execute the graph creation
    #[cfg(feature = "client")]
    pub async fn execute(self) -> Result<()> {
        let request = CreateGraphRequest {
            name: self.name,
            description: self.description,
        };

        let url = format!("{}/api/v1/graphs", self.client.url());
        let _response: CreateGraphResponse = self.client.post(&url, &request).await?;
        Ok(())
    }
}

// Request/Response types for HTTP API

#[derive(Debug, Serialize)]
struct CreateGraphRequest {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateGraphResponse {
    #[allow(dead_code)]
    success: bool,
}

#[derive(Debug, Serialize)]
struct AddNodesRequest {
    #[allow(dead_code)]
    graph: String,
    nodes: Vec<GraphNode>,
}

#[derive(Debug, Deserialize)]
struct AddNodesResponse {
    added_count: usize,
}

#[derive(Debug, Serialize)]
struct AddEdgesRequest {
    #[allow(dead_code)]
    graph: String,
    edges: Vec<GraphEdge>,
}

#[derive(Debug, Deserialize)]
struct AddEdgesResponse {
    added_count: usize,
}

#[derive(Debug, Serialize)]
struct TraversalRequest {
    #[allow(dead_code)]
    graph: String,
    start_node: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    relationships: Option<Vec<String>>,
    direction: TraversalDirection,
    max_depth: usize,
    limit: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    filter: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ProximaClient;
    use serde_json::json;

    #[test]
    fn test_graph_node_builder() {
        let node = GraphNode::new("node_1")
            .with_label("Person")
            .with_property("name", "Alice")
            .with_property("age", 30);

        assert_eq!(node.id, "node_1");
        assert_eq!(node.label, Some("Person".to_string()));
        assert_eq!(
            node.properties.get("name"),
            Some(&serde_json::json!("Alice"))
        );
        assert_eq!(node.properties.get("age"), Some(&serde_json::json!(30)));
    }

    #[test]
    fn test_graph_edge_builder() {
        let edge = GraphEdge::new("person_1", "person_2", "KNOWS")
            .with_property("since", "2020")
            .with_weight(0.9);

        assert_eq!(edge.source, "person_1");
        assert_eq!(edge.target, "person_2");
        assert_eq!(edge.relationship, "KNOWS");
        assert_eq!(edge.weight, Some(0.9));
    }

    #[test]
    fn test_traversal_direction() {
        let dir = TraversalDirection::default();
        assert_eq!(dir, TraversalDirection::Outgoing);
    }

    #[test]
    fn graph_node_serialization_skips_absent_vector_and_defaults_fields() {
        let node: GraphNode = serde_json::from_value(json!({"id": "n1"})).unwrap();
        assert_eq!(node.id, "n1");
        assert_eq!(node.label, None);
        assert!(node.properties.is_empty());
        assert_eq!(node.vector, None);

        let serialized = serde_json::to_value(GraphNode::new("n2")).unwrap();
        assert_eq!(
            serialized,
            json!({"id": "n2", "label": null, "properties": {}})
        );

        let with_vector = GraphNode::new("n3").with_vector(vec![0.1, 0.2]);
        assert_eq!(with_vector.vector, Some(vec![0.1, 0.2]));
    }

    #[test]
    fn graph_edge_serialization_skips_absent_weight_and_defaults_properties() {
        let edge: GraphEdge = serde_json::from_value(json!({
            "source": "a",
            "target": "b",
            "relationship": "KNOWS"
        }))
        .unwrap();

        assert_eq!(edge.source, "a");
        assert_eq!(edge.target, "b");
        assert_eq!(edge.relationship, "KNOWS");
        assert!(edge.properties.is_empty());
        assert_eq!(edge.weight, None);

        let serialized = serde_json::to_value(edge).unwrap();
        assert_eq!(
            serialized,
            json!({"source": "a", "target": "b", "relationship": "KNOWS", "properties": {}})
        );
    }

    #[test]
    fn traversal_direction_serializes_as_lowercase_api_value() {
        assert_eq!(
            serde_json::to_value(TraversalDirection::Outgoing).unwrap(),
            json!("outgoing")
        );
        assert_eq!(
            serde_json::to_value(TraversalDirection::Incoming).unwrap(),
            json!("incoming")
        );
        assert_eq!(
            serde_json::to_value(TraversalDirection::Both).unwrap(),
            json!("both")
        );
    }

    #[test]
    fn graph_handle_and_builders_record_fluent_state() {
        let client = ProximaClient::for_tests("http://localhost:5678");
        let handle = GraphHandle::new(&client, "knowledge");

        assert_eq!(handle.name(), "knowledge");

        let node = handle
            .add_node()
            .id("person_1")
            .label("Person")
            .property("name", "Alice")
            .vector(&[0.1, 0.2]);
        assert_eq!(node.id.as_deref(), Some("person_1"));
        assert_eq!(node.label.as_deref(), Some("Person"));
        assert_eq!(node.properties["name"], json!("Alice"));
        assert_eq!(node.vector, Some(vec![0.1, 0.2]));

        let edge = handle
            .add_edge()
            .from("person_1")
            .to("person_2")
            .relationship("KNOWS")
            .property("since", 2020)
            .weight(0.5);
        assert_eq!(edge.source.as_deref(), Some("person_1"));
        assert_eq!(edge.target.as_deref(), Some("person_2"));
        assert_eq!(edge.relationship.as_deref(), Some("KNOWS"));
        assert_eq!(edge.properties["since"], json!(2020));
        assert_eq!(edge.weight, Some(0.5));

        let traversal = handle
            .traverse()
            .start("person_1")
            .relationship("KNOWS")
            .relationships(vec!["LIKES".to_string()])
            .incoming()
            .outgoing()
            .both()
            .direction(TraversalDirection::Incoming)
            .max_depth(4)
            .limit(25)
            .filter("age > 30");
        assert_eq!(traversal.start_node.as_deref(), Some("person_1"));
        assert_eq!(
            traversal.relationships,
            vec!["KNOWS".to_string(), "LIKES".to_string()]
        );
        assert_eq!(traversal.direction, TraversalDirection::Incoming);
        assert_eq!(traversal.max_depth, 4);
        assert_eq!(traversal.limit, 25);
        assert_eq!(traversal.filter.as_deref(), Some("age > 30"));
    }

    #[tokio::test]
    async fn node_edge_and_traversal_builders_validate_required_fields_before_network() {
        let client = ProximaClient::for_tests("http://localhost:5678");
        let handle = GraphHandle::new(&client, "knowledge");

        let node_error = handle.add_node().execute().await.unwrap_err();
        assert!(
            matches!(node_error, ProximaError::Internal(message) if message == "Node ID is required")
        );

        let edge_error = handle.add_edge().execute().await.unwrap_err();
        assert!(
            matches!(edge_error, ProximaError::Internal(message) if message == "Source node ID is required")
        );

        let edge_error = handle.add_edge().from("a").execute().await.unwrap_err();
        assert!(
            matches!(edge_error, ProximaError::Internal(message) if message == "Target node ID is required")
        );

        let edge_error = handle
            .add_edge()
            .from("a")
            .to("b")
            .execute()
            .await
            .unwrap_err();
        assert!(
            matches!(edge_error, ProximaError::Internal(message) if message == "Relationship type is required")
        );

        let traversal_error = handle.traverse().execute().await.unwrap_err();
        assert!(
            matches!(traversal_error, ProximaError::Internal(message) if message == "Start node is required for traversal")
        );
    }

    #[test]
    fn graph_request_response_dtos_match_api_shape() {
        let create = CreateGraphRequest {
            name: "knowledge".to_string(),
            description: Some("domain graph".to_string()),
        };
        assert_eq!(
            serde_json::to_value(create).unwrap(),
            json!({"name": "knowledge", "description": "domain graph"})
        );

        let node_request = AddNodesRequest {
            graph: "knowledge".to_string(),
            nodes: vec![GraphNode::new("n1").with_label("Person")],
        };
        assert_eq!(node_request.graph, "knowledge");
        assert_eq!(node_request.nodes[0].id, "n1");

        let edge_request = AddEdgesRequest {
            graph: "knowledge".to_string(),
            edges: vec![GraphEdge::new("a", "b", "KNOWS")],
        };
        assert_eq!(edge_request.graph, "knowledge");
        assert_eq!(edge_request.edges[0].relationship, "KNOWS");

        let traversal = TraversalRequest {
            graph: "knowledge".to_string(),
            start_node: "n1".to_string(),
            relationships: None,
            direction: TraversalDirection::Both,
            max_depth: 3,
            limit: 10,
            filter: None,
        };
        assert_eq!(
            serde_json::to_value(traversal).unwrap(),
            json!({
                "graph": "knowledge",
                "start_node": "n1",
                "direction": "both",
                "max_depth": 3,
                "limit": 10
            })
        );

        let created: CreateGraphResponse =
            serde_json::from_value(json!({"success": true})).unwrap();
        assert!(created.success);
        let nodes: AddNodesResponse = serde_json::from_value(json!({"added_count": 2})).unwrap();
        assert_eq!(nodes.added_count, 2);
        let edges: AddEdgesResponse = serde_json::from_value(json!({"added_count": 1})).unwrap();
        assert_eq!(edges.added_count, 1);
    }

    #[test]
    fn traversal_result_deserializes_default_paths() {
        let result: TraversalResult = serde_json::from_value(json!({
            "nodes": [{"id": "n1"}],
            "edges": []
        }))
        .unwrap();

        assert_eq!(result.nodes.len(), 1);
        assert!(result.edges.is_empty());
        assert!(result.paths.is_empty());
    }

    #[test]
    fn graph_builder_records_name_and_description() {
        let client = ProximaClient::for_tests("http://localhost:5678");
        let builder = GraphBuilder::new(&client, "knowledge").description("domain graph");

        assert_eq!(builder.name, "knowledge");
        assert_eq!(builder.description.as_deref(), Some("domain graph"));
    }
}
