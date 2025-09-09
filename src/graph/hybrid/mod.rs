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

//! # Vector-Graph Integration for Hybrid Queries
//!
//! This module implements the integration layer between ProximaDB's vector and graph engines,
//! enabling powerful hybrid queries that combine semantic similarity with graph relationships.
//!
//! ## Key Features
//!
//! - **Hybrid Query Processing**: Combine vector similarity and graph traversal in single queries
//! - **Semantic Graph Traversal**: Follow edges based on embedding similarity rather than just relationships
//! - **Vector-Guided Path Finding**: Use embeddings to guide path selection in graph traversals
//! - **Similarity-Filtered Neighborhoods**: Find similar nodes within N hops of a starting node
//! - **Cross-Modal Optimization**: Optimize queries across both vector and graph dimensions
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │           Hybrid Query Engine           │
//! ├─────────────────────────────────────────┤
//! │                                         │
//! │  ┌──────────────┐  ┌─────────────────┐  │
//! │  │ Vector Query │  │ Graph Traversal │  │
//! │  │   Engine     │  │     Engine      │  │
//! │  └──────┬───────┘  └─────────┬───────┘  │
//! │         │                    │          │
//! │         │  ┌─────────────────┴─────┐    │
//! │         └─▶│ Fusion & Ranking      │    │
//! │            │      Engine           │    │
//! │            └───────────────────────┘    │
//! ├─────────────────────────────────────────┤
//! │           Arc Memory Pool               │
//! │  ┌────────────┬──────────┬──────────┐   │
//! │  │   Nodes    │  Edges   │ Vectors  │   │
//! │  │ Properties │ Metadata │Embeddings│   │
//! │  └────────────┴──────────┴──────────┘   │
//! └─────────────────────────────────────────┘
//! ```

use crate::core::error::ProximaDBError;
use crate::graph::{
    Node, Edge, NodeId, EdgeId, GraphMemoryPool,
    query::{QueryResult, QueryContext, QueryStats},
};
use crate::services::vector_operations_service::VectorOperationsService;
use crate::core::service_types::VectorRecord;
use std::collections::{HashMap, HashSet, BTreeMap};
use std::sync::Arc;
use serde::{Serialize, Deserialize};

/// Hybrid query engine for vector-graph integration
pub struct HybridQueryEngine {
    /// Reference to graph memory pool
    graph_memory: Arc<GraphMemoryPool>,
    /// Reference to vector operations service
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
#[derive(Debug, Clone, Copy)]
pub enum FusionStrategy {
    /// Vector results filtered by graph constraints
    VectorFirst,
    /// Graph results ranked by vector similarity  
    GraphFirst,
    /// Combined ranking using both signals
    Balanced,
    /// Custom weighted combination
    Weighted { vector_weight: f32, graph_weight: f32 },
}

/// Optimization flags for hybrid queries
#[derive(Debug, Clone)]
pub struct HybridOptimizations {
    /// Use progressive search for vector component
    pub use_progressive_search: bool,
    /// Cache intermediate results
    pub cache_intermediates: bool,
    /// Parallelize vector and graph operations
    pub parallel_execution: bool,
    /// Use early termination when possible
    pub early_termination: bool,
}

/// Hybrid query specification
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeFilter {
    /// Property name to filter on
    pub property: String,
    /// Filter operator
    pub operator: FilterOperator,
    /// Expected value
    pub value: serde_json::Value,
}

/// Edge filter for graph component
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeFilter {
    /// Property name to filter on
    pub property: String,
    /// Filter operator
    pub operator: FilterOperator,
    /// Expected value
    pub value: serde_json::Value,
}

/// Filter operators
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FilterOperator {
    Equal,
    NotEqual,
    GreaterThan,
    GreaterThanOrEqual,
    LessThan,
    LessThanOrEqual,
    In,
    NotIn,
    Contains,
    StartsWith,
    EndsWith,
    Regex,
}

/// Traversal algorithms for graph component
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TraversalAlgorithm {
    BFS,
    DFS,
    Dijkstra,
    SemanticBFS, // BFS guided by vector similarity
    SemanticDFS, // DFS guided by vector similarity
}

/// Fusion configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FusionConfig {
    /// Fusion strategy
    pub strategy: FusionStrategy,
    /// Custom weights for balanced fusion
    pub weights: Option<FusionWeights>,
    /// Ranking function
    pub ranking: RankingFunction,
}

/// Weights for fusion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FusionWeights {
    /// Weight for vector similarity score
    pub vector_weight: f32,
    /// Weight for graph relevance score
    pub graph_weight: f32,
    /// Weight for structural properties
    pub structure_weight: f32,
}

/// Ranking function for results
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Serialize)]
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
#[derive(Debug, Serialize)]
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

/// Vector candidate in debug info
#[derive(Debug, Serialize)]
pub struct VectorCandidate {
    pub node_id: NodeId,
    pub similarity: f32,
    pub vector_record: VectorRecord,
}

