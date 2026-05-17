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

//! # Graph Storage Engines
//!
//! ProximaDB implements multiple graph engines optimized for different workloads.
//! Under canonical durable graph storage, these engines consume `ProximaRecord`
//! node/edge records and maintain rebuildable topology projections rather than
//! owning a separate durable graph record model.
//!
//! - **ORION**: In-memory CSR projection for real-time traversal (1M+ edges/sec)
//! - **PULSAR**: Distributed engine for sharded graphs (1B+ nodes) [Phase 2]
//! - **QUASAR**: Hybrid hot/cold tiering for cost optimization [Phase 3]
//!
//! ## Embedding Storage Modes
//!
//! Graph engines support three embedding storage modes:
//!
//! - **None** (DEFAULT): No embeddings stored - pure graph, best performance
//! - **Cold**: Embeddings in vector engine (SST/HELIX/VIPER) - SKS with large graphs
//! - **Memory**: Embeddings cached in memory - SKS-heavy workloads, consumer override
//!
//! CSR (Compressed Sparse Row) format NEVER contains embedding data and is not
//! the durable authority for node or edge facts. Embeddings are optionally
//! stored in separate vector storage engines.

/// Generic graph traversal algorithms (BFS, DFS, Dijkstra, A*) usable across all engines.
pub mod generic_traversal;
/// ORION in-memory CSR projection engine for real-time traversal at 1M+ edges/sec.
pub mod orion;

// PULSAR: Distributed graph engine (experimental)
// Enable with: cargo build --features distributed-graph
#[cfg(feature = "distributed-graph")]
pub mod pulsar;

// QUASAR: Hybrid hot/cold tiering engine (experimental)
// Enable with: cargo build --features tiered-graph
#[cfg(feature = "tiered-graph")]
pub mod quasar;

// Provide stub modules when features are disabled to maintain API compatibility
#[cfg(not(feature = "distributed-graph"))]
pub mod pulsar {
    //! # PULSAR Graph Engine - RETIREMENT CANDIDATE
    //!
    //! This module is disabled by default and is marked for retirement in v0.3.0.
    //! To enable PULSAR, build with:
    //! ```bash
    //! cargo build --features distributed-graph
    //! ```
    //!
    //! **Note**: PULSAR is a legacy experimental engine.
    //! For production sharding, use ORION with application-level sharding
    //! as per the Phase 6 consolidation mandate.

    use crate::graph::{Edge, EdgeId, Node, NodeId};
    use proximadb_kernel::error::ProximaDBError;
    use std::sync::Arc;

    /// PULSAR configuration (stub)
    #[derive(Debug, Clone, Default)]
    pub struct PulsarConfig {
        /// Number of shards for data distribution
        pub shard_count: usize,
        /// Replication factor (stub field for API compatibility)
        pub replication_factor: u8,
        /// Consistency level (stub - uses default Quorum)
        pub consistency_level: u8,
        /// Enable cross-shard query optimization (stub field for API compatibility)
        pub cross_shard_optimization: bool,
        /// Maximum concurrent queries (stub field for API compatibility)
        pub max_concurrent_queries: usize,
    }

    /// PULSAR engine (stub - requires distributed-graph feature)
    #[derive(Debug)]
    pub struct PulsarGraphEngine;

