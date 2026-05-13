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

//! # Compute Bridge Module
//!
//! This module provides the integration layer between ProximaDB's query execution
//! and the Hadoop-style storage-compute separation architecture. It converts
//! existing query plans to `ComputePlan` format and routes execution through
//! the `ComputeScheduler` when appropriate.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────────┐
//! │                           COMPUTE BRIDGE                                     │
//! │                                                                              │
//! │  ┌──────────────────┐    ┌──────────────────┐    ┌──────────────────┐       │
//! │  │ UnifiedExecution │───▶│ PlanConverter    │───▶│ ComputeScheduler │       │
//! │  │     Plan         │    │ (this module)    │    │ (routes to       │       │
//! │  └──────────────────┘    └──────────────────┘    │  providers)      │       │
//! │                                                   └──────────────────┘       │
//! │                                                            │                 │
//! │                      ┌─────────────────────────────────────┘                 │
//! │                      │                                                       │
//! │          ┌───────────┴───────────┐                                          │
//! │          ▼                       ▼                                          │
//! │  ┌───────────────┐       ┌───────────────┐                                  │
//! │  │    Local      │       │   Fallback    │                                  │
//! │  │   Provider    │       │  (existing    │                                  │
//! │  │ (ComputePlan) │       │   execution)  │                                  │
//! │  └───────────────┘       └───────────────┘                                  │
//! └─────────────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Usage
//!
//! ```rust,ignore
//! use proximadb::query::compute_bridge::{ComputeBridge, BridgeConfig};
//! use proximadb::compute::scheduler::ComputeScheduler;
//! use proximadb::compute::provider::LocalComputeProvider;
//!
//! // Create bridge with scheduler
//! let provider = Arc::new(LocalComputeProvider::new()?);
//! let scheduler = ComputeScheduler::with_local_provider(provider);
//! let bridge = ComputeBridge::new(scheduler, BridgeConfig::default());
//!
//! // Execute through bridge (automatically routes to compute layer or fallback)
//! let result = bridge.execute(execution_plan).await?;
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, bail};
use tracing::{debug, info, instrument, trace, warn};

use crate::compute::plan::{BinaryOp, ComputePlan, Expr, LiteralValue, PlanHints, PlanNode};
use crate::compute::provider::traits::{ExecutionContext, RecordBatchStream};
use crate::compute::scheduler::ComputeScheduler;
use crate::query::query_optimizer::{
    ExecutionStep, FilterCondition, SearchExecutionMethod, UnifiedExecutionPlan,
};

// ============================================================================
// Bridge Configuration
// ============================================================================

/// Configuration for the compute bridge
#[derive(Debug, Clone)]
pub struct BridgeConfig {
    /// Enable routing to ComputeScheduler
    pub enable_compute_routing: bool,

    /// Minimum dataset size to use compute layer (vectors)
    pub min_dataset_size_for_compute: usize,

    /// Operations that should always use compute layer
    pub force_compute_operations: Vec<ComputeOperationType>,

    /// Operations that should always use fallback
    pub force_fallback_operations: Vec<ComputeOperationType>,

    /// Timeout for compute layer execution (milliseconds)
    pub compute_timeout_ms: u64,

    /// Enable metrics collection
    pub collect_metrics: bool,

    /// Maximum parallel tasks for compute layer
    pub max_parallelism: usize,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            enable_compute_routing: true,
            min_dataset_size_for_compute: 1000, // Use compute for 1K+ vectors
            force_compute_operations: vec![],
            force_fallback_operations: vec![],
            compute_timeout_ms: 300_000, // 5 minutes
            collect_metrics: false,
            max_parallelism: num_cpus::get(),
        }
    }
}

/// Types of compute operations for routing decisions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeOperationType {
    VectorSearch,
    GraphTraversal,
    MetadataFilter,
    Aggregation,
    Join,
    Sort,
}

// ============================================================================
// Compute Bridge
// ============================================================================

