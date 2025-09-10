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

//! # Query Planner with Cost-Based Optimization
//!
//! This module implements a sophisticated query planner that uses cost-based optimization
//! to generate efficient execution plans for graph queries. It supports:
//!
//! - **Selectivity Estimation**: Predict result set sizes using statistics
//! - **Index Selection**: Choose optimal indexes based on query patterns
//! - **Join Order Optimization**: Order multi-hop traversals for minimum cost
//! - **Plan Caching**: Cache query plans for repeated queries
//! - **Statistics Integration**: Use real-time statistics for accurate cost estimates

use crate::core::error::ProximaDBError;
use crate::utils::Uuid;
use crate::graph::{NodeId, EdgeId, GraphMemoryPool};
use super::{QueryResult, QueryContext, QueryStats};
use super::ast::CompiledPattern;
use std::collections::{HashMap, HashSet, BTreeMap};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use serde::{Serialize, Deserialize};

/// Cost-based query planner
pub struct QueryPlanner {
    /// Statistics for cost estimation
    stats: Arc<RwLock<GraphStatistics>>,
    /// Cached query plans
    plan_cache: Arc<RwLock<HashMap<String, CachedPlan>>>,
    /// Configuration parameters
    config: PlannerConfig,
}

/// Configuration for query planning
#[derive(Debug, Clone)]
pub struct PlannerConfig {
    /// Maximum number of cached plans
    pub max_cached_plans: usize,
    /// Plan cache TTL in seconds
    pub plan_cache_ttl_sec: u64,
    /// Cost model parameters
    pub cost_model: CostModel,
    /// Enable/disable specific optimizations
    pub optimizations: OptimizationFlags,
}

/// Cost model parameters for estimation
#[derive(Debug, Clone)]
pub struct CostModel {
    /// Cost per node access (base cost)
    pub node_access_cost: f64,
    /// Cost per edge traversal
    pub edge_traversal_cost: f64,
    /// Cost per index seek
    pub index_seek_cost: f64,
    /// Cost per index scan (per row)
    pub index_scan_cost: f64,
    /// Memory cost factor
    pub memory_cost_factor: f64,
    /// Cache hit benefit factor
    pub cache_hit_benefit: f64,
}

/// Optimization flags
#[derive(Debug, Clone)]
pub struct OptimizationFlags {
    /// Enable index selection optimization
    pub use_indexes: bool,
    /// Enable join order optimization
    pub optimize_joins: bool,
    /// Enable predicate pushdown
    pub push_down_predicates: bool,
    /// Enable parallel execution
    pub enable_parallel: bool,
}

/// Graph statistics for cost estimation
#[derive(Debug, Default)]
pub struct GraphStatistics {
    /// Total number of nodes
    pub node_count: u64,
    /// Total number of edges
    pub edge_count: u64,
    /// Average node degree (outgoing edges)
    pub avg_node_degree: f64,
    /// Label selectivity (label -> cardinality)
    pub label_selectivity: HashMap<String, u64>,
    /// Property selectivity (property -> distinct_values)
    pub property_selectivity: HashMap<String, u64>,
    /// Edge type selectivity (type -> count)
    pub edge_type_selectivity: HashMap<String, u64>,
    /// Index statistics (index_name -> stats)
    pub index_stats: HashMap<String, IndexStats>,
}

/// Statistics for a specific index
#[derive(Debug, Clone)]
pub struct IndexStats {
    /// Number of entries in index
    pub cardinality: u64,
    /// Index selectivity (0.0 = highly selective, 1.0 = not selective)
    pub selectivity: f64,
    /// Average index seek time in microseconds
    pub avg_seek_time_us: f64,
    /// Last update timestamp
    pub last_updated: Instant,
}

/// Query execution plan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryPlan {
    /// Unique plan ID
    pub id: String,
    /// Plan steps in execution order
    pub steps: Vec<PlanStep>,
    /// Estimated total cost
    pub estimated_cost: CostEstimate,
    /// Expected result size
    pub estimated_result_size: usize,
    /// Plan creation timestamp
    pub created_at: std::time::SystemTime,
}

/// Individual plan step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    /// Step type
    pub step_type: PlanStepType,
    /// Step-specific parameters
    pub parameters: HashMap<String, serde_json::Value>,
    /// Estimated cost for this step
    pub cost: CostEstimate,
    /// Expected output cardinality
    pub output_cardinality: usize,
}