    impl PulsarGraphEngine {
        /// Create a new PULSAR engine (requires `distributed-graph` feature).
        pub fn new(_config: PulsarConfig) -> Result<Self, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "PULSAR is a retirement candidate and requires 'distributed-graph' feature."
                    .to_string(),
            ))
        }

        /// Cross-shard traversal (stub - returns error when feature disabled)
        pub async fn cross_shard_traversal(
            &self,
            _start_node_id: &str,
            _max_depth: u32,
        ) -> Result<Vec<Arc<Node>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "PULSAR cross_shard_traversal requires 'distributed-graph' feature".to_string(),
            ))
        }
    }

    #[async_trait::async_trait]
    impl super::GraphEngine for PulsarGraphEngine {
        async fn insert_node(&self, _node: Node) -> Result<Arc<Node>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "PULSAR disabled".to_string(),
            ))
        }
        fn get_node(&self, _id: &NodeId) -> Result<Option<Arc<Node>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "PULSAR disabled".to_string(),
            ))
        }
        async fn update_node(&self, _node: Node) -> Result<Arc<Node>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "PULSAR disabled".to_string(),
            ))
        }
        async fn delete_node(&self, _id: &NodeId) -> Result<Option<Arc<Node>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "PULSAR disabled".to_string(),
            ))
        }
        async fn insert_edge(&self, _edge: Edge) -> Result<Arc<Edge>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "PULSAR disabled".to_string(),
            ))
        }
        fn get_edge(&self, _id: &EdgeId) -> Result<Option<Arc<Edge>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "PULSAR disabled".to_string(),
            ))
        }
        async fn update_edge(&self, _edge: Edge) -> Result<Arc<Edge>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "PULSAR disabled".to_string(),
            ))
        }
        async fn delete_edge(&self, _id: &EdgeId) -> Result<Option<Arc<Edge>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "PULSAR disabled".to_string(),
            ))
        }
        fn get_outgoing_edges(
            &self,
            _node_id: &NodeId,
            _edge_type: Option<&str>,
        ) -> Result<Vec<Arc<Edge>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "PULSAR disabled".to_string(),
            ))
        }
        fn get_incoming_edges(
            &self,
            _node_id: &NodeId,
            _edge_type: Option<&str>,
        ) -> Result<Vec<Arc<Edge>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "PULSAR disabled".to_string(),
            ))
        }
        fn get_neighbors(
            &self,
            _node_id: &NodeId,
            _edge_type: Option<&str>,
        ) -> Result<Vec<Arc<Node>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "PULSAR disabled".to_string(),
            ))
        }
        fn get_nodes_by_label(&self, _label: &str) -> Result<Vec<Arc<Node>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "PULSAR disabled".to_string(),
            ))
        }
        fn node_count(&self) -> Result<usize, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "PULSAR disabled".to_string(),
            ))
        }
        fn edge_count(&self) -> Result<usize, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "PULSAR disabled".to_string(),
            ))
        }
        fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "PULSAR disabled".to_string(),
            ))
        }
    }
}

#[cfg(not(feature = "tiered-graph"))]
pub mod quasar {
    //! # QUASAR Graph Engine - RETIREMENT CANDIDATE
    //!
    //! This module is disabled by default and is marked for retirement in v0.3.0.
    //! To enable QUASAR, build with:
    //! ```bash
    //! cargo build --features tiered-graph
    //! ```
    //!
    //! **Note**: QUASAR is a legacy experimental engine.
    //! For production use, use ORION as per the Phase 6 consolidation mandate.

    use crate::graph::{Edge, EdgeId, Node, NodeId};
    use proximadb_kernel::error::ProximaDBError;
    use std::sync::Arc;

    /// QUASAR configuration (stub)
    #[derive(Debug, Clone, Default)]
    pub struct QuasarConfig {
        /// Maximum size of hot tier (in number of nodes) - stub field for API compatibility
        pub hot_tier_max_nodes: usize,
        /// Maximum hot tier memory in MB (stub field for API compatibility)
        pub hot_tier_max_memory_mb: usize,
        /// Cold tier storage path (stub field for API compatibility)
        pub cold_tier_path: std::path::PathBuf,
        /// Cold migration threshold in seconds (stub field for API compatibility)
        pub cold_migration_threshold_secs: u64,
        /// Hot promotion threshold in seconds (stub field for API compatibility)
        pub hot_promotion_threshold_secs: u64,
        /// Migration interval in seconds (stub field for API compatibility)
        pub migration_interval_secs: u64,
        /// Cold storage backend (0=Sst, 1=Parquet, 2=Json) - stub field for API compatibility
        pub cold_storage_backend: u8,
    }

    /// QUASAR engine (stub - requires tiered-graph feature)
    #[derive(Debug)]
    pub struct QuasarGraphEngine;

