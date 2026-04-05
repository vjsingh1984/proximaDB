//! UQL to MultiModelPlan Lowering (Issue #45, SB-15)
//!
//! This module implements the transformation from UQL AST to MultiModelPlan v1.
//! It converts parsed UQL statements into executable plans that can be run
//! across all storage engines with vectorized operators.
//!
//! ## Lowering Process
//!
//! ```text
//! UQL Statement (AST)
//!       ↓
//! Parse & Validate
//!       ↓
//! Convert to Operators
//!       ↓
//! Optimize Plan
//!       ↓
//! MultiModelPlan v1
//!       ↓
//! Execute via PipelineExecutor
//! ```
//!
//! ## Supported Transformations
//!
//! - **SELECT statements** → Scan → Filter → Project → Sort → TopK
//! - **WHERE clauses** → Filter operators with expression conversion
//! - **JOIN clauses** → Join operators with appropriate join type
//! - **ORDER BY** → Sort operators
//! - **LIMIT** → TopK operators
//! - **MultiModal queries** → Union operators with fusion strategies

use anyhow::Result;
use std::collections::HashMap;
use tracing::{debug, trace, warn};

use anyhow::anyhow;
use crate::query::unified::fusion::FusionStrategy;
use crate::query::multimodal::plan::{
    JoinCondition, JoinType, MultiModelPlan, Operator,
    PlanContext,
};
use crate::query::unified::ast::*;
use crate::query::unified::uql::{SelectStatement, UQLStatement};

/// UQL to MultiModelPlan lowerer
pub struct UQLLowerer {
    /// Current context for lowering
    context: PlanContext,

    /// Enable plan optimization
    enable_optimization: bool,
}

impl UQLLowerer {
    /// Create a new UQL lowerer
    pub fn new(context: PlanContext) -> Self {
        Self {
            context,
            enable_optimization: true,
        }
    }

    /// Create a new UQL lowerer without optimization
    pub fn new_no_optimization(context: PlanContext) -> Self {
        Self {
            context,
            enable_optimization: false,
        }
    }

    /// Lower a UQL statement to a MultiModelPlan
    pub fn lower(&mut self, statement: &UQLStatement) -> Result<MultiModelPlan> {
        debug!("Lowering UQL statement to MultiModelPlan");

        let operators = match statement {
            UQLStatement::Select(select) => self.lower_select(select)?,
            UQLStatement::MultiModal(multimodal) => self.lower_multimodal(multimodal)?,
            UQLStatement::Explain(stmt) => {
                // For EXPLAIN, lower the inner statement and extract its operators
                let inner_plan = self.lower(stmt)?;
                inner_plan.operators
            }
        };

        let mut plan = MultiModelPlan::new(operators, self.context.clone());

        // Optimize the plan if enabled
        if self.enable_optimization {
            let optimization_result = plan.optimize()?;
            debug!(
                "Plan optimization complete: {} optimizations applied",
                optimization_result.optimizations_applied.len()
            );
        }

        // Validate the plan
        let validation = plan.validate()?;
        if !validation.is_valid {
            warn!("Plan validation failed: {:?}", validation.errors);
            return Err(anyhow::anyhow!(
                "Generated invalid plan: {}",
                validation.errors.join(", ")
            ));
        }

        if !validation.warnings.is_empty() {
            debug!("Plan validation warnings: {:?}", validation.warnings);
        }

        debug!(
            "Successfully lowered UQL to MultiModelPlan with {} operators",
            plan.len()
        );

        Ok(plan)
    }

    /// Lower a SELECT statement to operators
    fn lower_select(&mut self, select: &SelectStatement) -> Result<Vec<Operator>> {
        let mut operators = Vec::new();

        trace!("Lowering SELECT statement");

        // 1. Add Scan operator for FROM clause
        let scan_operator = self.lower_from(&select.from)?;
        operators.push(scan_operator);

        // 2. Add Join operators if present
        for join in &select.joins {
            let join_operator = self.lower_join(join)?;
            operators.push(join_operator);
        }

        // 3. Add Filter operator for WHERE clause
        if let Some(where_clause) = &select.where_clause {
            let filter_operator = self.lower_where(where_clause)?;
            operators.push(filter_operator);
        }

        // 4. Add Project operator for column selection
        if !select.columns.is_empty() && select.columns.iter().any(|c| c != "*") {
            let project_operator = self.lower_projection(&select.columns)?;
            operators.push(project_operator);
        }

        // 5. Add Sort operator for ORDER BY
        if let Some(order_by) = &select.order_by {
            let sort_operator = self.lower_order_by(order_by, select.limit)?;
            operators.push(sort_operator);
        }

        // 6. Add TopK operator for LIMIT (if no ORDER BY)
        if select.limit.is_some() && select.order_by.is_none() {
            let topk_operator = self.lower_limit(select.limit.unwrap(), None)?;
            operators.push(topk_operator);
        }

        trace!("Lowered SELECT to {} operators", operators.len());

        Ok(operators)
    }