/// Types of plan steps
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PlanStepType {
    /// Scan all nodes with optional filter
    NodeScan {
        labels: Option<Vec<String>>,
        property_filters: Vec<PropertyFilter>,
    },
    /// Index seek operation
    IndexSeek {
        index_name: String,
        key_value: serde_json::Value,
    },
    /// Index scan with range
    IndexScan {
        index_name: String,
        start_key: Option<serde_json::Value>,
        end_key: Option<serde_json::Value>,
    },
    /// Graph traversal operation
    Traverse {
        algorithm: TraversalAlgorithm,
        max_depth: Option<u32>,
        edge_filters: Vec<EdgeFilter>,
    },
    /// Join two result sets
    Join {
        join_type: JoinType,
        left_key: String,
        right_key: String,
    },
    /// Filter results
    Filter {
        condition: FilterCondition,
    },
    /// Project/select specific fields
    Project {
        fields: Vec<String>,
    },
    /// Sort results
    Sort {
        fields: Vec<SortField>,
    },
    /// Limit results
    Limit {
        count: usize,
        offset: Option<usize>,
    },
}

/// Property filter for node selection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropertyFilter {
    pub property_name: String,
    pub operator: FilterOperator,
    pub value: serde_json::Value,
}

/// Edge filter for traversal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeFilter {
    pub edge_type: Option<String>,
    pub property_filters: Vec<PropertyFilter>,
}

/// Filter operators
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FilterOperator {
    Equal,
    NotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    In,
    NotIn,
    Contains,
    StartsWith,
    EndsWith,
    Regex,
}

/// Join types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JoinType {
    Inner,
    LeftOuter,
    RightOuter,
    FullOuter,
}

/// Filter conditions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FilterCondition {
    Simple(PropertyFilter),
    And(Vec<FilterCondition>),
    Or(Vec<FilterCondition>),
    Not(Box<FilterCondition>),
}

/// Traversal algorithms
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TraversalAlgorithm {
    BFS,
    DFS,
    Dijkstra,
    AStar,
}

/// Sort field specification
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SortField {
    pub field_name: String,
    pub ascending: bool,
}

/// Cost estimation breakdown
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostEstimate {
    /// CPU cost (arbitrary units)
    pub cpu_cost: f64,
    /// I/O cost (arbitrary units)
    pub io_cost: f64,
    /// Memory cost (arbitrary units)
    pub memory_cost: f64,
    /// Total cost
    pub total_cost: f64,
}

/// Cached query plan with metadata
#[derive(Debug, Clone)]
struct CachedPlan {
    plan: QueryPlan,
    access_count: u64,
    last_accessed: Instant,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            max_cached_plans: 1000,
            plan_cache_ttl_sec: 3600, // 1 hour
            cost_model: CostModel::default(),
            optimizations: OptimizationFlags::default(),
        }
    }
}

impl Default for CostModel {
    fn default() -> Self {
        Self {
            node_access_cost: 1.0,
            edge_traversal_cost: 2.0,
            index_seek_cost: 0.5,
            index_scan_cost: 0.1,
            memory_cost_factor: 0.001,
            cache_hit_benefit: 0.5,
        }
    }
}

impl Default for OptimizationFlags {
    fn default() -> Self {
        Self {
            use_indexes: true,
            optimize_joins: true,
            push_down_predicates: true,
            enable_parallel: true,
        }
    }
}

impl CostEstimate {
    pub fn new(cpu: f64, io: f64, memory: f64) -> Self {
        Self {
            cpu_cost: cpu,
            io_cost: io,
            memory_cost: memory,
            total_cost: cpu + io + memory,
        }
    }
    
    pub fn zero() -> Self {
        Self::new(0.0, 0.0, 0.0)
    }
    
    pub fn add(&self, other: &CostEstimate) -> CostEstimate {
        CostEstimate::new(
            self.cpu_cost + other.cpu_cost,
            self.io_cost + other.io_cost,
            self.memory_cost + other.memory_cost,
        )
    }
}

impl QueryPlanner {
    /// Create a new query planner
    pub fn new() -> Self {
        Self {
            stats: Arc::new(RwLock::new(GraphStatistics::default())),
            plan_cache: Arc::new(RwLock::new(HashMap::new())),
            config: PlannerConfig::default(),
        }
    }
    