    impl QuasarGraphEngine {
        /// Create a new QUASAR engine (requires `tiered-graph` feature).
        pub async fn new(_config: QuasarConfig) -> Result<Self, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "QUASAR is a retirement candidate and requires 'tiered-graph' feature.".to_string(),
            ))
        }
    }

    #[async_trait::async_trait]
    impl super::GraphEngine for QuasarGraphEngine {
        async fn insert_node(&self, _node: Node) -> Result<Arc<Node>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "QUASAR disabled".to_string(),
            ))
        }
        fn get_node(&self, _id: &NodeId) -> Result<Option<Arc<Node>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "QUASAR disabled".to_string(),
            ))
        }
        async fn update_node(&self, _node: Node) -> Result<Arc<Node>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "QUASAR disabled".to_string(),
            ))
        }
        async fn delete_node(&self, _id: &NodeId) -> Result<Option<Arc<Node>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "QUASAR disabled".to_string(),
            ))
        }
        async fn insert_edge(&self, _edge: Edge) -> Result<Arc<Edge>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "QUASAR disabled".to_string(),
            ))
        }
        fn get_edge(&self, _id: &EdgeId) -> Result<Option<Arc<Edge>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "QUASAR disabled".to_string(),
            ))
        }
        async fn update_edge(&self, _edge: Edge) -> Result<Arc<Edge>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "QUASAR disabled".to_string(),
            ))
        }
        async fn delete_edge(&self, _id: &EdgeId) -> Result<Option<Arc<Edge>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "QUASAR disabled".to_string(),
            ))
        }
        fn get_outgoing_edges(
            &self,
            _node_id: &NodeId,
            _edge_type: Option<&str>,
        ) -> Result<Vec<Arc<Edge>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "QUASAR disabled".to_string(),
            ))
        }
        fn get_incoming_edges(
            &self,
            _node_id: &NodeId,
            _edge_type: Option<&str>,
        ) -> Result<Vec<Arc<Edge>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "QUASAR disabled".to_string(),
            ))
        }
        fn get_neighbors(
            &self,
            _node_id: &NodeId,
            _edge_type: Option<&str>,
        ) -> Result<Vec<Arc<Node>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "QUASAR disabled".to_string(),
            ))
        }
        fn get_nodes_by_label(&self, _label: &str) -> Result<Vec<Arc<Node>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "QUASAR disabled".to_string(),
            ))
        }
        fn node_count(&self) -> Result<usize, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "QUASAR disabled".to_string(),
            ))
        }
        fn edge_count(&self) -> Result<usize, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "QUASAR disabled".to_string(),
            ))
        }
        fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>, ProximaDBError> {
            Err(ProximaDBError::NotImplemented(
                "QUASAR disabled".to_string(),
            ))
        }
    }
}

use proximadb_kernel::error::ProximaDBError;
type Result<T> = std::result::Result<T, ProximaDBError>;
use crate::core::serialization::CompressionAlgorithm;
use crate::graph::{Edge, EdgeId, Node, NodeId};
use crate::metrics::collectors::MetricsSample;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Embedding storage mode for graph engines
///
/// Controls how node embeddings are stored relative to the graph structure.
/// CSR (Compressed Sparse Row) format NEVER contains embedding data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmbeddingMode {
    /// No embeddings stored - pure graph workloads (DEFAULT, best performance)
    ///
    /// Use this for:
    /// - Pure graph traversal/analytics
    /// - Pattern matching
    /// - Path finding
    /// - When embeddings are not needed
    #[default]
    None,

    /// Embeddings stored in cold tier vector engine (SST/HELIX/VIPER)
    ///
    /// Use this for:
    /// - SKS (Semantic Knowledge Search) with large graphs
    /// - When graph + embeddings don't fit in memory
    /// - Cost-optimized production deployments
    Cold,

    /// Embeddings cached in memory (consumer override)
    ///
    /// Use this for:
    /// - SKS-heavy workloads with smaller graphs
    /// - When latency is critical
    /// - Development/testing
    Memory,
}

impl EmbeddingMode {
    /// Parse embedding mode from config string
    pub fn parse_from_config(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "cold" => EmbeddingMode::Cold,
            "memory" => EmbeddingMode::Memory,
            _ => EmbeddingMode::None, // Default for "none" or invalid
        }
    }

    /// Check if embeddings are stored at all
    pub fn stores_embeddings(&self) -> bool {
        !matches!(self, EmbeddingMode::None)
    }

    /// Check if embeddings are in cold tier
    pub fn is_cold(&self) -> bool {
        matches!(self, EmbeddingMode::Cold)
    }

    /// Check if embeddings are in memory
    pub fn is_memory(&self) -> bool {
        matches!(self, EmbeddingMode::Memory)
    }
}