/// Graph candidate in debug info
#[derive(Debug, Serialize)]
pub struct GraphCandidate {
    pub node_id: NodeId,
    pub distance: u32,
    pub path_length: usize,
    pub centrality_score: Option<f32>,
}

/// Fusion process details
#[derive(Debug, Serialize)]
pub struct FusionDetails {
    pub strategy_used: String,
    pub weights_applied: Option<FusionWeights>,
    pub candidates_before_fusion: usize,
    pub candidates_after_fusion: usize,
    pub fusion_time_ms: u64,
}

/// Performance metrics for hybrid queries
#[derive(Debug, Serialize)]
pub struct HybridPerformanceMetrics {
    pub total_time_ms: u64,
    pub vector_time_ms: u64,
    pub graph_time_ms: u64,
    pub fusion_time_ms: u64,
    pub memory_used_mb: f32,
    pub vector_candidates_evaluated: usize,
    pub graph_nodes_visited: usize,
    pub cache_hits: usize,
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
        let (vector_candidates, graph_candidates) = if self.config.optimizations.parallel_execution {
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
        let fused_results = self.fuse_results(
            &vector_candidates,
            &graph_candidates,
            &query.fusion,
        ).await?;
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
            let threshold = vector_comp.threshold
                .unwrap_or(self.config.default_similarity_threshold);
            let max_results = vector_comp.max_results
                .unwrap_or(self.config.max_vector_candidates);
            
            // TODO: This is a placeholder - integrate with actual VectorOperationsService
            // For now, we'll simulate vector search results based on nodes with embeddings
            for entry in self.graph_memory.nodes.iter() {
                let node = entry.value();
                
                if let Some(_embedding) = &node.embedding {
                    // In a real implementation, we would:
                    // 1. Extract the embedding vector
                    // 2. Compute similarity with query vector
                    // 3. Filter by threshold
                    // 4. Create VectorRecord
                    
                    // Placeholder: assign random similarity for demonstration
                    let similarity = 0.8; // This would be computed properly
                    
                    if similarity >= threshold && candidates.len() < max_results {
                        candidates.push(VectorCandidate {
                            node_id: node.id.clone(),
                            similarity,
                            vector_record: VectorRecord {
                                id: node.id.clone(),
                                vector: node.embedding.as_ref().unwrap().clone(),
                                metadata: std::collections::HashMap::new(),
                                created_at: node.created_at.map(|t| t.seconds).unwrap_or(0),
                                updated_at: node.updated_at.map(|t| t.seconds).unwrap_or(0),
                            },
                        });
                    }
                }
            }
            
            // Sort by similarity (descending)
            candidates.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap_or(std::cmp::Ordering::Equal));
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
            let max_depth = graph_comp.max_depth.unwrap_or(self.config.max_traversal_depth);
            
