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

//! Foundation graph-engine contract: the [`GraphEngine`] trait + the value
//! types it surfaces (stats, memory, persistence config, export formats).
//! Moved out of the root `graph::engines` module so a graph engine (ORION) and
//! its callers can depend on the contract without a cyclic dependency on the
//! root crate (ORION extraction cascade, slice 6a).

use std::path::Path;
use std::sync::Arc;

use proximadb_compression_types::CompressionAlgorithm;
use proximadb_graph_model::{Edge, EdgeId, Node, NodeId};
use proximadb_kernel::error::ProximaDBError;

type Result<T> = std::result::Result<T, ProximaDBError>;

/// Engine performance statistics.
#[derive(Debug, Clone, Default)]
pub struct GraphEngineStats {
    /// Total number of nodes
    pub node_count: usize,
    /// Total number of edges
    pub edge_count: usize,
    /// Average degree (edges per node)
    pub avg_degree: f64,
    /// Number of connected components
    pub connected_components: usize,
    /// Total operations performed
    pub total_operations: u64,
    /// Cache hit ratio (if applicable)
    pub cache_hit_ratio: f64,
    /// Index efficiency metric
    pub index_efficiency: f64,
    /// Time spent in operations (microseconds)
    pub total_time_us: u64,
}

/// Memory usage metrics.
#[derive(Debug, Clone, Default)]
pub struct MemoryUsage {
    /// Memory used by nodes (bytes)
    pub nodes_memory: usize,
    /// Memory used by edges (bytes)
    pub edges_memory: usize,
    /// Memory used by indexes (bytes)
    pub indexes_memory: usize,
    /// Memory used by caches (bytes)
    pub cache_memory: usize,
    /// Total memory used (bytes)
    pub total_memory: usize,
    /// Peak memory usage (bytes)
    pub peak_memory: usize,
}

/// Graph engine trait for common operations across all engines
#[async_trait::async_trait]
pub trait GraphEngine: Send + Sync {
    /// Insert a node (async for durability - waits for WAL write)
    async fn insert_node(&self, node: Node) -> Result<Arc<Node>>;

    /// Get a node by ID
    fn get_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>>;

    /// Update a node (async for durability - waits for WAL write)
    async fn update_node(&self, node: Node) -> Result<Arc<Node>>;

    /// Delete a node (async for durability - waits for WAL write)
    async fn delete_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>>;

    /// Insert an edge (async for durability - waits for WAL write)
    async fn insert_edge(&self, edge: Edge) -> Result<Arc<Edge>>;