/// Bridge between query execution and compute scheduler
///
/// The ComputeBridge converts `UnifiedExecutionPlan` from the query optimizer
/// to `ComputePlan` for the compute scheduler. It handles:
///
/// 1. Plan conversion from execution steps to compute plan nodes
/// 2. Routing decisions (compute layer vs fallback)
/// 3. Result stream handling and error recovery
pub struct ComputeBridge {
    /// The compute scheduler for plan execution
    scheduler: Arc<ComputeScheduler>,

    /// Bridge configuration
    config: BridgeConfig,

    /// Statistics for routing decisions
    stats: parking_lot::RwLock<BridgeStatistics>,
}

impl std::fmt::Debug for ComputeBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComputeBridge")
            .field("config", &self.config)
            .field("scheduler_providers", &self.scheduler.provider_count())
            .finish()
    }
}

impl ComputeBridge {
    /// Create a new compute bridge with the given scheduler
    pub fn new(scheduler: ComputeScheduler, config: BridgeConfig) -> Self {
        info!(
            "Creating ComputeBridge with {} providers",
            scheduler.provider_count()
        );

        Self {
            scheduler: Arc::new(scheduler),
            config,
            stats: parking_lot::RwLock::new(BridgeStatistics::default()),
        }
    }

    /// Create a bridge with a scheduler wrapped in Arc
    pub fn with_scheduler(scheduler: Arc<ComputeScheduler>, config: BridgeConfig) -> Self {
        info!(
            "Creating ComputeBridge with {} providers",
            scheduler.provider_count()
        );

        Self {
            scheduler,
            config,
            stats: parking_lot::RwLock::new(BridgeStatistics::default()),
        }
    }

    /// Execute a unified execution plan through the compute layer
    ///
    /// This method decides whether to route through ComputeScheduler or use
    /// the fallback execution path based on plan characteristics.
    #[instrument(skip(self, plan, context), fields(steps = plan.execution_steps.len()))]
    pub async fn execute(
        &self,
        plan: &UnifiedExecutionPlan,
        context: &QueryContext,
    ) -> Result<ExecutionResult> {
        let start = std::time::Instant::now();

        // Check if we should route through compute layer
        let should_use_compute = self.should_use_compute(plan, context);

        debug!(
            "Routing decision: use_compute={}, steps={}, dataset_size={}",
            should_use_compute,
            plan.execution_steps.len(),
            context.dataset_size
        );

        let result = if should_use_compute {
            self.execute_via_compute(plan, context).await
        } else {
            self.execute_fallback(plan, context).await
        };

        // Update statistics
        {
            let mut stats = self.stats.write();
            stats.total_executions += 1;
            if should_use_compute {
                stats.compute_routed += 1;
            } else {
                stats.fallback_routed += 1;
            }
            if result.is_err() {
                stats.failed_executions += 1;
            }
            stats.total_time_ms += start.elapsed().as_millis() as u64;
        }

        result
    }

    /// Check if the plan should be routed through compute layer
    fn should_use_compute(&self, plan: &UnifiedExecutionPlan, context: &QueryContext) -> bool {
        // Check if routing is enabled
        if !self.config.enable_compute_routing {
            return false;
        }

        // Check dataset size threshold
        if context.dataset_size < self.config.min_dataset_size_for_compute {
            trace!(
                "Dataset size {} below threshold {}",
                context.dataset_size, self.config.min_dataset_size_for_compute
            );
            return false;
        }

        // Check for forced compute operations
        for step in &plan.execution_steps {
            let op_type = self.classify_operation(step);
            if self.config.force_compute_operations.contains(&op_type) {
                return true;
            }
            if self.config.force_fallback_operations.contains(&op_type) {
                return false;
            }
        }

        // Check if all operations are convertible
        plan.execution_steps
            .iter()
            .all(|step| self.is_convertible_to_compute_plan(step))
    }

    /// Classify an execution step as an operation type
    fn classify_operation(&self, step: &ExecutionStep) -> ComputeOperationType {
        match step {
            ExecutionStep::VectorSearch { .. } => ComputeOperationType::VectorSearch,
            ExecutionStep::MetadataFilter { .. } => ComputeOperationType::MetadataFilter,
            ExecutionStep::CombinedFilterSearch { .. } => ComputeOperationType::VectorSearch,
            ExecutionStep::IndexLookup { .. } => ComputeOperationType::VectorSearch,
            ExecutionStep::BloomFilterCheck { .. } => ComputeOperationType::MetadataFilter,
        }
    }