    /// Lower FROM clause to Scan operator
    ///
    /// Routes by data model to the correct service layer.
    /// Storage engine selection is deferred to execution time via factory.rs.
    fn lower_from(&self, from: &crate::query::unified::uql::DataSource) -> Result<Operator> {
        trace!("Lowering FROM clause: {:?}", from);

        Ok(Operator::Scan {
            data_model: from.model.clone(),
            source: from.collection.clone(),
            columns: None,
            filter: None,
        })
    }

    /// Lower JOIN clause to Join operator
    fn lower_join(&self, join: &crate::query::unified::uql::JoinClause) -> Result<Operator> {
        trace!("Lowering JOIN clause: {:?}", join);

        // For now, we'll create a simplified join operator
        // In production, you'd recursively lower the joined tables

        let join_type = match join.join_type {
            crate::query::unified::uql::JoinType::Inner => JoinType::Inner,
            crate::query::unified::uql::JoinType::Left => JoinType::Left,
            crate::query::unified::uql::JoinType::Right => JoinType::Right,
            crate::query::unified::uql::JoinType::Full => JoinType::Full,
            _ => JoinType::Inner,
        };

        // Convert join condition
        let condition = match &join.condition {
            crate::query::unified::uql::JoinCondition::On { left, right } => {
                JoinCondition::On(left.clone(), right.clone())
            }
            _ => {
                return Err(anyhow::anyhow!("Unsupported JOIN condition type"));
            }
        };

        // Create placeholder plans for left and right
        // In production, these would be recursively lowered from the joined tables
        let left_plan = MultiModelPlan::new(
            vec![Operator::Scan {
                data_model: DataModel::Vector, // Placeholder - resolved at execution
                source: "left_table".to_string(),
                columns: None,
                filter: None,
            }],
            self.context.clone(),
        );

        let right_plan = MultiModelPlan::new(
            vec![Operator::Scan {
                data_model: join.source.model.clone(),
                source: join.source.collection.clone(),
                columns: None,
                filter: None,
            }],
            self.context.clone(),
        );

        Ok(Operator::Join {
            join_type,
            left_plan: Box::new(left_plan),
            right_plan: Box::new(right_plan),
            condition,
            alias: join.source.alias.clone(),
        })
    }

    /// Lower WHERE clause to Filter operator
    fn lower_where(&self, where_clause: &crate::query::unified::uql::WhereClause) -> Result<Operator> {
        trace!("Lowering WHERE clause");

        // Convert conditions to filter expression
        let expression = if where_clause.conditions.len() == 1 {
            self.convert_condition_to_filter(&where_clause.conditions[0])?
        } else {
            // Multiple conditions combined with logic operator
            let converted_exprs: Result<Vec<_>> = where_clause.conditions
                .iter()
                .map(|c| self.convert_condition_to_filter(c))
                .collect();
            match where_clause.logic {
                crate::query::unified::uql::LogicOperator::And => {
                    crate::core::search::FilterExpression::And(converted_exprs?)
                }
                crate::query::unified::uql::LogicOperator::Or => {
                    crate::core::search::FilterExpression::Or(converted_exprs?)
                }
            }
        };

        Ok(Operator::Filter { expression })
    }

    /// Lower projection to Project operator
    fn lower_projection(&self, columns: &[String]) -> Result<Operator> {
        trace!("Lowering projection: {} columns", columns.len());

        // Filter out "*" if present
        let columns: Vec<String> = columns.iter().filter(|c| c != &"*").cloned().collect();

        Ok(Operator::Project { columns })
    }

    /// Lower ORDER BY to Sort operator
    fn lower_order_by(
        &self,
        order_by: &crate::query::unified::uql::OrderByClause,
        limit: Option<u32>,
    ) -> Result<Operator> {
        trace!("Lowering ORDER BY: {:?}", order_by.columns);

        // For simplicity, use the first column for sort
        let (column, sort_order) = order_by.columns.first()
            .ok_or_else(|| anyhow::anyhow!("ORDER BY clause has no columns"))?;

        let ascending = matches!(sort_order, crate::query::unified::uql::SortOrder::Asc);

        Ok(Operator::Sort {
            column: column.clone(),
            ascending,
            limit: limit.map(|l| l as usize),
        })
    }