    /// Create query planner with custom configuration
    pub fn with_config(config: PlannerConfig) -> Self {
        Self {
            stats: Arc::new(RwLock::new(GraphStatistics::default())),
            plan_cache: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }
    
    /// Update graph statistics
    pub fn update_statistics(&self, memory_pool: &Arc<GraphMemoryPool>) -> QueryResult<()> {
        let mut stats = self.stats.write().map_err(|_| {
            ProximaDBError::internal("Failed to acquire stats write lock")
        })?;
        
        // Update basic counts
        stats.node_count = memory_pool.node_count() as u64;
        stats.edge_count = memory_pool.edge_count() as u64;
        
        // Calculate average node degree
        if stats.node_count > 0 {
            stats.avg_node_degree = stats.edge_count as f64 / stats.node_count as f64;
        }
        
        // Update label selectivity
        stats.label_selectivity.clear();
        for entry in memory_pool.label_indexes.iter() {
            let label = entry.key().clone();
            let count = entry.value().len() as u64;
            stats.label_selectivity.insert(label, count);
        }
        
        // Update edge type selectivity
        stats.edge_type_selectivity.clear();
        for entry in memory_pool.edge_type_indexes.iter() {
            let edge_type = entry.key().clone();
            let count = entry.value().len() as u64;
            stats.edge_type_selectivity.insert(edge_type, count);
        }

        // NEW: Update property selectivity and index stats
        stats.property_selectivity.clear();
        stats.index_stats.clear();

        for (prop_name, prop_index) in memory_pool.node_property_indexes.iter() {
            stats.property_selectivity.insert(prop_name.clone(), prop_index.stats.unique_values as u64);
            stats.index_stats.insert(
                format!("node_prop_{}", prop_name),
                IndexStats {
                    cardinality: prop_index.stats.total_entries as u64,
                    selectivity: if stats.node_count > 0 {
                        prop_index.stats.total_entries as f64 / stats.node_count as f64
                    } else {
                        0.0
                    },
                    avg_seek_time_us: 0.0, // TODO: Populate with actual benchmark data
                    last_updated: Instant::now(),
                },
            );
        }

        for (prop_name, prop_index) in memory_pool.edge_property_indexes.iter() {
            stats.property_selectivity.insert(prop_name.clone(), prop_index.stats.unique_values as u64);
            stats.index_stats.insert(
                format!("edge_prop_{}", prop_name),
                IndexStats {
                    cardinality: prop_index.stats.total_entries as u64,
                    selectivity: if stats.edge_count > 0 {
                        prop_index.stats.total_entries as f64 / stats.edge_count as f64
                    } else {
                        0.0
                    },
                    avg_seek_time_us: 0.0, // TODO: Populate with actual benchmark data
                    last_updated: Instant::now(),
                },
            );
        }
        
        Ok(())
    }
    
    /// Create an optimized query plan
    pub fn create_plan(
        &self,
        query_type: &str,
        parameters: &HashMap<String, serde_json::Value>,
    ) -> QueryResult<QueryPlan> {
        let start_time = Instant::now();
        
        // Generate cache key
        let cache_key = self.generate_cache_key(query_type, parameters);
        
        // Check plan cache first
        if let Some(cached_plan) = self.get_cached_plan(&cache_key)? {
            return Ok(cached_plan);
        }
        
        // Create new plan based on query type
        let plan = match query_type {
            "node_by_label" => self.plan_node_by_label_query(parameters)?,
            "node_by_property" => self.plan_node_by_property_query(parameters)?,
            "traverse_bfs" => self.plan_traversal_query(parameters, TraversalAlgorithm::BFS)?,
            "traverse_dfs" => self.plan_traversal_query(parameters, TraversalAlgorithm::DFS)?,
            "shortest_path" => self.plan_shortest_path_query(parameters)?,
            "pattern_match" => self.plan_pattern_match_query(parameters)?,
            _ => return Err(ProximaDBError::invalid_argument(&format!(
                "Unknown query type: {}", query_type
            ))),
        };
        
        // Cache the plan
        self.cache_plan(cache_key, &plan)?;
        
        Ok(plan)
    }
    
    /// Plan a node-by-label query
    fn plan_node_by_label_query(
        &self,
        parameters: &HashMap<String, serde_json::Value>,
    ) -> QueryResult<QueryPlan> {
        let label = parameters.get("label")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProximaDBError::invalid_argument("Missing 'label' parameter"))?;
        
        let stats = self.stats.read().map_err(|_| {
            ProximaDBError::internal("Failed to acquire stats read lock")
        })?;
        
        // Estimate selectivity
        let label_cardinality = stats.label_selectivity.get(label).copied().unwrap_or(0);
        let selectivity = if stats.node_count > 0 {
            label_cardinality as f64 / stats.node_count as f64
        } else {
            0.0
        };
        
        // Choose strategy based on selectivity
        let step = if self.config.optimizations.use_indexes && selectivity < 0.5 {
            // Use index if available and selective
            PlanStep {
                step_type: PlanStepType::IndexSeek {
                    index_name: format!("label_index_{}", label),
                    key_value: serde_json::Value::String(label.to_string()),
                },
                parameters: HashMap::new(),
                cost: CostEstimate::new(
                    self.config.cost_model.index_seek_cost,
                    0.0,
                    label_cardinality as f64 * self.config.cost_model.memory_cost_factor,
                ),
                output_cardinality: label_cardinality as usize,
            }
        } else {
            // Full scan with filter
            PlanStep {
                step_type: PlanStepType::NodeScan {
                    labels: Some(vec![label.to_string()]),
                    property_filters: vec![],
                },
                parameters: HashMap::new(),
                cost: CostEstimate::new(
                    stats.node_count as f64 * self.config.cost_model.node_access_cost,
                    0.0,
                    label_cardinality as f64 * self.config.cost_model.memory_cost_factor,
                ),
                output_cardinality: label_cardinality as usize,
            }
        };
        
        Ok(QueryPlan {
            id: Uuid::new_v4().to_string(),
            steps: vec![step.clone()],
            estimated_cost: step.cost.clone(),
            estimated_result_size: step.output_cardinality,
            created_at: std::time::SystemTime::now(),
        })
    }
    
