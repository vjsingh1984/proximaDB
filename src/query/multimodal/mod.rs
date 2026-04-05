//! MultiModel Query Module - Unified Cross-Model Query Execution
//!
//! This module provides unified query execution across all ProximaDB storage engines.
//! It implements the MultiModelPlan contract for vectorized cross-model execution.

pub mod plan;

// Re-export main types for convenience
pub use plan::{
    AggregateExpression, AggregateFunction, ExecutionPriority, JoinCondition, JoinType,
    MultiModelPlan, Operator, OperatorContract, PlanContext, PlanHints, PlanOptimizationResult,
    PlanStats, PlanValidationResult,
};
