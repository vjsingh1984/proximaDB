/*
 * Copyright 2025 ProximaDB
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

//! Transaction Operations for Different Data Models

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

/// Operations for vector data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VectorOperation {
    /// Insert vectors
    Insert {
        collection: String,
        ids: Vec<String>,
        vectors: Vec<Vec<f32>>,
        metadata: Vec<HashMap<String, serde_json::Value>>,
    },
    /// Update vectors
    Update {
        collection: String,
        ids: Vec<String>,
        vectors: Option<Vec<Vec<f32>>>,
        metadata: Option<Vec<HashMap<String, serde_json::Value>>>,
    },
    /// Delete vectors
    Delete {
        collection: String,
        ids: Vec<String>,
    },
    /// Upsert vectors
    Upsert {
        collection: String,
        ids: Vec<String>,
        vectors: Vec<Vec<f32>>,
        metadata: Vec<HashMap<String, serde_json::Value>>,
    },
}

impl VectorOperation {
    /// Get collection name for this operation
    pub fn collection(&self) -> &str {
        match self {
            VectorOperation::Insert { collection, .. } => collection,
            VectorOperation::Update { collection, .. } => collection,
            VectorOperation::Delete { collection, .. } => collection,
            VectorOperation::Upsert { collection, .. } => collection,
        }
    }

    /// Get affected IDs
    pub fn affected_ids(&self) -> &[String] {
        match self {
            VectorOperation::Insert { ids, .. } => ids,
            VectorOperation::Update { ids, .. } => ids,
            VectorOperation::Delete { ids, .. } => ids,
            VectorOperation::Upsert { ids, .. } => ids,
        }
    }

    /// Check if this is a write operation
    pub fn is_write(&self) -> bool {
        true // All vector operations are writes
    }
}

/// Operations for document data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DocumentOperation {
    /// Insert documents
    Insert {
        collection: String,
        documents: Vec<serde_json::Value>,
    },
    /// Update documents
    Update {
        collection: String,
        filter: serde_json::Value,
        update: serde_json::Value,
        upsert: bool,
    },
    /// Delete documents
    Delete {
        collection: String,
        filter: serde_json::Value,
    },
    /// Replace document
    Replace {
        collection: String,
        id: String,
        document: serde_json::Value,
    },
}

impl DocumentOperation {
    /// Get collection name
    pub fn collection(&self) -> &str {
        match self {
            DocumentOperation::Insert { collection, .. } => collection,
            DocumentOperation::Update { collection, .. } => collection,
            DocumentOperation::Delete { collection, .. } => collection,
            DocumentOperation::Replace { collection, .. } => collection,
        }
    }

    /// Check if this is a write operation
    pub fn is_write(&self) -> bool {
        true
    }
}

/// Operations for graph data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GraphOperation {
    /// Create node
    CreateNode {
        graph: String,
        id: String,
        label: String,
        properties: HashMap<String, serde_json::Value>,
    },
    /// Update node
    UpdateNode {
        graph: String,
        id: String,
        properties: HashMap<String, serde_json::Value>,
    },
    /// Delete node
    DeleteNode {
        graph: String,
        id: String,
    },
    /// Create edge
    CreateEdge {
        graph: String,
        source: String,
        target: String,
        edge_type: String,
        properties: HashMap<String, serde_json::Value>,
    },
    /// Update edge
    UpdateEdge {
        graph: String,
        source: String,
        target: String,
        edge_type: String,
        properties: HashMap<String, serde_json::Value>,
    },
    /// Delete edge
    DeleteEdge {
        graph: String,
        source: String,
        target: String,
        edge_type: Option<String>,
    },
}

impl GraphOperation {
    /// Get graph name
    pub fn graph(&self) -> &str {
        match self {
            GraphOperation::CreateNode { graph, .. } => graph,
            GraphOperation::UpdateNode { graph, .. } => graph,
            GraphOperation::DeleteNode { graph, .. } => graph,
            GraphOperation::CreateEdge { graph, .. } => graph,
            GraphOperation::UpdateEdge { graph, .. } => graph,
            GraphOperation::DeleteEdge { graph, .. } => graph,
        }
    }

    /// Get affected node IDs
    pub fn affected_node_ids(&self) -> Vec<&str> {
        match self {
            GraphOperation::CreateNode { id, .. } => vec![id],
            GraphOperation::UpdateNode { id, .. } => vec![id],
            GraphOperation::DeleteNode { id, .. } => vec![id],
            GraphOperation::CreateEdge { source, target, .. } => vec![source, target],
            GraphOperation::UpdateEdge { source, target, .. } => vec![source, target],
            GraphOperation::DeleteEdge { source, target, .. } => vec![source, target],
        }
    }

    /// Check if this is a write operation
    pub fn is_write(&self) -> bool {
        true
    }
}

/// Operations for observability data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ObservabilityOperation {
    /// Ingest logs
    IngestLogs {
        namespace: String,
        logs: Vec<serde_json::Value>,
    },
    /// Ingest metrics
    IngestMetrics {
        namespace: String,
        metrics: Vec<serde_json::Value>,
    },
    /// Ingest traces
    IngestTraces {
        namespace: String,
        traces: Vec<serde_json::Value>,
    },
    /// Delete logs
    DeleteLogs {
        namespace: String,
        filter: serde_json::Value,
    },
}

impl ObservabilityOperation {
    /// Get namespace
    pub fn namespace(&self) -> &str {
        match self {
            ObservabilityOperation::IngestLogs { namespace, .. } => namespace,
            ObservabilityOperation::IngestMetrics { namespace, .. } => namespace,
            ObservabilityOperation::IngestTraces { namespace, .. } => namespace,
            ObservabilityOperation::DeleteLogs { namespace, .. } => namespace,
        }
    }

    /// Check if this is a write operation
    pub fn is_write(&self) -> bool {
        true
    }
}

/// Unified operation type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MultiModelOperation {
    /// Vector store operation
    Vector(VectorOperation),
    /// Document store operation
    Document(DocumentOperation),
    /// Graph store operation
    Graph(GraphOperation),
    /// Observability store operation
    Observability(ObservabilityOperation),
}

impl MultiModelOperation {
    /// Get the data model type
    pub fn model_type(&self) -> &'static str {
        match self {
            MultiModelOperation::Vector(_) => "vector",
            MultiModelOperation::Document(_) => "document",
            MultiModelOperation::Graph(_) => "graph",
            MultiModelOperation::Observability(_) => "observability",
        }
    }

    /// Get the target container (collection/graph/namespace)
    pub fn target(&self) -> &str {
        match self {
            MultiModelOperation::Vector(op) => op.collection(),
            MultiModelOperation::Document(op) => op.collection(),
            MultiModelOperation::Graph(op) => op.graph(),
            MultiModelOperation::Observability(op) => op.namespace(),
        }
    }

    /// Check if this is a write operation
    pub fn is_write(&self) -> bool {
        match self {
            MultiModelOperation::Vector(op) => op.is_write(),
            MultiModelOperation::Document(op) => op.is_write(),
            MultiModelOperation::Graph(op) => op.is_write(),
            MultiModelOperation::Observability(op) => op.is_write(),
        }
    }
}

/// Rollback information for an operation
#[derive(Debug, Clone)]
pub struct OperationRollback {
    /// Original operation
    pub operation: MultiModelOperation,
    /// Compensating operation (inverse)
    pub compensation: Option<MultiModelOperation>,
    /// Snapshot data for restore
    pub snapshot_data: Option<Vec<u8>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_operation() {
        let op = VectorOperation::Insert {
            collection: "embeddings".to_string(),
            ids: vec!["v1".to_string(), "v2".to_string()],
            vectors: vec![vec![0.1, 0.2], vec![0.3, 0.4]],
            metadata: vec![HashMap::new(), HashMap::new()],
        };

        assert_eq!(op.collection(), "embeddings");
        assert_eq!(op.affected_ids().len(), 2);
        assert!(op.is_write());
    }

    #[test]
    fn test_document_operation() {
        let op = DocumentOperation::Insert {
            collection: "users".to_string(),
            documents: vec![serde_json::json!({"name": "Alice"})],
        };

        assert_eq!(op.collection(), "users");
        assert!(op.is_write());
    }

    #[test]
    fn test_graph_operation() {
        let op = GraphOperation::CreateEdge {
            graph: "social".to_string(),
            source: "user1".to_string(),
            target: "user2".to_string(),
            edge_type: "FOLLOWS".to_string(),
            properties: HashMap::new(),
        };

        assert_eq!(op.graph(), "social");
        assert_eq!(op.affected_node_ids().len(), 2);
    }

    #[test]
    fn test_multi_model_operation() {
        let vector_op = MultiModelOperation::Vector(VectorOperation::Insert {
            collection: "test".to_string(),
            ids: vec![],
            vectors: vec![],
            metadata: vec![],
        });

        assert_eq!(vector_op.model_type(), "vector");
        assert_eq!(vector_op.target(), "test");
    }
}