            // Execute traversal from each start node
            for start_node_id in &graph_comp.start_nodes {
                let traversal_results = match graph_comp.algorithm {
                    TraversalAlgorithm::BFS => {
                        self.execute_bfs_traversal(start_node_id, max_depth, graph_comp).await?
                    }
                    TraversalAlgorithm::DFS => {
                        self.execute_dfs_traversal(start_node_id, max_depth, graph_comp).await?
                    }
                    TraversalAlgorithm::SemanticBFS => {
                        self.execute_semantic_bfs_traversal(start_node_id, max_depth, graph_comp).await?
                    }
                    TraversalAlgorithm::SemanticDFS => {
                        self.execute_semantic_dfs_traversal(start_node_id, max_depth, graph_comp).await?
                    }
                    TraversalAlgorithm::Dijkstra => {
                        self.execute_dijkstra_traversal(start_node_id, max_depth, graph_comp).await?
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
                if !graph_comp.edge_types.is_empty() && 
                   !graph_comp.edge_types.contains(&edge.edge_type) {
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
        // For now, use BFS implementation
        // TODO: Implement proper DFS with recursion/stack
        self.execute_bfs_traversal(start_node_id, max_depth, graph_comp).await
    }
    
    /// Execute semantic BFS (guided by vector similarity)
    async fn execute_semantic_bfs_traversal(
        &self,
        start_node_id: &NodeId,
        max_depth: u32,
        graph_comp: &GraphQueryComponent,
    ) -> QueryResult<Vec<GraphCandidate>> {
        // For now, fall back to regular BFS
        // TODO: Implement semantic guidance using node embeddings
        self.execute_bfs_traversal(start_node_id, max_depth, graph_comp).await
    }
    
    /// Execute semantic DFS (guided by vector similarity)
    async fn execute_semantic_dfs_traversal(
        &self,
        start_node_id: &NodeId,
        max_depth: u32,
        graph_comp: &GraphQueryComponent,
    ) -> QueryResult<Vec<GraphCandidate>> {
        // For now, fall back to regular DFS
        // TODO: Implement semantic guidance using node embeddings
        self.execute_dfs_traversal(start_node_id, max_depth, graph_comp).await
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
                
                if !distances.contains_key(&edge.to_node_id) || 
                   distances[&edge.to_node_id] > new_distance {
                    distances.insert(edge.to_node_id.clone(), new_distance);
                }
            }
        }
        
        Ok(candidates)
    }
    
    /// Check if edge matches filters
    fn edge_matches_filters(
        &self,
        edge: &Edge,
        filters: &[EdgeFilter],
    ) -> QueryResult<bool> {
        for filter in filters {
            if let Some(prop_value) = edge.properties.get(&filter.property) {
                let json_value = self.property_value_to_json(prop_value);
                
                if !self.evaluate_filter_operator(&json_value, &filter.operator, &filter.value)? {
                    return Ok(false);
                }
            } else {
                // Property doesn't exist - only passes for NotEqual and NotIn
                match filter.operator {
                    FilterOperator::NotEqual | FilterOperator::NotIn => {},
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
                serde_json::Value::Number(serde_json::Number::from_f64(*d).unwrap_or_default())
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
            FilterOperator::GreaterThanOrEqual => self.compare_values(actual, expected).map(|cmp| cmp >= 0),
            FilterOperator::LessThan => self.compare_values(actual, expected).map(|cmp| cmp < 0),
            FilterOperator::LessThanOrEqual => self.compare_values(actual, expected).map(|cmp| cmp <= 0),
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
                if let (serde_json::Value::String(haystack), serde_json::Value::String(needle)) = (actual, expected) {
                    Ok(haystack.contains(needle))
                } else {
                    Ok(false)
                }
            }
            FilterOperator::StartsWith => {
                if let (serde_json::Value::String(haystack), serde_json::Value::String(prefix)) = (actual, expected) {
                    Ok(haystack.starts_with(prefix))
                } else {
                    Ok(false)
                }
            }
            FilterOperator::EndsWith => {
                if let (serde_json::Value::String(haystack), serde_json::Value::String(suffix)) = (actual, expected) {
                    Ok(haystack.ends_with(suffix))
                } else {
                    Ok(false)
                }
            }
            FilterOperator::Regex => {
                if let (serde_json::Value::String(text), serde_json::Value::String(pattern)) = (actual, expected) {
                    let regex = regex::Regex::new(pattern)
                        .map_err(|e| ProximaDBError::invalid_argument(&format!("Invalid regex: {}", e)))?;
                    Ok(regex.is_match(text))
                } else {
                    Ok(false)
                }
            }
        }
    }
    
    /// Compare two JSON values
    fn compare_values(
        &self,
        a: &serde_json::Value,
        b: &serde_json::Value,
    ) -> QueryResult<i32> {
        match (a, b) {
            (serde_json::Value::Number(n1), serde_json::Value::Number(n2)) => {
                let f1 = n1.as_f64().unwrap_or(0.0);
                let f2 = n2.as_f64().unwrap_or(0.0);
                Ok(f1.partial_cmp(&f2).unwrap_or(std::cmp::Ordering::Equal) as i32)
            }
            (serde_json::Value::String(s1), serde_json::Value::String(s2)) => {
                Ok(s1.cmp(s2) as i32)
            }
            _ => Err(ProximaDBError::invalid_argument("Cannot compare values of different types")),
        }
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
            node_scores.insert(candidate.node_id.clone(), (Some(candidate.similarity), None));
        }
        
        // Collect graph scores (using inverse distance as relevance)
        for candidate in graph_candidates {
            let graph_score = 1.0 / (candidate.distance as f32 + 1.0);
            let entry = node_scores.entry(candidate.node_id.clone()).or_insert((None, None));
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
                    FusionStrategy::Weighted { vector_weight, graph_weight } => {
                        let v_score = vector_score.unwrap_or(0.0);
                        let g_score = graph_score.unwrap_or(0.0);
                        (v_score * vector_weight + g_score * graph_weight) / (vector_weight + graph_weight)
                    }
                };
                
                results.push(HybridNodeResult {
                    node: (*node).clone(),
                    score: combined_score,
                    vector_score,
                    graph_score,
                    path: None, // TODO: Implement path tracking
                    metadata: HashMap::new(),
                });
            }
        }
        
        // Sort by combined score (descending)
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        
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
        // This is a placeholder implementation
        // TODO: Implement proper semantic path finding using embeddings
        
        // For now, use simple BFS to find any path
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
                    parent.insert(edge.to_node_id.clone(), (current_id.clone(), edge.id.clone()));
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
        let mut path = Vec::new();
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
    use crate::graph::GraphMemoryPool;
    use crate::proto::proximadb_v1::{property_value::Value, PropertyValue};
    
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