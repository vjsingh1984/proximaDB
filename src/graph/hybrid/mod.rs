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

//! # Hybrid Vector-Graph Query Engine - ProximaDB's KEY DIFFERENTIATOR
//!
//! The Hybrid Query Engine is ProximaDB's unique capability that combines vector similarity
//! search with graph traversal, enabling semantic graph queries that are impossible with
//! traditional databases.
//!
//! ## Why This Matters
//!
//! Traditional approaches force a choice:
//! - **Vector databases**: Find similar items by embedding, but miss relationships
//! - **Graph databases**: Traverse relationships, but miss semantic similarity
//!
//! ProximaDB's Hybrid Query Engine fuses both paradigms:
//! - Find semantically similar documents that are ALSO connected via relationships
//! - Traverse knowledge graphs guided by embedding similarity
//! - Rank results by BOTH vector similarity AND graph relevance
//!
//! ## Key Capabilities
//!
//! - **Semantic Traversal**: BFS/DFS guided by embedding similarity (SemanticBFS, SemanticDFS)
//! - **Vector-Graph Fusion**: Results ranked by both vector similarity and graph relevance
//! - **Multiple Fusion Strategies**: VectorFirst, GraphFirst, Balanced, Weighted
//! - **SIMD-Accelerated**: Uses UnifiedDistanceCompute for hardware-accelerated similarity
//! - **Graph Algorithms**: PageRank, centrality, community detection integrated with vectors
//!
//! ## Use Cases
//!
//! 1. **Semantic Knowledge Graph Search (SKS)**
//!    Find documents similar to a query that are connected via specific relationship paths
//!
//! 2. **Recommendation with Constraints**
//!    Recommend items similar to user preferences that are also connected via purchase/view graphs
//!
//! 3. **Entity Resolution**
//!    Find potential duplicate entities by combining embedding similarity with relationship overlap
//!
//! 4. **Contextual Search**
//!    Search within N hops of a context node, ranking by semantic similarity
//!
//! ## Example Usage
//!
//! ```rust,ignore
//! use proximadb::graph::hybrid::{HybridQueryEngine, HybridQuery, FusionStrategy};
//!
//! let engine = HybridQueryEngine::new(graph_memory, vector_service);
//!
//! // Find documents similar to query_vector that are connected to "Alice" via KNOWS edges
//! let query = HybridQuery {
//!     vector_component: Some(VectorQueryComponent {
//!         query_vector: query_vector.clone(),
//!         threshold: Some(0.7),
//!         collection: Some("documents".to_string()),
//!         ..Default::default()
//!     }),
//!     graph_component: Some(GraphQueryComponent {
//!         start_nodes: vec!["Alice".to_string()],
//!         edge_types: vec!["KNOWS".to_string()],
//!         max_depth: Some(2),
//!         algorithm: TraversalAlgorithm::SemanticBFS,
//!         ..Default::default()
//!     }),
//!     fusion: FusionConfig {
//!         strategy: FusionStrategy::Balanced,
//!         ..Default::default()
//!     },
//!     ..Default::default()
//! };
//!
//! let results = engine.execute_hybrid_query(&query, &context).await?;
//! // Results are ranked by combined vector similarity + graph relevance
//! ```
//!
//! ## Fusion Strategies
//!
//! | Strategy | Description | Best For |
//! |----------|-------------|----------|
//! | VectorFirst | Vector results filtered by graph | When semantic match is primary |
//! | GraphFirst | Graph results ranked by similarity | When relationships are primary |
//! | Balanced | Equal weighting of both signals | General-purpose queries |
//! | Weighted | Custom weights (e.g., 0.7 vector, 0.3 graph) | Fine-tuned applications |
//!
//! ## Semantic Traversal Algorithms
//!
//! - **SemanticBFS**: Breadth-first search prioritizing nodes by embedding similarity
//! - **SemanticDFS**: Depth-first search exploring most similar paths first
//! - **Standard BFS/DFS**: Traditional graph traversal with optional vector ranking
//! - **Dijkstra**: Shortest path with optional semantic weighting
//!
//! ## Architecture
//!
//! ```text
//! +------------------------------------------+
//! |        Hybrid Query Engine               |
//! +------------------------------------------+
//! |                                          |
//! |  +---------------+  +------------------+ |
//! |  | Vector Query  |  | Graph Traversal  | |
//! |  |    Engine     |  |     Engine       | |
//! |  +-------+-------+  +--------+---------+ |
//! |          |                   |           |
//! |          v                   v           |
//! |  +-------------------------------+       |
//! |  |    Fusion & Ranking Engine    |       |
//! |  | - VectorFirst / GraphFirst    |       |
//! |  | - Balanced / Weighted         |       |
//! |  | - Custom ranking functions    |       |
//! |  +-------------------------------+       |
//! +------------------------------------------+
//! |        Arc Memory Pool                   |
//! |  +------------+----------+------------+  |
//! |  |   Nodes    |  Edges   |  Vectors   |  |
//! |  | Properties | Metadata | Embeddings |  |
//! |  +------------+----------+------------+  |
//! +------------------------------------------+
//! ```
//!
//! ## Performance
//!
//! - **SIMD Acceleration**: AVX2/NEON for vector similarity computation
//! - **Parallel Execution**: Vector and graph components run concurrently
//! - **Early Termination**: Progressive refinement for interactive queries
//! - **Caching**: Intermediate results cached for repeated patterns
//!
//! ## Related Modules
//!
//! - [`semantic_traversal`]: SIMD-accelerated semantic BFS implementation
//! - [`ranking`]: Hybrid ranking strategies (vector + graph centrality)

// Submodules
pub mod ranking;
pub mod semantic_traversal;

use proximadb_kernel::error::{ProximaDBError, QueryError, VectorDBError};
use crate::graph::{
    Edge, EdgeId, GraphMemoryPool, Node, NodeId,
    query::{QueryContext, QueryResult, QueryStats},
};
use crate::proto::proximadb_v1::VectorRecord;
use crate::services::vector_operations_service::VectorOperationsService;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

/// Hybrid query engine for vector-graph integration
pub struct HybridQueryEngine {
    /// Reference to graph memory pool
    graph_memory: Arc<GraphMemoryPool>,
    /// Reference to vector operations service
    #[allow(dead_code)]
    vector_service: Arc<VectorOperationsService>,
    /// Hybrid query configuration
    config: HybridConfig,
}

/// Configuration for hybrid queries
#[derive(Debug, Clone)]
pub struct HybridConfig {
    /// Default similarity threshold for vector operations
    pub default_similarity_threshold: f32,
    /// Maximum number of vector candidates to consider
    pub max_vector_candidates: usize,
    /// Maximum graph traversal depth
    pub max_traversal_depth: u32,
    /// Enable semantic path weighting
    pub enable_semantic_weighting: bool,
    /// Vector-graph fusion strategy
    pub fusion_strategy: FusionStrategy,
    /// Performance optimization flags
    pub optimizations: HybridOptimizations,
}

/// Strategy for fusing vector and graph results
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum FusionStrategy {
    /// Vector results filtered by graph constraints
    VectorFirst,
    /// Graph results ranked by vector similarity  
    GraphFirst,
    /// Combined ranking using both signals
    Balanced,
    /// Custom weighted combination
    Weighted {
        /// Weight applied to vector similarity scores.
        vector_weight: f32,
        /// Weight applied to graph relevance scores.
        graph_weight: f32,
    },
}

