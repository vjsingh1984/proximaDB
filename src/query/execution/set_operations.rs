//! Set Operations and CTE Execution for ProximaDB SQL Frontend
//!
//! This module implements UNION, INTERSECT, EXCEPT operations and
//! Common Table Expression (CTE) execution planning.

use crate::query::ast::{Cte, Query, SetOp};
use crate::query::execution::{
    ExecutionOperation, ExecutionPlan, ExecutionStrategy, SeedingStrategy,
};
use anyhow::Result;
use tracing::debug;

/// Extension trait for ExecutionPlanner to handle set operations and CTEs
pub trait SetOperationPlanner {
    /// Plan Common Table Expression (CTE) queries
    fn plan_cte(&self, ctes: &[Cte], query: &Query) -> Result<ExecutionPlan>;

    /// Plan set operations (UNION, INTERSECT, EXCEPT)
    fn plan_set_operation(
        &self,
        left: &Query,
        op: &SetOp,
        all: bool,
        right: &Query,
    ) -> Result<ExecutionPlan>;
}

impl SetOperationPlanner for crate::query::execution::planner::ExecutionPlanner {
    /// Plan Common Table Expression (CTE) queries
    fn plan_cte(&self, ctes: &[Cte], query: &Query) -> Result<ExecutionPlan> {
        debug!(
            "Planning CTE query with {} common table expressions",
            ctes.len()
        );

        let mut operations = Vec::new();
        let mut total_cost = 0.0;

        // Plan each CTE as a temporary table
        for cte in ctes {
            debug!("Planning CTE: {}", cte.name);

            // Create execution plan for the CTE query
            let cte_plan = self.create_plan(&cte.query)?;
            total_cost += cte_plan.estimated_cost;

            // Add CTE materialization operation
            operations.push(ExecutionOperation::CteMaterialization {
                cte_name: cte.name.clone(),
                query_plan: Box::new(cte_plan),
            });
        }

        // Plan the main query that references CTEs
        let main_plan = self.create_plan(query)?;
        operations.extend(main_plan.execution_steps);
        total_cost += main_plan.estimated_cost;

        Ok(ExecutionPlan::runtime(
            ExecutionStrategy::Relational,
            operations,
            total_cost,
            vec!["CTE materialization".to_string()],
            vec!["CTEs materialized before main query".to_string()],
            SeedingStrategy::None,
            main_plan.limit,
            main_plan.offset,
        ))
    }

    /// Plan set operations (UNION, INTERSECT, EXCEPT)
    fn plan_set_operation(
        &self,
        left: &Query,
        op: &SetOp,
        all: bool,
        right: &Query,
    ) -> Result<ExecutionPlan> {
        debug!("Planning set operation: {:?} (ALL: {})", op, all);

        // Plan left and right queries
        let left_plan = self.create_plan(left)?;
        let right_plan = self.create_plan(right)?;

        let mut operations = Vec::new();

        // Add operations for left query
        operations.extend(left_plan.execution_steps);

        // Add operations for right query
        operations.extend(right_plan.execution_steps);

        // Add set operation
        let set_operation = match op {
            SetOp::Union => ExecutionOperation::SetUnion {
                left_results: "left_query".to_string(),
                right_results: "right_query".to_string(),
                distinct: !all, // UNION ALL vs UNION DISTINCT
            },
            SetOp::Intersect => ExecutionOperation::SetIntersect {
                left_results: "left_query".to_string(),
                right_results: "right_query".to_string(),
                distinct: !all,
            },
            SetOp::Except => ExecutionOperation::SetExcept {
                left_results: "left_query".to_string(),
                right_results: "right_query".to_string(),
                distinct: !all,
            },
        };

        operations.push(set_operation);

        let total_cost = left_plan.estimated_cost + right_plan.estimated_cost + 100.0; // Set operation cost

        Ok(ExecutionPlan::runtime(
            ExecutionStrategy::Relational,
            operations,
            total_cost,
            vec![format!("Set operation: {:?}", op)],
            vec!["Set operations may require result buffering".to_string()],
            SeedingStrategy::None,
            None,
            None,
        ))
    }
}

#[cfg(test)]
mod tests {
    use crate::query::ast::*;

    #[test]
    fn test_set_operation_planning() {
        // This would require a full ExecutionPlanner setup
        // For now, we verify the traits and structures compile correctly

        let union_op = SetOp::Union;
        let intersect_op = SetOp::Intersect;
        let except_op = SetOp::Except;

        // Verify enum variants exist
        assert!(matches!(union_op, SetOp::Union));
        assert!(matches!(intersect_op, SetOp::Intersect));
        assert!(matches!(except_op, SetOp::Except));
    }

    #[test]
    fn test_cte_structures() {
        let cte = Cte {
            name: "test_cte".to_string(),
            query: Box::new(Query::Select(Select {
                projection: vec![],
                from: vec![],
                joins: vec![],
                selection: None,
                group_by: vec![],
                having: None,
                order_by: vec![],
                limit: None,
                offset: None,
            })),
        };

        assert_eq!(cte.name, "test_cte");
    }
}