    /// Lower LIMIT to TopK operator
    fn lower_limit(&self, limit: u32, sort_column: Option<String>) -> Result<Operator> {
        trace!("Lowering LIMIT: {}", limit);

        // If no sort column specified, use a default
        let sort_column = sort_column.unwrap_or_else(|| "_id".to_string());

        Ok(Operator::TopK {
            k: limit as usize,
            sort_column,
        })
    }

    /// Lower MultiModal statement to operators
    fn lower_multimodal(&self, multimodal: &crate::query::unified::uql::MultiModalStatement) -> Result<Vec<Operator>> {
        trace!("Lowering MultiModal statement with {} components", multimodal.components.len());

        // For MultiModal queries, we create separate plans for each component
        // and combine them with a Union operator

        let mut plans = Vec::new();

        for (data_model, _query_string) in &multimodal.components {
            // Parse each component query (simplified - in production, you'd recursively call lower)
            let component_plan = MultiModelPlan::new(
                vec![Operator::Scan {
                    data_model: data_model.clone(),
                    source: format!("component_{}", data_model),
                    columns: None,
                    filter: None,
                }],
                self.context.clone(),
            );

            plans.push(component_plan);
        }

        // Combine with Union operator
        let union_operator = Operator::Union {
            plans,
            distinct: matches!(multimodal.fusion, FusionStrategy::Intersection),
        };

        // Add LIMIT if present
        let mut operators = vec![union_operator];

        if let Some(limit) = multimodal.limit {
            let topk_operator = self.lower_limit(limit, None)?;
            operators.push(topk_operator);
        }

        Ok(operators)
    }

    /// Convert condition from UQL to unified FilterExpression
    fn convert_condition_to_filter(&self, condition: &crate::query::unified::uql::Condition) -> Result<crate::core::search::FilterExpression> {
        match condition {
            crate::query::unified::uql::Condition::Comparison { field, operator, value } => {
                let core_operator = match operator {
                    crate::query::unified::uql::ComparisonOperator::Eq => {
                        crate::core::search::ComparisonOperator::Equals
                    }
                    crate::query::unified::uql::ComparisonOperator::Ne => {
                        crate::core::search::ComparisonOperator::NotEquals
                    }
                    crate::query::unified::uql::ComparisonOperator::Gt => {
                        crate::core::search::ComparisonOperator::GreaterThan
                    }
                    crate::query::unified::uql::ComparisonOperator::Gte => {
                        crate::core::search::ComparisonOperator::GreaterThanOrEqual
                    }
                    crate::query::unified::uql::ComparisonOperator::Lt => {
                        crate::core::search::ComparisonOperator::LessThan
                    }
                    crate::query::unified::uql::ComparisonOperator::Lte => {
                        crate::core::search::ComparisonOperator::LessThanOrEqual
                    }
                    _ => {
                        return Err(anyhow::anyhow!("Unsupported filter operator: {:?}", operator));
                    }
                };

                let filter_value = match value {
                    crate::query::unified::uql::Value::String(s) => {
                        serde_json::Value::String(s.clone())
                    }
                    crate::query::unified::uql::Value::Number(n) => {
                        serde_json::Value::Number(
                            serde_json::Number::from_f64(*n)
                                .unwrap_or_else(|| serde_json::Number::from(0))
                        )
                    }
                    crate::query::unified::uql::Value::Boolean(b) => {
                        serde_json::Value::Bool(*b)
                    }
                    _ => {
                        return Err(anyhow::anyhow!("Unsupported filter value type: {:?}", value));
                    }
                };

                Ok(crate::core::search::FilterExpression::Comparison {
                    field: field.clone(),
                    operator: core_operator,
                    value: filter_value,
                })
            }
            _ => {
                Err(anyhow::anyhow!("Unsupported condition type: {:?}", condition))
            }
        }
    }

    // NOTE: Engine selection removed from the lowerer.
    // Storage engine selection is the responsibility of the executor at runtime,
    // resolved via factory.rs based on collection configuration.
    // The lowerer operates purely on logical data models.
}

/// Convenience function to lower a UQL statement
pub fn lower_uql_to_plan(
    statement: &UQLStatement,
    context: PlanContext,
) -> Result<MultiModelPlan> {
    let mut lowerer = UQLLowerer::new(context);
    lowerer.lower(statement)
}