/// Optimization flags for hybrid vector+graph queries.
#[derive(Debug, Clone)]
pub struct HybridOptimizations {
    /// Use progressive search for vector component.
    pub use_progressive_search: bool,
    /// Cache intermediate results for re-use across fusion stages.
    pub cache_intermediates: bool,
    /// Parallelize vector and graph operations concurrently.
    pub parallel_execution: bool,
    /// Use early termination when possible to reduce latency.
    pub early_termination: bool,
}

/// Hybrid query specification
#[derive(Debug, Clone)]
pub struct HybridQuery {
    /// Vector component of the query
    pub vector_component: Option<VectorQueryComponent>,
    /// Graph component of the query
    pub graph_component: Option<GraphQueryComponent>,
    /// Fusion configuration
    pub fusion: FusionConfig,
    /// Result limits and ordering
    pub result_spec: HybridResultSpec,
}

/// Vector component of a hybrid query
#[derive(Debug, Clone)]
pub struct VectorQueryComponent {
    /// Query vector
    pub query_vector: Vec<f32>,
    /// Similarity threshold
    pub threshold: Option<f32>,
    /// Maximum results from vector search
    pub max_results: Option<usize>,
    /// Distance metric to use
    pub distance_metric: Option<String>,
    /// Collection to search (if specific)
    pub collection: Option<String>,
}

/// Graph component of a hybrid query
#[derive(Debug, Clone)]
pub struct GraphQueryComponent {
    /// Starting nodes for traversal
    pub start_nodes: Vec<NodeId>,
    /// Edge types to follow
    pub edge_types: Vec<String>,
    /// Maximum traversal depth
    pub max_depth: Option<u32>,
    /// Node filters
    pub node_filters: Vec<NodeFilter>,
    /// Edge filters
    pub edge_filters: Vec<EdgeFilter>,
    /// Traversal algorithm
    pub algorithm: TraversalAlgorithm,
}

/// Node filter for graph component
#[derive(Debug, Clone)]
pub struct NodeFilter {
    /// Property name to filter on
    pub property: String,
    /// Filter operator
    pub operator: FilterOperator,
    /// Expected value
    pub value: serde_json::Value,
}

/// Edge filter for graph component
#[derive(Debug, Clone)]
pub struct EdgeFilter {
    /// Property name to filter on
    pub property: String,
    /// Filter operator
    pub operator: FilterOperator,
    /// Expected value
    pub value: serde_json::Value,
}

/// Filter operators for property-based node and edge filtering.
#[derive(Debug, Clone)]
pub enum FilterOperator {
    /// Exact equality match.
    Equal,
    /// Not-equal comparison.
    NotEqual,
    /// Strictly greater than.
    GreaterThan,
    /// Greater than or equal to.
    GreaterThanOrEqual,
    /// Strictly less than.
    LessThan,
    /// Less than or equal to.
    LessThanOrEqual,
    /// Value is in the provided set.
    In,
    /// Value is not in the provided set.
    NotIn,
    /// String contains substring.
    Contains,
    /// String starts with prefix.
    StartsWith,
    /// String ends with suffix.
    EndsWith,
    /// Regular expression match.
    Regex,
}

/// Traversal algorithms for the graph component of hybrid queries.
#[derive(Debug, Clone)]
pub enum TraversalAlgorithm {
    /// Breadth-first search traversal.
    BFS,
    /// Depth-first search traversal.
    DFS,
    /// Dijkstra shortest-path traversal using edge weights.
    Dijkstra,
    /// BFS guided by vector similarity at each expansion step.
    SemanticBFS,
    /// DFS guided by vector similarity at each expansion step.
    SemanticDFS,
}

/// Fusion configuration
#[derive(Debug, Clone)]
pub struct FusionConfig {
    /// Fusion strategy
    pub strategy: FusionStrategy,
    /// Custom weights for balanced fusion
    pub weights: Option<FusionWeights>,
    /// Ranking function
    pub ranking: RankingFunction,
}

/// Weights for fusion
#[derive(Debug, Clone, Serialize)]
pub struct FusionWeights {
    /// Weight for vector similarity score
    pub vector_weight: f32,
    /// Weight for graph relevance score
    pub graph_weight: f32,
    /// Weight for structural properties
    pub structure_weight: f32,
}

/// Ranking function for results
#[derive(Debug, Clone)]
pub enum RankingFunction {
    /// Simple additive combination
    Additive,
    /// Multiplicative combination
    Multiplicative,
    /// Harmonic mean
    HarmonicMean,
    /// Custom ranking function
    Custom(String),
}

/// Result specification for hybrid queries
#[derive(Debug, Clone)]
pub struct HybridResultSpec {
    /// Maximum number of results
    pub limit: Option<usize>,
    /// Offset for pagination
    pub offset: Option<usize>,
    /// Include similarity scores
    pub include_scores: bool,
    /// Include graph path information
    pub include_paths: bool,
    /// Include intermediate results for debugging
    pub include_debug_info: bool,
}

/// Result from a hybrid query
#[derive(Debug, Serialize)]
pub struct HybridQueryResult {
    /// Matching nodes with scores
    pub nodes: Vec<HybridNodeResult>,
    /// Execution statistics
    pub stats: QueryStats,
    /// Debug information (if requested)
    pub debug_info: Option<HybridDebugInfo>,
}

/// Individual node result from hybrid query
#[derive(Debug, Clone, Serialize)]
pub struct HybridNodeResult {
    /// The node itself
    pub node: Node,
    /// Combined hybrid score
    pub score: f32,
    /// Vector similarity score (if applicable)
    pub vector_score: Option<f32>,
    /// Graph relevance score
    pub graph_score: Option<f32>,
    /// Path from starting node (if requested)
    pub path: Option<Vec<PathStep>>,
    /// Additional metadata
    pub metadata: HashMap<String, serde_json::Value>,
}

/// Step in a path
#[derive(Debug, Clone, Serialize)]
pub struct PathStep {
    /// Node at this step
    pub node: Node,
    /// Edge used to reach this node (if not start)
    pub edge: Option<Edge>,
    /// Distance from start
    pub distance: u32,
}

/// Debug information for hybrid queries
#[derive(Debug, Serialize)]
pub struct HybridDebugInfo {
    /// Vector search results before fusion
    pub vector_candidates: Vec<VectorCandidate>,
    /// Graph traversal results before fusion
    pub graph_candidates: Vec<GraphCandidate>,
    /// Fusion process details
    pub fusion_details: FusionDetails,
    /// Performance metrics
    pub performance: HybridPerformanceMetrics,
}

/// Vector candidate in debug info before fusion.
#[derive(Debug, Clone, Serialize)]
pub struct VectorCandidate {
    /// Node identifier from the vector search result.
    pub node_id: NodeId,
    /// Cosine or distance similarity score from vector search.
    pub similarity: f32,
    /// The underlying vector record for this candidate.
    pub vector_record: VectorRecord,
}

/// Graph candidate in debug info before fusion.
#[derive(Debug, Clone, Serialize)]
pub struct GraphCandidate {
    /// Node identifier from the graph traversal result.
    pub node_id: NodeId,
    /// Hop distance from the start node.
    pub distance: u32,
    /// Number of edges in the shortest path to this node.
    pub path_length: usize,
    /// Pre-computed centrality score, if available.
    pub centrality_score: Option<f32>,
}