#[cfg(test)]
mod embedding_mode_tests {
    use super::EmbeddingMode;

    #[test]
    fn test_embedding_mode_default() {
        // Default mode should be None (pure graph, best performance)
        let mode = EmbeddingMode::default();
        assert_eq!(mode, EmbeddingMode::None);
        assert!(!mode.stores_embeddings());
    }

    #[test]
    fn test_embedding_mode_parse_from_config() {
        // Test parsing from config strings
        assert_eq!(
            EmbeddingMode::parse_from_config("none"),
            EmbeddingMode::None
        );
        assert_eq!(
            EmbeddingMode::parse_from_config("None"),
            EmbeddingMode::None
        );
        assert_eq!(
            EmbeddingMode::parse_from_config("NONE"),
            EmbeddingMode::None
        );
        assert_eq!(
            EmbeddingMode::parse_from_config("cold"),
            EmbeddingMode::Cold
        );
        assert_eq!(
            EmbeddingMode::parse_from_config("Cold"),
            EmbeddingMode::Cold
        );
        assert_eq!(
            EmbeddingMode::parse_from_config("COLD"),
            EmbeddingMode::Cold
        );
        assert_eq!(
            EmbeddingMode::parse_from_config("memory"),
            EmbeddingMode::Memory
        );
        assert_eq!(
            EmbeddingMode::parse_from_config("Memory"),
            EmbeddingMode::Memory
        );
        assert_eq!(
            EmbeddingMode::parse_from_config("MEMORY"),
            EmbeddingMode::Memory
        );

        // Invalid strings should default to None
        assert_eq!(
            EmbeddingMode::parse_from_config("invalid"),
            EmbeddingMode::None
        );
        assert_eq!(EmbeddingMode::parse_from_config(""), EmbeddingMode::None);
    }

    #[test]
    fn test_embedding_mode_stores_embeddings() {
        assert!(!EmbeddingMode::None.stores_embeddings());
        assert!(EmbeddingMode::Cold.stores_embeddings());
        assert!(EmbeddingMode::Memory.stores_embeddings());
    }

    #[test]
    fn test_embedding_mode_is_cold() {
        assert!(!EmbeddingMode::None.is_cold());
        assert!(EmbeddingMode::Cold.is_cold());
        assert!(!EmbeddingMode::Memory.is_cold());
    }

    #[test]
    fn test_embedding_mode_is_memory() {
        assert!(!EmbeddingMode::None.is_memory());
        assert!(!EmbeddingMode::Cold.is_memory());
        assert!(EmbeddingMode::Memory.is_memory());
    }
}

/// Engine performance statistics integrated with unified metrics framework
#[derive(Debug, Clone, Default)]
pub struct EngineStats {
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

impl EngineStats {
    /// Convert to unified metrics sample for integration with metrics framework
    pub fn to_metrics_sample(&self, engine_name: &str) -> MetricsSample {
        let mut values = HashMap::new();
        values.insert("node_count".to_string(), self.node_count as f64);
        values.insert("edge_count".to_string(), self.edge_count as f64);
        values.insert("avg_degree".to_string(), self.avg_degree);
        values.insert(
            "connected_components".to_string(),
            self.connected_components as f64,
        );
        values.insert("total_operations".to_string(), self.total_operations as f64);
        values.insert("cache_hit_ratio".to_string(), self.cache_hit_ratio);
        values.insert("index_efficiency".to_string(), self.index_efficiency);
        values.insert("total_time_us".to_string(), self.total_time_us as f64);

        MetricsSample {
            timestamp: std::time::Instant::now(),
            collector: format!("graph_engine_{}", engine_name),
            values,
        }
    }
}

/// Memory usage metrics integrated with unified metrics framework
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

impl MemoryUsage {
    /// Convert to unified metrics sample for integration with metrics framework
    pub fn to_metrics_sample(&self, engine_name: &str) -> MetricsSample {
        let mut values = HashMap::new();
        values.insert("nodes_memory_bytes".to_string(), self.nodes_memory as f64);
        values.insert("edges_memory_bytes".to_string(), self.edges_memory as f64);
        values.insert(
            "indexes_memory_bytes".to_string(),
            self.indexes_memory as f64,
        );
        values.insert("cache_memory_bytes".to_string(), self.cache_memory as f64);
        values.insert("total_memory_bytes".to_string(), self.total_memory as f64);
        values.insert("peak_memory_bytes".to_string(), self.peak_memory as f64);

        MetricsSample {
            timestamp: std::time::Instant::now(),
            collector: format!("graph_memory_{}", engine_name),
            values,
        }
    }
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

