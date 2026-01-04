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
    pub fn with_property(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
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
    pub fn new(source: impl Into<String>, target: impl Into<String>, relationship: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            relationship: relationship.into(),
            properties: HashMap::new(),
            weight: None,
        }
    }

    /// Add a property
    pub fn with_property(mut self, key: impl Into<String>, value: impl Into<serde_json::Value>) -> Self {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TraversalDirection {
    /// Traverse outgoing edges
    Outgoing,
    /// Traverse incoming edges
    Incoming,
    /// Traverse both directions
    Both,
}

impl Default for TraversalDirection {
    fn default() -> Self {
        TraversalDirection::Outgoing
    }
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
        let url = format!("{}/api/v1/graphs/{}/nodes/{}", self.client.url(), self.name, id);
        match self.client.get::<GraphNode>(&url).await {
            Ok(node) => Ok(Some(node)),
            Err(ProximaError::Network(crate::error::NetworkError::HttpError { status: 404, .. })) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Delete a node by ID
    #[cfg(feature = "client")]
    pub async fn delete_node(&self, id: &str) -> Result<()> {
        let url = format!("{}/api/v1/graphs/{}/nodes/{}", self.client.url(), self.name, id);
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
        let id = self.id.ok_or_else(|| {
            ProximaError::Internal("Node ID is required".to_string())
        })?;

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

        let url = format!("{}/api/v1/graphs/{}/nodes", self.handle.client.url(), self.handle.name);
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
        let source = self.source.ok_or_else(|| {
            ProximaError::Internal("Source node ID is required".to_string())
        })?;
        let target = self.target.ok_or_else(|| {
            ProximaError::Internal("Target node ID is required".to_string())
        })?;
        let relationship = self.relationship.ok_or_else(|| {
            ProximaError::Internal("Relationship type is required".to_string())
        })?;

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

        let url = format!("{}/api/v1/graphs/{}/edges", self.handle.client.url(), self.handle.name);
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

        let url = format!("{}/api/v1/graphs/{}/traverse", self.handle.client.url(), self.handle.name);
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

    #[test]
    fn test_graph_node_builder() {
        let node = GraphNode::new("node_1")
            .with_label("Person")
            .with_property("name", "Alice")
            .with_property("age", 30);

        assert_eq!(node.id, "node_1");
        assert_eq!(node.label, Some("Person".to_string()));
        assert_eq!(node.properties.get("name"), Some(&serde_json::json!("Alice")));
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
}