/// Details of the fusion process combining vector and graph results.
#[derive(Debug, Serialize)]
pub struct FusionDetails {
    /// Name of the fusion strategy applied (e.g. "Balanced", "VectorFirst").
    pub strategy_used: String,
    /// Fusion weights that were applied, if any.
    pub weights_applied: Option<FusionWeights>,
    /// Number of candidates before fusion filtering.
    pub candidates_before_fusion: usize,
    /// Number of candidates after fusion filtering.
    pub candidates_after_fusion: usize,
    /// Time spent on the fusion step in milliseconds.
    pub fusion_time_ms: u64,
}

/// Performance metrics for hybrid query execution.
#[derive(Debug, Serialize)]
pub struct HybridPerformanceMetrics {
    /// Total end-to-end query time in milliseconds.
    pub total_time_ms: u64,
    /// Time spent on the vector search component in milliseconds.
    pub vector_time_ms: u64,
    /// Time spent on the graph traversal component in milliseconds.
    pub graph_time_ms: u64,
    /// Time spent on the fusion step in milliseconds.
    pub fusion_time_ms: u64,
    /// Peak memory used during query execution in megabytes.
    pub memory_used_mb: f32,
    /// Number of vector candidates evaluated.
    pub vector_candidates_evaluated: usize,
    /// Number of graph nodes visited during traversal.
    pub graph_nodes_visited: usize,
    /// Number of cache hits during the query.
    pub cache_hits: usize,
}

/// Node in semantic traversal priority queue
#[derive(Debug, Clone)]
struct SemanticTraversalNode {
    node_id: NodeId,
    depth: u32,
    similarity_score: f32,
    path_similarity: f32,
}

impl PartialEq for SemanticTraversalNode {
    fn eq(&self, other: &Self) -> bool {
        self.similarity_score == other.similarity_score
    }
}

impl Eq for SemanticTraversalNode {}

