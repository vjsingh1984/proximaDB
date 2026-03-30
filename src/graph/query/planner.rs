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

use super::QueryResult;
use super::ast::CompiledPattern;
use crate::core::error::{QueryError, VectorDBError};
use crate::graph::GraphMemoryPool;
use crate::utils::Uuid;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Instant;

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

/// Trait for extensible cost estimation
pub trait CostEstimator: Send + Sync {
    /// Estimate cost of node scan
    fn estimate_scan_cost(&self, cardinality: usize) -> f64;

    /// Estimate cost of index seek
    fn estimate_index_seek_cost(&self, cardinality: usize) -> f64;

    /// Estimate cost of edge expansion
    fn estimate_expand_cost(&self, input_card: usize, avg_degree: f64) -> f64;

    /// Estimate cost of pattern matching
    fn estimate_pattern_cost(&self, pattern: &CompiledPattern, stats: &GraphStatistics) -> f64;

    /// Estimate selectivity of node pattern
    fn estimate_node_selectivity(
        &self,
        pattern: &super::ast::NodePattern,
        stats: &GraphStatistics,
    ) -> f64;

    /// Estimate selectivity of edge pattern
    fn estimate_edge_selectivity(
        &self,
        pattern: &super::ast::EdgePattern,
        stats: &GraphStatistics,
    ) -> f64;

    /// Estimate selectivity of WHERE clause
    fn estimate_where_selectivity(
        &self,
        clause: &super::ast::WhereClause,
        stats: &GraphStatistics,
    ) -> f64;
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
    /// CPU cost per operation
    pub cpu_cost: f64,
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
#[derive(Debug, Clone, Default)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
    Filter { condition: FilterCondition },
    /// Project/select specific fields
    Project { fields: Vec<String> },
    /// Sort results
    Sort { fields: Vec<SortField> },
    /// Limit results
    Limit { count: usize, offset: Option<usize> },
}

/// Property filter for node selection
#[derive(Debug, Clone)]
pub struct PropertyFilter {
    pub property_name: String,
    pub operator: FilterOperator,
    pub value: serde_json::Value,
}

/// Edge filter for traversal
#[derive(Debug, Clone)]
pub struct EdgeFilter {
    pub edge_type: Option<String>,
    pub property_filters: Vec<PropertyFilter>,
}

/// Filter operators
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
pub enum JoinType {
    Inner,
    LeftOuter,
    RightOuter,
    FullOuter,
}

/// Filter conditions
#[derive(Debug, Clone)]
pub enum FilterCondition {
    Simple(PropertyFilter),
    And(Vec<FilterCondition>),
    Or(Vec<FilterCondition>),
    Not(Box<FilterCondition>),
}

/// Traversal algorithms
#[derive(Debug, Clone)]
pub enum TraversalAlgorithm {
    BFS,
    DFS,
    Dijkstra,
    AStar,
}

/// Sort field specification
#[derive(Debug, Clone)]
pub struct SortField {
    pub field_name: String,
    pub ascending: bool,
}

/// Cost estimation breakdown
#[derive(Debug, Clone)]
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
    #[allow(dead_code)]
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
            cpu_cost: 0.1,
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

/// Implementation of CostEstimator trait for CostModel
impl CostEstimator for CostModel {
    fn estimate_scan_cost(&self, cardinality: usize) -> f64 {
        cardinality as f64 * self.node_access_cost
    }

    fn estimate_index_seek_cost(&self, cardinality: usize) -> f64 {
        self.index_seek_cost + (cardinality as f64 * self.index_scan_cost)
    }

    fn estimate_expand_cost(&self, input_card: usize, avg_degree: f64) -> f64 {
        input_card as f64 * avg_degree * self.edge_traversal_cost
    }

    fn estimate_pattern_cost(&self, pattern: &CompiledPattern, stats: &GraphStatistics) -> f64 {
        let mut total_cost = 0.0;

        // Estimate node pattern costs
        for node_pattern in &pattern.nodes {
            let selectivity = self.estimate_node_selectivity(node_pattern, stats);
            let cardinality = (stats.node_count as f64 * selectivity).max(1.0) as usize;
            total_cost += self.estimate_scan_cost(cardinality);
        }

        // Estimate edge pattern costs
        for edge_pattern in &pattern.edges {
            let selectivity = self.estimate_edge_selectivity(edge_pattern, stats);
            let cardinality = (stats.edge_count as f64 * selectivity).max(1.0) as usize;
            total_cost += self.edge_traversal_cost * cardinality as f64;
        }

        // Estimate WHERE clause costs
        for where_clause in &pattern.where_clauses {
            let selectivity = self.estimate_where_selectivity(where_clause, stats);
            total_cost += self.cpu_cost * selectivity;
        }

        total_cost
    }

    fn estimate_node_selectivity(
        &self,
        pattern: &super::ast::NodePattern,
        stats: &GraphStatistics,
    ) -> f64 {
        let mut selectivity = 1.0;

        // Apply label selectivity
        if !pattern.labels.is_empty() {
            for label in &pattern.labels {
                if let Some(&count) = stats.label_selectivity.get(label) {
                    let label_sel = if stats.node_count > 0 {
                        count as f64 / stats.node_count as f64
                    } else {
                        0.1
                    };
                    selectivity *= label_sel;
                } else {
                    // Unknown label, assume 10% selectivity
                    selectivity *= 0.1;
                }
            }
        }

        // Apply property constraint selectivity
        if !pattern.properties.is_empty() {
            // Each property constraint reduces result set
            // Use histogram-based estimation if available, otherwise default to 0.1 per constraint
            for (prop_name, _constraint) in &pattern.properties {
                if let Some(&distinct_values) = stats.property_selectivity.get(prop_name) {
                    // Selectivity = 1 / distinct_values (assuming uniform distribution)
                    let prop_sel = if distinct_values > 0 {
                        1.0 / distinct_values as f64
                    } else {
                        0.1
                    };
                    selectivity *= prop_sel;
                } else {
                    selectivity *= 0.1;
                }
            }
        }

        selectivity.max(0.001) // Minimum selectivity of 0.1%
    }

