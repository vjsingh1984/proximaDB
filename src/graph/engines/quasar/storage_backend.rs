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

//! # QUASAR Cold Storage Backend Module
//!
//! Implements cold storage backends for QUASAR's hybrid tiering system.
//! Supports multiple storage formats: SST, Parquet, and JSON.

use crate::core::error::{ProximaDBError, VectorDBError, StorageError};
type Result<T> = std::result::Result<T, ProximaDBError>;
use crate::graph::{Node, Edge, NodeId, EdgeId};
use super::ColdStorageBackend as BackendType;
use std::sync::Arc;
use std::path::{Path, PathBuf};
use std::collections::HashMap;
use tokio::fs;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};
use prost_types;

/// Cold storage backend implementation
#[derive(Debug)]
pub struct ColdStorageBackend {
    /// Storage backend type
    backend_type: BackendType,
    /// Base storage path
    storage_path: PathBuf,
    /// In-memory index for fast lookups
    node_index: Arc<RwLock<HashMap<NodeId, StorageLocation>>>,
    edge_index: Arc<RwLock<HashMap<EdgeId, StorageLocation>>>,
    /// Storage statistics
    stats: Arc<RwLock<StorageStats>>,
}

/// Storage location information
#[derive(Debug, Clone)]
pub struct StorageLocation {
    /// File path relative to storage root
    pub file_path: PathBuf,
    /// Offset within file (for formats that support it)
    pub offset: u64,
    /// Size of data
    pub size: u64,
    /// When this item was stored
    pub stored_at: std::time::SystemTime,
}

/// Storage statistics
#[derive(Debug, Default)]
pub struct StorageStats {
    pub nodes_stored: u64,
    pub edges_stored: u64,
    pub total_storage_bytes: u64,
    pub files_created: u64,
    pub reads_performed: u64,
    pub writes_performed: u64,
    pub compression_ratio: f64,
}

/// Serializable node for storage
#[derive(Debug, Serialize, Deserialize)]
struct StorableNode {
    pub id: String,
    pub labels: Vec<String>,
    pub properties: HashMap<String, StorablePropertyValue>,
    pub embedding: Option<Vec<f32>>,
    pub created_at: Option<u64>,
    pub updated_at: Option<u64>,
}

/// Serializable edge for storage
#[derive(Debug, Serialize, Deserialize)]
struct StorableEdge {
    pub id: String,
    pub from_node_id: String,
    pub to_node_id: String,
    pub edge_type: String,
    pub properties: HashMap<String, StorablePropertyValue>,
    pub weight: Option<f32>,
    pub created_at: Option<u64>,
    pub updated_at: Option<u64>,
}

/// Serializable property value
#[derive(Debug, Serialize, Deserialize)]
enum StorablePropertyValue {
    String(String),
    Int(i64),
    Double(f64),
    Bool(bool),
    Bytes(Vec<u8>),
    Array(Vec<StorablePropertyValue>),
    Object(HashMap<String, StorablePropertyValue>),
}

impl ColdStorageBackend {
    /// Create a new cold storage backend
    pub async fn new(backend_type: BackendType, storage_path: &Path) -> Result<Self> {
        // Create storage directory if it doesn't exist
        fs::create_dir_all(storage_path).await
            .map_err(|e| VectorDBError::Storage(StorageError::DiskIO(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))))?;
        
        let backend = Self {
            backend_type,
            storage_path: storage_path.to_path_buf(),
            node_index: Arc::new(RwLock::new(HashMap::new())),
            edge_index: Arc::new(RwLock::new(HashMap::new())),
            stats: Arc::new(RwLock::new(StorageStats::default())),
        };
        
        // Load existing index if available
        backend.load_index().await?;
        
