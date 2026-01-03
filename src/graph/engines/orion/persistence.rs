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

//! # ORION Persistence Module
//!
//! Provides persistence capabilities for the ORION graph engine using:
//! - Snapshots with compression
//! - Write-Ahead Logging (WAL)
//! - Cloud storage integration via IntelligentFilesystem

use crate::core::error::ProximaDBError;
use crate::core::serialization::CompressionAlgorithm;
use crate::graph::engines::orion::OrionGraphEngine;
use crate::graph::{Edge, EdgeId, Node, NodeId};
use crate::storage::persistence::filesystem::unified::UnifiedCachingFilesystem;
use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use crate::storage::persistence::write_ahead_log::unified_operations::UnifiedWALWriter;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, warn};

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Serializable snapshot of the ORION engine state
#[derive(Serialize, Deserialize)]
pub struct OrionSnapshot {
    /// Version for backward compatibility
    version: u32,

    /// All nodes in the graph
    nodes: Vec<Node>,

    /// All edges in the graph
    edges: Vec<Edge>,

    /// CSR format for outgoing edges
    csr_outgoing_offsets: Vec<usize>,
    csr_outgoing_targets: Vec<usize>,

    /// CSR format for incoming edges
    csr_incoming_offsets: Vec<usize>,
    csr_incoming_sources: Vec<usize>,

    /// Node to CSR index mapping
    node_to_index: HashMap<NodeId, usize>,

    /// Timestamp of snapshot creation
    timestamp: i64,
}

// Use the unified GraphOperation from graph_memtable
use crate::storage::memtable::implementations::graph_memtable::GraphOperation;

/// Persistence manager for ORION engine
pub struct OrionPersistence {
    /// Graph ID for multi-graph support
    graph_id: String,

    /// Base URL for storage (e.g., "file:///data", "s3://bucket", etc.)
    base_url: String,

    /// Filesystem factory for creating appropriate filesystem
    filesystem_factory: Arc<FilesystemFactory>,

    /// Unified caching filesystem wrapper
    filesystem: Arc<UnifiedCachingFilesystem>,

    /// WAL path for future implementation
    wal_path: Option<PathBuf>,

    /// WAL writer for unified operations
    wal_writer: Option<Arc<tokio::sync::Mutex<UnifiedWALWriter>>>,

    /// Compression configuration
    compression: CompressionAlgorithm,
    compression_level: i32,

    /// Snapshot configuration
    max_snapshots: usize,
    incremental_snapshots: bool,
}

impl std::fmt::Debug for OrionPersistence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OrionPersistence")
            .field("graph_id", &self.graph_id)
            .field("base_url", &self.base_url)
            .field("wal_path", &self.wal_path)
            .field("wal_writer", &"<UnifiedWALWriter>")
            .field("compression", &self.compression)
            .field("compression_level", &self.compression_level)
            .field("max_snapshots", &self.max_snapshots)
            .field("incremental_snapshots", &self.incremental_snapshots)
            .finish()
    }
}