    /// Plan a node-by-property query
    fn plan_node_by_property_query(
        &self,
        parameters: &HashMap<String, serde_json::Value>,
    ) -> QueryResult<QueryPlan> {
        let property_name = parameters.get("property_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProximaDBError::invalid_argument("Missing 'property_name' parameter"))?;
        
        let property_value = parameters.get("property_value")
            .ok_or_else(|| ProximaDBError::invalid_argument("Missing 'property_value' parameter"))?;
        
        let stats = self.stats.read().map_err(|_| {
            ProximaDBError::internal("Failed to acquire stats read lock")
        })?;
        
        // Estimate selectivity based on property statistics
        let distinct_values = stats.property_selectivity.get(property_name).copied().unwrap_or(1);
        let estimated_cardinality = if distinct_values > 0 {
            (stats.node_count / distinct_values).max(1)
        } else {
            1
        };
        
        let selectivity = estimated_cardinality as f64 / stats.node_count as f64;
        
        // Choose strategy based on selectivity
        let step = if self.config.optimizations.use_indexes && selectivity < 0.3 {
            // Use property index
            PlanStep {
                step_type: PlanStepType::IndexSeek {
                    index_name: format!("property_index_{}", property_name),
                    key_value: property_value.clone(),
                },
                parameters: HashMap::new(),
                cost: CostEstimate::new(
                    self.config.cost_model.index_seek_cost,
                    0.0,
                    estimated_cardinality as f64 * self.config.cost_model.memory_cost_factor,
                ),
                output_cardinality: estimated_cardinality as usize,
            }
        } else {
            // Full scan with property filter
            PlanStep {
                step_type: PlanStepType::NodeScan {
                    labels: None,
                    property_filters: vec![PropertyFilter {
                        property_name: property_name.to_string(),
                        operator: FilterOperator::Equal,
                        value: property_value.clone(),
                    }],
                },
                parameters: HashMap::new(),
                cost: CostEstimate::new(
                    stats.node_count as f64 * self.config.cost_model.node_access_cost,
                    0.0,
                    estimated_cardinality as f64 * self.config.cost_model.memory_cost_factor,
                ),
                output_cardinality: estimated_cardinality as usize,
            }
        };
        
        Ok(QueryPlan {
            id: Uuid::new_v4().to_string(),
            steps: vec![step.clone()],
            estimated_cost: step.cost.clone(),
            estimated_result_size: step.output_cardinality,
            created_at: std::time::SystemTime::now(),
        })
    }
    
    /// Plan a graph traversal query
    fn plan_traversal_query(
        &self,
        parameters: &HashMap<String, serde_json::Value>,
        algorithm: TraversalAlgorithm,
    ) -> QueryResult<QueryPlan> {
        let start_node = parameters.get("start_node")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProximaDBError::invalid_argument("Missing 'start_node' parameter"))?;
        
        let max_depth = parameters.get("max_depth")
            .and_then(|v| v.as_u64())
            .map(|d| d as u32);
        
        let edge_type = parameters.get("edge_type")
            .and_then(|v| v.as_str());
        
        let stats = self.stats.read().map_err(|_| {
            ProximaDBError::internal("Failed to acquire stats read lock")
        })?;
        
        // Estimate traversal cost based on graph statistics
        let avg_degree = stats.avg_node_degree;
        let depth = max_depth.unwrap_or(3) as f64;
        
        // Exponential growth estimate with branching factor
        let estimated_nodes_visited = if avg_degree > 1.0 {
            (avg_degree.powf(depth) - 1.0) / (avg_degree - 1.0)
        } else {
            depth
        };
        
        let estimated_edges_traversed = estimated_nodes_visited * avg_degree;
        
        // Apply edge type selectivity if specified
        let (nodes_visited, edges_traversed) = if let Some(edge_type) = edge_type {
            let edge_type_count = stats.edge_type_selectivity.get(edge_type).copied().unwrap_or(0);
            let edge_type_selectivity = if stats.edge_count > 0 {
                edge_type_count as f64 / stats.edge_count as f64
            } else {
                0.1
            };
            
            (
                (estimated_nodes_visited * edge_type_selectivity).max(1.0) as usize,
                (estimated_edges_traversed * edge_type_selectivity).max(1.0),
            )
        } else {
            (estimated_nodes_visited as usize, estimated_edges_traversed)
        };
        
        let step = PlanStep {
            step_type: PlanStepType::Traverse {
                algorithm,
                max_depth,
                edge_filters: if let Some(et) = edge_type {
                    vec![EdgeFilter {
                        edge_type: Some(et.to_string()),
                        property_filters: vec![],
                    }]
                } else {
                    vec![]
                },
            },
            parameters: parameters.clone(),
            cost: CostEstimate::new(
                nodes_visited as f64 * self.config.cost_model.node_access_cost +
                edges_traversed * self.config.cost_model.edge_traversal_cost,
                0.0,
                nodes_visited as f64 * self.config.cost_model.memory_cost_factor,
            ),
            output_cardinality: nodes_visited,
        };
        
        Ok(QueryPlan {
            id: Uuid::new_v4().to_string(),
            steps: vec![step.clone()],
            estimated_cost: step.cost.clone(),
            estimated_result_size: step.output_cardinality,
            created_at: std::time::SystemTime::now(),
        })
    }
    