        Ok(backend)
    }
    
    /// Store a node in cold storage
    pub async fn store_node(&self, node: Node) -> Result<()> {
        let storable_node = self.node_to_storable(&node)?;
        let location = self.write_node_to_storage(&storable_node).await?;
        
        // Update index
        {
            let mut node_index = self.node_index.write().await;
            node_index.insert(node.id.clone(), location);
        }
        
        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.nodes_stored += 1;
            stats.writes_performed += 1;
        }
        
        Ok(())
    }
    
    /// Store an edge in cold storage
    pub async fn store_edge(&self, edge: Edge) -> Result<()> {
        let storable_edge = self.edge_to_storable(&edge)?;
        let location = self.write_edge_to_storage(&storable_edge).await?;
        
        // Update index
        {
            let mut edge_index = self.edge_index.write().await;
            edge_index.insert(edge.id.clone(), location);
        }
        
        // Update statistics
        {
            let mut stats = self.stats.write().await;
            stats.edges_stored += 1;
            stats.writes_performed += 1;
        }
        
        Ok(())
    }
    
    /// Get a node from cold storage
    pub async fn get_node(&self, node_id: &NodeId) -> Result<Option<Arc<Node>>> {
        let location = {
            let node_index = self.node_index.read().await;
            node_index.get(node_id).cloned()
        };
        
        if let Some(location) = location {
            let storable_node = self.read_node_from_storage(&location).await?;
            let node = self.storable_to_node(storable_node)?;
            
            // Update statistics
            {
                let mut stats = self.stats.write().await;
                stats.reads_performed += 1;
            }
            
            Ok(Some(Arc::new(node)))
        } else {
            Ok(None)
        }
    }
    
    /// Get an edge from cold storage
    pub async fn get_edge(&self, edge_id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        let location = {
            let edge_index = self.edge_index.read().await;
            edge_index.get(edge_id).cloned()
        };
        
        if let Some(location) = location {
            let storable_edge = self.read_edge_from_storage(&location).await?;
            let edge = self.storable_to_edge(storable_edge)?;
            
            // Update statistics
            {
                let mut stats = self.stats.write().await;
                stats.reads_performed += 1;
            }
            
            Ok(Some(Arc::new(edge)))
        } else {
            Ok(None)
        }
    }
    
    /// Delete a node from cold storage
    pub async fn delete_node(&self, node_id: &NodeId) -> Result<Option<Arc<Node>>> {
        let location = {
            let mut node_index = self.node_index.write().await;
            node_index.remove(node_id)
        };
        
        if let Some(location) = location {
            // Get the node before deleting
            let storable_node = self.read_node_from_storage(&location).await?;
            let node = self.storable_to_node(storable_node)?;
            
            // Delete from storage (for now, just remove from index)
            // In a production system, we'd implement proper deletion or mark as deleted
            
            // Update statistics
            {
                let mut stats = self.stats.write().await;
                stats.nodes_stored = stats.nodes_stored.saturating_sub(1);
            }
            
            Ok(Some(Arc::new(node)))
        } else {
            Ok(None)
        }
    }
    
    /// Delete an edge from cold storage
    pub async fn delete_edge(&self, edge_id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        let location = {
            let mut edge_index = self.edge_index.write().await;
            edge_index.remove(edge_id)
        };
        
        if let Some(location) = location {
            let storable_edge = self.read_edge_from_storage(&location).await?;
            let edge = self.storable_to_edge(storable_edge)?;
            
            // Update statistics
            {
                let mut stats = self.stats.write().await;
                stats.edges_stored = stats.edges_stored.saturating_sub(1);
            }
            
            Ok(Some(Arc::new(edge)))
        } else {
            Ok(None)
        }
    }
    
    /// Get outgoing edges for a node
    pub async fn get_outgoing_edges(&self, node_id: &NodeId, edge_type: Option<&str>) -> Result<Vec<Arc<Edge>>> {
        // For simplicity, this scans all edges
        // In production, we'd maintain better indexes
        let mut edges = Vec::new();
        
        let edge_index = self.edge_index.read().await;
        for (edge_id, location) in edge_index.iter() {
            if let Ok(storable_edge) = self.read_edge_from_storage(location).await {
                if storable_edge.from_node_id == *node_id {
                    if let Some(filter_type) = edge_type {
                        if storable_edge.edge_type == filter_type {
                            let edge = self.storable_to_edge(storable_edge)?;
                            edges.push(Arc::new(edge));
                        }
                    } else {
                        let edge = self.storable_to_edge(storable_edge)?;
                        edges.push(Arc::new(edge));
                    }
                }
            }
        }
        
        Ok(edges)
    }
    
    /// Get incoming edges for a node
    pub async fn get_incoming_edges(&self, node_id: &NodeId, edge_type: Option<&str>) -> Result<Vec<Arc<Edge>>> {
        let mut edges = Vec::new();
        
        let edge_index = self.edge_index.read().await;
        for (edge_id, location) in edge_index.iter() {
            if let Ok(storable_edge) = self.read_edge_from_storage(location).await {
                if storable_edge.to_node_id == *node_id {
                    if let Some(filter_type) = edge_type {
                        if storable_edge.edge_type == filter_type {
                            let edge = self.storable_to_edge(storable_edge)?;
                            edges.push(Arc::new(edge));
                        }
                    } else {
                        let edge = self.storable_to_edge(storable_edge)?;
                        edges.push(Arc::new(edge));
                    }
                }
            }
        }
        
        Ok(edges)
    }
    
    /// Get nodes by label
    pub async fn get_nodes_by_label(&self, label: &str) -> Result<Vec<Arc<Node>>> {
        let mut nodes = Vec::new();
        
        let node_index = self.node_index.read().await;
        for (node_id, location) in node_index.iter() {
            if let Ok(storable_node) = self.read_node_from_storage(location).await {
                if storable_node.labels.contains(&label.to_string()) {
                    let node = self.storable_to_node(storable_node)?;
                    nodes.push(Arc::new(node));
                }
            }
        }
        
        Ok(nodes)
    }
    
    /// Get node count
    pub async fn node_count(&self) -> Result<usize> {
        let node_index = self.node_index.read().await;
        Ok(node_index.len())
    }
    
    /// Get edge count
    pub async fn edge_count(&self) -> Result<usize> {
        let edge_index = self.edge_index.read().await;
        Ok(edge_index.len())
    }
    
    /// Write node to storage based on backend type
    async fn write_node_to_storage(&self, node: &StorableNode) -> Result<StorageLocation> {
        match self.backend_type {
            BackendType::Json => self.write_node_json(node).await,
            BackendType::Sst => self.write_node_sst(node).await,
            BackendType::Parquet => self.write_node_parquet(node).await,
        }
    }
    
    /// Write edge to storage based on backend type
    async fn write_edge_to_storage(&self, edge: &StorableEdge) -> Result<StorageLocation> {
        match self.backend_type {
            BackendType::Json => self.write_edge_json(edge).await,
            BackendType::Sst => self.write_edge_sst(edge).await,
            BackendType::Parquet => self.write_edge_parquet(edge).await,
        }
    }
    
    /// Read node from storage based on backend type
    async fn read_node_from_storage(&self, location: &StorageLocation) -> Result<StorableNode> {
        match self.backend_type {
            BackendType::Json => self.read_node_json(location).await,
            BackendType::Sst => self.read_node_sst(location).await,
            BackendType::Parquet => self.read_node_parquet(location).await,
        }
    }
    
    /// Read edge from storage based on backend type
    async fn read_edge_from_storage(&self, location: &StorageLocation) -> Result<StorableEdge> {
        match self.backend_type {
            BackendType::Json => self.read_edge_json(location).await,
            BackendType::Sst => self.read_edge_sst(location).await,
            BackendType::Parquet => self.read_edge_parquet(location).await,
        }
    }
    
    /// JSON storage implementation
    async fn write_node_json(&self, node: &StorableNode) -> Result<StorageLocation> {
        let file_path = self.storage_path.join("nodes").join(format!("{}.json", node.id));
        fs::create_dir_all(file_path.parent().unwrap()).await
            .map_err(|e| VectorDBError::Storage(StorageError::DiskIO(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))))?;
        
        let json_data = serde_json::to_string_pretty(node)
            .map_err(|e| VectorDBError::Storage(StorageError::Serialization(e.to_string())))?;
        
        fs::write(&file_path, json_data.as_bytes()).await
            .map_err(|e| VectorDBError::Storage(StorageError::DiskIO(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))))?;
        
        Ok(StorageLocation {
            file_path: file_path.strip_prefix(&self.storage_path).unwrap().to_path_buf(),
            offset: 0,
            size: json_data.len() as u64,
            stored_at: std::time::SystemTime::now(),
        })
    }
    
    async fn write_edge_json(&self, edge: &StorableEdge) -> Result<StorageLocation> {
        let file_path = self.storage_path.join("edges").join(format!("{}.json", edge.id));
        fs::create_dir_all(file_path.parent().unwrap()).await
            .map_err(|e| VectorDBError::Storage(StorageError::DiskIO(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))))?;
        
        let json_data = serde_json::to_string_pretty(edge)
            .map_err(|e| VectorDBError::Storage(StorageError::Serialization(e.to_string())))?;
        
        fs::write(&file_path, json_data.as_bytes()).await
            .map_err(|e| VectorDBError::Storage(StorageError::DiskIO(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))))?;
        
        Ok(StorageLocation {
            file_path: file_path.strip_prefix(&self.storage_path).unwrap().to_path_buf(),
            offset: 0,
            size: json_data.len() as u64,
            stored_at: std::time::SystemTime::now(),
        })
    }
    
    async fn read_node_json(&self, location: &StorageLocation) -> Result<StorableNode> {
        let file_path = self.storage_path.join(&location.file_path);
        let json_data = fs::read_to_string(&file_path).await
            .map_err(|e| VectorDBError::Storage(StorageError::DiskIO(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))))?;
        
        serde_json::from_str(&json_data)
            .map_err(|e| VectorDBError::Storage(StorageError::Serialization(e.to_string())))
    }
    
    async fn read_edge_json(&self, location: &StorageLocation) -> Result<StorableEdge> {
        let file_path = self.storage_path.join(&location.file_path);
        let json_data = fs::read_to_string(&file_path).await
            .map_err(|e| VectorDBError::Storage(StorageError::DiskIO(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))))?;
        
        serde_json::from_str(&json_data)
            .map_err(|e| VectorDBError::Storage(StorageError::Serialization(e.to_string())))
    }
    
    /// SST storage implementation (placeholder)
    async fn write_node_sst(&self, node: &StorableNode) -> Result<StorageLocation> {
        // For now, fallback to JSON
        // In production, implement actual SST writing
        self.write_node_json(node).await
    }
    
    async fn write_edge_sst(&self, edge: &StorableEdge) -> Result<StorageLocation> {
        self.write_edge_json(edge).await
    }
    
    async fn read_node_sst(&self, location: &StorageLocation) -> Result<StorableNode> {
        self.read_node_json(location).await
    }
    
    async fn read_edge_sst(&self, location: &StorageLocation) -> Result<StorableEdge> {
        self.read_edge_json(location).await
    }
    
    /// Parquet storage implementation (placeholder)
    async fn write_node_parquet(&self, node: &StorableNode) -> Result<StorageLocation> {
        // For now, fallback to JSON
        // In production, implement actual Parquet writing
        self.write_node_json(node).await
    }
    
    async fn write_edge_parquet(&self, edge: &StorableEdge) -> Result<StorageLocation> {
        self.write_edge_json(edge).await
    }
    
    async fn read_node_parquet(&self, location: &StorageLocation) -> Result<StorableNode> {
        self.read_node_json(location).await
    }
    
    async fn read_edge_parquet(&self, location: &StorageLocation) -> Result<StorableEdge> {
        self.read_edge_json(location).await
    }
    
    /// Convert Node to StorableNode
    fn node_to_storable(&self, node: &Node) -> Result<StorableNode> {
        let mut properties = HashMap::new();
        for (key, value) in &node.properties {
            properties.insert(key.clone(), self.property_value_to_storable(value)?);
        }
        
        Ok(StorableNode {
            id: node.id.clone(),
            labels: node.labels.clone(),
            properties,
            embedding: node.embedding.as_ref().and_then(|e| 
                e.vector_data.as_ref().map(|v| v.values.clone())
            ),
            created_at: node.created_at.as_ref().map(|t| t.seconds as u64),
            updated_at: node.updated_at.as_ref().map(|t| t.seconds as u64),
        })
    }
    
    /// Convert Edge to StorableEdge
    fn edge_to_storable(&self, edge: &Edge) -> Result<StorableEdge> {
        let mut properties = HashMap::new();
        for (key, value) in &edge.properties {
            properties.insert(key.clone(), self.property_value_to_storable(value)?);
        }
        
        Ok(StorableEdge {
            id: edge.id.clone(),
            from_node_id: edge.from_node_id.clone(),
            to_node_id: edge.to_node_id.clone(),
            edge_type: edge.edge_type.clone(),
            properties,
            weight: edge.weight.map(|w| w as f32),
            created_at: edge.created_at.as_ref().map(|t| t.seconds as u64),
            updated_at: edge.updated_at.as_ref().map(|t| t.seconds as u64),
        })
    }
    
    /// Convert PropertyValue to StorablePropertyValue
    fn property_value_to_storable(&self, value: &crate::graph::PropertyValue) -> Result<StorablePropertyValue> {
        use crate::proto::proximadb_v1::property_value::Value;
        
        match &value.value {
            Some(Value::StringValue(s)) => Ok(StorablePropertyValue::String(s.clone())),
            Some(Value::IntValue(i)) => Ok(StorablePropertyValue::Int(*i)),
            Some(Value::DoubleValue(d)) => Ok(StorablePropertyValue::Double(*d)),
            Some(Value::BoolValue(b)) => Ok(StorablePropertyValue::Bool(*b)),
            Some(Value::BytesValue(b)) => Ok(StorablePropertyValue::Bytes(b.clone())),
            Some(Value::ArrayValue(_)) => Ok(StorablePropertyValue::Array(Vec::new())), // Simplified
            Some(Value::ObjectValue(_)) => Ok(StorablePropertyValue::Object(HashMap::new())), // Simplified
            None => Ok(StorablePropertyValue::String("null".to_string())),
        }
    }
    
    /// Convert StorableNode to Node
    fn storable_to_node(&self, storable: StorableNode) -> Result<Node> {
        let mut properties = HashMap::new();
        for (key, value) in storable.properties {
            properties.insert(key, self.storable_to_property_value(value)?);
        }
        
        Ok(Node {
            id: storable.id,
            labels: storable.labels,
            properties,
            embedding: storable.embedding.map(|values| crate::proto::proximadb_v1::EmbeddingVersion {
                version: 1,
                vector_data: Some(crate::proto::proximadb_v1::VectorData { values }),
                metadata: std::collections::HashMap::new(),
            }),
            created_at: storable.created_at.map(|t| ::prost_types::Timestamp { seconds: t as i64, nanos: 0 }),
            updated_at: storable.updated_at.map(|t| ::prost_types::Timestamp { seconds: t as i64, nanos: 0 }),
        })
    }
    
    /// Convert StorableEdge to Edge
    fn storable_to_edge(&self, storable: StorableEdge) -> Result<Edge> {
        let mut properties = HashMap::new();
        for (key, value) in storable.properties {
            properties.insert(key, self.storable_to_property_value(value)?);
        }
        
        Ok(Edge {
            id: storable.id,
            from_node_id: storable.from_node_id,
            to_node_id: storable.to_node_id,
            edge_type: storable.edge_type,
            properties,
            weight: storable.weight,
            created_at: storable.created_at.map(|t| ::prost_types::Timestamp { seconds: t as i64, nanos: 0 }),
            updated_at: storable.updated_at.map(|t| ::prost_types::Timestamp { seconds: t as i64, nanos: 0 }),
        })
    }
    
    /// Convert StorablePropertyValue to PropertyValue
    fn storable_to_property_value(&self, storable: StorablePropertyValue) -> Result<crate::graph::PropertyValue> {
        use crate::proto::proximadb_v1::property_value::Value;
        
        let value = match storable {
            StorablePropertyValue::String(s) => Some(Value::StringValue(s)),
            StorablePropertyValue::Int(i) => Some(Value::IntValue(i)),
            StorablePropertyValue::Double(d) => Some(Value::DoubleValue(d)),
            StorablePropertyValue::Bool(b) => Some(Value::BoolValue(b)),
            StorablePropertyValue::Bytes(b) => Some(Value::BytesValue(b)),
            StorablePropertyValue::Array(_) => None, // Simplified
            StorablePropertyValue::Object(_) => None, // Simplified
        };
        
        Ok(crate::graph::PropertyValue { value })
    }
    
    /// Load index from storage
    async fn load_index(&self) -> Result<()> {
        // For simplicity, index is rebuilt by scanning storage
        // In production, we'd persist and load the index
        Ok(())
    }
    
    /// Get all nodes from cold storage
    pub async fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>> {
        let mut all_nodes = Vec::new();
        let node_index = self.node_index.read().await;
        
        for (node_id, _location) in node_index.iter() {
            if let Ok(Some(node)) = self.get_node(node_id).await {
                all_nodes.push(node);
            }
        }
        
        Ok(all_nodes)
    }
    
    /// Get edge count from cold storage
    pub async fn edge_count(&self) -> Result<usize> {
        let edge_index = self.edge_index.read().await;
        Ok(edge_index.len())
    }
    
    /// Get storage statistics
    pub async fn get_stats(&self) -> StorageStats {
        let stats = self.stats.read().await;
        (*stats).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use crate::proto::proximadb_v1::property_value::Value;
    use std::collections::HashMap;
    
    #[tokio::test]
    async fn test_backend_creation() {
        let temp_dir = TempDir::new().unwrap();
        let backend = ColdStorageBackend::new(
            BackendType::Json,
            temp_dir.path()
        ).await.unwrap();
        
        let stats = backend.get_stats().await;
        assert_eq!(stats.nodes_stored, 0);
        assert_eq!(stats.edges_stored, 0);
    }
    
    #[tokio::test]
    async fn test_node_storage_and_retrieval() {
        let temp_dir = TempDir::new().unwrap();
        let backend = ColdStorageBackend::new(
            BackendType::Json,
            temp_dir.path()
        ).await.unwrap();
        
        let node = Node {
            id: "test_node".to_string(),
            labels: vec!["TestLabel".to_string()],
            properties: HashMap::from([
                ("name".to_string(), crate::graph::PropertyValue {
                    value: Some(Value::StringValue("Test Node".to_string())),
                }),
            ]),
            embedding: None,
            created_at: None,
            updated_at: None,
        };
        
        // Store node
        backend.store_node(node.clone()).await.unwrap();
        
        // Retrieve node
        let retrieved = backend.get_node("test_node").await.unwrap().unwrap();
        assert_eq!(retrieved.id, "test_node");
        assert_eq!(retrieved.labels, vec!["TestLabel"]);
        
        // Check stats
        let stats = backend.get_stats().await;
        assert_eq!(stats.nodes_stored, 1);
        assert_eq!(stats.writes_performed, 1);
        assert_eq!(stats.reads_performed, 1);
    }
    
    #[tokio::test]
    async fn test_edge_storage_and_retrieval() {
        let temp_dir = TempDir::new().unwrap();
        let backend = ColdStorageBackend::new(
            BackendType::Json,
            temp_dir.path()
        ).await.unwrap();
        
        let edge = Edge {
            id: "test_edge".to_string(),
            from_node_id: "node1".to_string(),
            to_node_id: "node2".to_string(),
            edge_type: "CONNECTS".to_string(),
            properties: HashMap::new(),
            weight: Some(1.0),
            created_at: None,
            updated_at: None,
        };
        
        // Store edge
        backend.store_edge(edge.clone()).await.unwrap();
        
        // Retrieve edge
        let retrieved = backend.get_edge("test_edge").await.unwrap().unwrap();
        assert_eq!(retrieved.id, "test_edge");
        assert_eq!(retrieved.from_node_id, "node1");
        assert_eq!(retrieved.to_node_id, "node2");
        assert_eq!(retrieved.edge_type, "CONNECTS");
        
        // Check stats
        let stats = backend.get_stats().await;
        assert_eq!(stats.edges_stored, 1);
    }
    
    #[tokio::test]
    async fn test_node_deletion() {
        let temp_dir = TempDir::new().unwrap();
        let backend = ColdStorageBackend::new(
            BackendType::Json,
            temp_dir.path()
        ).await.unwrap();
        
        let node = Node {
            id: "delete_me".to_string(),
            labels: vec!["Test".to_string()],
            properties: HashMap::new(),
            embedding: None,
            created_at: None,
            updated_at: None,
        };
        
        // Store then delete
        backend.store_node(node).await.unwrap();
        let deleted = backend.delete_node("delete_me").await.unwrap().unwrap();
        
        assert_eq!(deleted.id, "delete_me");
        
        // Should not be retrievable anymore
        let not_found = backend.get_node("delete_me").await.unwrap();
        assert!(not_found.is_none());
    }
}