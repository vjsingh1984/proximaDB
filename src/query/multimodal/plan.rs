//! MultiModelPlan v1 - Unified Query Contract for Vectorized Cross-Model Execution
//!
//! This module implements the MultiModelPlan contract that enables unified query
//! execution across all ProximaDB storage engines (SST, HELIX, VIPER, SWIFT, NOVA, RAPTOR).
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                    MultiModelPlan v1                        │
//! │  - Unified operator contract for all storage engines        │
//! │  - Zero-copy operations with selection vectors             │
//! │  - Cross-model joins and federated aggregation            │
//! └──────────────────────┬────────────────────────────────────────┘
//!                        │
//!                        ▼
//!     ┌─────────────────────────────────────────┐
//!     │         Operator Pipeline                │
//!     ├─────────────────────────────────────────┤
//!     │ Scan → Filter → Project → Join → Agg    │
//!     │         ↓         ↓         ↓         ↓  │
//!     │    Selection vectors (zero-copy)        │
//!     └─────────────────────────────────────────┘
//!                        │
//!                        ▼
//!     ┌─────────────────────────────────────────┐
//!     │      Storage Engine Dispatch            │
//!     ├─────────────────────────────────────────┤
//!     │ SST │ HELIX │ VIPER │ SWIFT │ NOVA │... │
//!     └─────────────────────────────────────────┘
//! ```
//!
//! ## Key Features
//!
//! - **Unified Operators**: Scan, Filter, Project, Join, Aggregate, Sort, TopK
//! - **Zero-Copy**: Selection vectors enable efficient operator chaining
//! - **Cross-Model**: Join data from different storage engines
//! - **Pushdown**: Filter and projection pushdown to storage engines
//! - **Vectorized**: All operators use Arrow compute kernels
//!
//! ## Design Principles
//!
//! 1. **Composability**: Operators can be combined in any order
//! 2. **Zero-Copy**: Selection vectors avoid row copying
//! 3. **Storage Agnostic**: Same operators work for all engines
//! 4. **Extensible**: Easy to add new operators and optimizations
//! 5. **Serializable**: Plans can be serialized for distributed execution

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, trace};

use crate::compute::pipeline_executor::PipelineOperator as ComputeOperator;
use crate::core::search::{FilterExpression, SearchParams};
use crate::proto::proximadb_v1::Collection;
use crate::query::unified::ast::DataModel;

/// MultiModelPlan v1 - Unified query execution plan
///
/// Represents a complete query execution plan that can be executed
/// across multiple storage engines with vectorized operators.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiModelPlan {
    /// Unique plan identifier
    pub plan_id: String,

    /// Plan version for serialization compatibility
    pub version: u32,

    /// Pipeline of operators to execute
    pub operators: Vec<Operator>,

    /// Execution context and metadata
    pub context: PlanContext,

    /// Optimization hints
    pub hints: PlanHints,
}

impl MultiModelPlan {
    /// Create a new MultiModelPlan
    pub fn new(operators: Vec<Operator>, context: PlanContext) -> Self {
        let plan_id = format!("plan_{}", uuid::Uuid::new_v4());

        Self {
            plan_id,
            version: 1,
            operators,
            context,
            hints: PlanHints::default(),
        }
    }

    /// Get the number of operators in the plan
    pub fn len(&self) -> usize {
        self.operators.len()
    }

    /// Check if the plan is empty
    pub fn is_empty(&self) -> bool {
        self.operators.is_empty()
    }

    /// Add an operator to the end of the pipeline
    pub fn add_operator(&mut self, operator: Operator) {
        self.operators.push(operator);
    }

    /// Get plan statistics
    pub fn stats(&self) -> PlanStats {
        let mut stats = PlanStats::default();

        for operator in &self.operators {
            match operator {
                Operator::Scan { .. } => stats.scan_count += 1,
                Operator::Filter { .. } => stats.filter_count += 1,
                Operator::Project { .. } => stats.project_count += 1,
                Operator::Join { .. } => stats.join_count += 1,
                Operator::Aggregate { .. } => stats.aggregate_count += 1,
                Operator::Sort { .. } => stats.sort_count += 1,
                Operator::TopK { .. } => stats.topk_count += 1,
                Operator::Union { .. } => stats.union_count += 1,
            }
        }

        stats.operator_count = self.operators.len();
        stats
    }