    // ===== Performance & Benchmarking Methods =====

    /// Get engine performance statistics
    fn get_engine_stats(&self) -> Result<EngineStats> {
        // Default implementation for backward compatibility
        Ok(EngineStats::default())
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

/// Engine type enumeration for factory creation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphEngineType {
    /// ORION: In-memory CSR format engine (Production-ready).
    Orion,
    /// PULSAR: Distributed sharded engine (Experimental / Retirement Candidate).
    Pulsar,
    /// QUASAR: Hybrid hot/cold tiering engine (Experimental / Retirement Candidate).
    Quasar,
}

/// Enum wrapper for different graph engine implementations
#[derive(Debug)]
pub enum GraphEngineImpl {
    /// ORION in-memory CSR engine instance.
    Orion(orion::OrionGraphEngine),
    /// PULSAR distributed sharded engine instance (Legacy / Retirement Candidate).
    Pulsar(pulsar::PulsarGraphEngine),
    /// QUASAR hybrid hot/cold tiering engine instance (Legacy / Retirement Candidate).
    Quasar(quasar::QuasarGraphEngine),
}

impl GraphEngineImpl {
    /// Create a new graph engine based on type and configuration
    pub fn new(engine_type: GraphEngineType, config: GraphEngineConfig) -> Result<Self> {
        match engine_type {
            GraphEngineType::Orion => {
                let engine = orion::OrionGraphEngine::new();
                Ok(GraphEngineImpl::Orion(engine))
            }
            GraphEngineType::Pulsar => {
                let pulsar_config = config.pulsar_config.unwrap_or_default();
                let engine = pulsar::PulsarGraphEngine::new(pulsar_config)?;
                Ok(GraphEngineImpl::Pulsar(engine))
            }
            GraphEngineType::Quasar => {
                let _quasar_config = config.quasar_config.unwrap_or_default();
                // Note: This needs async, so we'll provide a different factory method
                Err(ProximaDBError::InvalidInput(
                    "Use new_quasar_async for QUASAR engine".to_string(),
                ))
            }
        }
    }

    /// Create a Quasar engine asynchronously
    pub async fn new_quasar_async(config: quasar::QuasarConfig) -> Result<Self> {
        let engine = quasar::QuasarGraphEngine::new(config).await?;
        Ok(GraphEngineImpl::Quasar(engine))
    }
}

// Implement GraphEngine for GraphEngineImpl by delegating to the underlying engine
#[async_trait::async_trait]
impl GraphEngine for GraphEngineImpl {
    async fn insert_node(&self, node: Node) -> Result<Arc<Node>> {
        tracing::debug!("GraphEngineImpl::insert_node called for node: {}", node.id);
        match self {
            GraphEngineImpl::Orion(engine) => {
                tracing::debug!("Delegating to Orion engine");
                engine.insert_node(node).await
            }
            GraphEngineImpl::Pulsar(engine) => engine.insert_node(node).await,
            GraphEngineImpl::Quasar(engine) => engine.insert_node(node).await,
        }
    }

    fn get_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>> {
        match self {
            GraphEngineImpl::Orion(engine) => engine.get_node(id),
            GraphEngineImpl::Pulsar(engine) => engine.get_node(id),
            GraphEngineImpl::Quasar(engine) => engine.get_node(id),
        }
    }