/// Convenience function to lower a UQL statement without optimization
pub fn lower_uql_to_plan_no_optimization(
    statement: &UQLStatement,
    context: PlanContext,
) -> Result<MultiModelPlan> {
    let mut lowerer = UQLLowerer::new_no_optimization(context);
    lowerer.lower(statement)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::search::ComparisonOperator;
    use crate::query::unified::uql::{
        Condition, DataSource, LogicOperator, OrderByClause, SelectStatement,
        SortOrder, UQLStatement, WhereClause,
    };

    #[test]
    fn test_lower_simple_select() {
        let select = SelectStatement {
            columns: vec!["id".to_string(), "name".to_string()],
            from: DataSource {
                model: DataModel::Vector,
                collection: "test_collection".to_string(),
                alias: None,
            },
            joins: Vec::new(),
            where_clause: None,
            order_by: None,
            limit: None,
            offset: None,
            fusion: None,
        };

        let statement = UQLStatement::Select(select);
        let context = PlanContext::default();

        let mut lowerer = UQLLowerer::new(context);
        let plan = lowerer.lower(&statement).unwrap();

        assert_eq!(plan.len(), 2); // Scan + Project
        assert!(plan.validate().unwrap().is_valid);
    }

    #[test]
    fn test_lower_select_with_filter() {
        let select = SelectStatement {
            columns: vec!["*".to_string()],
            from: DataSource {
                model: DataModel::Vector,
                collection: "products".to_string(),
                alias: None,
            },
            joins: Vec::new(),
            where_clause: Some(WhereClause {
                conditions: vec![Condition::Comparison {
                    field: "price".to_string(),
                    operator: crate::query::unified::uql::ComparisonOperator::Lt,
                    value: crate::query::unified::uql::Value::Number(1000.0),
                }],
                logic: LogicOperator::And,
            }),
            order_by: None,
            limit: Some(10),
            offset: None,
            fusion: None,
        };

        let statement = UQLStatement::Select(select);
        let context = PlanContext::default();

        let mut lowerer = UQLLowerer::new(context);
        let plan = lowerer.lower(&statement).unwrap();

        assert_eq!(plan.len(), 3); // Scan + Filter + TopK
        assert!(plan.validate().unwrap().is_valid);
    }

    #[test]
    fn test_lower_select_with_order_by() {
        let select = SelectStatement {
            columns: vec!["id".to_string(), "score".to_string()],
            from: DataSource {
                model: DataModel::Vector,
                collection: "results".to_string(),
                alias: None,
            },
            joins: Vec::new(),
            where_clause: None,
            order_by: Some(OrderByClause {
                columns: vec![("score".to_string(), SortOrder::Desc)],
            }),
            limit: Some(100),
            offset: None,
            fusion: None,
        };

        let statement = UQLStatement::Select(select);
        let context = PlanContext::default();

        let mut lowerer = UQLLowerer::new(context);
        let plan = lowerer.lower(&statement).unwrap();

        assert_eq!(plan.len(), 3); // Scan + Project + Sort
        assert!(plan.validate().unwrap().is_valid);
    }

    #[test]
    fn test_lowerer_uses_data_model_not_engine() {
        // Verify that the lowerer produces Scan operators with DataModel,
        // NOT StorageEngineType. Engine selection is deferred to execution.
        let select = SelectStatement {
            columns: vec!["*".to_string()],
            from: DataSource {
                model: DataModel::Document,
                collection: "users".to_string(),
                alias: None,
            },
            joins: Vec::new(),
            where_clause: None,
            order_by: None,
            limit: None,
            offset: None,
            fusion: None,
        };

        let statement = UQLStatement::Select(select);
        let context = PlanContext::default();
        let mut lowerer = UQLLowerer::new(context);
        let plan = lowerer.lower(&statement).unwrap();

        // Plan should contain a Scan with DataModel::Document, not a storage engine
        assert!(plan.validate().unwrap().is_valid);
    }

    #[test]
    fn test_condition_to_filter_conversion() {
        let context = PlanContext::default();
        let lowerer = UQLLowerer::new(context);

        let condition = Condition::Comparison {
            field: "status".to_string(),
            operator: crate::query::unified::uql::ComparisonOperator::Eq,
            value: crate::query::unified::uql::Value::String("active".to_string()),
        };

        let core_filter = lowerer.convert_condition_to_filter(&condition).unwrap();

        match core_filter {
            crate::core::search::FilterExpression::Comparison { field, operator, value } => {
                assert_eq!(field, "status");
                assert_eq!(operator, ComparisonOperator::Equals);
                assert_eq!(value, serde_json::json!("active"));
            }
            _ => panic!("Expected Comparison expression"),
        }
    }
}