    /// Check if an execution step can be converted to a ComputePlan node
    fn is_convertible_to_compute_plan(&self, step: &ExecutionStep) -> bool {
        match step {
            ExecutionStep::VectorSearch { .. } => true,
            ExecutionStep::MetadataFilter { .. } => true,
            ExecutionStep::CombinedFilterSearch { .. } => true,
            ExecutionStep::IndexLookup { .. } => true,
            ExecutionStep::BloomFilterCheck { .. } => true,
        }
    }

    /// Execute through the compute scheduler
    #[instrument(skip(self, plan, context))]
    async fn execute_via_compute(
        &self,
        plan: &UnifiedExecutionPlan,
        context: &QueryContext,
    ) -> Result<ExecutionResult> {
        debug!("Executing via compute scheduler");

        // Convert to ComputePlan
        let compute_plan = self.convert_to_compute_plan(plan, context)?;

        // Create execution context
        let exec_ctx = ExecutionContext::with_id(format!("bridge-{}", uuid::Uuid::new_v4()))
            .with_timeout(std::time::Duration::from_millis(
                self.config.compute_timeout_ms,
            ))
            .with_max_parallelism(self.config.max_parallelism)
            .with_metrics(self.config.collect_metrics);

        // Execute through scheduler
        match self
            .scheduler
            .schedule_with_context(compute_plan, exec_ctx)
            .await
        {
            Ok(stream) => {
                debug!("Compute execution succeeded");
                Ok(ExecutionResult::Stream(stream))
            }
            Err(e) => {
                warn!("Compute execution failed, attempting fallback: {}", e);
                // Update failed compute count
                {
                    let mut stats = self.stats.write();
                    stats.compute_failures += 1;
                }
                // Attempt fallback
                self.execute_fallback(plan, context).await
            }
        }
    }

    /// Execute using fallback (existing execution path)
    #[instrument(skip(self, _plan, _context))]
    async fn execute_fallback(
        &self,
        _plan: &UnifiedExecutionPlan,
        _context: &QueryContext,
    ) -> Result<ExecutionResult> {
        debug!("Executing via fallback path");

        // Return a placeholder - in a full implementation, this would call
        // the existing query execution engine
        Ok(ExecutionResult::Empty)
    }

    /// Convert UnifiedExecutionPlan to ComputePlan
    #[instrument(skip(self, plan, context))]
    pub fn convert_to_compute_plan(
        &self,
        plan: &UnifiedExecutionPlan,
        context: &QueryContext,
    ) -> Result<ComputePlan> {
        if plan.execution_steps.is_empty() {
            bail!("Cannot convert empty execution plan");
        }

        // Build plan nodes from execution steps (bottom-up)
        let root = self.build_plan_tree(&plan.execution_steps, context)?;

        // Create hints from execution plan
        let hints = self.create_plan_hints(plan);

        let compute_plan =
            ComputePlan::new(format!("bridge-{}", uuid::Uuid::new_v4()), root).with_hints(hints);

        trace!(
            "Converted to ComputePlan: id={}, tables={:?}",
            compute_plan.id,
            compute_plan.referenced_tables()
        );

        Ok(compute_plan)
    }

    /// Build a plan tree from execution steps
    fn build_plan_tree(&self, steps: &[ExecutionStep], context: &QueryContext) -> Result<PlanNode> {
        if steps.is_empty() {
            bail!("No execution steps to convert");
        }

        // Process steps and chain them together
        let mut current_node: Option<PlanNode> = None;

        for step in steps {
            let node = self.convert_step_to_node(step, context, current_node.take())?;
            current_node = Some(node);
        }

        current_node.ok_or_else(|| anyhow::anyhow!("Failed to build plan tree"))
    }