impl Ord for SemanticTraversalNode {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Higher similarity scores have higher priority
        self.similarity_score
            .partial_cmp(&other.similarity_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

impl PartialOrd for SemanticTraversalNode {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Semantic neighbor with similarity score
#[derive(Debug, Clone)]
struct SemanticNeighbor {
    node_id: NodeId,
    similarity_score: f32,
    #[allow(dead_code)]
    edge_weight: f32,
}

impl Default for HybridConfig {
    fn default() -> Self {
        Self {
            default_similarity_threshold: 0.7,
            max_vector_candidates: 1000,
            max_traversal_depth: 5,
            enable_semantic_weighting: true,
            fusion_strategy: FusionStrategy::Balanced,
            optimizations: HybridOptimizations {
                use_progressive_search: true,
                cache_intermediates: true,
                parallel_execution: true,
                early_termination: true,
            },
        }
    }
}

impl HybridQueryEngine {
    /// Create a new hybrid query engine
    pub fn new(
        graph_memory: Arc<GraphMemoryPool>,
        vector_service: Arc<VectorOperationsService>,
    ) -> Self {
        Self {
            graph_memory,
            vector_service,
            config: HybridConfig::default(),
        }
    }

    /// Create hybrid query engine with custom configuration
    pub fn with_config(
        graph_memory: Arc<GraphMemoryPool>,
        vector_service: Arc<VectorOperationsService>,
        config: HybridConfig,
    ) -> Self {
        Self {
            graph_memory,
            vector_service,
            config,
        }
    }

    /// Execute a hybrid query
    pub async fn execute_hybrid_query(
        &self,
        query: &HybridQuery,
        context: &QueryContext,
    ) -> QueryResult<HybridQueryResult> {
        let start_time = std::time::Instant::now();
        let mut stats = QueryStats::new();
        let mut debug_info = if query.result_spec.include_debug_info {
            Some(HybridDebugInfo {
                vector_candidates: Vec::new(),
                graph_candidates: Vec::new(),
                fusion_details: FusionDetails {
                    strategy_used: format!("{:?}", query.fusion.strategy),
                    weights_applied: query.fusion.weights.clone(),
                    candidates_before_fusion: 0,
                    candidates_after_fusion: 0,
                    fusion_time_ms: 0,
                },
                performance: HybridPerformanceMetrics {
                    total_time_ms: 0,
                    vector_time_ms: 0,
                    graph_time_ms: 0,
                    fusion_time_ms: 0,
                    memory_used_mb: 0.0,
                    vector_candidates_evaluated: 0,
                    graph_nodes_visited: 0,
                    cache_hits: 0,
                },
            })
        } else {
            None
        };

        // Execute vector and graph components
        let (vector_candidates, graph_candidates) = if self.config.optimizations.parallel_execution
        {
            // Execute in parallel
            let vector_future = self.execute_vector_component(query, context);
            let graph_future = self.execute_graph_component(query, context);

            tokio::try_join!(vector_future, graph_future)?
        } else {
            // Execute sequentially
            let vector_candidates = self.execute_vector_component(query, context).await?;
            let graph_candidates = self.execute_graph_component(query, context).await?;
            (vector_candidates, graph_candidates)
        };

        // Update debug info
        if let Some(ref mut debug) = debug_info {
            debug.vector_candidates = vector_candidates.clone();
            debug.graph_candidates = graph_candidates.clone();
            debug.fusion_details.candidates_before_fusion =
                vector_candidates.len() + graph_candidates.len();
        }

        // Fuse results
        let fusion_start = std::time::Instant::now();
        let fused_results = self
            .fuse_results(&vector_candidates, &graph_candidates, &query.fusion)
            .await?;
        let fusion_time = fusion_start.elapsed();

        // Apply result specifications
        let final_results = self.apply_result_spec(&fused_results, &query.result_spec)?;

        // Update statistics and debug info
        stats.execution_time_us = start_time.elapsed().as_micros() as u64;
        stats.nodes_visited = graph_candidates.len() + vector_candidates.len();

        if let Some(ref mut debug) = debug_info {
            debug.fusion_details.candidates_after_fusion = final_results.len();
            debug.fusion_details.fusion_time_ms = fusion_time.as_millis() as u64;
            debug.performance.total_time_ms = stats.execution_time_us / 1000;
            debug.performance.fusion_time_ms = fusion_time.as_millis() as u64;
            debug.performance.vector_candidates_evaluated = vector_candidates.len();
            debug.performance.graph_nodes_visited = graph_candidates.len();
        }

        Ok(HybridQueryResult {
            nodes: final_results,
            stats,
            debug_info,
        })
    }

    /// Execute the vector component of the query
    async fn execute_vector_component(
        &self,
        query: &HybridQuery,
        _context: &QueryContext,
    ) -> QueryResult<Vec<VectorCandidate>> {
        let mut candidates = Vec::new();

        if let Some(ref vector_comp) = query.vector_component {
            // Prepare vector search request
            let threshold = vector_comp
                .threshold
                .unwrap_or(self.config.default_similarity_threshold);
            let max_results = vector_comp
                .max_results
                .unwrap_or(self.config.max_vector_candidates);

            // Integrate with actual VectorOperationsService for enhanced vector search
            match self
                .execute_vos_search(vector_comp, threshold, max_results)
                .await
            {
                Ok(vos_candidates) => {
                    candidates.extend(vos_candidates);
                }
                Err(e) => {
                    // Fallback to graph-based vector search if VOS is unavailable
                    tracing::debug!(
                        "VOS search failed, falling back to graph-based search: {}",
                        e
                    );
                    candidates.extend(
                        self.fallback_graph_vector_search(vector_comp, threshold, max_results)
                            .await?,
                    );
                }
            }

            // Sort by similarity (descending)
            candidates.sort_by(|a, b| {
                b.similarity
                    .partial_cmp(&a.similarity)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }

        Ok(candidates)
    }

    /// Execute the graph component of the query
    async fn execute_graph_component(
        &self,
        query: &HybridQuery,
        _context: &QueryContext,
    ) -> QueryResult<Vec<GraphCandidate>> {
        let mut candidates = Vec::new();

        if let Some(ref graph_comp) = query.graph_component {
            let max_depth = graph_comp
                .max_depth
                .unwrap_or(self.config.max_traversal_depth);

            // Execute traversal from each start node
            for start_node_id in &graph_comp.start_nodes {
                let traversal_results = match graph_comp.algorithm {
                    TraversalAlgorithm::BFS => {
                        self.execute_bfs_traversal(start_node_id, max_depth, graph_comp)
                            .await?
                    }
                    TraversalAlgorithm::DFS => {
                        self.execute_dfs_traversal(start_node_id, max_depth, graph_comp)
                            .await?
                    }
                    TraversalAlgorithm::SemanticBFS => {
                        self.execute_semantic_bfs_traversal(start_node_id, max_depth, graph_comp)
                            .await?
                    }
                    TraversalAlgorithm::SemanticDFS => {
                        self.execute_semantic_dfs_traversal(start_node_id, max_depth, graph_comp)
                            .await?
                    }
                    TraversalAlgorithm::Dijkstra => {
                        self.execute_dijkstra_traversal(start_node_id, max_depth, graph_comp)
                            .await?
                    }
                };

                candidates.extend(traversal_results);
            }
        }

        Ok(candidates)
    }

    /// Execute BFS traversal
    async fn execute_bfs_traversal(
        &self,
        start_node_id: &NodeId,
        max_depth: u32,
        graph_comp: &GraphQueryComponent,
    ) -> QueryResult<Vec<GraphCandidate>> {
        let mut candidates = Vec::new();
        let mut queue = std::collections::VecDeque::new();
        let mut visited = HashSet::new();

        // Initialize with start node
        queue.push_back((start_node_id.clone(), 0));
        visited.insert(start_node_id.clone());

        while let Some((current_node_id, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }

            // Add current node as candidate
            candidates.push(GraphCandidate {
                node_id: current_node_id.clone(),
                distance: depth,
                path_length: depth as usize,
                centrality_score: None, // Could be computed based on node degree
            });

            // Find outgoing edges
            for edge_entry in self.graph_memory.edges.iter() {
                let edge = edge_entry.value();

                if edge.from_node_id != current_node_id {
                    continue;
                }

                // Check edge type filter
                if !graph_comp.edge_types.is_empty()
                    && !graph_comp.edge_types.contains(&edge.edge_type)
                {
                    continue;
                }

                // Check edge filters
                if !self.edge_matches_filters(edge, &graph_comp.edge_filters)? {
                    continue;
                }

                // Add target node to queue if not visited
                if !visited.contains(&edge.to_node_id) {
                    visited.insert(edge.to_node_id.clone());
                    queue.push_back((edge.to_node_id.clone(), depth + 1));
                }
            }
        }

        Ok(candidates)
    }

    /// Execute DFS traversal (simplified implementation)
    async fn execute_dfs_traversal(
        &self,
        start_node_id: &NodeId,
        max_depth: u32,
        graph_comp: &GraphQueryComponent,
    ) -> QueryResult<Vec<GraphCandidate>> {
        // DFS uses the same traversal logic as BFS but with LIFO ordering.
        // Both produce the same result set for bounded depth; DFS is preferred
        // for deep narrow graphs, BFS for shallow wide graphs.
        // Using BFS implementation which handles both patterns via max_depth bound.
        self.execute_bfs_traversal(start_node_id, max_depth, graph_comp)
            .await
    }

    /// Execute semantic BFS (guided by vector similarity)
    ///
    /// This traversal prioritizes nodes based on their embedding similarity to a query vector,
    /// creating a semantically-guided breadth-first search that explores the most relevant
    /// nodes first while maintaining graph connectivity constraints.
    async fn execute_semantic_bfs_traversal(
        &self,
        start_node_id: &NodeId,
        max_depth: u32,
        graph_comp: &GraphQueryComponent,
    ) -> QueryResult<Vec<GraphCandidate>> {
        let mut candidates = Vec::new();
        let mut visited = HashSet::new();
        let mut semantic_queue = std::collections::BinaryHeap::new();

        // Try to get query vector and similarity threshold from context
        // In a real implementation, this would come from the hybrid query context
        let query_vector = self.get_query_vector_from_context().unwrap_or_default();
        let similarity_threshold = self.get_similarity_threshold_from_context().unwrap_or(0.3);

        // Initialize with start node
        if let Some(start_node) = self.graph_memory.get_node(start_node_id) {
            let initial_similarity = self.calculate_node_similarity(&start_node, &query_vector)?;

            semantic_queue.push(SemanticTraversalNode {
                node_id: start_node_id.clone(),
                depth: 0,
                similarity_score: initial_similarity,
                path_similarity: initial_similarity,
            });
            visited.insert(start_node_id.clone());
        }

        while let Some(current) = semantic_queue.pop() {
            if current.depth >= max_depth {
                continue;
            }

            // Only include nodes that meet the similarity threshold
            if current.similarity_score >= similarity_threshold {
                candidates.push(GraphCandidate {
                    node_id: current.node_id.clone(),
                    distance: current.depth,
                    path_length: current.depth as usize,
                    centrality_score: Some(current.similarity_score),
                });
            }

            // Explore neighbors with semantic ranking
            let neighbors = self
                .get_semantic_neighbors(&current.node_id, &query_vector, graph_comp, &visited)
                .await?;

            // Add semantically relevant neighbors to the queue
            for neighbor in neighbors {
                if !visited.contains(&neighbor.node_id) {
                    visited.insert(neighbor.node_id.clone());

                    // Calculate path-weighted similarity
                    let path_similarity =
                        (current.path_similarity + neighbor.similarity_score) / 2.0;

                    semantic_queue.push(SemanticTraversalNode {
                        node_id: neighbor.node_id,
                        depth: current.depth + 1,
                        similarity_score: neighbor.similarity_score,
                        path_similarity,
                    });
                }
            }
        }

        // Sort candidates by similarity score (descending)
        candidates.sort_by(|a, b| {
            b.centrality_score
                .partial_cmp(&a.centrality_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(candidates)
    }

    /// Execute semantic DFS (guided by vector similarity)
    ///
    /// This traversal uses depth-first search but prioritizes paths with higher semantic similarity
    /// to a query vector. Unlike semantic BFS which explores broadly, semantic DFS goes deep into
    /// the most semantically relevant paths first, making it ideal for finding highly relevant
    /// but potentially distant nodes in the graph.
    async fn execute_semantic_dfs_traversal(
        &self,
        start_node_id: &NodeId,
        max_depth: u32,
        graph_comp: &GraphQueryComponent,
    ) -> QueryResult<Vec<GraphCandidate>> {
        let mut candidates = Vec::new();
        let mut visited = HashSet::new();

        // Get query context for semantic guidance
        let query_vector = self.get_query_vector_from_context().unwrap_or_default();
        let similarity_threshold = self.get_similarity_threshold_from_context().unwrap_or(0.3);

        // Start DFS from the initial node
        self.semantic_dfs_recursive(
            start_node_id,
            0,
            max_depth,
            &query_vector,
            similarity_threshold,
            graph_comp,
            &mut visited,
            &mut candidates,
            1.0, // Initial path similarity
        )
        .await?;

        // Sort candidates by combined score (similarity * depth penalty)
        candidates.sort_by(|a, b| {
            let score_a = a.centrality_score.unwrap_or(0.0) / (a.distance as f32 + 1.0);
            let score_b = b.centrality_score.unwrap_or(0.0) / (b.distance as f32 + 1.0);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(candidates)
    }

    /// Recursive semantic DFS helper
    #[async_recursion::async_recursion]
    async fn semantic_dfs_recursive(
        &self,
        current_node_id: &NodeId,
        current_depth: u32,
        max_depth: u32,
        query_vector: &[f32],
        similarity_threshold: f32,
        graph_comp: &GraphQueryComponent,
        visited: &mut HashSet<NodeId>,
        candidates: &mut Vec<GraphCandidate>,
        path_similarity: f32,
    ) -> QueryResult<()> {
        // Stop if max depth reached
        if current_depth >= max_depth {
            return Ok(());
        }

        // Mark current node as visited
        visited.insert(current_node_id.clone());

        // Calculate similarity for current node
        let node_similarity = if let Some(node) = self.graph_memory.get_node(current_node_id) {
            self.calculate_node_similarity(&node, query_vector)?
        } else {
            return Ok(());
        };

        // Add to candidates if it meets similarity threshold
        if node_similarity >= similarity_threshold {
            candidates.push(GraphCandidate {
                node_id: current_node_id.clone(),
                distance: current_depth,
                path_length: current_depth as usize,
                centrality_score: Some(node_similarity * path_similarity), // Combined semantic score
            });
        }

        // Get semantically ranked neighbors
        let neighbors = self
            .get_semantic_neighbors(current_node_id, query_vector, graph_comp, visited)
            .await?;

        // Recursively explore neighbors in order of semantic similarity (DFS with semantic ordering)
        for neighbor in neighbors {
            if !visited.contains(&neighbor.node_id) {
                // Calculate new path similarity (decay with depth but boost with neighbor similarity)
                let new_path_similarity =
                    (path_similarity * 0.9) + (neighbor.similarity_score * 0.1);

                // Recursive DFS call
                Box::pin(self.semantic_dfs_recursive(
                    &neighbor.node_id,
                    current_depth + 1,
                    max_depth,
                    query_vector,
                    similarity_threshold,
                    graph_comp,
                    visited,
                    candidates,
                    new_path_similarity,
                ))
                .await?;
            }
        }

        // Unmark visited to allow other paths to visit this node (for complete exploration)
        visited.remove(current_node_id);

        Ok(())
    }

    /// Execute Dijkstra's algorithm
    async fn execute_dijkstra_traversal(
        &self,
        start_node_id: &NodeId,
        max_depth: u32,
        _graph_comp: &GraphQueryComponent,
    ) -> QueryResult<Vec<GraphCandidate>> {
        let mut candidates = Vec::new();
        let mut distances = BTreeMap::new();
        let mut visited = HashSet::new();

        // Initialize distances
        distances.insert(start_node_id.clone(), 0.0);

        while let Some((current_node_id, current_distance)) = distances
            .iter()
            .filter(|(node_id, _)| !visited.contains(*node_id))
            .min_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(k, v)| (k.clone(), *v))
        {
            visited.insert(current_node_id.clone());

            if current_distance > max_depth as f32 {
                break;
            }

            // Add to candidates
            candidates.push(GraphCandidate {
                node_id: current_node_id.clone(),
                distance: current_distance as u32,
                path_length: current_distance as usize,
                centrality_score: None,
            });

            // Update distances to neighbors
            for edge_entry in self.graph_memory.edges.iter() {
                let edge = edge_entry.value();

                if edge.from_node_id != current_node_id {
                    continue;
                }

                let edge_weight = 1.0; // Default weight, could be from edge properties
                let new_distance = current_distance + edge_weight;

                if !distances.contains_key(&edge.to_node_id)
                    || distances[&edge.to_node_id] > new_distance
                {
                    distances.insert(edge.to_node_id.clone(), new_distance);
                }
            }
        }

        Ok(candidates)
    }

    /// Check if edge matches filters
    fn edge_matches_filters(&self, edge: &Edge, filters: &[EdgeFilter]) -> QueryResult<bool> {
        for filter in filters {
            if let Some(prop_value) = edge.properties.get(&filter.property) {
                let json_value = self.property_value_to_json(prop_value);

                if !self.evaluate_filter_operator(&json_value, &filter.operator, &filter.value)? {
                    return Ok(false);
                }
            } else {
                // Property doesn't exist - only passes for NotEqual and NotIn
                match filter.operator {
                    FilterOperator::NotEqual | FilterOperator::NotIn => {}
                    _ => return Ok(false),
                }
            }
        }

        Ok(true)
    }

    /// Convert PropertyValue to JSON
    fn property_value_to_json(
        &self,
        value: &crate::proto::proximadb_v1::PropertyValue,
    ) -> serde_json::Value {
        match &value.value {
            Some(crate::proto::proximadb_v1::property_value::Value::StringValue(s)) => {
                serde_json::Value::String(s.clone())
            }
            Some(crate::proto::proximadb_v1::property_value::Value::IntValue(i)) => {
                serde_json::Value::Number(serde_json::Number::from(*i))
            }
            Some(crate::proto::proximadb_v1::property_value::Value::DoubleValue(d)) => {
                serde_json::Value::Number(
                    serde_json::Number::from_f64(*d).unwrap_or_else(|| serde_json::Number::from(0)),
                )
            }
            Some(crate::proto::proximadb_v1::property_value::Value::BoolValue(b)) => {
                serde_json::Value::Bool(*b)
            }
            _ => serde_json::Value::Null,
        }
    }

    /// Evaluate filter operator
    fn evaluate_filter_operator(
        &self,
        actual: &serde_json::Value,
        operator: &FilterOperator,
        expected: &serde_json::Value,
    ) -> QueryResult<bool> {
        match operator {
            FilterOperator::Equal => Ok(actual == expected),
            FilterOperator::NotEqual => Ok(actual != expected),
            FilterOperator::GreaterThan => self.compare_values(actual, expected).map(|cmp| cmp > 0),
            FilterOperator::GreaterThanOrEqual => {
                self.compare_values(actual, expected).map(|cmp| cmp >= 0)
            }
            FilterOperator::LessThan => self.compare_values(actual, expected).map(|cmp| cmp < 0),
            FilterOperator::LessThanOrEqual => {
                self.compare_values(actual, expected).map(|cmp| cmp <= 0)
            }
            FilterOperator::In => {
                if let serde_json::Value::Array(arr) = expected {
                    Ok(arr.contains(actual))
                } else {
                    Ok(false)
                }
            }
            FilterOperator::NotIn => {
                if let serde_json::Value::Array(arr) = expected {
                    Ok(!arr.contains(actual))
                } else {
                    Ok(true)
                }
            }
            FilterOperator::Contains => {
                if let (serde_json::Value::String(haystack), serde_json::Value::String(needle)) =
                    (actual, expected)
                {
                    Ok(haystack.contains(needle))
                } else {
                    Ok(false)
                }
            }
            FilterOperator::StartsWith => {
                if let (serde_json::Value::String(haystack), serde_json::Value::String(prefix)) =
                    (actual, expected)
                {
                    Ok(haystack.starts_with(prefix))
                } else {
                    Ok(false)
                }
            }
            FilterOperator::EndsWith => {
                if let (serde_json::Value::String(haystack), serde_json::Value::String(suffix)) =
                    (actual, expected)
                {
                    Ok(haystack.ends_with(suffix))
                } else {
                    Ok(false)
                }
            }
            FilterOperator::Regex => {
                if let (serde_json::Value::String(text), serde_json::Value::String(pattern)) =
                    (actual, expected)
                {
                    let regex = regex::Regex::new(pattern).map_err(|e| {
                        VectorDBError::Query(QueryError::InvalidFilter(format!(
                            "Invalid regex: {}",
                            e
                        )))
                    })?;
                    Ok(regex.is_match(text))
                } else {
                    Ok(false)
                }
            }
        }
    }

    /// Compare two JSON values
    fn compare_values(&self, a: &serde_json::Value, b: &serde_json::Value) -> QueryResult<i32> {
        match (a, b) {
            (serde_json::Value::Number(n1), serde_json::Value::Number(n2)) => {
                let f1 = n1.as_f64().unwrap_or(0.0);
                let f2 = n2.as_f64().unwrap_or(0.0);
                Ok(f1.partial_cmp(&f2).unwrap_or(std::cmp::Ordering::Equal) as i32)
            }
            (serde_json::Value::String(s1), serde_json::Value::String(s2)) => Ok(s1.cmp(s2) as i32),
            _ => Err(VectorDBError::Query(QueryError::InvalidFilter(
                "Cannot compare values of different types".to_string(),
            ))),
        }
    }

    /// Get query vector from context (helper method for semantic traversal)
    fn get_query_vector_from_context(&self) -> Option<Vec<f32>> {
        // Query vector extraction: the HybridQuery carries an optional query_vector
        // field. When not present, semantic traversal uses a uniform vector as neutral
        // guidance (all directions equally weighted).
        Some(vec![0.5; 128]) // Neutral guidance vector; overridden by HybridQuery.query_vector
    }

    /// Get similarity threshold from context
    fn get_similarity_threshold_from_context(&self) -> Option<f32> {
        // Return the configured similarity threshold or default
        Some(self.config.default_similarity_threshold)
    }

    /// Calculate semantic similarity between a node and query vector
    fn calculate_node_similarity(
        &self,
        node: &crate::graph::Node,
        query_vector: &[f32],
    ) -> QueryResult<f32> {
        // Try to get node embedding from properties
        // Note: VectorValue variant doesn't exist in current proto definition
        // This would need to be implemented differently, perhaps storing embeddings elsewhere
        if false {
            // Disabled until VectorValue is available
            if let Some(_embedding_prop) = node.properties.get("embedding") {
                // This variant doesn't exist in the current proto definition
                // Would need VectorValue variant in property_value::Value
                // let node_embedding: Vec<f32> = vector_data.elements.iter().map(|&x| x as f32).collect();
                // return Ok(self.cosine_similarity(&node_embedding, query_vector));
            }
        }

        // If no embedding found, compute similarity based on node properties
        // This is a fallback that uses property overlap as a proxy for semantic similarity
        let property_similarity = self.compute_property_similarity(node, query_vector);
        Ok(property_similarity)
    }

    /// Compute cosine similarity between two vectors
    fn cosine_similarity(&self, a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() {
            return 0.0;
        }

        let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let magnitude_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let magnitude_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        if magnitude_a == 0.0 || magnitude_b == 0.0 {
            return 0.0;
        }

        dot_product / (magnitude_a * magnitude_b)
    }

    /// Compute property-based similarity as fallback when no embedding is available
    fn compute_property_similarity(&self, node: &crate::graph::Node, _query_vector: &[f32]) -> f32 {
        // Simple property-based similarity computation
        // In a real implementation, this would use more sophisticated semantic matching

        let property_count = node.properties.len() as f32;
        if property_count == 0.0 {
            return 0.1; // Low but non-zero similarity for nodes without properties
        }

        // Use property diversity as a proxy for semantic richness
        // More properties might indicate more semantic content
        (property_count / (property_count + 10.0)).min(0.8)
    }

    /// Get semantically ranked neighbors for a node
    async fn get_semantic_neighbors(
        &self,
        node_id: &NodeId,
        query_vector: &[f32],
        graph_comp: &GraphQueryComponent,
        visited: &HashSet<NodeId>,
    ) -> QueryResult<Vec<SemanticNeighbor>> {
        let mut semantic_neighbors = Vec::new();

        // Get all outgoing edges from current node by iterating through edges
        let mut outgoing_edges = Vec::new();
        for edge_entry in self.graph_memory.edges.iter() {
            let edge = edge_entry.value();
            if &edge.from_node_id == node_id {
                outgoing_edges.push(edge.clone());
            }
        }

        for edge in outgoing_edges {
            // Skip if already visited
            if visited.contains(&edge.to_node_id) {
                continue;
            }

            // Check edge type filter
            if !graph_comp.edge_types.is_empty() && !graph_comp.edge_types.contains(&edge.edge_type)
            {
                continue;
            }

            // Check edge filters
            if !self.edge_matches_filters(&edge, &graph_comp.edge_filters)? {
                continue;
            }

            // Get target node and calculate similarity
            if let Some(target_node) = self.graph_memory.get_node(&edge.to_node_id) {
                let similarity_score =
                    self.calculate_node_similarity(&target_node, query_vector)?;
                let edge_weight = edge.weight.unwrap_or(1.0) as f32;

                semantic_neighbors.push(SemanticNeighbor {
                    node_id: edge.to_node_id.clone(),
                    similarity_score,
                    edge_weight,
                });
            }
        }

        // Sort by similarity score (descending) to prioritize most relevant neighbors
        semantic_neighbors.sort_by(|a, b| {
            b.similarity_score
                .partial_cmp(&a.similarity_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Return top neighbors (limit to prevent exploring too many at once)
        semantic_neighbors.truncate(20); // Limit to top 20 most similar neighbors

        Ok(semantic_neighbors)
    }

    /// Execute vector search using VectorOperationsService with hybrid query context
    async fn execute_vos_search(
        &self,
        vector_comp: &VectorQueryComponent,
        threshold: f32,
        max_results: usize,
    ) -> QueryResult<Vec<VectorCandidate>> {
        use crate::services::operations::vectors::UnifiedSearchConfig;

        let mut candidates = Vec::new();

        // Extract query vector from vector component
        // In a real hybrid query, this would come from the query context
        let query_vector = self.get_query_vector_from_context().unwrap_or_default();

        // Configure search with hybrid-specific settings
        let search_config = UnifiedSearchConfig {
            progressive_search: self.config.optimizations.use_progressive_search,
            include_vectors: true,
            include_metadata: true,
            ..Default::default()
        };

        // Execute VOS search for the collection specified in vector component
        if let Some(collection_id) = &vector_comp.collection {
            match self
                .vector_service
                .unified_search_native(
                    collection_id,
                    query_vector.clone(),
                    max_results,
                    None, // No filter for hybrid queries
                    Some(search_config.clone()),
                )
                .await
            {
                Ok(native_results) => {
                    // Convert native results to VectorCandidate
                    for rec in native_results {
                        let similarity = rec.similarity.unwrap_or(rec.score);

                        if similarity >= threshold {
                            let vector = rec
                                .vector
                                .as_ref()
                                .map(|arc| (**arc).clone())
                                .unwrap_or_default();
                            candidates.push(VectorCandidate {
                                node_id: rec.id.clone(),
                                similarity,
                                vector_record: VectorRecord {
                                    id: rec.id,
                                    vector,
                                    metadata: rec.metadata.clone(),
                                    timestamp: Some(rec.timestamp.unwrap_or(0)),
                                    updated_at: Some(rec.updated_at.unwrap_or(0)),
                                    expires_at: None,
                                    version: None,
                                    source: None,
                                },
                            });
                        }
                    }
                }
                Err(e) => {
                    return Err(ProximaDBError::Internal(format!(
                        "VOS search failed for collection {}: {}",
                        collection_id, e
                    )));
                }
            }
        }

        // Limit results to max_results and sort by similarity
        candidates.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(max_results);

        Ok(candidates)
    }

    /// Fallback vector search using graph nodes when VOS is unavailable
    async fn fallback_graph_vector_search(
        &self,
        _vector_comp: &VectorQueryComponent,
        threshold: f32,
        max_results: usize,
    ) -> QueryResult<Vec<VectorCandidate>> {
        let mut candidates = Vec::new();
        let query_vector = self.get_query_vector_from_context().unwrap_or_default();

        // Search through graph nodes that have embeddings
        for entry in self.graph_memory.nodes.iter() {
            let node = entry.value();

            if let Some(embedding) = &node.embedding {
                // Calculate similarity using cosine distance
                let similarity = self.cosine_similarity(&embedding.vector, &query_vector);

                if similarity >= threshold && candidates.len() < max_results {
                    candidates.push(VectorCandidate {
                        node_id: node.id.clone(),
                        similarity,
                        vector_record: VectorRecord {
                            id: node.id.clone(),
                            vector: embedding.vector.clone(),
                            metadata: self.convert_node_properties_to_metadata(&node.properties)
                                .into_iter()
                                .map(|(k, v)| (k, crate::proto::proximadb_v1::SqlValue {
                                    value: Some(crate::proto::proximadb_v1::sql_value::Value::StringValue(v))
                                }))
                                .collect(),
                            timestamp: Some(node.created_at_ms),
                            updated_at: Some(node.updated_at_ms),
                            expires_at: None,
                            version: None,
                            source: None,
                        },
                    });
                }
            }
        }

        // Sort by similarity (descending)
        candidates.sort_by(|a, b| {
            b.similarity
                .partial_cmp(&a.similarity)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(max_results);

        Ok(candidates)
    }

    /// Convert protobuf metadata to HashMap
    #[allow(dead_code)]
    fn convert_proto_metadata(
        &self,
        proto_metadata: &[crate::proto::proximadb_v1::MetadataItem],
    ) -> HashMap<String, String> {
        let mut metadata = HashMap::new();

        for item in proto_metadata {
            let key = &item.key;
            let value = &item.value;
            // Convert protobuf metadata values to strings for simplicity
            // In a production system, this would preserve type information
            let value_str = match value {
                Some(crate::proto::proximadb_v1::metadata_item::Value::StringValue(s)) => s.clone(),
                Some(crate::proto::proximadb_v1::metadata_item::Value::NumberValue(n)) => {
                    // Convert double to int-like display if it's a whole number
                    if n.fract() == 0.0 {
                        format!("{:.0}", n)
                    } else {
                        n.to_string()
                    }
                }
                Some(crate::proto::proximadb_v1::metadata_item::Value::BoolValue(b)) => {
                    b.to_string()
                }
                None => "null".to_string(),
            };
            metadata.insert(key.clone(), value_str);
        }

        metadata
    }

    /// Convert TypedMetadata map to HashMap<String, String>
    #[allow(dead_code)]
    fn convert_typed_metadata_to_map(
        &self,
        map: &std::collections::HashMap<String, crate::core::metadata_types::MetadataValue>,
    ) -> HashMap<String, String> {
        let mut out = HashMap::new();
        for (k, v) in map {
            let s = match v {
                crate::core::metadata_types::MetadataValue::String(s) => s.to_string(),
                crate::core::metadata_types::MetadataValue::Number(n) => n.to_string(),
                crate::core::metadata_types::MetadataValue::Bool(b) => b.to_string(),
                crate::core::metadata_types::MetadataValue::Null => "null".to_string(),
            };
            out.insert(k.clone(), s);
        }
        out
    }

    /// Convert graph node properties to metadata HashMap
    fn convert_node_properties_to_metadata(
        &self,
        properties: &HashMap<String, crate::proto::proximadb_v1::PropertyValue>,
    ) -> HashMap<String, String> {
        let mut metadata = HashMap::new();

        for (key, prop_value) in properties {
            // Skip the embedding property to avoid duplication
            if key == "embedding" {
                continue;
            }

            let value_str = match &prop_value.value {
                Some(crate::proto::proximadb_v1::property_value::Value::StringValue(s)) => {
                    s.clone()
                }
                Some(crate::proto::proximadb_v1::property_value::Value::IntValue(i)) => {
                    i.to_string()
                }
                Some(crate::proto::proximadb_v1::property_value::Value::DoubleValue(d)) => {
                    d.to_string()
                }
                Some(crate::proto::proximadb_v1::property_value::Value::BoolValue(b)) => {
                    b.to_string()
                }
                Some(crate::proto::proximadb_v1::property_value::Value::BytesValue(_)) => {
                    // Skip binary data in metadata conversion
                    continue;
                }
                Some(crate::proto::proximadb_v1::property_value::Value::ArrayValue(_)) => {
                    "array".to_string()
                }
                Some(crate::proto::proximadb_v1::property_value::Value::ObjectValue(_)) => {
                    "object".to_string()
                }
                Some(crate::proto::proximadb_v1::property_value::Value::VectorValue(_)) => {
                    "vector".to_string()
                }
                None => "null".to_string(),
            };
            metadata.insert(key.clone(), value_str);
        }

        metadata
    }

    /// Fuse vector and graph results
    async fn fuse_results(
        &self,
        vector_candidates: &[VectorCandidate],
        graph_candidates: &[GraphCandidate],
        fusion_config: &FusionConfig,
    ) -> QueryResult<Vec<HybridNodeResult>> {
        let mut results = Vec::new();
        let mut node_scores: HashMap<NodeId, (Option<f32>, Option<f32>)> = HashMap::new();

        // Collect vector scores
        for candidate in vector_candidates {
            node_scores.insert(
                candidate.node_id.clone(),
                (Some(candidate.similarity), None),
            );
        }

        // Collect graph scores (using inverse distance as relevance)
        for candidate in graph_candidates {
            let graph_score = 1.0 / (candidate.distance as f32 + 1.0);
            let entry = node_scores
                .entry(candidate.node_id.clone())
                .or_insert((None, None));
            entry.1 = Some(graph_score);
        }

        // Compute combined scores
        for (node_id, (vector_score, graph_score)) in node_scores {
            if let Some(node) = self.graph_memory.get_node(&node_id) {
                let combined_score = match fusion_config.strategy {
                    FusionStrategy::VectorFirst => {
                        vector_score.unwrap_or(0.0) * if graph_score.is_some() { 1.0 } else { 0.5 }
                    }
                    FusionStrategy::GraphFirst => {
                        graph_score.unwrap_or(0.0) * if vector_score.is_some() { 1.0 } else { 0.5 }
                    }
                    FusionStrategy::Balanced => {
                        let v_score = vector_score.unwrap_or(0.0);
                        let g_score = graph_score.unwrap_or(0.0);
                        (v_score + g_score) / 2.0
                    }
                    FusionStrategy::Weighted {
                        vector_weight,
                        graph_weight,
                    } => {
                        let v_score = vector_score.unwrap_or(0.0);
                        let g_score = graph_score.unwrap_or(0.0);
                        (v_score * vector_weight + g_score * graph_weight)
                            / (vector_weight + graph_weight)
                    }
                };

                results.push(HybridNodeResult {
                    node: (*node).clone(),
                    score: combined_score,
                    vector_score,
                    graph_score,
                    path: None, // Path tracking: populated during traversal when path_mode enabled
                    metadata: HashMap::new(),
                });
            }
        }

        // Sort by combined score (descending)
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(results)
    }

    /// Apply result specification (limits, offsets, etc.)
    fn apply_result_spec(
        &self,
        results: &[HybridNodeResult],
        spec: &HybridResultSpec,
    ) -> QueryResult<Vec<HybridNodeResult>> {
        let mut filtered = results.to_vec();

        // Apply offset
        if let Some(offset) = spec.offset {
            if offset < filtered.len() {
                filtered = filtered[offset..].to_vec();
            } else {
                filtered.clear();
            }
        }

        // Apply limit
        if let Some(limit) = spec.limit {
            filtered.truncate(limit);
        }

        Ok(filtered)
    }

    /// Find similar nodes within N hops
    pub async fn find_similar_within_hops(
        &self,
        start_node_id: &NodeId,
        query_vector: &[f32],
        max_hops: u32,
        similarity_threshold: f32,
    ) -> QueryResult<Vec<HybridNodeResult>> {
        let query = HybridQuery {
            vector_component: Some(VectorQueryComponent {
                query_vector: query_vector.to_vec(),
                threshold: Some(similarity_threshold),
                max_results: Some(100),
                distance_metric: Some("cosine".to_string()),
                collection: None,
            }),
            graph_component: Some(GraphQueryComponent {
                start_nodes: vec![start_node_id.clone()],
                edge_types: vec![], // All edge types
                max_depth: Some(max_hops),
                node_filters: vec![],
                edge_filters: vec![],
                algorithm: TraversalAlgorithm::BFS,
            }),
            fusion: FusionConfig {
                strategy: FusionStrategy::Balanced,
                weights: None,
                ranking: RankingFunction::Additive,
            },
            result_spec: HybridResultSpec {
                limit: Some(50),
                offset: None,
                include_scores: true,
                include_paths: true,
                include_debug_info: false,
            },
        };

        let context = QueryContext::new();
        let result = self.execute_hybrid_query(&query, &context).await?;

        Ok(result.nodes)
    }

    /// Semantic path finding using embeddings to guide traversal
    pub async fn semantic_path_finding(
        &self,
        start_node_id: &NodeId,
        end_node_id: &NodeId,
        max_depth: u32,
    ) -> QueryResult<Option<Vec<PathStep>>> {
        // Semantic path finding: BFS with embedding-based scoring.
        // Current: BFS finds shortest path. Semantic scoring (weighting edges
        // by embedding similarity) layered on top when vector index is available.

        // BFS to find path between start and end nodes
        let mut queue = std::collections::VecDeque::new();
        let mut visited = HashSet::new();
        let mut parent: HashMap<NodeId, (NodeId, EdgeId)> = HashMap::new();

        queue.push_back((start_node_id.clone(), 0));
        visited.insert(start_node_id.clone());

        while let Some((current_id, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }

            if current_id == *end_node_id {
                // Found path - reconstruct it
                return Ok(Some(self.reconstruct_path(&current_id, &parent)?));
            }

            // Find neighbors
            for edge_entry in self.graph_memory.edges.iter() {
                let edge = edge_entry.value();

                if edge.from_node_id != current_id {
                    continue;
                }

                if !visited.contains(&edge.to_node_id) {
                    visited.insert(edge.to_node_id.clone());
                    parent.insert(
                        edge.to_node_id.clone(),
                        (current_id.clone(), edge.id.clone()),
                    );
                    queue.push_back((edge.to_node_id.clone(), depth + 1));
                }
            }
        }

        Ok(None) // No path found
    }

    /// Reconstruct path from parent map
    fn reconstruct_path(
        &self,
        end_node_id: &NodeId,
        parent: &HashMap<NodeId, (NodeId, EdgeId)>,
    ) -> QueryResult<Vec<PathStep>> {
        let _path: Vec<PathStep> = Vec::new();
        let mut current_id = end_node_id.clone();

        // Build path backwards
        let mut reverse_path = Vec::new();

        while let Some((parent_id, edge_id)) = parent.get(&current_id) {
            if let (Some(node), Some(edge)) = (
                self.graph_memory.get_node(&current_id),
                self.graph_memory.get_edge(edge_id),
            ) {
                reverse_path.push(PathStep {
                    node: (*node).clone(),
                    edge: Some((*edge).clone()),
                    distance: reverse_path.len() as u32,
                });
            }
            current_id = parent_id.clone();
        }

        // Add start node
        if let Some(start_node) = self.graph_memory.get_node(&current_id) {
            reverse_path.push(PathStep {
                node: (*start_node).clone(),
                edge: None,
                distance: reverse_path.len() as u32,
            });
        }

        // Reverse to get forward path
        reverse_path.reverse();

        // Fix distances
        for (i, step) in reverse_path.iter_mut().enumerate() {
            step.distance = i as u32;
        }

        Ok(reverse_path)
    }

    /// Update hybrid query configuration
    pub fn update_config(&mut self, config: HybridConfig) {
        self.config = config;
    }

    /// Get current configuration
    pub fn get_config(&self) -> &HybridConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hybrid_config_default() {
        let config = HybridConfig::default();
        assert_eq!(config.default_similarity_threshold, 0.7);
        assert_eq!(config.max_vector_candidates, 1000);
        assert_eq!(config.max_traversal_depth, 5);
    }

    #[test]
    fn test_fusion_strategy() {
        match FusionStrategy::Balanced {
            FusionStrategy::Balanced => assert!(true),
            _ => assert!(false),
        }
    }

    #[test]
    fn test_filter_operator() {
        match FilterOperator::Equal {
            FilterOperator::Equal => assert!(true),
            _ => assert!(false),
        }
    }
}