    /// Plan a shortest path query
    fn plan_shortest_path_query(
        &self,
        parameters: &HashMap<String, serde_json::Value>,
    ) -> QueryResult<QueryPlan> {
        // Similar to traversal but with Dijkstra algorithm
        let mut dijkstra_params = parameters.clone();
        dijkstra_params.insert("algorithm".to_string(), serde_json::Value::String("dijkstra".to_string()));
        
        self.plan_traversal_query(&dijkstra_params, TraversalAlgorithm::Dijkstra)
    }

    /// Plan a pattern query from a CompiledPattern
    pub fn plan_pattern_query(
        &self,
        pattern: &CompiledPattern,
    ) -> QueryResult<QueryPlan> {
        let stats = self.stats.read().map_err(|_| {
            ProximaDBError::internal("Failed to acquire stats read lock")
        })?;

        let mut best_plan: Option<QueryPlan> = None;
        let mut min_cost = f64::MAX;

        // Iterate through all node patterns as potential starting points
        for starting_node_pattern in &pattern.nodes {
            let mut current_steps = Vec::new();
            let mut current_estimated_cost = CostEstimate::zero();
            let mut current_estimated_result_size = 0;

            let node_count = stats.node_count as f64;
            let mut initial_cardinality = node_count;
            let mut steps = Vec::new();
            let mut estimated_cost = CostEstimate::zero();
            let mut estimated_result_size = 0;

            // Estimate selectivity based on labels
            if !node_pattern.labels.is_empty() {
                // Use the most selective label if multiple are present
                let mut label_selectivity = 1.0;
                for label in &node_pattern.labels {
                    if let Some(&count) = stats.label_selectivity.get(label) {
                        label_selectivity *= (count as f64 / node_count).min(1.0);
                    } else {
                        // If label not found in stats, assume low selectivity (e.g., 10%)
                        label_selectivity *= 0.1;
                    }
                }
                current_cardinality *= label_selectivity;
            }

            // Estimate selectivity based on properties (simplified)
            if !node_pattern.properties.is_empty() {
                // For each property, assume a default selectivity (e.g., 10%)
                // TODO: Use actual property selectivity from stats
                current_cardinality *= 0.1_f64.powi(node_pattern.properties.len() as i32);
            }

            let estimated_output_cardinality = current_cardinality.max(1.0) as usize;

            // Choose strategy based on selectivity and index availability
            let step_cost;
            let step_type;

            // Simplified logic: if labels are present, assume index seek is possible
            if self.config.optimizations.use_indexes && !node_pattern.labels.is_empty() {
                step_type = PlanStepType::IndexSeek {
                    index_name: format!("label_index_{}", node_pattern.labels.first().unwrap()),
                    key_value: serde_json::Value::String(node_pattern.labels.first().unwrap().clone()),
                };
                step_cost = CostEstimate::new(
                    self.config.cost_model.index_seek_cost,
                    0.0,
                    estimated_output_cardinality as f64 * self.config.cost_model.memory_cost_factor,
                );
            } else {
                step_type = PlanStepType::NodeScan {
                    labels: if !node_pattern.labels.is_empty() {
                        Some(node_pattern.labels.clone())
                    } else {
                        None
                    },
                    property_filters: Vec::new(), // TODO: Convert PropertyConstraint to PropertyFilter
                };
                step_cost = CostEstimate::new(
                    node_count * self.config.cost_model.node_access_cost,
                    0.0,
                    estimated_output_cardinality as f64 * self.config.cost_model.memory_cost_factor,
                );
            }

            let node_scan_step = PlanStep {
                step_type,
                parameters: HashMap::new(),
                cost: step_cost,
                output_cardinality: estimated_output_cardinality,
            };
            estimated_cost = estimated_cost.add(&node_scan_step.cost);
            estimated_result_size = node_scan_step.output_cardinality;
            steps.push(node_scan_step);
        }

        // Step 2: Handle Edge Patterns (simplified to a generic traversal for now)
        if !pattern.edges.is_empty() || !pattern.paths.is_empty() {
            let avg_degree = stats.avg_node_degree;
            let traversal_cost = estimated_result_size as f64 * avg_degree * self.config.cost_model.edge_traversal_cost;
            let traversal_output_cardinality = (estimated_result_size as f64 * avg_degree).max(1.0) as usize;

            let traversal_step = PlanStep {
                step_type: PlanStepType::Traverse {
                    algorithm: TraversalAlgorithm::BFS, // Default to BFS
                    max_depth: None, // TODO: Infer from path patterns
                    edge_filters: Vec::new(), // TODO: Convert EdgePattern properties to EdgeFilter
                },
                parameters: HashMap::new(),
                cost: CostEstimate::new(traversal_cost, 0.0, traversal_output_cardinality as f64 * self.config.cost_model.memory_cost_factor),
                output_cardinality: traversal_output_cardinality,
            };
            estimated_cost = estimated_cost.add(&traversal_step.cost);
            steps.push(traversal_step);
        }

        // Step 3: Handle WHERE clauses
        if !pattern.where_clauses.is_empty() {
            // Assume WHERE clause reduces cardinality by 50% (simplified)
            let filter_cardinality = (estimated_result_size as f64 * 0.5).max(1.0) as usize;
            let filter_step = PlanStep {
                step_type: PlanStepType::Filter {
                    condition: FilterCondition::And(Vec::new()), // TODO: Convert WhereClause to FilterCondition
                },
                parameters: HashMap::new(),
                cost: CostEstimate::new(filter_cardinality as f64 * self.config.cost_model.cpu_cost, 0.0, 0.0), // CPU cost for filtering
                output_cardinality: filter_cardinality,
            };
            estimated_cost = estimated_cost.add(&filter_step.cost);
            estimated_result_size = filter_cardinality;
            steps.push(filter_step);
        }

        // Step 4: Handle RETURN clause (simplified to Project and Limit)
        if !pattern.return_spec.variables.is_empty() || !pattern.return_spec.projections.is_empty() {
            let project_step = PlanStep {
                step_type: PlanStepType::Project {
                    fields: pattern.return_spec.variables.clone(), // Simplified
                },
                parameters: HashMap::new(),
                cost: CostEstimate::new(estimated_result_size as f64 * self.config.cost_model.cpu_cost, 0.0, 0.0),
                output_cardinality: estimated_result_size,
            };
            estimated_cost = estimated_cost.add(&project_step.cost);
            steps.push(project_step);
        }

        if let Some(limit) = pattern.return_spec.limit {
            let limit_step = PlanStep {
                step_type: PlanStepType::Limit {
                    count: limit as usize,
                    offset: pattern.return_spec.skip.map(|s| s as usize),
                },
                parameters: HashMap::new(),
                cost: CostEstimate::zero(), // Minimal cost
                output_cardinality: limit as usize,
            };
            estimated_cost = estimated_cost.add(&limit_step.cost);
            steps.push(limit_step);
        }

        // Set current_steps and current_estimated_cost for comparison
        current_steps = steps;
        current_estimated_cost = estimated_cost;

        // Compare with best plan found so far
        if current_estimated_cost.total_cost < min_cost {
            min_cost = current_estimated_cost.total_cost;
            best_plan = Some(QueryPlan {
                id: Uuid::new_v4().to_string(),
                steps: current_steps,
                estimated_cost: current_estimated_cost,
                estimated_result_size: current_estimated_result_size,
                created_at: std::time::SystemTime::now(),
            });
        }
    // End of the for loop

        best_plan.ok_or_else(|| ProximaDBError::invalid_argument("Could not generate a plan for the given pattern"))
    }
    