    /// Validate the plan for correctness
    pub fn validate(&self) -> Result<PlanValidationResult> {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();

        // Check if plan is empty
        if self.is_empty() {
            warnings.push("Empty plan - no operators to execute".to_string());
        }

        // Validate operator sequence
        let mut has_scan = false;

        for (idx, operator) in self.operators.iter().enumerate() {
            match operator {
                Operator::Scan { source, .. } => {
                    has_scan = true;
                    if source.is_empty() {
                        errors.push(format!("Operator {}: Scan has empty source", idx));
                    }
                }
                Operator::Join { .. } => {
                    if !has_scan {
                        errors.push(format!("Operator {}: Join before any Scan operator", idx));
                    }
                }
                Operator::Aggregate { .. } => {
                    // Aggregate validation - currently no specific rules
                }
                Operator::Filter { expression } => {
                    // Validate filter expression
                    if let Err(e) = self.validate_filter_expression(expression) {
                        errors.push(format!("Operator {}: Invalid filter: {}", idx, e));
                    }
                }
                Operator::Project { columns } => {
                    if columns.is_empty() {
                        warnings.push(format!("Operator {}: Project with no columns", idx));
                    }
                }
                _ => {}
            }
        }

        // Check if plan has at least one scan
        if !has_scan && !self.is_empty() {
            warnings.push("Plan has no Scan operator - may not produce results".to_string());
        }

        // Check for invalid operator sequences
        for (idx, (current, next)) in self
            .operators
            .iter()
            .zip(self.operators.iter().skip(1))
            .enumerate()
        {
            // Check if aggregate comes after join (potentially inefficient)
            if matches!(current, Operator::Join { .. })
                && matches!(next, Operator::Aggregate { .. })
            {
                warnings.push(format!(
                    "Operator {}: Aggregate immediately after Join - consider reordering",
                    idx
                ));
            }
        }

        Ok(PlanValidationResult {
            is_valid: errors.is_empty(),
            errors,
            warnings,
        })
    }

    /// Validate a filter expression
    fn validate_filter_expression(&self, expression: &FilterExpression) -> Result<()> {
        use crate::core::search::FilterExpression::*;

        match expression {
            Comparison {
                field,
                operator,
                value,
            } => {
                if field.is_empty() {
                    return Err(anyhow::anyhow!("Filter has empty field name"));
                }
                // Validate value based on operator
                match operator {
                    crate::core::search::ComparisonOperator::In => {
                        if let Some(arr) = value.as_array() {
                            if arr.is_empty() {
                                return Err(anyhow::anyhow!("IN filter has empty array"));
                            }
                        }
                    }
                    crate::core::search::ComparisonOperator::Between => {
                        if let Some(arr) = value.as_array() {
                            if arr.len() != 2 {
                                return Err(anyhow::anyhow!(
                                    "BETWEEN filter requires exactly 2 values"
                                ));
                            }
                        }
                    }
                    _ => {}
                }
                Ok(())
            }
            And(exprs) | Or(exprs) => {
                for expr in exprs {
                    self.validate_filter_expression(expr)?;
                }
                Ok(())
            }
            Not(expr) => self.validate_filter_expression(expr),
        }
    }

    /// Optimize the plan
    ///
    /// Applies various optimization passes to improve performance:
    /// - Filter pushdown to storage engines
    /// - Projection pushdown to reduce data transfer
    /// - Operator reordering for efficiency
    pub fn optimize(&mut self) -> Result<PlanOptimizationResult> {
        let mut optimizations_applied = Vec::new();
        let original_stats = self.stats();

        // Optimization 1: Filter pushdown
        let filters_pushed = self.pushdown_filters();
        if filters_pushed > 0 {
            optimizations_applied.push(format!("Pushed down {} filters", filters_pushed));
        }

        // Optimization 2: Projection pushdown
        let projections_pushed = self.pushdown_projections();
        if projections_pushed > 0 {
            optimizations_applied.push(format!("Pushed down {} projections", projections_pushed));
        }

        // Optimization 3: Operator reordering
        let reordered = self.reorder_operators()?;
        if reordered {
            optimizations_applied.push("Reordered operators for efficiency".to_string());
        }

        let optimized_stats = self.stats();

        Ok(PlanOptimizationResult {
            optimizations_applied,
            original_stats,
            optimized_stats,
        })
    }