    async fn update_node(&self, node: Node) -> Result<Arc<Node>> {
        match self {
            GraphEngineImpl::Orion(engine) => engine.update_node(node).await,
            GraphEngineImpl::Pulsar(engine) => engine.update_node(node).await,
            GraphEngineImpl::Quasar(engine) => engine.update_node(node).await,
        }
    }

    async fn delete_node(&self, id: &NodeId) -> Result<Option<Arc<Node>>> {
        match self {
            GraphEngineImpl::Orion(engine) => GraphEngine::delete_node(engine, id).await,
            GraphEngineImpl::Pulsar(engine) => GraphEngine::delete_node(engine, id).await,
            GraphEngineImpl::Quasar(engine) => GraphEngine::delete_node(engine, id).await,
        }
    }

    async fn insert_edge(&self, edge: Edge) -> Result<Arc<Edge>> {
        match self {
            GraphEngineImpl::Orion(engine) => engine.insert_edge(edge).await,
            GraphEngineImpl::Pulsar(engine) => engine.insert_edge(edge).await,
            GraphEngineImpl::Quasar(engine) => engine.insert_edge(edge).await,
        }
    }

    fn get_edge(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        match self {
            GraphEngineImpl::Orion(engine) => engine.get_edge(id),
            GraphEngineImpl::Pulsar(engine) => engine.get_edge(id),
            GraphEngineImpl::Quasar(engine) => engine.get_edge(id),
        }
    }

    async fn update_edge(&self, edge: Edge) -> Result<Arc<Edge>> {
        match self {
            GraphEngineImpl::Orion(engine) => engine.update_edge(edge).await,
            GraphEngineImpl::Pulsar(engine) => engine.update_edge(edge).await,
            GraphEngineImpl::Quasar(engine) => engine.update_edge(edge).await,
        }
    }

    async fn delete_edge(&self, id: &EdgeId) -> Result<Option<Arc<Edge>>> {
        match self {
            GraphEngineImpl::Orion(engine) => GraphEngine::delete_edge(engine, id).await,
            GraphEngineImpl::Pulsar(engine) => GraphEngine::delete_edge(engine, id).await,
            GraphEngineImpl::Quasar(engine) => GraphEngine::delete_edge(engine, id).await,
        }
    }

    fn get_outgoing_edges(
        &self,
        node_id: &NodeId,
        edge_type: Option<&str>,
    ) -> Result<Vec<Arc<Edge>>> {
        match self {
            GraphEngineImpl::Orion(engine) => engine.get_outgoing_edges(node_id, edge_type),
            GraphEngineImpl::Pulsar(engine) => engine.get_outgoing_edges(node_id, edge_type),
            GraphEngineImpl::Quasar(engine) => engine.get_outgoing_edges(node_id, edge_type),
        }
    }

    fn get_incoming_edges(
        &self,
        node_id: &NodeId,
        edge_type: Option<&str>,
    ) -> Result<Vec<Arc<Edge>>> {
        match self {
            GraphEngineImpl::Orion(engine) => engine.get_incoming_edges(node_id, edge_type),
            GraphEngineImpl::Pulsar(engine) => engine.get_incoming_edges(node_id, edge_type),
            GraphEngineImpl::Quasar(engine) => engine.get_incoming_edges(node_id, edge_type),
        }
    }

    fn get_neighbors(&self, node_id: &NodeId, edge_type: Option<&str>) -> Result<Vec<Arc<Node>>> {
        match self {
            GraphEngineImpl::Orion(engine) => engine.get_neighbors(node_id, edge_type),
            GraphEngineImpl::Pulsar(engine) => engine.get_neighbors(node_id, edge_type),
            GraphEngineImpl::Quasar(engine) => engine.get_neighbors(node_id, edge_type),
        }
    }

    fn get_nodes_by_label(&self, label: &str) -> Result<Vec<Arc<Node>>> {
        match self {
            GraphEngineImpl::Orion(engine) => engine.get_nodes_by_label(label),
            GraphEngineImpl::Pulsar(engine) => engine.get_nodes_by_label(label),
            GraphEngineImpl::Quasar(engine) => engine.get_nodes_by_label(label),
        }
    }