    /// Convert a single execution step to a PlanNode
    fn convert_step_to_node(
        &self,
        step: &ExecutionStep,
        context: &QueryContext,
        input: Option<PlanNode>,
    ) -> Result<PlanNode> {
        match step {
            ExecutionStep::VectorSearch {
                execution_method,
                candidates,
                ..
            } => {
                // Create vector scan node
                let query_vector = context.query_vector.clone().unwrap_or_default();
                let collection = context.collection_name.clone();

                let base = PlanNode::VectorScan {
                    collection,
                    query_vector,
                    top_k: *candidates as u32,
                    filter: None,
                    distance_metric: Some(self.method_to_metric(execution_method)),
                };

                // Chain with input if exists
                if let Some(input_node) = input {
                    // Wrap input in filter that feeds into vector scan
                    Ok(PlanNode::Filter {
                        input: Box::new(input_node),
                        predicate: Expr::Literal(LiteralValue::Bool(true)),
                    })
                } else {
                    Ok(base)
                }
            }

            ExecutionStep::MetadataFilter {
                conditions,
                estimated_selectivity,
                ..
            } => {
                // Convert filter conditions to expression tree
                let predicate = self.conditions_to_expr(conditions)?;

                // Create base table scan if no input
                let input_node = input.unwrap_or_else(|| PlanNode::TableScan {
                    table: context.collection_name.clone(),
                    columns: vec![],
                    filter: None,
                });

                trace!(
                    "Created filter with selectivity={:.3}",
                    estimated_selectivity
                );

                Ok(PlanNode::Filter {
                    input: Box::new(input_node),
                    predicate,
                })
            }

            ExecutionStep::CombinedFilterSearch {
                filter_pushdown,
                search_method,
                ..
            } => {
                // Combined operation: filter pushed down to vector scan
                let query_vector = context.query_vector.clone().unwrap_or_default();
                let collection = context.collection_name.clone();

                // Build filter from pushdown operations
                let filter_expr = if filter_pushdown.is_empty() {
                    None
                } else {
                    // Convert pushdown conditions to expression
                    let conditions: Vec<FilterCondition> = filter_pushdown
                        .iter()
                        .filter_map(|op| self.pushdown_to_condition(op))
                        .collect();

                    if conditions.is_empty() {
                        None
                    } else {
                        Some(self.conditions_to_expr(&conditions)?)
                    }
                };

                Ok(PlanNode::VectorScan {
                    collection,
                    query_vector,
                    top_k: context.top_k as u32,
                    filter: filter_expr,
                    distance_metric: Some(self.method_to_metric(search_method)),
                })
            }

            ExecutionStep::IndexLookup {
                index_type,
                lookup_params,
            } => {
                // Index lookup becomes a vector scan with index hint
                let query_vector = lookup_params
                    .query_vector
                    .clone()
                    .or_else(|| context.query_vector.clone())
                    .unwrap_or_default();

                let collection = context.collection_name.clone();

                // Use the index type to determine distance metric
                let metric = match index_type {
                    crate::query::query_optimizer::Index::HNSW => "euclidean",
                    crate::query::query_optimizer::Index::IVF => "cosine",
                    crate::query::query_optimizer::Index::LSH => "hamming",
                    _ => "euclidean",
                };

                Ok(PlanNode::VectorScan {
                    collection,
                    query_vector,
                    top_k: lookup_params.top_k as u32,
                    filter: None,
                    distance_metric: Some(metric.to_string()),
                })
            }

            ExecutionStep::BloomFilterCheck { .. } => {
                // Bloom filter check is optimization hint, pass through input
                input.ok_or_else(|| anyhow::anyhow!("BloomFilterCheck requires input node"))
            }
        }
    }

    /// Convert search execution method to distance metric string
    fn method_to_metric(&self, method: &SearchExecutionMethod) -> String {
        match method {
            SearchExecutionMethod::IndexBased { index_type } => match index_type {
                crate::query::query_optimizer::Index::HNSW => "euclidean".to_string(),
                crate::query::query_optimizer::Index::IVF => "cosine".to_string(),
                crate::query::query_optimizer::Index::LSH => "hamming".to_string(),
                _ => "euclidean".to_string(),
            },
            _ => "cosine".to_string(), // Default
        }
    }