    /// Get an edge by ID
    fn get_edge(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>>;

    /// Update an edge (async for durability - waits for WAL write)
    async fn update_edge(&self, edge: Edge) -> Result<Arc<Edge>>;

    /// Delete an edge (async for durability - waits for WAL write)
    async fn delete_edge(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>>;

    /// Get outgoing edges from a node
    fn get_outgoing_edges(
        &self,
        node_id: &NodeId,
        edge_type: Option<&str>,
    ) -> Result<Vec<Arc<Edge>>>;

    /// Get incoming edges to a node
    fn get_incoming_edges(
        &self,
        node_id: &NodeId,
        edge_type: Option<&str>,
    ) -> Result<Vec<Arc<Edge>>>;

    /// Get neighbors of a node
    fn get_neighbors(&self, node_id: &NodeId, edge_type: Option<&str>) -> Result<Vec<Arc<Node>>>;

    /// Get nodes by label
    fn get_nodes_by_label(&self, label: &str) -> Result<Vec<Arc<Node>>>;

    /// Get total node count
    fn node_count(&self) -> Result<usize>;

    /// Get total edge count
    fn edge_count(&self) -> Result<usize>;

    /// Get all nodes
    fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>>;

    /// Get all edges. Default returns empty (mocks / engines that don't persist);
    /// real engines override it. Used by canonical-store recovery re-population
    /// (TD-066 Part 2) to re-drive every recovered edge into the canonical store.
    fn get_all_edges(&self) -> Result<Vec<Arc<Edge>>> {
        Ok(Vec::new())
    }

    // ===== Performance & Benchmarking Methods =====

    /// Get engine performance statistics
    fn get_engine_stats(&self) -> Result<GraphEngineStats> {
        // Default implementation for backward compatibility
        Ok(GraphEngineStats::default())
    }

    /// Get memory usage metrics
    fn get_memory_usage(&self) -> Result<MemoryUsage> {
        Ok(MemoryUsage::default())
    }

    // ===== Bulk Operations for Benchmarking =====

    /// Bulk insert nodes (optimized for batch operations)
    async fn bulk_insert_nodes(&self, nodes: Vec<Node>) -> Result<Vec<Arc<Node>>> {
        // Default implementation delegates to single insert
        let mut results = Vec::with_capacity(nodes.len());
        for node in nodes {
            results.push(self.insert_node(node).await?);
        }
        Ok(results)
    }

    /// Bulk insert edges (optimized for batch operations)
    async fn bulk_insert_edges(&self, edges: Vec<Edge>) -> Result<Vec<Arc<Edge>>> {
        // Default implementation delegates to single insert
        let mut results = Vec::with_capacity(edges.len());
        for edge in edges {
            results.push(self.insert_edge(edge).await?);
        }
        Ok(results)
    }

    /// Bulk delete nodes
    async fn bulk_delete_nodes(&self, node_ids: Vec<NodeId>) -> Result<Vec<Option<Arc<Node>>>> {
        let mut results = Vec::with_capacity(node_ids.len());
        for id in node_ids {
            results.push(self.delete_node(&id).await?);
        }
        Ok(results)
    }

    /// Bulk delete edges
    async fn bulk_delete_edges(&self, edge_ids: Vec<EdgeId>) -> Result<Vec<Option<Arc<Edge>>>> {
        let mut results = Vec::with_capacity(edge_ids.len());
        for id in edge_ids {
            results.push(self.delete_edge(&id).await?);
        }
        Ok(results)
    }

    // ===== Engine Optimization Methods =====

    /// Optimize internal storage structures
    fn optimize_storage(&self) -> Result<()> {
        // Default no-op implementation
        Ok(())
    }

    /// Rebuild indexes for better query performance
    fn rebuild_indexes(&self) -> Result<()> {
        // Default no-op implementation
        Ok(())
    }

    /// Compact storage to reduce memory footprint
    fn compact_storage(&self) -> Result<()> {
        // Default no-op implementation
        Ok(())
    }

    /// Clear all data (useful for benchmarking)
    async fn clear_all(&self) -> Result<()> {
        // Default implementation: delete all nodes and edges
        let all_nodes = self.get_all_nodes()?;
        for node in all_nodes {
            self.delete_node(&node.id).await?;
        }
        Ok(())
    }

    // ===== Persistence Methods (Critical for Production) =====

    /// Save graph snapshot to persistent storage
    /// Uses UnifiedCachingFilesystem for cloud-native storage support
    async fn save_snapshot(&self, _path: &Path) -> Result<()> {
        // Default implementation - engines should override for optimal performance
        Err(ProximaDBError::NotImplemented(
            "Graph persistence not implemented for this engine".to_string(),
        ))
    }

    /// Load graph from persistent storage
    /// Restores graph state from a previous snapshot
    async fn load_snapshot(&self, _path: &Path) -> Result<()> {
        Err(ProximaDBError::NotImplemented(
            "Graph persistence not implemented for this engine".to_string(),
        ))
    }

    /// Create incremental checkpoint (for WAL-style persistence)
    async fn checkpoint(&self) -> Result<()> {
        // Default no-op - engines can implement incremental checkpointing
        Ok(())
    }

    /// Export graph to standard format (GraphML, GEXF, etc.)
    async fn export(&self, _format: GraphExportFormat, _path: &Path) -> Result<()> {
        Err(ProximaDBError::NotImplemented(
            "Graph export not implemented for this engine".to_string(),
        ))
    }

    /// Import graph from standard format
    async fn import(&self, _format: GraphExportFormat, _path: &Path) -> Result<()> {
        Err(ProximaDBError::NotImplemented(
            "Graph import not implemented for this engine".to_string(),
        ))
    }

    /// Get persistence configuration for this engine
    fn get_persistence_config(&self) -> PersistenceConfig {
        PersistenceConfig::default()
    }

    /// Check if the engine supports persistence
    fn supports_persistence(&self) -> bool {
        false // Default: no persistence support
    }
}

/// Graph export/import formats for interoperability
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphExportFormat {
    /// GraphML - XML-based format for graph exchange
    GraphML,
    /// GEXF - Graph Exchange XML Format
    GEXF,
    /// JSON - Native JSON representation
    Json,
    /// Binary - ProximaDB binary format (most efficient)
    ProximaBinary,
    /// CSV - Node/Edge lists in CSV format
    Csv,
}

/// Persistence configuration for graph engines
#[derive(Debug, Clone)]
pub struct PersistenceConfig {
    /// Enable automatic snapshots
    pub auto_snapshot: bool,
    /// Snapshot interval in seconds
    pub snapshot_interval_secs: u64,
    /// Enable WAL (Write-Ahead Log) for crash recovery
    pub enable_wal: bool,
    /// WAL directory path
    pub wal_path: Option<String>,
    /// Compression algorithm for snapshots (using unified compression module)
    pub compression: CompressionAlgorithm,
    /// Compression level (algorithm-specific, typically 1-9 or 1-22 for Zstd)
    pub compression_level: i32,
    /// Use cloud storage for snapshots (S3, Azure, GCS)
    pub use_cloud_storage: bool,
    /// Cloud storage bucket/container
    pub cloud_storage_path: Option<String>,
    /// Enable incremental snapshots (only save changes since last snapshot)
    pub incremental_snapshots: bool,
    /// Maximum number of snapshots to retain
    pub max_snapshots: usize,
}

impl Default for PersistenceConfig {
    fn default() -> Self {
        Self {
            auto_snapshot: false,
            snapshot_interval_secs: 3600, // 1 hour default
            enable_wal: false,
            wal_path: None,
            compression: CompressionAlgorithm::Zstd, // Best general-purpose compression
            compression_level: 3,                    // Balanced speed/ratio for Zstd
            use_cloud_storage: false,
            cloud_storage_path: None,
            incremental_snapshots: true,
            max_snapshots: 10,
        }
    }
}