    /// Plan a pattern matching query (simplified for now)
    pub fn plan_pattern_match_query(
        &self,
        parameters: &HashMap<String, serde_json::Value>,
    ) -> QueryResult<QueryPlan> {
        // For now, assume the 'pattern' parameter contains the Cypher-like string
        let pattern_str = parameters.get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProximaDBError::invalid_argument("Missing 'pattern' parameter for pattern match query"))?;

        // Use the QueryParser to parse the pattern string into a CompiledPattern
        let parser = super::parser::QueryParser::new(); // Assuming parser is in super::parser
        let compiled_pattern = parser.parse(pattern_str)?;

        // Now plan the compiled pattern
        self.plan_pattern_query(&compiled_pattern)
    }
    
    /// Generate cache key for query
    fn generate_cache_key(
        &self,
        query_type: &str,
        parameters: &HashMap<String, serde_json::Value>,
    ) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        query_type.hash(&mut hasher);
        
        // Sort parameters for consistent hashing
        let mut sorted_params: Vec<_> = parameters.iter().collect();
        sorted_params.sort_by_key(|&(k, _)| k);
        
        for (key, value) in sorted_params {
            key.hash(&mut hasher);
            value.to_string().hash(&mut hasher);
        }
        
        format!("plan_{:016x}", hasher.finish())
    }
    
    /// Get cached plan if available and not expired
    fn get_cached_plan(&self, cache_key: &str) -> QueryResult<Option<QueryPlan>> {
        let cache = self.plan_cache.read().map_err(|_| {
            ProximaDBError::internal("Failed to acquire plan cache read lock")
        })?;
        
        if let Some(cached) = cache.get(cache_key) {
            let age = cached.last_accessed.elapsed();
            if age.as_secs() < self.config.plan_cache_ttl_sec {
                return Ok(Some(cached.plan.clone()));
            }
        }
        
        Ok(None)
    }
    
    /// Cache a query plan
    fn cache_plan(&self, cache_key: String, plan: &QueryPlan) -> QueryResult<()> {
        let mut cache = self.plan_cache.write().map_err(|_| {
            ProximaDBError::internal("Failed to acquire plan cache write lock")
        })?;
        
        // Remove expired plans if cache is full
        if cache.len() >= self.config.max_cached_plans {
            let now = Instant::now();
            let expired_keys: Vec<_> = cache
                .iter()
                .filter(|(_, cached)| {
                    now.duration_since(cached.last_accessed).as_secs() > self.config.plan_cache_ttl_sec
                })
                .map(|(k, _)| k.clone())
                .collect();
            
            for key in expired_keys {
                cache.remove(&key);
            }
            
            // If still full, remove least recently used
            if cache.len() >= self.config.max_cached_plans {
                if let Some((lru_key, _)) = cache
                    .iter()
                    .min_by_key(|(_, cached)| cached.last_accessed)
                    .map(|(k, v)| (k.clone(), v.clone()))
                {
                    cache.remove(&lru_key);
                }
            }
        }
        
        cache.insert(cache_key, CachedPlan {
            plan: plan.clone(),
            access_count: 1,
            last_accessed: Instant::now(),
        });
        
        Ok(())
    }
    
    /// Get query planner statistics
    pub fn get_statistics(&self) -> QueryResult<GraphStatistics> {
        let stats = self.stats.read().map_err(|_| {
            ProximaDBError::internal("Failed to acquire stats read lock")
        })?;
        
        Ok(stats.clone())
    }
    
    /// Clear plan cache
    pub fn clear_cache(&self) -> QueryResult<()> {
        let mut cache = self.plan_cache.write().map_err(|_| {
            ProximaDBError::internal("Failed to acquire plan cache write lock")
        })?;
        
        cache.clear();
        Ok(())
    }
}