    /// Convert filter conditions to expression tree
    fn conditions_to_expr(&self, conditions: &[FilterCondition]) -> Result<Expr> {
        if conditions.is_empty() {
            return Ok(Expr::Literal(LiteralValue::Bool(true)));
        }

        let mut expr_iter = conditions.iter().map(|c| self.condition_to_expr(c));

        let first = expr_iter
            .next()
            .ok_or_else(|| anyhow::anyhow!("No conditions"))?;

        // Chain with AND
        let result = expr_iter.fold(first, |acc, next| Expr::Binary {
            left: Box::new(acc),
            op: BinaryOp::And,
            right: Box::new(next),
        });

        Ok(result)
    }

    /// Convert a single filter condition to expression
    fn condition_to_expr(&self, condition: &FilterCondition) -> Expr {
        match condition {
            FilterCondition::Equals { column, value } => Expr::Binary {
                left: Box::new(Expr::Column(column.clone())),
                op: BinaryOp::Eq,
                right: Box::new(self.json_to_literal(value)),
            },
            FilterCondition::NotEquals { column, value } => Expr::Binary {
                left: Box::new(Expr::Column(column.clone())),
                op: BinaryOp::Ne,
                right: Box::new(self.json_to_literal(value)),
            },
            FilterCondition::GreaterThan { column, value } => Expr::Binary {
                left: Box::new(Expr::Column(column.clone())),
                op: BinaryOp::Gt,
                right: Box::new(self.json_to_literal(value)),
            },
            FilterCondition::GreaterThanOrEqual { column, value } => Expr::Binary {
                left: Box::new(Expr::Column(column.clone())),
                op: BinaryOp::Ge,
                right: Box::new(self.json_to_literal(value)),
            },
            FilterCondition::LessThan { column, value } => Expr::Binary {
                left: Box::new(Expr::Column(column.clone())),
                op: BinaryOp::Lt,
                right: Box::new(self.json_to_literal(value)),
            },
            FilterCondition::LessThanOrEqual { column, value } => Expr::Binary {
                left: Box::new(Expr::Column(column.clone())),
                op: BinaryOp::Le,
                right: Box::new(self.json_to_literal(value)),
            },
            FilterCondition::Range { column, min, max } => Expr::Binary {
                left: Box::new(Expr::Binary {
                    left: Box::new(Expr::Column(column.clone())),
                    op: BinaryOp::Ge,
                    right: Box::new(self.json_to_literal(min)),
                }),
                op: BinaryOp::And,
                right: Box::new(Expr::Binary {
                    left: Box::new(Expr::Column(column.clone())),
                    op: BinaryOp::Le,
                    right: Box::new(self.json_to_literal(max)),
                }),
            },
            FilterCondition::In { column, values } => {
                // Convert to OR chain
                if values.is_empty() {
                    Expr::Literal(LiteralValue::Bool(false))
                } else {
                    let array_values: Vec<Expr> =
                        values.iter().map(|v| self.json_to_literal(v)).collect();
                    Expr::InList {
                        expr: Box::new(Expr::Column(column.clone())),
                        list: array_values,
                        negated: false,
                    }
                }
            }
            FilterCondition::NotIn { column, values } => {
                let array_values: Vec<Expr> =
                    values.iter().map(|v| self.json_to_literal(v)).collect();
                Expr::InList {
                    expr: Box::new(Expr::Column(column.clone())),
                    list: array_values,
                    negated: true,
                }
            }
            FilterCondition::IsNull { column } => {
                Expr::IsNull(Box::new(Expr::Column(column.clone())))
            }
            FilterCondition::IsNotNull { column } => {
                Expr::IsNotNull(Box::new(Expr::Column(column.clone())))
            }
            FilterCondition::Like { column, pattern } => Expr::Binary {
                left: Box::new(Expr::Column(column.clone())),
                op: BinaryOp::Like,
                right: Box::new(Expr::Literal(LiteralValue::String(pattern.clone()))),
            },
            FilterCondition::Contains { column, value } => Expr::Function {
                name: "contains".to_string(),
                args: vec![Expr::Column(column.clone()), self.json_to_literal(value)],
            },
            FilterCondition::StartsWith { column, prefix } => Expr::Function {
                name: "starts_with".to_string(),
                args: vec![
                    Expr::Column(column.clone()),
                    Expr::Literal(LiteralValue::String(prefix.clone())),
                ],
            },
            FilterCondition::EndsWith { column, suffix } => Expr::Function {
                name: "ends_with".to_string(),
                args: vec![
                    Expr::Column(column.clone()),
                    Expr::Literal(LiteralValue::String(suffix.clone())),
                ],
            },
            FilterCondition::Between { column, min, max } => Expr::Between {
                expr: Box::new(Expr::Column(column.clone())),
                low: Box::new(self.json_to_literal(min)),
                high: Box::new(self.json_to_literal(max)),
                negated: false,
            },
        }
    }