    fn node_count(&self) -> Result<usize> {
        match self {
            GraphEngineImpl::Orion(engine) => engine.node_count(),
            GraphEngineImpl::Pulsar(engine) => engine.node_count(),
            GraphEngineImpl::Quasar(engine) => engine.node_count(),
        }
    }

    fn edge_count(&self) -> Result<usize> {
        match self {
            GraphEngineImpl::Orion(engine) => engine.edge_count(),
            GraphEngineImpl::Pulsar(engine) => engine.edge_count(),
            GraphEngineImpl::Quasar(engine) => engine.edge_count(),
        }
    }

    fn get_all_nodes(&self) -> Result<Vec<Arc<Node>>> {
        match self {
            GraphEngineImpl::Orion(engine) => engine.get_all_nodes(),
            GraphEngineImpl::Pulsar(engine) => engine.get_all_nodes(),
            GraphEngineImpl::Quasar(engine) => engine.get_all_nodes(),
        }
    }

    // Delegate other methods with default implementations
    fn get_engine_stats(&self) -> Result<EngineStats> {
        match self {
            GraphEngineImpl::Orion(engine) => engine.get_engine_stats(),
            GraphEngineImpl::Pulsar(engine) => engine.get_engine_stats(),
            GraphEngineImpl::Quasar(engine) => engine.get_engine_stats(),
        }
    }

    fn get_memory_usage(&self) -> Result<MemoryUsage> {
        match self {
            GraphEngineImpl::Orion(engine) => engine.get_memory_usage(),
            GraphEngineImpl::Pulsar(engine) => engine.get_memory_usage(),
            GraphEngineImpl::Quasar(engine) => engine.get_memory_usage(),
        }
    }

    // ===== Bulk Operations - Critical for Performance =====
    // These MUST be delegated to underlying engines to avoid O(n) per-edge overhead

    async fn bulk_insert_nodes(&self, nodes: Vec<Node>) -> Result<Vec<Arc<Node>>> {
        match self {
            GraphEngineImpl::Orion(engine) => engine.bulk_insert_nodes(nodes).await,
            GraphEngineImpl::Pulsar(engine) => engine.bulk_insert_nodes(nodes).await,
            GraphEngineImpl::Quasar(engine) => engine.bulk_insert_nodes(nodes).await,
        }
    }

    async fn bulk_insert_edges(&self, edges: Vec<Edge>) -> Result<Vec<Arc<Edge>>> {
        match self {
            GraphEngineImpl::Orion(engine) => engine.bulk_insert_edges(edges).await,
            GraphEngineImpl::Pulsar(engine) => engine.bulk_insert_edges(edges).await,
            GraphEngineImpl::Quasar(engine) => engine.bulk_insert_edges(edges).await,
        }
    }

    async fn bulk_delete_nodes(&self, node_ids: Vec<NodeId>) -> Result<Vec<Option<Arc<Node>>>> {
        match self {
            GraphEngineImpl::Orion(engine) => engine.bulk_delete_nodes(node_ids).await,
            GraphEngineImpl::Pulsar(engine) => engine.bulk_delete_nodes(node_ids).await,
            GraphEngineImpl::Quasar(engine) => engine.bulk_delete_nodes(node_ids).await,
        }
    }

    async fn bulk_delete_edges(&self, edge_ids: Vec<EdgeId>) -> Result<Vec<Option<Arc<Edge>>>> {
        match self {
            GraphEngineImpl::Orion(engine) => engine.bulk_delete_edges(edge_ids).await,
            GraphEngineImpl::Pulsar(engine) => engine.bulk_delete_edges(edge_ids).await,
            GraphEngineImpl::Quasar(engine) => engine.bulk_delete_edges(edge_ids).await,
        }
    }
}

/// Graph engine factory for creating different engine types
pub struct GraphEngineFactory;

impl GraphEngineFactory {
    /// Create a graph engine based on type and configuration
    pub fn create_engine(
        engine_type: GraphEngineType,
        config: GraphEngineConfig,
    ) -> Result<GraphEngineImpl> {
        GraphEngineImpl::new(engine_type, config)
    }

    /// Create QUASAR engine asynchronously (required for initialization)
    pub async fn create_quasar_engine_async(
        config: quasar::QuasarConfig,
    ) -> Result<GraphEngineImpl> {
        GraphEngineImpl::new_quasar_async(config).await
    }