    /// Push down filters to storage engines
    fn pushdown_filters(&mut self) -> usize {
        let pushdown_count = 0;

        // For now, this is a placeholder. In production, you would:
        // 1. Identify filters that can be pushed to Scan operators
        // 2. Move them earlier in the pipeline
        // 3. Combine multiple filters if possible
        // 4. Validate storage engine capabilities

        trace!("Filter pushdown optimization: {} filters", pushdown_count);
        pushdown_count
    }

    /// Push down projections to reduce data transfer
    fn pushdown_projections(&mut self) -> usize {
        let pushdown_count = 0;

        // For now, this is a placeholder. In production, you would:
        // 1. Identify which columns are needed for the final result
        // 2. Trace column usage through all operators
        // 3. Add projection operators early in the pipeline
        // 4. Remove unnecessary columns as early as possible

        trace!(
            "Projection pushdown optimization: {} projections",
            pushdown_count
        );
        pushdown_count
    }

    /// Reorder operators for better performance
    fn reorder_operators(&mut self) -> Result<bool> {
        let reordered = false;

        // For now, this is a placeholder. In production, you would:
        // 1. Analyze operator costs and selectivities
        // 2. Move selective filters early
        // 3. Reorder joins based on table sizes
        // 4. Consider pushing aggregates before joins when possible

        trace!("Operator reordering optimization: {}", reordered);
        Ok(reordered)
    }

    /// Convert to compute pipeline operators for execution
    ///
    /// This bridges the MultiModelPlan to the PipelineExecutor for actual execution.
    /// Federated operators (Join, Aggregate) are handled separately.
    pub fn to_compute_operators(&self) -> Vec<ComputeOperator> {
        self.operators
            .iter()
            .filter_map(|op| self.operator_to_compute(op))
            .collect()
    }

    /// Convert a single operator to compute operator
    fn operator_to_compute(&self, operator: &Operator) -> Option<ComputeOperator> {
        match operator {
            Operator::Scan { source, .. } => Some(ComputeOperator::Scan {
                source: source.clone(),
            }),
            Operator::Filter { expression } => Some(ComputeOperator::Filter {
                expression: expression.clone(),
            }),
            Operator::Project { columns } => Some(ComputeOperator::Project {
                columns: columns.clone(),
            }),
            Operator::Sort {
                column,
                ascending,
                limit,
            } => Some(ComputeOperator::Sort {
                column: column.clone(),
                ascending: *ascending,
                limit: *limit,
            }),
            Operator::TopK { k, sort_column } => Some(ComputeOperator::TopK {
                k: *k,
                sort_column: sort_column.clone(),
            }),
            // Federated operators don't map to compute operators
            Operator::Join { .. } | Operator::Aggregate { .. } | Operator::Union { .. } => None,
        }
    }
}

/// MultiModel operator for unified query execution
///
/// Extends the compute PipelineOperator with federated operators
/// like Join and Aggregate that work across storage engines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Operator {
    /// Scan operator - read data from a data model's service layer
    ///
    /// Engine selection is deferred to execution time via factory.rs config,
    /// not hardcoded per data model. The executor resolves the appropriate
    /// storage engine based on the collection's configuration.
    Scan {
        /// Logical data model (Vector, Document, Graph, Observability)
        /// The executor routes to the correct service layer, not a storage engine.
        data_model: DataModel,
        /// Source identifier (collection ID, graph name, log namespace, etc.)
        source: String,
        /// Optional column projection at scan level
        columns: Option<Vec<String>>,
        /// Optional filter at scan level
        filter: Option<FilterExpression>,
    },

    /// Filter operator - apply filter predicate
    Filter {
        /// Filter expression
        expression: FilterExpression,
    },

    /// Project operator - select specific columns
    Project {
        /// Column names to project
        columns: Vec<String>,
    },

    /// Join operator - combine data from multiple sources
    Join {
        /// Join type (inner, left, right, full)
        join_type: JoinType,
        /// Left side plan
        left_plan: Box<MultiModelPlan>,
        /// Right side plan
        right_plan: Box<MultiModelPlan>,
        /// Join condition
        condition: JoinCondition,
        /// Optional alias for the joined result
        alias: Option<String>,
    },

    /// Aggregate operator - group by and aggregation
    Aggregate {
        /// Group by columns
        group_by: Vec<String>,
        /// Aggregate expressions
        aggregates: Vec<AggregateExpression>,
        /// Optional alias for the result
        alias: Option<String>,
    },

    /// Sort operator - sort by specified column
    Sort {
        /// Sort column name
        column: String,
        /// Ascending or descending
        ascending: bool,
        /// Optional limit on number of results
        limit: Option<usize>,
    },

    /// TopK operator - select top K results
    TopK {
        /// K value
        k: usize,
        /// Sort column for ranking
        sort_column: String,
    },

    /// Union operator - combine results from multiple plans
    Union {
        /// Plans to union
        plans: Vec<MultiModelPlan>,
        /// Remove duplicates
        distinct: bool,
    },
}