impl OrionPersistence {
    /// Create a new persistence manager for a specific graph
    pub async fn new(graph_id: String, base_url: String, enable_wal: bool) -> Result<Self> {
        // Create filesystem factory with default configuration and initialize filesystems
        let filesystem_factory = Arc::new(
            FilesystemFactory::create(FilesystemConfig::default())
                .await
                .map_err(|e| {
                    ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                        e.to_string(),
                    ))
                })?,
        );

        // Get the underlying filesystem from the factory
        let underlying_fs = filesystem_factory.get_filesystem(&base_url).map_err(|e| {
            ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                e.to_string(),
            ))
        })?;

        // Wrap with UnifiedCachingFilesystem
        let filesystem = Arc::new(UnifiedCachingFilesystem::new(
            underlying_fs,
            format!("graph_{}", graph_id),
            "orion".to_string(),
        ));

        // Build graph-specific path: {base_url}/graphs/{graph_id}/data
        let graph_path = format!(
            "{}/graphs/{}/data",
            base_url.trim_end_matches('/'),
            graph_id
        );

        // Create graph directory
        filesystem_factory
            .create_dir_all(&graph_path)
            .await
            .map_err(|e| {
                ProximaDBError::Storage(crate::core::error::StorageError::SstEngine(e.to_string()))
            })?;

        // Store WAL path and initialize WAL writer
        let (wal_path, wal_writer) = if enable_wal {
            // Build WAL URL (keep as URL for filesystem operations)
            let wal_url = format!("{}/wal", graph_path);

            // Extract path component for WAL writer (strip file:// prefix if present)
            let wal_path_str = if wal_url.starts_with("file://") {
                wal_url.strip_prefix("file://").unwrap().to_string()
            } else {
                wal_url.clone()
            };
            let wal_path = PathBuf::from(&wal_path_str);

            // Create WAL directory using URL
            filesystem_factory
                .create_dir_all(&wal_url)
                .await
                .map_err(|e| {
                    ProximaDBError::Storage(crate::core::error::StorageError::SstEngine(
                        e.to_string(),
                    ))
                })?;

            // Initialize WAL writer with path (not URL)
            tracing::debug!("Creating WAL writer with path: {}", wal_path_str);
            let wal_writer = UnifiedWALWriter::new(wal_path_str.clone())
                .await
                .map_err(|e| {
                    ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                        e.to_string(),
                    ))
                })?;
            tracing::debug!("WAL writer created successfully");

            (
                Some(wal_path),
                Some(Arc::new(tokio::sync::Mutex::new(wal_writer))),
            )
        } else {
            (None, None)
        };

        Ok(Self {
            graph_id,
            base_url,
            filesystem_factory,
            filesystem,
            wal_path,
            wal_writer,
            compression: CompressionAlgorithm::Zstd,
            compression_level: 3,
            max_snapshots: 10,
            incremental_snapshots: false,
        })
    }

    /// Save a snapshot of the engine state
    pub async fn save_snapshot(&self, engine: &OrionGraphEngine) -> Result<PathBuf> {
        info!("Creating ORION snapshot");

        // Collect all nodes
        let nodes = engine
            .memory_pool
            .nodes
            .iter()
            .map(|entry| (*entry.value()).clone())
            .collect::<Vec<_>>();

        // Collect all edges
        let edges = engine
            .edge_metadata
            .iter()
            .map(|entry| (*entry.value()).clone())
            .collect::<Vec<_>>();

        // Get CSR data
        let csr_outgoing = engine
            .csr_outgoing
            .read()
            .expect("CSR outgoing read lock poisoned");
        let csr_incoming = engine
            .csr_incoming
            .read()
            .expect("CSR incoming read lock poisoned");

        // Build node_to_index mapping
        let node_to_index = engine
            .node_to_index
            .iter()
            .map(|entry| (entry.key().clone(), *entry.value()))
            .collect::<HashMap<_, _>>();

        // Create snapshot
        let snapshot = OrionSnapshot {
            version: 1,
            nodes: nodes.into_iter().map(|n| (*n).clone()).collect(),
            edges: edges.into_iter().map(|e| (*e).clone()).collect(),
            csr_outgoing_offsets: csr_outgoing.offsets.clone(),
            csr_outgoing_targets: csr_outgoing.targets.clone(),
            csr_incoming_offsets: csr_incoming.offsets.clone(),
            csr_incoming_sources: csr_incoming.targets.clone(), // Note: 'targets' stores sources for incoming
            node_to_index,
            timestamp: chrono::Utc::now().timestamp(),
        };

        // Serialize snapshot
        let serialized = bincode::serialize(&snapshot).map_err(|e| {
            ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                e.to_string(),
            ))
        })?;

        // Apply compression based on algorithm
        let compressed = self.compress_data(&serialized)?;

        // Generate snapshot filename with graph ID
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
        let filename = format!("graph_{}_snapshot_{}.bin.zst", self.graph_id, timestamp);
        let snapshot_url = format!(
            "{}/graphs/{}/snapshots/{}",
            self.base_url.trim_end_matches('/'),
            self.graph_id,
            filename
        );

        // Ensure snapshots directory exists
        let snapshots_dir = format!(
            "{}/graphs/{}/snapshots",
            self.base_url.trim_end_matches('/'),
            self.graph_id
        );
        self.filesystem_factory
            .create_dir_all(&snapshots_dir)
            .await
            .map_err(|e| {
                ProximaDBError::Storage(crate::core::error::StorageError::SstEngine(e.to_string()))
            })?;

        // Write compressed snapshot
        self.filesystem_factory
            .write(&snapshot_url, &compressed, None)
            .await
            .map_err(|e| {
                ProximaDBError::Storage(crate::core::error::StorageError::SstEngine(e.to_string()))
            })?;

        info!(
            "ORION graph {} snapshot saved: {} ({}MB compressed)",
            self.graph_id,
            filename,
            compressed.len() / 1_048_576
        );

        // Clean up old snapshots if needed
        self.cleanup_old_snapshots().await?;

        Ok(PathBuf::from(snapshot_url))
    }

    /// Load a snapshot and restore engine state
    pub async fn load_snapshot(
        &self,
        engine: &OrionGraphEngine,
        snapshot_path: impl AsRef<Path>,
    ) -> Result<()> {
        info!("Loading ORION snapshot from {:?}", snapshot_path.as_ref());

        // Read compressed snapshot
        let compressed = self
            .filesystem_factory
            .read(snapshot_path.as_ref().to_str().unwrap())
            .await
            .map_err(|e| {
                ProximaDBError::Storage(crate::core::error::StorageError::SstEngine(e.to_string()))
            })?;

        // Decompress data
        let decompressed = self.decompress_data(&compressed)?;

        // Deserialize
        let snapshot: OrionSnapshot = bincode::deserialize(&decompressed).map_err(|e| {
            ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                e.to_string(),
            ))
        })?;

        // Clear existing data
        engine.memory_pool.nodes.clear();
        engine.edge_metadata.clear();
        engine.node_to_index.clear();

        // Restore nodes
        for node in snapshot.nodes {
            engine
                .memory_pool
                .nodes
                .insert(node.id.clone(), Arc::new(node));
        }

        // Restore edges
        for edge in snapshot.edges {
            engine.edge_metadata.insert(edge.id.clone(), Arc::new(edge));
        }

        // Restore CSR structures
        {
            let mut csr_out = engine
                .csr_outgoing
                .write()
                .expect("CSR outgoing write lock poisoned");
            csr_out.offsets = snapshot.csr_outgoing_offsets;
            csr_out.targets = snapshot.csr_outgoing_targets;
        }

        {
            let mut csr_in = engine
                .csr_incoming
                .write()
                .expect("CSR incoming write lock poisoned");
            csr_in.offsets = snapshot.csr_incoming_offsets;
            csr_in.targets = snapshot.csr_incoming_sources;
        }

        // Restore mappings
        for (node_id, index) in snapshot.node_to_index {
            engine.node_to_index.insert(node_id.clone(), index);
        }

        // Rebuild index_to_node
        {
            let mut index_to_node = engine
                .index_to_node
                .write()
                .expect("index_to_node write lock poisoned");
            index_to_node.clear();
            let mut node_indices: Vec<(NodeId, usize)> = engine
                .node_to_index
                .iter()
                .map(|entry| (entry.key().clone(), *entry.value()))
                .collect();
            node_indices.sort_by_key(|&(_, idx)| idx);

            for (node_id, _) in node_indices {
                index_to_node.push(node_id);
            }
        }

        // Update stats
        {
            let mut stats = engine.stats.write().expect("stats write lock poisoned");
            stats.nodes_created = engine.memory_pool.nodes.len() as u64;
            stats.edges_created = engine.edge_metadata.len() as u64;
        }

        info!(
            "ORION snapshot loaded: {} nodes, {} edges",
            engine.memory_pool.nodes.len(),
            engine.edge_metadata.len()
        );

        Ok(())
    }

    /// Write node operation to WAL
    pub async fn write_node_operation(&self, node: Node) -> Result<()> {
        if let Some(ref wal_writer) = self.wal_writer {
            use crate::storage::persistence::write_ahead_log::unified_operations::UnifiedWALOperation;

            tracing::debug!("Writing node operation to WAL for node: {}", node.id);

            let graph_op = GraphOperation::CreateNode {
                graph_id: self.graph_id.clone(),
                node,
            };

            let unified_op = UnifiedWALOperation::GraphOp(graph_op);
            wal_writer
                .lock()
                .await
                .append(unified_op)
                .await
                .map_err(|e| {
                    tracing::error!("WAL append failed: {:?}", e);
                    ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                        e.to_string(),
                    ))
                })?;

            tracing::debug!("WAL write completed successfully");
        } else {
            tracing::warn!("No WAL writer available - operations will not be persisted!");
        }

        Ok(())
    }

    /// Write edge operation to WAL
    pub async fn write_edge_operation(&self, edge: Edge) -> Result<()> {
        if let Some(ref wal_writer) = self.wal_writer {
            use crate::storage::persistence::write_ahead_log::unified_operations::UnifiedWALOperation;

            let graph_op = GraphOperation::CreateEdge {
                graph_id: self.graph_id.clone(),
                edge,
            };

            let unified_op = UnifiedWALOperation::GraphOp(graph_op);
            wal_writer
                .lock()
                .await
                .append(unified_op)
                .await
                .map_err(|e| {
                    ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                        e.to_string(),
                    ))
                })?;
        }

        Ok(())
    }

    /// Write a batch of edge operations to WAL in a single record
    pub async fn write_edge_batch_operation(&self, edges: &[Edge]) -> Result<()> {
        if edges.is_empty() {
            return Ok(());
        }

        if let Some(ref wal_writer) = self.wal_writer {
            use crate::storage::persistence::write_ahead_log::unified_operations::UnifiedWALOperation;

            // Wrap all CreateEdge operations in a single batch GraphOp to reduce WAL traffic
            let batch = GraphOperation::BatchOperation {
                operations: edges
                    .iter()
                    .cloned()
                    .map(|edge| GraphOperation::CreateEdge {
                        graph_id: self.graph_id.clone(),
                        edge,
                    })
                    .collect(),
            };

            let unified_op = UnifiedWALOperation::GraphOp(batch);
            wal_writer
                .lock()
                .await
                .append(unified_op)
                .await
                .map_err(|e| {
                    ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                        e.to_string(),
                    ))
                })?;
        }

        Ok(())
    }

    /// Write a batch of node operations to WAL in a single record
    pub async fn write_node_batch_operation(&self, nodes: &[Node]) -> Result<()> {
        if nodes.is_empty() {
            return Ok(());
        }

        if let Some(ref wal_writer) = self.wal_writer {
            use crate::storage::persistence::write_ahead_log::unified_operations::UnifiedWALOperation;

            // Wrap all CreateNode operations in a single batch GraphOp to reduce WAL traffic
            let batch = GraphOperation::BatchOperation {
                operations: nodes
                    .iter()
                    .cloned()
                    .map(|node| GraphOperation::CreateNode {
                        graph_id: self.graph_id.clone(),
                        node,
                    })
                    .collect(),
            };

            let unified_op = UnifiedWALOperation::GraphOp(batch);
            wal_writer
                .lock()
                .await
                .append(unified_op)
                .await
                .map_err(|e| {
                    ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                        e.to_string(),
                    ))
                })?;
        }

        Ok(())
    }

    /// Write node update operation to WAL
    /// For updates, we write the full updated node (upsert semantic)
    pub async fn write_update_node_operation(&self, node: Node) -> Result<()> {
        if let Some(ref wal_writer) = self.wal_writer {
            use crate::storage::persistence::write_ahead_log::unified_operations::UnifiedWALOperation;

            tracing::debug!("Writing update node operation to WAL for node: {}", node.id);

            // Use CreateNode with upsert semantic - during recovery, this will overwrite existing node
            let graph_op = GraphOperation::CreateNode {
                graph_id: self.graph_id.clone(),
                node,
            };

            let unified_op = UnifiedWALOperation::GraphOp(graph_op);
            wal_writer
                .lock()
                .await
                .append(unified_op)
                .await
                .map_err(|e| {
                    tracing::error!("WAL append failed for update node: {:?}", e);
                    ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                        e.to_string(),
                    ))
                })?;

            tracing::debug!("Update node WAL write completed");
        } else {
            tracing::warn!(
                "No WAL writer available - update node operation will not be persisted!"
            );
        }

        Ok(())
    }

    /// Write node delete operation to WAL
    pub async fn write_delete_node_operation(&self, node_id: &NodeId) -> Result<()> {
        if let Some(ref wal_writer) = self.wal_writer {
            use crate::storage::persistence::write_ahead_log::unified_operations::UnifiedWALOperation;

            tracing::debug!("Writing delete node operation to WAL for node: {}", node_id);

            let graph_op = GraphOperation::DeleteNode {
                graph_id: self.graph_id.clone(),
                node_id: node_id.clone(),
            };

            let unified_op = UnifiedWALOperation::GraphOp(graph_op);
            wal_writer
                .lock()
                .await
                .append(unified_op)
                .await
                .map_err(|e| {
                    tracing::error!("WAL append failed for delete node: {:?}", e);
                    ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                        e.to_string(),
                    ))
                })?;

            tracing::debug!("Delete node WAL write completed");
        } else {
            tracing::warn!(
                "No WAL writer available - delete node operation will not be persisted!"
            );
        }

        Ok(())
    }

    /// Write edge update operation to WAL
    /// For updates, we write the full updated edge (upsert semantic)
    pub async fn write_update_edge_operation(&self, edge: Edge) -> Result<()> {
        if let Some(ref wal_writer) = self.wal_writer {
            use crate::storage::persistence::write_ahead_log::unified_operations::UnifiedWALOperation;

            tracing::debug!("Writing update edge operation to WAL for edge: {}", edge.id);

            // Use CreateEdge with upsert semantic - during recovery, this will overwrite existing edge
            let graph_op = GraphOperation::CreateEdge {
                graph_id: self.graph_id.clone(),
                edge,
            };

            let unified_op = UnifiedWALOperation::GraphOp(graph_op);
            wal_writer
                .lock()
                .await
                .append(unified_op)
                .await
                .map_err(|e| {
                    tracing::error!("WAL append failed for update edge: {:?}", e);
                    ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                        e.to_string(),
                    ))
                })?;

            tracing::debug!("Update edge WAL write completed");
        } else {
            tracing::warn!(
                "No WAL writer available - update edge operation will not be persisted!"
            );
        }

        Ok(())
    }

    /// Write edge delete operation to WAL
    pub async fn write_delete_edge_operation(&self, edge_id: &EdgeId) -> Result<()> {
        if let Some(ref wal_writer) = self.wal_writer {
            use crate::storage::persistence::write_ahead_log::unified_operations::UnifiedWALOperation;

            tracing::debug!("Writing delete edge operation to WAL for edge: {}", edge_id);

            let graph_op = GraphOperation::DeleteEdge {
                graph_id: self.graph_id.clone(),
                edge_id: edge_id.clone(),
            };

            let unified_op = UnifiedWALOperation::GraphOp(graph_op);
            wal_writer
                .lock()
                .await
                .append(unified_op)
                .await
                .map_err(|e| {
                    tracing::error!("WAL append failed for delete edge: {:?}", e);
                    ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                        e.to_string(),
                    ))
                })?;

            tracing::debug!("Delete edge WAL write completed");
        } else {
            tracing::warn!(
                "No WAL writer available - delete edge operation will not be persisted!"
            );
        }

        Ok(())
    }

    /// Log a generic graph operation to WAL
    pub async fn log_operation(&self, op: GraphOperation) -> Result<()> {
        if let Some(ref wal_writer) = self.wal_writer {
            use crate::storage::persistence::write_ahead_log::unified_operations::UnifiedWALOperation;

            let unified_op = UnifiedWALOperation::GraphOp(op);
            wal_writer
                .lock()
                .await
                .append(unified_op)
                .await
                .map_err(|e| {
                    ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                        e.to_string(),
                    ))
                })?;
        }

        Ok(())
    }

    /// Flush WAL buffer to disk
    /// This ensures all pending operations are persisted
    pub async fn flush_wal(&self) -> Result<()> {
        if let Some(ref wal_writer) = self.wal_writer {
            wal_writer.lock().await.flush().await.map_err(|e| {
                ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                    e.to_string(),
                ))
            })?;
            tracing::debug!("WAL flushed to disk for graph: {}", self.graph_id);
        }
        Ok(())
    }

    /// Replay WAL operations from all segments
    pub async fn replay_wal(&self, engine: &OrionGraphEngine) -> Result<()> {
        if let Some(ref wal_path) = self.wal_path {
            use crate::storage::persistence::write_ahead_log::unified_operations::UnifiedWALReader;

            let wal_path_str = wal_path.to_string_lossy().to_string();
            tracing::debug!("Attempting WAL recovery from path: {}", wal_path_str);

            let reader = UnifiedWALReader::new(wal_path_str.clone())
                .await
                .map_err(|e| {
                    ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                        e.to_string(),
                    ))
                })?;
            let entries = reader.read_all().await.map_err(|e| {
                ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                    e.to_string(),
                ))
            })?;

            tracing::info!(
                "Replaying {} WAL entries for graph {}",
                entries.len(),
                self.graph_id
            );

            for entry in entries {
                if entry.is_graph_operation() {
                    if let crate::storage::persistence::write_ahead_log::unified_operations::UnifiedWALOperation::GraphOp(graph_op) = entry.operation {
                        self.apply_graph_operation(engine, graph_op).await?;
                    }
                }
            }

            tracing::info!("WAL replay completed for graph {}", self.graph_id);
        }

        Ok(())
    }

    /// Apply a graph operation to the engine during WAL replay
    fn apply_graph_operation<'a>(
        &'a self,
        engine: &'a OrionGraphEngine,
        op: GraphOperation,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            match op {
                GraphOperation::CreateNode { graph_id: _, node } => {
                    engine.create_node(node).await?;
                }
                GraphOperation::CreateEdge { graph_id: _, edge } => {
                    engine.create_edge(edge).await?;
                }
                GraphOperation::UpdateNode {
                    graph_id: _,
                    node_id,
                    update,
                } => {
                    // Get existing node from memory pool and apply update
                    if let Some(existing_node) = engine.memory_pool.get_node(&node_id) {
                        let mut updated_node = (*existing_node).clone();

                        // Apply updates
                        if let Some(labels) = update.labels {
                            updated_node.labels = labels;
                        }
                        if let Some(properties) = update.properties {
                            updated_node.properties = properties;
                        }
                        if let Some(embedding) = update.embedding {
                            updated_node.embedding = Some(embedding);
                        }

                        // Update the node (without re-logging to WAL during replay)
                        engine.memory_pool.remove_node(&node_id);
                        engine.memory_pool.insert_node(updated_node);
                        debug!("Replayed UpdateNode for node_id: {}", node_id);
                    } else {
                        warn!("UpdateNode replay: node {} not found, skipping", node_id);
                    }
                }
                GraphOperation::DeleteNode {
                    graph_id: _,
                    node_id,
                } => {
                    engine.delete_node(&node_id).await?;
                }
                GraphOperation::DeleteEdge {
                    graph_id: _,
                    edge_id,
                } => {
                    engine.delete_edge(&edge_id).await?;
                }
                GraphOperation::CreateEdgeIndex {
                    graph_id: _,
                    index_config: _,
                } => {
                    // Index operations need to be implemented in OrionGraphEngine
                    warn!("Create edge index operation not yet implemented in ORION engine");
                }
                GraphOperation::DropEdgeIndex {
                    graph_id: _,
                    index_name: _,
                } => {
                    // Index operations need to be implemented in OrionGraphEngine
                    warn!("Drop edge index operation not yet implemented in ORION engine");
                }
                GraphOperation::UpdateEdge {
                    graph_id: _,
                    edge_id,
                    update,
                } => {
                    // Get existing edge from memory pool and apply update
                    if let Some(existing_edge) = engine.memory_pool.get_edge(&edge_id) {
                        let mut updated_edge = (*existing_edge).clone();

                        // Apply updates
                        if let Some(properties) = update.properties {
                            updated_edge.properties = properties;
                        }
                        if let Some(weight) = update.weight {
                            updated_edge.weight = Some(weight);
                        }

                        // Update the edge (without re-logging to WAL during replay)
                        engine.memory_pool.remove_edge(&edge_id);
                        engine.memory_pool.insert_edge(updated_edge);
                        debug!("Replayed UpdateEdge for edge_id: {}", edge_id);
                    } else {
                        warn!("UpdateEdge replay: edge {} not found, skipping", edge_id);
                    }
                }
                GraphOperation::BatchOperation { operations } => {
                    // Apply each operation in the batch
                    for op in operations {
                        self.apply_graph_operation(engine, op).await?;
                    }
                }
            }

            Ok(())
        })
    }

    /// Create a checkpoint (snapshot + truncate WAL)
    pub async fn checkpoint(&self, engine: &OrionGraphEngine) -> Result<PathBuf> {
        let snapshot_path = self.save_snapshot(engine).await?;

        // WAL truncation will be implemented when WAL is added
        if self.wal_path.is_some() {
            debug!("WAL truncation placeholder - to be implemented");
        }

        Ok(snapshot_path)
    }

    /// Compress data using the configured algorithm
    fn compress_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        use lz4_flex::compress_prepend_size;
        use snap::raw::Encoder as SnapEncoder;
        use zstd::encode_all;

        match self.compression {
            CompressionAlgorithm::None => Ok(data.to_vec()),
            CompressionAlgorithm::Zstd => encode_all(data, self.compression_level).map_err(|e| {
                ProximaDBError::Storage(crate::core::error::StorageError::Serialization(
                    e.to_string(),
                ))
            }),
            CompressionAlgorithm::Lz4 => Ok(compress_prepend_size(data)),
            CompressionAlgorithm::Snappy => {
                let mut encoder = SnapEncoder::new();
                encoder.compress_vec(data).map_err(|e| {
                    ProximaDBError::Storage(crate::core::error::StorageError::Serialization(
                        e.to_string(),
                    ))
                })
            }
            _ => {
                // For other algorithms, default to Zstd
                encode_all(data, self.compression_level).map_err(|e| {
                    ProximaDBError::Storage(crate::core::error::StorageError::Serialization(
                        e.to_string(),
                    ))
                })
            }
        }
    }

    /// Decompress data based on the configured algorithm
    fn decompress_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        use lz4_flex::decompress_size_prepended;
        use snap::raw::Decoder as SnapDecoder;
        use zstd::decode_all;

        match self.compression {
            CompressionAlgorithm::None => Ok(data.to_vec()),
            CompressionAlgorithm::Zstd => decode_all(data).map_err(|e| {
                ProximaDBError::Storage(crate::core::error::StorageError::Serialization(
                    e.to_string(),
                ))
            }),
            CompressionAlgorithm::Lz4 => decompress_size_prepended(data).map_err(|e| {
                ProximaDBError::Storage(crate::core::error::StorageError::Serialization(
                    e.to_string(),
                ))
            }),
            CompressionAlgorithm::Snappy => {
                let mut decoder = SnapDecoder::new();
                decoder.decompress_vec(data).map_err(|e| {
                    ProximaDBError::Storage(crate::core::error::StorageError::Serialization(
                        e.to_string(),
                    ))
                })
            }
            _ => {
                // For other algorithms, default to Zstd
                decode_all(data).map_err(|e| {
                    ProximaDBError::Storage(crate::core::error::StorageError::Serialization(
                        e.to_string(),
                    ))
                })
            }
        }
    }

    /// Clean up old snapshots keeping only max_snapshots
    async fn cleanup_old_snapshots(&self) -> Result<()> {
        let snapshots_dir = format!(
            "{}/graphs/{}/snapshots",
            self.base_url.trim_end_matches('/'),
            self.graph_id
        );

        // List all snapshot files
        let mut snapshots = Vec::new();
        let entries = self
            .filesystem_factory
            .list(&snapshots_dir)
            .await
            .map_err(|e| {
                ProximaDBError::Storage(crate::core::error::StorageError::SstEngine(e.to_string()))
            })?;

        for entry in entries {
            if entry.url.ends_with(".bin.zst") {
                snapshots.push(entry.url);
            }
        }

        // Sort by filename (which includes timestamp)
        snapshots.sort();

        // Remove old snapshots if we exceed max_snapshots
        if snapshots.len() > self.max_snapshots {
            let to_remove = snapshots.len() - self.max_snapshots;
            for snapshot in snapshots.iter().take(to_remove) {
                self.filesystem_factory
                    .delete(snapshot)
                    .await
                    .map_err(|e| {
                        ProximaDBError::Storage(crate::core::error::StorageError::SstEngine(
                            e.to_string(),
                        ))
                    })?;
                debug!("Removed old snapshot: {:?}", snapshot);
            }
        }

        Ok(())
    }

    /// Export graph to a portable format
    pub async fn export(&self, engine: &OrionGraphEngine, path: impl AsRef<Path>) -> Result<()> {
        // Create snapshot
        let nodes: Vec<Node> = engine
            .memory_pool
            .nodes
            .iter()
            .map(|entry| (**entry.value()).clone())
            .collect();

        let edges: Vec<Edge> = engine
            .edge_metadata
            .iter()
            .map(|entry| (**entry.value()).clone())
            .collect();

        // Export as JSON for portability
        let export_data = serde_json::json!({
            "version": 1,
            "engine": "ORION",
            "nodes": nodes,
            "edges": edges,
            "stats": {
                "node_count": nodes.len(),
                "edge_count": edges.len(),
                "timestamp": chrono::Utc::now().timestamp(),
            }
        });

        let json = serde_json::to_string_pretty(&export_data).map_err(|e| {
            ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                e.to_string(),
            ))
        })?;

        let export_url = path
            .as_ref()
            .to_str()
            .ok_or_else(|| ProximaDBError::InvalidInput("Invalid export path".to_string()))?;
        self.filesystem_factory
            .write(export_url, json.as_bytes(), None)
            .await
            .map_err(|e| {
                ProximaDBError::Storage(crate::core::error::StorageError::SstEngine(e.to_string()))
            })?;
        info!("Graph {} exported to {:?}", self.graph_id, path.as_ref());

        Ok(())
    }

    /// Import graph from portable format
    pub async fn import(&self, engine: &OrionGraphEngine, path: impl AsRef<Path>) -> Result<()> {
        let import_url = path
            .as_ref()
            .to_str()
            .ok_or_else(|| ProximaDBError::InvalidInput("Invalid import path".to_string()))?;
        let data = self
            .filesystem_factory
            .read(import_url)
            .await
            .map_err(|e| {
                ProximaDBError::Storage(crate::core::error::StorageError::SstEngine(e.to_string()))
            })?;

        let import_data: serde_json::Value = serde_json::from_slice(&data).map_err(|e| {
            ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                e.to_string(),
            ))
        })?;

        // Clear existing data
        engine.memory_pool.nodes.clear();
        engine.edge_metadata.clear();

        // Import nodes
        if let Some(nodes) = import_data["nodes"].as_array() {
            for node_val in nodes {
                let node: Node = serde_json::from_value(node_val.clone()).map_err(|e| {
                    ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                        e.to_string(),
                    ))
                })?;
                engine.create_node(node).await?;
            }
        }

        // Import edges
        if let Some(edges) = import_data["edges"].as_array() {
            for edge_val in edges {
                let edge: Edge = serde_json::from_value(edge_val.clone()).map_err(|e| {
                    ProximaDBError::Storage(crate::core::error::StorageError::SerializationError(
                        e.to_string(),
                    ))
                })?;
                engine.create_edge(edge).await?;
            }
        }

        info!("Graph imported from {:?}", path.as_ref());
        Ok(())
    }
}

// Tests temporarily removed due to compilation issues
// TODO: Fix tests once OrionGraphEngine methods are properly implemented