    fn estimate_edge_selectivity(
        &self,
        pattern: &super::ast::EdgePattern,
        stats: &GraphStatistics,
    ) -> f64 {
        let mut selectivity = 1.0;

        // Apply edge type selectivity
        if !pattern.edge_types.is_empty() {
            for edge_type in &pattern.edge_types {
                if let Some(&count) = stats.edge_type_selectivity.get(edge_type) {
                    let type_sel = if stats.edge_count > 0 {
                        count as f64 / stats.edge_count as f64
                    } else {
                        0.1
                    };
                    selectivity *= type_sel;
                } else {
                    selectivity *= 0.1;
                }
            }
        }

        // Apply property constraint selectivity
        if !pattern.properties.is_empty() {
            selectivity *= 0.1_f64.powi(pattern.properties.len() as i32);
        }

        selectivity.max(0.001)
    }

    fn estimate_where_selectivity(
        &self,
        clause: &super::ast::WhereClause,
        stats: &GraphStatistics,
    ) -> f64 {
        use super::ast::WhereClause;

        match clause {
            WhereClause::Property {
                property,
                constraint,
                ..
            } => {
                // Estimate based on property statistics and constraint type
                if let Some(&distinct_values) = stats.property_selectivity.get(property) {
                    match constraint {
                        super::ast::PropertyConstraint::Equals(_) => {
                            // Equality: 1 / distinct_values
                            if distinct_values > 0 {
                                1.0 / distinct_values as f64
                            } else {
                                0.1
                            }
                        }
                        super::ast::PropertyConstraint::GreaterThan(_)
                        | super::ast::PropertyConstraint::LessThan(_) => {
                            // Range: assume 30% selectivity
                            0.3
                        }
                        super::ast::PropertyConstraint::GreaterThanOrEqual(_)
                        | super::ast::PropertyConstraint::GreaterOrEqual(_)
                        | super::ast::PropertyConstraint::LessThanOrEqual(_)
                        | super::ast::PropertyConstraint::LessOrEqual(_) => {
                            // Range with equality: assume 35% selectivity
                            0.35
                        }
                        super::ast::PropertyConstraint::NotEquals(_) => {
                            // Not equals: (distinct_values - 1) / distinct_values
                            if distinct_values > 1 {
                                (distinct_values - 1) as f64 / distinct_values as f64
                            } else {
                                0.9
                            }
                        }
                        super::ast::PropertyConstraint::In(values) => {
                            // IN clause: values.len() / distinct_values
                            if distinct_values > 0 {
                                (values.len() as f64 / distinct_values as f64).min(1.0)
                            } else {
                                0.5
                            }
                        }
                        super::ast::PropertyConstraint::Contains(_)
                        | super::ast::PropertyConstraint::StartsWith(_)
                        | super::ast::PropertyConstraint::EndsWith(_) => {
                            // String operations: assume 20% selectivity
                            0.2
                        }
                        _ => 0.1,
                    }
                } else {
                    // Unknown property, assume 10% selectivity
                    0.1
                }
            }
            WhereClause::And(left, right) => {
                // AND: multiply selectivities
                let left_sel = self.estimate_where_selectivity(left, stats);
                let right_sel = self.estimate_where_selectivity(right, stats);
                left_sel * right_sel
            }
            WhereClause::Or(left, right) => {
                // OR: use inclusion-exclusion principle
                let left_sel = self.estimate_where_selectivity(left, stats);
                let right_sel = self.estimate_where_selectivity(right, stats);
                left_sel + right_sel - (left_sel * right_sel)
            }
            WhereClause::Not(inner) => {
                // NOT: 1 - selectivity
                let inner_sel = self.estimate_where_selectivity(inner, stats);
                1.0 - inner_sel
            }
        }
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
            VectorDBError::Internal("Failed to acquire stats write lock".to_string())
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

        // Store node_count and edge_count to avoid borrowing conflicts
        let node_count = stats.node_count;
        let edge_count = stats.edge_count;

        for entry in memory_pool.node_property_indexes.iter() {
            let prop_name = entry.key().clone();
            let prop_values = entry.value();
            // Use length of property values as a rough estimate for stats
            let unique_values = prop_values.len() as u64;
            let total_entries = prop_values.len() as u64;

            stats
                .property_selectivity
                .insert(prop_name.clone(), unique_values);
            stats.index_stats.insert(
                format!("node_prop_{}", prop_name),
                IndexStats {
                    cardinality: total_entries,
                    selectivity: if node_count > 0 {
                        total_entries as f64 / node_count as f64
                    } else {
                        0.0
                    },
                    avg_seek_time_us: 0.0, // TODO: Populate with actual benchmark data
                    last_updated: Instant::now(),
                },
            );
        }

        for entry in memory_pool.edge_property_indexes.iter() {
            let prop_name = entry.key().clone();
            let prop_values = entry.value();
            // Use length of property values as a rough estimate for stats
            let unique_values = prop_values.len() as u64;
            let total_entries = prop_values.len() as u64;

            stats
                .property_selectivity
                .insert(prop_name.clone(), unique_values);
            stats.index_stats.insert(
                format!("edge_prop_{}", prop_name),
                IndexStats {
                    cardinality: total_entries,
                    selectivity: if edge_count > 0 {
                        total_entries as f64 / edge_count as f64
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
        let _start_time = Instant::now();

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
            _ => {
                return Err(VectorDBError::InvalidInput(format!(
                    "Unknown query type: {}",
                    query_type
                )));
            }
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
        let label = parameters
            .get("label")
            .and_then(|v| v.as_str())
            .ok_or_else(|| VectorDBError::InvalidInput("Missing 'label' parameter".to_string()))?;

        let stats = self.stats.read().map_err(|_| {
            VectorDBError::Internal("Failed to acquire stats read lock".to_string())
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
        let property_name = parameters
            .get("property_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                VectorDBError::InvalidInput("Missing 'property_name' parameter".to_string())
            })?;

        let property_value = parameters.get("property_value").ok_or_else(|| {
            VectorDBError::InvalidInput("Missing 'property_value' parameter".to_string())
        })?;

        let stats = self.stats.read().map_err(|_| {
            VectorDBError::Internal("Failed to acquire stats read lock".to_string())
        })?;

        // Estimate selectivity based on property statistics
        let distinct_values = stats
            .property_selectivity
            .get(property_name)
            .copied()
            .unwrap_or(1);
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
        let _start_node = parameters
            .get("start_node")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                VectorDBError::InvalidInput("Missing 'start_node' parameter".to_string())
            })?;

        let max_depth = parameters
            .get("max_depth")
            .and_then(|v| v.as_u64())
            .map(|d| d as u32);

        let edge_type = parameters.get("edge_type").and_then(|v| v.as_str());

        let stats = self.stats.read().map_err(|_| {
            VectorDBError::Internal("Failed to acquire stats read lock".to_string())
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
            let edge_type_count = stats
                .edge_type_selectivity
                .get(edge_type)
                .copied()
                .unwrap_or(0);
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
                nodes_visited as f64 * self.config.cost_model.node_access_cost
                    + edges_traversed * self.config.cost_model.edge_traversal_cost,
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
        dijkstra_params.insert(
            "algorithm".to_string(),
            serde_json::Value::String("dijkstra".to_string()),
        );

        self.plan_traversal_query(&dijkstra_params, TraversalAlgorithm::Dijkstra)
    }

    /// Plan a pattern query from a CompiledPattern with advanced join order optimization
    pub fn plan_pattern_query(&self, pattern: &CompiledPattern) -> QueryResult<QueryPlan> {
        let stats = self.stats.read().map_err(|_| {
            VectorDBError::Internal("Failed to acquire stats read lock".to_string())
        })?;

        if pattern.nodes.is_empty() {
            return Err(VectorDBError::InvalidInput(
                "Pattern must contain at least one node".to_string(),
            ));
        }

        // Use CostEstimator trait for selectivity estimation
        let cost_estimator = &self.config.cost_model;

        // Find the most selective starting node pattern
        let (start_idx, start_selectivity) = pattern
            .nodes
            .iter()
            .enumerate()
            .map(|(idx, node_pattern)| {
                let selectivity = cost_estimator.estimate_node_selectivity(node_pattern, &stats);
                (idx, selectivity)
            })
            .min_by(|(_, sel1), (_, sel2)| {
                sel1.partial_cmp(sel2).unwrap_or(std::cmp::Ordering::Equal)
            })
            .ok_or_else(|| {
                VectorDBError::Query(QueryError::InvalidQuery(
                    "No viable starting node found in pattern".to_string(),
                ))
            })?;

        let starting_node = &pattern.nodes[start_idx];
        let start_cardinality = (stats.node_count as f64 * start_selectivity).max(1.0) as usize;

        // Build execution plan
        let mut steps = Vec::new();
        let mut total_cost = CostEstimate::zero();
        let mut current_cardinality = start_cardinality;

        // Step 1: Initial node access (most selective)
        let (node_step, node_cost) =
            self.plan_node_access(starting_node, start_cardinality, &stats)?;
        steps.push(node_step);
        total_cost = total_cost.add(&node_cost);

        // Step 2: Optimize edge traversal order if edges exist
        if !pattern.edges.is_empty() && self.config.optimizations.optimize_joins {
            let (edge_steps, edge_cost, edge_cardinality) = self.optimize_edge_join_order(
                &pattern.edges,
                current_cardinality,
                &stats,
                cost_estimator,
            )?;
            steps.extend(edge_steps);
            total_cost = total_cost.add(&edge_cost);
            current_cardinality = edge_cardinality;
        }

        // Step 3: Apply WHERE clauses with predicate pushdown
        if !pattern.where_clauses.is_empty() {
            for where_clause in &pattern.where_clauses {
                let where_selectivity =
                    cost_estimator.estimate_where_selectivity(where_clause, &stats);
                let filter_cardinality =
                    (current_cardinality as f64 * where_selectivity).max(1.0) as usize;

                let filter_step = PlanStep {
                    step_type: PlanStepType::Filter {
                        condition: FilterCondition::And(Vec::new()), // Simplified
                    },
                    parameters: HashMap::new(),
                    cost: CostEstimate::new(
                        current_cardinality as f64 * self.config.cost_model.cpu_cost,
                        0.0,
                        0.0,
                    ),
                    output_cardinality: filter_cardinality,
                };
                total_cost = total_cost.add(&filter_step.cost);
                current_cardinality = filter_cardinality;
                steps.push(filter_step);
            }
        }

        // Step 4: Handle ORDER BY
        if !pattern.return_spec.order_by.is_empty() {
            let sort_fields: Vec<SortField> = pattern
                .return_spec
                .order_by
                .iter()
                .map(|(field, ascending)| SortField {
                    field_name: field.clone(),
                    ascending: *ascending,
                })
                .collect();

            let sort_step = PlanStep {
                step_type: PlanStepType::Sort {
                    fields: sort_fields,
                },
                parameters: HashMap::new(),
                cost: CostEstimate::new(
                    // O(n log n) sort cost
                    current_cardinality as f64
                        * (current_cardinality as f64).log2()
                        * self.config.cost_model.cpu_cost,
                    0.0,
                    0.0,
                ),
                output_cardinality: current_cardinality,
            };
            total_cost = total_cost.add(&sort_step.cost);
            steps.push(sort_step);
        }

        // Step 5: Handle projections
        if !pattern.return_spec.variables.is_empty() || !pattern.return_spec.projections.is_empty()
        {
            let project_step = PlanStep {
                step_type: PlanStepType::Project {
                    fields: pattern.return_spec.variables.clone(),
                },
                parameters: HashMap::new(),
                cost: CostEstimate::new(
                    current_cardinality as f64 * self.config.cost_model.cpu_cost * 0.1, // Projection is cheap
                    0.0,
                    0.0,
                ),
                output_cardinality: current_cardinality,
            };
            total_cost = total_cost.add(&project_step.cost);
            steps.push(project_step);
        }

        // Step 6: Handle LIMIT/SKIP
        if let Some(limit) = pattern.return_spec.limit {
            let skip = pattern.return_spec.skip.unwrap_or(0);
            let final_cardinality = limit.min(current_cardinality.saturating_sub(skip));

            let limit_step = PlanStep {
                step_type: PlanStepType::Limit {
                    count: limit,
                    offset: pattern.return_spec.skip,
                },
                parameters: HashMap::new(),
                cost: CostEstimate::zero(), // Limit is essentially free
                output_cardinality: final_cardinality,
            };
            total_cost = total_cost.add(&limit_step.cost);
            current_cardinality = final_cardinality;
            steps.push(limit_step);
        }

        Ok(QueryPlan {
            id: Uuid::new_v4().to_string(),
            steps,
            estimated_cost: total_cost,
            estimated_result_size: current_cardinality,
            created_at: std::time::SystemTime::now(),
        })
    }

    /// Plan node access with index selection
    fn plan_node_access(
        &self,
        node_pattern: &super::ast::NodePattern,
        cardinality: usize,
        stats: &GraphStatistics,
    ) -> QueryResult<(PlanStep, CostEstimate)> {
        let step_type;
        let cost;

        // Choose between index seek and full scan
        if self.config.optimizations.use_indexes && !node_pattern.labels.is_empty() {
            let label = &node_pattern.labels[0];
            let selectivity = if let Some(&count) = stats.label_selectivity.get(label) {
                if stats.node_count > 0 {
                    count as f64 / stats.node_count as f64
                } else {
                    0.1
                }
            } else {
                0.1
            };

            // Use index if selectivity < 30%
            if selectivity < 0.3 {
                step_type = PlanStepType::IndexSeek {
                    index_name: format!("label_index_{}", label),
                    key_value: serde_json::Value::String(label.clone()),
                };
                cost = CostEstimate::new(
                    self.config.cost_model.index_seek_cost,
                    0.0,
                    cardinality as f64 * self.config.cost_model.memory_cost_factor,
                );
            } else {
                step_type = PlanStepType::NodeScan {
                    labels: Some(node_pattern.labels.clone()),
                    property_filters: Vec::new(),
                };
                cost = CostEstimate::new(
                    stats.node_count as f64 * self.config.cost_model.node_access_cost,
                    0.0,
                    cardinality as f64 * self.config.cost_model.memory_cost_factor,
                );
            }
        } else {
            step_type = PlanStepType::NodeScan {
                labels: if !node_pattern.labels.is_empty() {
                    Some(node_pattern.labels.clone())
                } else {
                    None
                },
                property_filters: Vec::new(),
            };
            cost = CostEstimate::new(
                stats.node_count as f64 * self.config.cost_model.node_access_cost,
                0.0,
                cardinality as f64 * self.config.cost_model.memory_cost_factor,
            );
        }

        Ok((
            PlanStep {
                step_type,
                parameters: HashMap::new(),
                cost: cost.clone(),
                output_cardinality: cardinality,
            },
            cost,
        ))
    }

    /// Optimize edge join order using greedy algorithm
    fn optimize_edge_join_order(
        &self,
        edges: &[super::ast::EdgePattern],
        input_cardinality: usize,
        stats: &GraphStatistics,
        cost_estimator: &dyn CostEstimator,
    ) -> QueryResult<(Vec<PlanStep>, CostEstimate, usize)> {
        let mut remaining_edges: Vec<_> = edges.iter().enumerate().collect();
        let mut steps = Vec::new();
        let mut total_cost = CostEstimate::zero();
        let mut current_cardinality = input_cardinality;

        // Greedy algorithm: pick edge with lowest cost at each step
        while !remaining_edges.is_empty() {
            let mut best_idx = 0;
            let mut best_cost = f64::MAX;
            let mut best_output_cardinality = 0;

            // Evaluate cost of each remaining edge
            for (i, (_, edge)) in remaining_edges.iter().enumerate() {
                let edge_selectivity = cost_estimator.estimate_edge_selectivity(edge, stats);
                let edge_output = (current_cardinality as f64
                    * stats.avg_node_degree
                    * edge_selectivity)
                    .max(1.0) as usize;
                let edge_cost = cost_estimator.estimate_expand_cost(
                    current_cardinality,
                    stats.avg_node_degree * edge_selectivity,
                );

                if edge_cost < best_cost {
                    best_idx = i;
                    best_cost = edge_cost;
                    best_output_cardinality = edge_output;
                }
            }

            // Add best edge to plan
            let (_, best_edge) = remaining_edges.remove(best_idx);

            let edge_step = PlanStep {
                step_type: PlanStepType::Traverse {
                    algorithm: TraversalAlgorithm::BFS,
                    max_depth: Some(1), // Single hop
                    edge_filters: if !best_edge.edge_types.is_empty() {
                        vec![EdgeFilter {
                            edge_type: Some(best_edge.edge_types[0].clone()),
                            property_filters: Vec::new(),
                        }]
                    } else {
                        vec![]
                    },
                },
                parameters: HashMap::new(),
                cost: CostEstimate::new(best_cost, 0.0, 0.0),
                output_cardinality: best_output_cardinality,
            };

            total_cost = total_cost.add(&edge_step.cost);
            current_cardinality = best_output_cardinality;
            steps.push(edge_step);
        }

        Ok((steps, total_cost, current_cardinality))
    }

    /// Plan a pattern matching query (simplified for now)
    pub fn plan_pattern_match_query(
        &self,
        parameters: &HashMap<String, serde_json::Value>,
    ) -> QueryResult<QueryPlan> {
        // For now, assume the 'pattern' parameter contains the Cypher-like string
        let pattern_str = parameters
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                VectorDBError::InvalidInput(
                    "Missing 'pattern' parameter for pattern match query".to_string(),
                )
            })?;

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
            VectorDBError::Internal("Failed to acquire plan cache read lock".to_string())
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
            VectorDBError::Internal("Failed to acquire plan cache write lock".to_string())
        })?;

        // Remove expired plans if cache is full
        if cache.len() >= self.config.max_cached_plans {
            let now = Instant::now();
            let expired_keys: Vec<_> = cache
                .iter()
                .filter(|(_, cached)| {
                    now.duration_since(cached.last_accessed).as_secs()
                        > self.config.plan_cache_ttl_sec
                })
                .map(|(k, _)| k.clone())
                .collect();

            for key in expired_keys {
                cache.remove(&key);
            }

            // If still full, remove least recently used
            if cache.len() >= self.config.max_cached_plans
                && let Some((lru_key, _)) = cache
                    .iter()
                    .min_by_key(|(_, cached)| cached.last_accessed)
                    .map(|(k, v)| (k.clone(), v.clone()))
                {
                    cache.remove(&lru_key);
                }
        }

        cache.insert(
            cache_key,
            CachedPlan {
                plan: plan.clone(),
                access_count: 1,
                last_accessed: Instant::now(),
            },
        );

        Ok(())
    }

    /// Get query planner statistics
    pub fn get_statistics(&self) -> QueryResult<GraphStatistics> {
        let stats = self.stats.read().map_err(|_| {
            VectorDBError::Internal("Failed to acquire stats read lock".to_string())
        })?;

        Ok((*stats).clone())
    }

    /// Clear plan cache
    pub fn clear_cache(&self) -> QueryResult<()> {
        let mut cache = self.plan_cache.write().map_err(|_| {
            VectorDBError::Internal("Failed to acquire plan cache write lock".to_string())
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
    use crate::graph::query::ast::{
        CompiledPattern, EdgeDirection, EdgePattern, NodePattern, PropertyConstraint, ReturnSpec,
        WhereClause,
    };

    #[test]
    fn test_query_planner_creation() {
        let planner = QueryPlanner::new();
        let stats = planner
            .get_statistics()
            .expect("Failed to get statistics in test");
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
        params1.insert(
            "label".to_string(),
            serde_json::Value::String("Person".to_string()),
        );

        let mut params2 = HashMap::new();
        params2.insert(
            "label".to_string(),
            serde_json::Value::String("Person".to_string()),
        );

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
        planner
            .update_statistics(&memory_pool)
            .expect("Failed to update statistics in test");

        let stats = planner
            .get_statistics()
            .expect("Failed to get statistics in test");
        assert_eq!(stats.node_count, 0);
        assert_eq!(stats.edge_count, 0);
        assert_eq!(stats.avg_node_degree, 0.0);
    }

    #[test]
    fn test_plan_node_by_label_query() {
        let planner = QueryPlanner::new();

        let mut params = HashMap::new();
        params.insert(
            "label".to_string(),
            serde_json::Value::String("Person".to_string()),
        );

        let plan = planner
            .plan_node_by_label_query(&params)
            .expect("Failed to create plan in test");

        assert!(!plan.id.is_empty());
        assert_eq!(plan.steps.len(), 1);

        match &plan.steps[0].step_type {
            PlanStepType::NodeScan { labels, .. } => {
                assert_eq!(
                    labels
                        .as_ref()
                        .expect("Labels should be present in NodeScan")[0],
                    "Person"
                );
            }
            PlanStepType::IndexSeek { .. } => {
                // Also valid if index is chosen
            }
            _ => panic!("Unexpected plan step type"),
        }
    }

    // ===== Tests for CostEstimator trait implementation =====

    #[test]
    fn test_cost_estimator_scan_cost() {
        let cost_model = CostModel::default();
        let cardinality = 1000;

        let cost = cost_model.estimate_scan_cost(cardinality);

        // Should be cardinality * node_access_cost
        let expected = cardinality as f64 * cost_model.node_access_cost;
        assert_eq!(cost, expected);
        assert!(cost > 0.0);
    }

    #[test]
    fn test_cost_estimator_index_seek_cost() {
        let cost_model = CostModel::default();
        let cardinality = 100;

        let cost = cost_model.estimate_index_seek_cost(cardinality);

        // Should be fixed seek cost + cardinality * index_scan_cost
        let expected =
            cost_model.index_seek_cost + (cardinality as f64 * cost_model.index_scan_cost);
        assert_eq!(cost, expected);
        assert!(cost > cost_model.index_seek_cost);
    }

    #[test]
    fn test_cost_estimator_expand_cost() {
        let cost_model = CostModel::default();
        let input_card = 10;
        let avg_degree = 5.0;

        let cost = cost_model.estimate_expand_cost(input_card, avg_degree);

        // Should be input_card * avg_degree * edge_traversal_cost
        let expected = input_card as f64 * avg_degree * cost_model.edge_traversal_cost;
        assert_eq!(cost, expected);
        assert!(cost > 0.0);
    }

    #[test]
    fn test_estimate_node_selectivity_no_constraints() {
        let cost_model = CostModel::default();
        let mut stats = GraphStatistics::default();
        stats.node_count = 1000;

        let pattern = NodePattern {
            variable: "n".to_string(),
            labels: vec![],
            properties: HashMap::new(),
            optional: false,
        };

        let selectivity = cost_model.estimate_node_selectivity(&pattern, &stats);

        // No labels or properties = 100% selectivity
        assert_eq!(selectivity, 1.0);
    }

    #[test]
    fn test_estimate_node_selectivity_with_label() {
        let cost_model = CostModel::default();
        let mut stats = GraphStatistics::default();
        stats.node_count = 1000;
        stats.label_selectivity.insert("Person".to_string(), 300);

        let pattern = NodePattern {
            variable: "n".to_string(),
            labels: vec!["Person".to_string()],
            properties: HashMap::new(),
            optional: false,
        };

        let selectivity = cost_model.estimate_node_selectivity(&pattern, &stats);

        // Label selectivity = 300/1000 = 0.3
        assert_eq!(selectivity, 0.3);
    }

    #[test]
    fn test_estimate_node_selectivity_multiple_labels() {
        let cost_model = CostModel::default();
        let mut stats = GraphStatistics::default();
        stats.node_count = 1000;
        stats.label_selectivity.insert("Person".to_string(), 300);
        stats.label_selectivity.insert("Employee".to_string(), 200);

        let pattern = NodePattern {
            variable: "n".to_string(),
            labels: vec!["Person".to_string(), "Employee".to_string()],
            properties: HashMap::new(),
            optional: false,
        };

        let selectivity = cost_model.estimate_node_selectivity(&pattern, &stats);

        // Multiple labels: multiply selectivities (0.3 * 0.2 = 0.06), min 0.001
        assert_eq!(selectivity, 0.06);
    }

    #[test]
    fn test_estimate_node_selectivity_with_property_equals() {
        let cost_model = CostModel::default();
        let mut stats = GraphStatistics::default();
        stats.node_count = 1000;
        stats.property_selectivity.insert("name".to_string(), 500);

        let mut properties = HashMap::new();
        properties.insert(
            "name".to_string(),
            PropertyConstraint::Equals(serde_json::json!("Alice")),
        );

        let pattern = NodePattern {
            variable: "n".to_string(),
            labels: vec![],
            properties,
            optional: false,
        };

        let selectivity = cost_model.estimate_node_selectivity(&pattern, &stats);

        // Property equals selectivity = 1/500 = 0.002
        assert_eq!(selectivity, 0.002);
    }

    #[test]
    fn test_estimate_edge_selectivity_with_type() {
        let cost_model = CostModel::default();
        let mut stats = GraphStatistics::default();
        stats.edge_count = 5000;
        stats
            .edge_type_selectivity
            .insert("KNOWS".to_string(), 1500);

        let pattern = EdgePattern {
            variable: Some("r".to_string()),
            from_variable: "a".to_string(),
            to_variable: "b".to_string(),
            edge_types: vec!["KNOWS".to_string()],
            properties: HashMap::new(),
            direction: EdgeDirection::Outgoing,
            optional: false,
        };

        let selectivity = cost_model.estimate_edge_selectivity(&pattern, &stats);

        // Edge type selectivity = 1500/5000 = 0.3
        assert_eq!(selectivity, 0.3);
    }

    #[test]
    fn test_estimate_where_selectivity_equals() {
        let cost_model = CostModel::default();
        let mut stats = GraphStatistics::default();
        stats.property_selectivity.insert("age".to_string(), 100);

        let clause = WhereClause::Property {
            variable: "n".to_string(),
            property: "age".to_string(),
            constraint: PropertyConstraint::Equals(serde_json::json!(30)),
        };

        let selectivity = cost_model.estimate_where_selectivity(&clause, &stats);

        // Equals selectivity = 1/100 = 0.01
        assert_eq!(selectivity, 0.01);
    }

    #[test]
    fn test_estimate_where_selectivity_greater_than() {
        let cost_model = CostModel::default();
        let mut stats = GraphStatistics::default();
        // Add property to stats to trigger the 0.3 selectivity path
        stats.property_selectivity.insert("age".to_string(), 100);

        let clause = WhereClause::Property {
            variable: "n".to_string(),
            property: "age".to_string(),
            constraint: PropertyConstraint::GreaterThan(serde_json::json!(30)),
        };

        let selectivity = cost_model.estimate_where_selectivity(&clause, &stats);

        // Range predicates with known property return 0.3
        assert_eq!(selectivity, 0.3);
    }

    #[test]
    fn test_estimate_where_selectivity_and() {
        let cost_model = CostModel::default();
        let mut stats = GraphStatistics::default();
        stats.property_selectivity.insert("age".to_string(), 100);
        stats.property_selectivity.insert("name".to_string(), 200);

        let clause1 = WhereClause::Property {
            variable: "n".to_string(),
            property: "age".to_string(),
            constraint: PropertyConstraint::Equals(serde_json::json!(30)),
        };

        let clause2 = WhereClause::Property {
            variable: "n".to_string(),
            property: "name".to_string(),
            constraint: PropertyConstraint::Equals(serde_json::json!("Alice")),
        };

        let and_clause = WhereClause::And(Box::new(clause1), Box::new(clause2));

        let selectivity = cost_model.estimate_where_selectivity(&and_clause, &stats);

        // AND: multiply selectivities (1/100 * 1/200 = 0.00005)
        assert_eq!(selectivity, 0.00005);
    }

    #[test]
    fn test_estimate_where_selectivity_or() {
        let cost_model = CostModel::default();
        let mut stats = GraphStatistics::default();
        stats.property_selectivity.insert("age".to_string(), 100);
        stats.property_selectivity.insert("name".to_string(), 200);

        let clause1 = WhereClause::Property {
            variable: "n".to_string(),
            property: "age".to_string(),
            constraint: PropertyConstraint::Equals(serde_json::json!(30)),
        };

        let clause2 = WhereClause::Property {
            variable: "n".to_string(),
            property: "name".to_string(),
            constraint: PropertyConstraint::Equals(serde_json::json!("Alice")),
        };

        let or_clause = WhereClause::Or(Box::new(clause1), Box::new(clause2));

        let selectivity = cost_model.estimate_where_selectivity(&or_clause, &stats);

        // OR: P(A) + P(B) - P(A∩B) = 0.01 + 0.005 - (0.01 * 0.005) = 0.01495
        assert_eq!(selectivity, 0.01495);
    }

    #[test]
    fn test_estimate_where_selectivity_not() {
        let cost_model = CostModel::default();
        let mut stats = GraphStatistics::default();
        stats.property_selectivity.insert("age".to_string(), 100);

        let clause = WhereClause::Property {
            variable: "n".to_string(),
            property: "age".to_string(),
            constraint: PropertyConstraint::Equals(serde_json::json!(30)),
        };

        let not_clause = WhereClause::Not(Box::new(clause));

        let selectivity = cost_model.estimate_where_selectivity(&not_clause, &stats);

        // NOT: 1 - selectivity = 1 - 0.01 = 0.99
        assert_eq!(selectivity, 0.99);
    }

    #[test]
    fn test_estimate_where_selectivity_in_clause() {
        let cost_model = CostModel::default();
        let mut stats = GraphStatistics::default();
        stats.property_selectivity.insert("age".to_string(), 100);

        let clause = WhereClause::Property {
            variable: "n".to_string(),
            property: "age".to_string(),
            constraint: PropertyConstraint::In(vec![
                serde_json::json!(30),
                serde_json::json!(40),
                serde_json::json!(50),
            ]),
        };

        let selectivity = cost_model.estimate_where_selectivity(&clause, &stats);

        // IN: values.len() / distinct_values = 3/100 = 0.03
        assert_eq!(selectivity, 0.03);
    }

    #[test]
    fn test_estimate_pattern_cost() {
        let cost_model = CostModel::default();
        let mut stats = GraphStatistics::default();
        stats.node_count = 1000;
        stats.edge_count = 5000;
        stats.label_selectivity.insert("Person".to_string(), 300);
        stats
            .edge_type_selectivity
            .insert("KNOWS".to_string(), 1500);

        let node1 = NodePattern {
            variable: "a".to_string(),
            labels: vec!["Person".to_string()],
            properties: HashMap::new(),
            optional: false,
        };

        let node2 = NodePattern {
            variable: "b".to_string(),
            labels: vec!["Person".to_string()],
            properties: HashMap::new(),
            optional: false,
        };

        let edge = EdgePattern {
            variable: Some("r".to_string()),
            from_variable: "a".to_string(),
            to_variable: "b".to_string(),
            edge_types: vec!["KNOWS".to_string()],
            properties: HashMap::new(),
            direction: EdgeDirection::Outgoing,
            optional: false,
        };

        let pattern = CompiledPattern {
            nodes: vec![node1, node2],
            edges: vec![edge],
            paths: vec![],
            where_clauses: vec![],
            with_clauses: vec![], // TD-019: WITH clause support
            return_spec: ReturnSpec {
                variables: vec!["a".to_string(), "b".to_string()],
                projections: vec![],
                distinct: false,
                order_by: vec![],
                limit: None,
                skip: None,
            },
            variables: HashMap::new(),
        };

        let cost = cost_model.estimate_pattern_cost(&pattern, &stats);

        // Should be positive and account for nodes + edges
        assert!(cost > 0.0);
    }

    #[test]
    fn test_plan_pattern_query_single_node() {
        let planner = QueryPlanner::new();
        let mut stats = planner
            .stats
            .write()
            .expect("Stats lock should not be poisoned in test");
        stats.node_count = 1000;
        stats.label_selectivity.insert("Person".to_string(), 300);
        drop(stats);

        let pattern = CompiledPattern {
            nodes: vec![NodePattern {
                variable: "n".to_string(),
                labels: vec!["Person".to_string()],
                properties: HashMap::new(),
                optional: false,
            }],
            edges: vec![],
            paths: vec![],
            where_clauses: vec![],
            with_clauses: vec![],
            return_spec: ReturnSpec {
                variables: vec!["n".to_string()],
                projections: vec![],
                distinct: false,
                order_by: vec![],
                limit: None,
                skip: None,
            },
            variables: HashMap::new(),
        };

        let plan = planner
            .plan_pattern_query(&pattern)
            .expect("Failed to create plan in test");

        assert!(!plan.id.is_empty());
        assert!(!plan.steps.is_empty());
        assert!(plan.estimated_cost.total_cost > 0.0);
        assert!(plan.estimated_result_size > 0);
    }

    #[test]
    fn test_plan_pattern_query_with_edge() {
        let planner = QueryPlanner::new();
        let mut stats = planner
            .stats
            .write()
            .expect("Stats lock should not be poisoned in test");
        stats.node_count = 1000;
        stats.edge_count = 5000;
        stats.avg_node_degree = 5.0;
        stats.label_selectivity.insert("Person".to_string(), 300);
        stats
            .edge_type_selectivity
            .insert("KNOWS".to_string(), 1500);
        drop(stats);

        let pattern = CompiledPattern {
            nodes: vec![
                NodePattern {
                    variable: "a".to_string(),
                    labels: vec!["Person".to_string()],
                    properties: HashMap::new(),
                    optional: false,
                },
                NodePattern {
                    variable: "b".to_string(),
                    labels: vec!["Person".to_string()],
                    properties: HashMap::new(),
                    optional: false,
                },
            ],
            edges: vec![EdgePattern {
                variable: Some("r".to_string()),
                from_variable: "a".to_string(),
                to_variable: "b".to_string(),
                edge_types: vec!["KNOWS".to_string()],
                properties: HashMap::new(),
                direction: EdgeDirection::Outgoing,
                optional: false,
            }],
            paths: vec![],
            where_clauses: vec![],
            with_clauses: vec![],
            return_spec: ReturnSpec {
                variables: vec!["a".to_string(), "b".to_string()],
                projections: vec![],
                distinct: false,
                order_by: vec![],
                limit: None,
                skip: None,
            },
            variables: HashMap::new(),
        };

        let plan = planner
            .plan_pattern_query(&pattern)
            .expect("Failed to create plan in test");

        assert!(!plan.id.is_empty());
        assert!(plan.steps.len() >= 2); // At least node access + expand
        assert!(plan.estimated_cost.total_cost > 0.0);
    }

    #[test]
    fn test_plan_pattern_query_with_where_clause() {
        let planner = QueryPlanner::new();
        let mut stats = planner
            .stats
            .write()
            .expect("Stats lock should not be poisoned in test");
        stats.node_count = 1000;
        stats.label_selectivity.insert("Person".to_string(), 300);
        stats.property_selectivity.insert("age".to_string(), 100);
        drop(stats);

        let pattern = CompiledPattern {
            nodes: vec![NodePattern {
                variable: "n".to_string(),
                labels: vec!["Person".to_string()],
                properties: HashMap::new(),
                optional: false,
            }],
            edges: vec![],
            paths: vec![],
            where_clauses: vec![WhereClause::Property {
                variable: "n".to_string(),
                property: "age".to_string(),
                constraint: PropertyConstraint::GreaterThan(serde_json::json!(30)),
            }],
            with_clauses: vec![],
            return_spec: ReturnSpec {
                variables: vec!["n".to_string()],
                projections: vec![],
                distinct: false,
                order_by: vec![],
                limit: None,
                skip: None,
            },
            variables: HashMap::new(),
        };

        let plan = planner
            .plan_pattern_query(&pattern)
            .expect("Failed to create plan in test");

        assert!(!plan.id.is_empty());
        // Should include filter step for WHERE clause
        let has_filter = plan
            .steps
            .iter()
            .any(|step| matches!(step.step_type, PlanStepType::Filter { .. }));
        assert!(has_filter);
    }

    #[test]
    fn test_plan_pattern_query_with_order_by() {
        let planner = QueryPlanner::new();
        let mut stats = planner
            .stats
            .write()
            .expect("Stats lock should not be poisoned in test");
        stats.node_count = 1000;
        stats.label_selectivity.insert("Person".to_string(), 300);
        drop(stats);

        let pattern = CompiledPattern {
            nodes: vec![NodePattern {
                variable: "n".to_string(),
                labels: vec!["Person".to_string()],
                properties: HashMap::new(),
                optional: false,
            }],
            edges: vec![],
            paths: vec![],
            where_clauses: vec![],
            with_clauses: vec![],
            return_spec: ReturnSpec {
                variables: vec!["n".to_string()],
                projections: vec![],
                distinct: false,
                order_by: vec![("n".to_string(), true)],
                limit: None,
                skip: None,
            },
            variables: HashMap::new(),
        };

        let plan = planner
            .plan_pattern_query(&pattern)
            .expect("Failed to create plan in test");

        assert!(!plan.id.is_empty());
        // Should include sort step for ORDER BY
        let has_sort = plan
            .steps
            .iter()
            .any(|step| matches!(step.step_type, PlanStepType::Sort { .. }));
        assert!(has_sort);
    }

    #[test]
    fn test_plan_pattern_query_with_limit() {
        let planner = QueryPlanner::new();
        let mut stats = planner
            .stats
            .write()
            .expect("Stats lock should not be poisoned in test");
        stats.node_count = 1000;
        stats.label_selectivity.insert("Person".to_string(), 300);
        drop(stats);

        let pattern = CompiledPattern {
            nodes: vec![NodePattern {
                variable: "n".to_string(),
                labels: vec!["Person".to_string()],
                properties: HashMap::new(),
                optional: false,
            }],
            edges: vec![],
            paths: vec![],
            where_clauses: vec![],
            with_clauses: vec![],
            return_spec: ReturnSpec {
                variables: vec!["n".to_string()],
                projections: vec![],
                distinct: false,
                order_by: vec![],
                limit: Some(10),
                skip: None,
            },
            variables: HashMap::new(),
        };

        let plan = planner
            .plan_pattern_query(&pattern)
            .expect("Failed to create plan in test");

        assert!(!plan.id.is_empty());
        // Estimated result size should respect limit
        assert!(plan.estimated_result_size <= 10);
    }
}