/// Join type specification
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum JoinType {
    Inner,
    Left,
    Right,
    Full,
    Cross,
}

/// Join condition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum JoinCondition {
    /// Equijoin on single column
    On(String, String),
    /// Equijoin on multiple columns
    OnMultiple(Vec<(String, String)>),
    /// Complex expression join
    Expression(FilterExpression),
}

/// Aggregate expression
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregateExpression {
    /// Aggregate function type
    pub function: AggregateFunction,
    /// Input column
    pub column: String,
    /// Optional alias for the result
    pub alias: Option<String>,
    /// DISTINCT modifier
    pub distinct: bool,
}

/// Aggregate function types
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AggregateFunction {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    StdDev,
    Variance,
    ArrayAgg,
}

/// Plan execution context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanContext {
    /// Collection metadata for referenced collections
    #[serde(skip)]
    pub collections: HashMap<String, Arc<Collection>>,

    /// Search parameters
    #[serde(skip)]
    pub search_params: SearchParams,

    /// Plan creation timestamp
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Query timeout (milliseconds)
    pub timeout_ms: Option<u64>,

    /// Memory limit (bytes)
    pub memory_limit_bytes: Option<usize>,

    /// Enable distributed execution
    pub enable_distributed: bool,

    /// Execution priority
    pub priority: ExecutionPriority,
}

impl Default for PlanContext {
    fn default() -> Self {
        Self {
            collections: HashMap::new(),
            search_params: SearchParams::default(),
            created_at: chrono::Utc::now(),
            timeout_ms: None,
            memory_limit_bytes: None,
            enable_distributed: false,
            priority: ExecutionPriority::Normal,
        }
    }
}

/// Execution priority
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum ExecutionPriority {
    Low,
    Normal,
    High,
    Urgent,
}

impl Default for ExecutionPriority {
    fn default() -> Self {
        Self::Normal
    }
}

/// Plan optimization hints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanHints {
    /// Suggest using index for specific column
    pub use_index: Option<String>,

    /// Suggest join order
    pub join_order: Option<Vec<String>>,

    /// Estimated row count
    pub estimated_rows: Option<usize>,

    /// Enable result caching
    pub enable_cache: bool,

    /// Custom optimization hints
    pub custom_hints: HashMap<String, serde_json::Value>,
}

impl Default for PlanHints {
    fn default() -> Self {
        Self {
            use_index: None,
            join_order: None,
            estimated_rows: None,
            enable_cache: false,
            custom_hints: HashMap::new(),
        }
    }
}

/// Plan validation result
#[derive(Debug, Clone)]
pub struct PlanValidationResult {
    pub is_valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Plan optimization result
#[derive(Debug, Clone)]
pub struct PlanOptimizationResult {
    pub optimizations_applied: Vec<String>,
    pub original_stats: PlanStats,
    pub optimized_stats: PlanStats,
}

/// Plan statistics
#[derive(Debug, Clone, Default)]
pub struct PlanStats {
    pub operator_count: usize,
    pub scan_count: usize,
    pub filter_count: usize,
    pub project_count: usize,
    pub join_count: usize,
    pub aggregate_count: usize,
    pub sort_count: usize,
    pub topk_count: usize,
    pub union_count: usize,
}

/// Operator contract trait for operator validation
pub trait OperatorContract {
    /// Validate the operator
    fn validate(&self) -> Result<()>;