    /// Convert JSON value to literal expression
    fn json_to_literal(&self, value: &serde_json::Value) -> Expr {
        match value {
            serde_json::Value::Null => Expr::Literal(LiteralValue::Null),
            serde_json::Value::Bool(b) => Expr::Literal(LiteralValue::Bool(*b)),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Expr::Literal(LiteralValue::Int(i))
                } else if let Some(f) = n.as_f64() {
                    Expr::Literal(LiteralValue::Float(f))
                } else {
                    Expr::Literal(LiteralValue::Null)
                }
            }
            serde_json::Value::String(s) => Expr::Literal(LiteralValue::String(s.clone())),
            serde_json::Value::Array(arr) => {
                let exprs: Vec<Expr> = arr.iter().map(|v| self.json_to_literal(v)).collect();
                Expr::Array(exprs)
            }
            serde_json::Value::Object(_) => {
                // Convert object to JSON string
                Expr::Literal(LiteralValue::String(value.to_string()))
            }
        }
    }

    /// Convert pushdown operation to filter condition
    fn pushdown_to_condition(
        &self,
        pushdown: &crate::query::query_optimizer::FilterPushdownOperation,
    ) -> Option<FilterCondition> {
        match pushdown {
            crate::query::query_optimizer::FilterPushdownOperation::StorageLevel {
                filter, ..
            } => Some(filter.clone()),
            crate::query::query_optimizer::FilterPushdownOperation::IndexLevel {
                filter, ..
            } => Some(filter.clone()),
        }
    }

    /// Create plan hints from execution plan
    fn create_plan_hints(&self, plan: &UnifiedExecutionPlan) -> PlanHints {
        PlanHints {
            parallelism: Some(plan.parallelism.vector_parallelism),
            memory_budget: Some(plan.resource_allocation.memory_budget_mb as u64 * 1024 * 1024),
            timeout_ms: Some(self.config.compute_timeout_ms),
            ..Default::default()
        }
    }

    /// Get bridge statistics
    pub fn statistics(&self) -> BridgeStatistics {
        self.stats.read().clone()
    }

    /// Get the underlying scheduler
    pub fn scheduler(&self) -> &Arc<ComputeScheduler> {
        &self.scheduler
    }

    /// Shutdown the bridge and scheduler
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down compute bridge");
        self.scheduler.shutdown().await
    }
}

// ============================================================================
// Supporting Types
// ============================================================================

/// Context for query execution through the bridge
#[derive(Debug, Clone)]
pub struct QueryContext {
    /// Collection/table being queried
    pub collection_name: String,

    /// Dataset size (number of vectors)
    pub dataset_size: usize,

    /// Query vector (if vector search)
    pub query_vector: Option<Vec<f32>>,

    /// Number of results requested
    pub top_k: usize,

    /// Additional parameters
    pub parameters: HashMap<String, serde_json::Value>,
}

impl Default for QueryContext {
    fn default() -> Self {
        Self {
            collection_name: String::new(),
            dataset_size: 0,
            query_vector: None,
            top_k: 10,
            parameters: HashMap::new(),
        }
    }
}