impl Default for QueryPlanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::GraphMemoryPool;
    
    #[test]
    fn test_query_planner_creation() {
        let planner = QueryPlanner::new();
        let stats = planner.get_statistics().unwrap();
        assert_eq!(stats.node_count, 0);
        assert_eq!(stats.edge_count, 0);
    }
    
    #[test]
    fn test_cost_estimate() {
        let cost1 = CostEstimate::new(10.0, 5.0, 2.0);
        let cost2 = CostEstimate::new(3.0, 7.0, 1.0);
        
        assert_eq!(cost1.total_cost, 17.0);
        
        let combined = cost1.add(&cost2);
        assert_eq!(combined.cpu_cost, 13.0);
        assert_eq!(combined.io_cost, 12.0);
        assert_eq!(combined.memory_cost, 3.0);
        assert_eq!(combined.total_cost, 28.0);
    }
    
    #[test]
    fn test_cache_key_generation() {
        let planner = QueryPlanner::new();
        
        let mut params1 = HashMap::new();
        params1.insert("label".to_string(), serde_json::Value::String("Person".to_string()));
        
        let mut params2 = HashMap::new();
        params2.insert("label".to_string(), serde_json::Value::String("Person".to_string()));
        
        let key1 = planner.generate_cache_key("node_by_label", &params1);
        let key2 = planner.generate_cache_key("node_by_label", &params2);
        
        assert_eq!(key1, key2); // Same parameters should generate same key
        
        let key3 = planner.generate_cache_key("node_by_property", &params1);
        assert_ne!(key1, key3); // Different query type should generate different key
    }
    
    #[test]
    fn test_statistics_update() {
        let planner = QueryPlanner::new();
        let memory_pool = Arc::new(GraphMemoryPool::new());
        
        // Update statistics with empty pool
        planner.update_statistics(&memory_pool).unwrap();
        
        let stats = planner.get_statistics().unwrap();
        assert_eq!(stats.node_count, 0);
        assert_eq!(stats.edge_count, 0);
        assert_eq!(stats.avg_node_degree, 0.0);
    }
    
    #[test]
    fn test_plan_node_by_label_query() {
        let planner = QueryPlanner::new();
        
        let mut params = HashMap::new();
        params.insert("label".to_string(), serde_json::Value::String("Person".to_string()));
        
        let plan = planner.plan_node_by_label_query(&params).unwrap();
        
        assert!(!plan.id.is_empty());
        assert_eq!(plan.steps.len(), 1);
        
        match &plan.steps[0].step_type {
            PlanStepType::NodeScan { labels, .. } => {
                assert_eq!(labels.as_ref().unwrap()[0], "Person");
            }
            PlanStepType::IndexSeek { .. } => {
                // Also valid if index is chosen
            }
            _ => panic!("Unexpected plan step type"),
        }
    }
}