    /// Get the operator's schema requirements
    fn required_columns(&self) -> Vec<String>;

    /// Get the operator's schema output
    fn output_columns(&self) -> Vec<String>;

    /// Estimate operator cost
    fn estimate_cost(&self, input_rows: usize) -> f64;
}

impl OperatorContract for Operator {
    fn validate(&self) -> Result<()> {
        match self {
            Operator::Scan {
                data_model, source, ..
            } => {
                if source.is_empty() {
                    return Err(anyhow::anyhow!("Scan operator has empty source"));
                }
                debug!("Validated Scan operator: {} for {:?}", source, data_model);
                Ok(())
            }
            Operator::Filter { expression } => {
                // Validate filter expression structure
                match expression {
                    FilterExpression::Comparison { field, .. } => {
                        if field.is_empty() {
                            return Err(anyhow::anyhow!("Filter has empty field name"));
                        }
                    }
                    FilterExpression::And(exprs) | FilterExpression::Or(exprs) => {
                        if exprs.is_empty() {
                            return Err(anyhow::anyhow!("And/Or filter has no expressions"));
                        }
                    }
                    FilterExpression::Not(expr) => {
                        if matches!(
                            expr.as_ref(),
                            FilterExpression::And(_) | FilterExpression::Or(_)
                        ) {
                            return Err(anyhow::anyhow!("Not(And/Or) not supported"));
                        }
                    }
                }
                debug!("Validated Filter operator");
                Ok(())
            }
            Operator::Project { columns } => {
                if columns.is_empty() {
                    return Err(anyhow::anyhow!("Project operator has no columns"));
                }
                debug!("Validated Project operator: {} columns", columns.len());
                Ok(())
            }
            Operator::Sort { column, .. } => {
                if column.is_empty() {
                    return Err(anyhow::anyhow!("Sort operator has empty column name"));
                }
                debug!("Validated Sort operator: {}", column);
                Ok(())
            }
            Operator::TopK { k, sort_column } => {
                if *k == 0 {
                    return Err(anyhow::anyhow!("TopK operator has k=0"));
                }
                if sort_column.is_empty() {
                    return Err(anyhow::anyhow!("TopK operator has empty sort column"));
                }
                debug!("Validated TopK operator: k={}, column={}", k, sort_column);
                Ok(())
            }
            Operator::Join {
                join_type,
                condition,
                ..
            } => {
                match condition {
                    JoinCondition::On(left, right) => {
                        if left.is_empty() || right.is_empty() {
                            return Err(anyhow::anyhow!("Join condition has empty column name"));
                        }
                    }
                    JoinCondition::OnMultiple(pairs) => {
                        if pairs.is_empty() {
                            return Err(anyhow::anyhow!("Join has empty condition"));
                        }
                    }
                    JoinCondition::Expression(expr) => {
                        if matches!(expr, FilterExpression::And(_) | FilterExpression::Or(_)) {
                            return Err(anyhow::anyhow!("Join condition cannot be And/Or"));
                        }
                    }
                }
                debug!("Validated Join operator: {:?}", join_type);
                Ok(())
            }
            Operator::Aggregate {
                group_by,
                aggregates,
                ..
            } => {
                if group_by.is_empty() && aggregates.is_empty() {
                    return Err(anyhow::anyhow!(
                        "Aggregate operator has no group_by or aggregates"
                    ));
                }
                for agg in aggregates {
                    if agg.column.is_empty() {
                        return Err(anyhow::anyhow!("Aggregate has empty column name"));
                    }
                }
                debug!(
                    "Validated Aggregate operator: {} groups, {} aggregates",
                    group_by.len(),
                    aggregates.len()
                );
                Ok(())
            }
            Operator::Union { plans, .. } => {
                if plans.is_empty() {
                    return Err(anyhow::anyhow!("Union operator has no plans"));
                }
                debug!("Validated Union operator: {} plans", plans.len());
                Ok(())
            }
        }
    }