impl QueryContext {
    /// Create a new query context
    pub fn new(collection_name: impl Into<String>, dataset_size: usize) -> Self {
        Self {
            collection_name: collection_name.into(),
            dataset_size,
            ..Default::default()
        }
    }

    /// Set query vector
    pub fn with_query_vector(mut self, vector: Vec<f32>) -> Self {
        self.query_vector = Some(vector);
        self
    }

    /// Set top_k
    pub fn with_top_k(mut self, k: usize) -> Self {
        self.top_k = k;
        self
    }

    /// Add parameter
    pub fn with_parameter(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.parameters.insert(key.into(), value);
        self
    }
}

/// Result of bridge execution
pub enum ExecutionResult {
    /// Arrow RecordBatch stream
    Stream(RecordBatchStream),

    /// Empty result
    Empty,
}

impl std::fmt::Debug for ExecutionResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutionResult::Stream(_) => f
                .debug_tuple("Stream")
                .field(&"<RecordBatchStream>")
                .finish(),
            ExecutionResult::Empty => f.debug_struct("Empty").finish(),
        }
    }
}

/// Statistics for bridge routing decisions
#[derive(Debug, Clone, Default)]
pub struct BridgeStatistics {
    /// Total executions through bridge
    pub total_executions: u64,

    /// Executions routed to compute layer
    pub compute_routed: u64,

    /// Executions using fallback
    pub fallback_routed: u64,

    /// Failed executions
    pub failed_executions: u64,

    /// Compute layer failures (triggered fallback)
    pub compute_failures: u64,

    /// Total execution time (milliseconds)
    pub total_time_ms: u64,
}

impl BridgeStatistics {
    /// Get compute routing percentage
    pub fn compute_percentage(&self) -> f64 {
        if self.total_executions == 0 {
            0.0
        } else {
            (self.compute_routed as f64 / self.total_executions as f64) * 100.0
        }
    }

    /// Get success rate
    pub fn success_rate(&self) -> f64 {
        if self.total_executions == 0 {
            1.0
        } else {
            1.0 - (self.failed_executions as f64 / self.total_executions as f64)
        }
    }