    /// Get available engine types
    pub fn available_engines() -> Vec<GraphEngineType> {
        vec![
            GraphEngineType::Orion,
            GraphEngineType::Pulsar,
            GraphEngineType::Quasar,
        ]
    }

    /// Get engine type from string
    pub fn engine_type_from_string(name: &str) -> Option<GraphEngineType> {
        match name.to_lowercase().as_str() {
            "orion" => Some(GraphEngineType::Orion),
            "pulsar" => Some(GraphEngineType::Pulsar),
            "quasar" => Some(GraphEngineType::Quasar),
            _ => None,
        }
    }
}

/// Configuration for graph engine creation
#[derive(Debug, Clone, Default)]
pub struct GraphEngineConfig {
    /// Optional PULSAR distributed engine configuration.
    pub pulsar_config: Option<pulsar::PulsarConfig>,
    /// Optional QUASAR hybrid tiering engine configuration.
    pub quasar_config: Option<quasar::QuasarConfig>,
}

/// Engine capabilities description
#[derive(Debug, Clone)]
pub struct EngineCapabilities {
    /// Engine name (e.g. "ORION", "PULSAR", "QUASAR").
    pub name: String,
    /// Human-readable description of the engine.
    pub description: String,
    /// List of supported features (e.g. "CSR storage", "DashMap concurrent access").
    pub features: Vec<String>,
    /// Recommended use cases for this engine.
    pub use_cases: Vec<String>,
    /// Performance characteristics (e.g. "1M+ edges/second traversal").
    pub performance_characteristics: Vec<String>,
}

impl GraphEngineFactory {
    /// Get capabilities for each engine type
    pub fn get_engine_capabilities(engine_type: GraphEngineType) -> EngineCapabilities {
        match engine_type {
            GraphEngineType::Orion => EngineCapabilities {
                name: "ORION".to_string(),
                description: "In-memory CSR format for real-time traversal".to_string(),
                features: vec![
                    "CSR (Compressed Sparse Row) storage".to_string(),
                    "Arc-based zero-copy sharing".to_string(),
                    "DashMap concurrent access".to_string(),
                    "Label and property indexes".to_string(),
                ],
                use_cases: vec![
                    "Real-time graph traversal".to_string(),
                    "Interactive graph queries".to_string(),
                    "Small to medium graphs (<1M nodes)".to_string(),
                ],
                performance_characteristics: vec![
                    "1M+ edges/second traversal".to_string(),
                    "<1μs node lookup".to_string(),
                    "<100 bytes/node memory overhead".to_string(),
                ],
            },
            GraphEngineType::Pulsar => EngineCapabilities {
                name: "PULSAR (Legacy)".to_string(),
                description: "Experimental distributed sharded engine (Retirement Candidate)"
                    .to_string(),
                features: vec![
                    "Consistent hash-based sharding".to_string(),
                    "Configurable replication (1-3x)".to_string(),
                    "Cross-shard query coordination".to_string(),
                    "Distributed BFS/DFS traversal".to_string(),
                ],
                use_cases: vec![
                    "Legacy distributed workloads".to_string(),
                    "Research and experimentation".to_string(),
                ],
                performance_characteristics: vec![
                    "Experimental horizontal scalability".to_string(),
                    "Note: Performance may vary significantly from production standards"
                        .to_string(),
                ],
            },
            GraphEngineType::Quasar => EngineCapabilities {
                name: "QUASAR (Legacy)".to_string(),
                description: "Experimental hybrid hot/cold tiering (Retirement Candidate)"
                    .to_string(),
                features: vec![
                    "Automatic hot/cold tiering".to_string(),
                    "LRU-based cache management".to_string(),
                    "Access pattern tracking".to_string(),
                    "Background data migration".to_string(),
                ],
                use_cases: vec![
                    "Cost-optimization research".to_string(),
                    "Sparse graph workloads (experimental)".to_string(),
                ],
                performance_characteristics: vec![
                    "Experimental storage cost savings".to_string(),
                    "Note: Cold tier has no canonical record binding in current version"
                        .to_string(),
                ],
            },
        }
    }
}