    fn required_columns(&self) -> Vec<String> {
        match self {
            Operator::Scan { columns, .. } => columns.clone().unwrap_or_default(),
            Operator::Filter { expression } => extract_columns_from_filter(expression),
            Operator::Project { columns } => columns.clone(),
            Operator::Sort { column, .. } => vec![column.clone()],
            Operator::TopK { sort_column, .. } => vec![sort_column.clone()],
            Operator::Join { condition, .. } => match condition {
                JoinCondition::On(left, right) => vec![left.clone(), right.clone()],
                JoinCondition::OnMultiple(pairs) => pairs
                    .iter()
                    .flat_map(|(l, r)| vec![l.clone(), r.clone()])
                    .collect(),
                JoinCondition::Expression(expr) => extract_columns_from_filter(expr),
            },
            Operator::Aggregate {
                group_by,
                aggregates,
                ..
            } => {
                let mut cols = group_by.clone();
                cols.extend(aggregates.iter().map(|a| a.column.clone()));
                cols
            }
            Operator::Union { .. } => vec![], // Union doesn't require specific columns
        }
    }

    fn output_columns(&self) -> Vec<String> {
        match self {
            Operator::Scan { columns, .. } => columns.clone().unwrap_or_default(),
            Operator::Filter { .. } => vec![], // Filter preserves all columns
            Operator::Project { columns } => columns.clone(),
            Operator::Sort { .. } => vec![], // Sort preserves all columns
            Operator::TopK { .. } => vec![], // TopK preserves all columns
            Operator::Join { alias, .. } => {
                // Output columns depend on join schema - placeholder
                alias.clone().map(|a| vec![a]).unwrap_or_default()
            }
            Operator::Aggregate {
                group_by,
                aggregates,
                ..
            } => {
                let mut cols = group_by.clone();
                cols.extend(aggregates.iter().filter_map(|a| a.alias.clone()));
                cols
            }
            Operator::Union { .. } => vec![], // Union output depends on unioned schemas
        }
    }