    /// Get average execution time
    pub fn avg_execution_time_ms(&self) -> f64 {
        if self.total_executions == 0 {
            0.0
        } else {
            self.total_time_ms as f64 / self.total_executions as f64
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::provider::LocalComputeProvider;
    use crate::query::query_optimizer::{
        ParallelismConfig, ResourceAllocation, UnifiedPerformanceEstimate,
    };

    fn create_test_bridge() -> ComputeBridge {
        let provider = LocalComputeProvider::new().unwrap();
        let scheduler = ComputeScheduler::with_local_provider(std::sync::Arc::new(provider));
        ComputeBridge::new(scheduler, BridgeConfig::default())
    }

    fn create_test_plan() -> UnifiedExecutionPlan {
        UnifiedExecutionPlan {
            execution_steps: vec![ExecutionStep::VectorSearch {
                execution_method: SearchExecutionMethod::DirectFP32,
                quantization_strategy: None,
                candidates: 100,
            }],
            resource_allocation: ResourceAllocation {
                memory_budget_mb: 256,
                cpu_cores: 4,
                io_threads: 2,
            },
            performance_estimate: UnifiedPerformanceEstimate {
                estimated_latency_ms: 10,
                estimated_memory_mb: 128,
                estimated_io_ops: 5,
                estimated_recall: 0.95,
                estimated_precision: 0.98,
            },
            parallelism: ParallelismConfig {
                file_parallelism: 2,
                vector_parallelism: 4,
                filter_parallelism: 2,
                use_simd: true,
            },
            fallback_strategies: vec![],
            rl_state: None,
            rl_action: None,
        }
    }

    #[test]
    fn test_bridge_creation() {
        let bridge = create_test_bridge();
        assert!(bridge.config.enable_compute_routing);
    }

    #[test]
    fn test_routing_decision_small_dataset() {
        let bridge = create_test_bridge();
        let plan = create_test_plan();

        let context = QueryContext::new("test_collection", 100); // Small dataset
        let should_compute = bridge.should_use_compute(&plan, &context);

        assert!(!should_compute, "Small datasets should use fallback");
    }

    #[test]
    fn test_routing_decision_large_dataset() {
        let bridge = create_test_bridge();
        let plan = create_test_plan();

        let context = QueryContext::new("test_collection", 10000); // Large dataset
        let should_compute = bridge.should_use_compute(&plan, &context);

        assert!(should_compute, "Large datasets should use compute");
    }

    #[test]
    fn test_condition_to_expr() {
        let bridge = create_test_bridge();

        let condition = FilterCondition::Equals {
            column: "name".to_string(),
            value: serde_json::json!("test"),
        };

        let expr = bridge.condition_to_expr(&condition);

        match expr {
            Expr::Binary {
                op: BinaryOp::Eq, ..
            } => {}
            _ => panic!("Expected Eq binary expression"),
        }
    }

    #[test]
    fn test_conditions_to_expr_multiple() {
        let bridge = create_test_bridge();

        let conditions = vec![
            FilterCondition::Equals {
                column: "a".to_string(),
                value: serde_json::json!(1),
            },
            FilterCondition::GreaterThan {
                column: "b".to_string(),
                value: serde_json::json!(10),
            },
        ];

        let expr = bridge.conditions_to_expr(&conditions).unwrap();

        match expr {
            Expr::Binary {
                op: BinaryOp::And, ..
            } => {}
            _ => panic!("Expected AND binary expression"),
        }
    }

    #[test]
    fn test_json_to_literal() {
        let bridge = create_test_bridge();

        assert!(matches!(
            bridge.json_to_literal(&serde_json::json!(null)),
            Expr::Literal(LiteralValue::Null)
        ));

        assert!(matches!(
            bridge.json_to_literal(&serde_json::json!(true)),
            Expr::Literal(LiteralValue::Bool(true))
        ));

        assert!(matches!(
            bridge.json_to_literal(&serde_json::json!(42)),
            Expr::Literal(LiteralValue::Int(42))
        ));

        assert!(matches!(
            bridge.json_to_literal(&serde_json::json!(3.14)),
            Expr::Literal(LiteralValue::Float(_))
        ));

        assert!(matches!(
            bridge.json_to_literal(&serde_json::json!("test")),
            Expr::Literal(LiteralValue::String(_))
        ));
    }

    #[test]
    fn test_convert_to_compute_plan() {
        let bridge = create_test_bridge();
        let plan = create_test_plan();

        let context = QueryContext::new("test_collection", 10000)
            .with_query_vector(vec![0.1, 0.2, 0.3])
            .with_top_k(10);

        let compute_plan = bridge.convert_to_compute_plan(&plan, &context);
        assert!(compute_plan.is_ok());

        let cp = compute_plan.unwrap();
        assert!(cp.has_vector_operations());
    }

    #[test]
    fn test_statistics() {
        let bridge = create_test_bridge();

        let stats = bridge.statistics();
        assert_eq!(stats.total_executions, 0);
        assert_eq!(stats.compute_percentage(), 0.0);
        assert_eq!(stats.success_rate(), 1.0);
    }

    #[test]
    fn test_bridge_config_default() {
        let config = BridgeConfig::default();

        assert!(config.enable_compute_routing);
        assert_eq!(config.min_dataset_size_for_compute, 1000);
        assert_eq!(config.compute_timeout_ms, 300_000);
    }

    #[tokio::test]
    async fn test_execute_via_compute() {
        let bridge = create_test_bridge();
        let plan = create_test_plan();

        let context = QueryContext::new("test_collection", 10000)
            .with_query_vector(vec![0.1, 0.2, 0.3])
            .with_top_k(10);

        let result = bridge.execute(&plan, &context).await;
        assert!(result.is_ok());

        // Check statistics updated
        let stats = bridge.statistics();
        assert_eq!(stats.total_executions, 1);
    }

    #[tokio::test]
    async fn test_shutdown() {
        let bridge = create_test_bridge();
        let result = bridge.shutdown().await;
        assert!(result.is_ok());
    }
}