    fn estimate_cost(&self, input_rows: usize) -> f64 {
        match self {
            Operator::Scan { .. } => input_rows as f64, // Linear scan
            Operator::Filter { .. } => input_rows as f64 * 0.5, // Assume 50% selectivity
            Operator::Project { .. } => input_rows as f64 * 0.1, // Cheap - just column selection
            Operator::Sort { .. } => input_rows as f64 * (input_rows as f64).log2(), // O(n log n)
            Operator::TopK { k, .. } => {
                input_rows as f64 + (*k as f64 * (input_rows as f64).log2())
            } // n + k log n
            Operator::Join { .. } => input_rows as f64 * input_rows as f64, // O(n*m) worst case
            Operator::Aggregate { .. } => input_rows as f64 * 1.5, // Grouping overhead
            Operator::Union { .. } => input_rows as f64, // Just concatenation
        }
    }
}

/// Extract column names from a filter expression
fn extract_columns_from_filter(expression: &FilterExpression) -> Vec<String> {
    match expression {
        FilterExpression::Comparison { field, .. } => vec![field.clone()],
        FilterExpression::And(exprs) | FilterExpression::Or(exprs) => exprs
            .iter()
            .flat_map(|e| extract_columns_from_filter(e))
            .collect(),
        FilterExpression::Not(expr) => extract_columns_from_filter(expr),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::search::ComparisonOperator;

    #[test]
    fn test_create_simple_plan() {
        let operators = vec![
            Operator::Scan {
                data_model: DataModel::Vector,
                source: "test_collection".to_string(),
                columns: None,
                filter: None,
            },
            Operator::Filter {
                expression: FilterExpression::Comparison {
                    field: "score".to_string(),
                    operator: ComparisonOperator::GreaterThan,
                    value: serde_json::json!(0.5),
                },
            },
        ];

        let context = PlanContext::default();
        let plan = MultiModelPlan::new(operators, context);

        assert_eq!(plan.len(), 2);
        assert!(!plan.is_empty());
    }

    #[test]
    fn test_plan_validation() {
        let operators = vec![
            Operator::Scan {
                data_model: DataModel::Vector,
                source: "test_collection".to_string(),
                columns: None,
                filter: None,
            },
            Operator::Project {
                columns: vec!["id".to_string(), "score".to_string()],
            },
        ];

        let context = PlanContext::default();
        let plan = MultiModelPlan::new(operators, context);

        let validation = plan.validate().unwrap();
        assert!(validation.is_valid);
        assert!(validation.errors.is_empty());
    }

    #[test]
    fn test_plan_validation_empty_source() {
        let operators = vec![Operator::Scan {
            data_model: DataModel::Vector,
            source: "".to_string(), // Empty source - invalid
            columns: None,
            filter: None,
        }];

        let context = PlanContext::default();
        let plan = MultiModelPlan::new(operators, context);

        let validation = plan.validate().unwrap();
        assert!(!validation.is_valid);
        assert!(!validation.errors.is_empty());
    }

    #[test]
    fn test_operator_contract_validation() {
        let scan_op = Operator::Scan {
            data_model: DataModel::Vector,
            source: "test".to_string(),
            columns: None,
            filter: None,
        };

        assert!(scan_op.validate().is_ok());

        let invalid_scan = Operator::Scan {
            data_model: DataModel::Vector,
            source: "".to_string(), // Invalid
            columns: None,
            filter: None,
        };

        assert!(invalid_scan.validate().is_err());
    }

    #[test]
    fn test_join_operator() {
        let left_plan = MultiModelPlan::new(
            vec![Operator::Scan {
                data_model: DataModel::Vector,
                source: "users".to_string(),
                columns: None,
                filter: None,
            }],
            PlanContext::default(),
        );

        let right_plan = MultiModelPlan::new(
            vec![Operator::Scan {
                data_model: DataModel::Document,
                source: "orders".to_string(),
                columns: None,
                filter: None,
            }],
            PlanContext::default(),
        );

        let join_op = Operator::Join {
            join_type: JoinType::Inner,
            left_plan: Box::new(left_plan),
            right_plan: Box::new(right_plan),
            condition: JoinCondition::On("user_id".to_string(), "id".to_string()),
            alias: Some("user_orders".to_string()),
        };

        assert!(join_op.validate().is_ok());
    }

    #[test]
    fn test_aggregate_operator() {
        let agg_op = Operator::Aggregate {
            group_by: vec!["category".to_string()],
            aggregates: vec![
                AggregateExpression {
                    function: AggregateFunction::Count,
                    column: "*".to_string(),
                    alias: Some("count".to_string()),
                    distinct: false,
                },
                AggregateExpression {
                    function: AggregateFunction::Avg,
                    column: "score".to_string(),
                    alias: Some("avg_score".to_string()),
                    distinct: false,
                },
            ],
            alias: None,
        };

        assert!(agg_op.validate().is_ok());
    }

    #[test]
    fn test_plan_stats() {
        let operators = vec![
            Operator::Scan {
                data_model: DataModel::Vector,
                source: "test".to_string(),
                columns: None,
                filter: None,
            },
            Operator::Filter {
                expression: FilterExpression::Comparison {
                    field: "score".to_string(),
                    operator: ComparisonOperator::GreaterThan,
                    value: serde_json::json!(0.5),
                },
            },
            Operator::TopK {
                k: 10,
                sort_column: "score".to_string(),
            },
        ];

        let context = PlanContext::default();
        let plan = MultiModelPlan::new(operators, context);

        let stats = plan.stats();
        assert_eq!(stats.operator_count, 3);
        assert_eq!(stats.scan_count, 1);
        assert_eq!(stats.filter_count, 1);
        assert_eq!(stats.topk_count, 1);
    }

    #[test]
    fn test_to_compute_operators() {
        let operators = vec![
            Operator::Scan {
                data_model: DataModel::Vector,
                source: "test".to_string(),
                columns: None,
                filter: None,
            },
            Operator::Project {
                columns: vec!["id".to_string(), "name".to_string()],
            },
        ];

        let context = PlanContext::default();
        let plan = MultiModelPlan::new(operators, context);

        let compute_ops = plan.to_compute_operators();
        assert_eq!(compute_ops.len(), 2);
    }

    #[test]
    fn test_operator_cost_estimation() {
        let scan_op = Operator::Scan {
            data_model: DataModel::Vector,
            source: "test".to_string(),
            columns: None,
            filter: None,
        };

        let cost = scan_op.estimate_cost(1000);
        assert_eq!(cost, 1000.0);

        let sort_op = Operator::Sort {
            column: "score".to_string(),
            ascending: false,
            limit: Some(10),
        };

        let sort_cost = sort_op.estimate_cost(1000);
        assert!(sort_cost > 1000.0); // Sort is more expensive than scan
    }
}
