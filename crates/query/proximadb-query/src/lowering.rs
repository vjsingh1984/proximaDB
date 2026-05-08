//! UQL to MultiModelPlan lowering for the extracted query runtime layer.

use anyhow::Result;
use proximadb_filter_expression::{ComparisonOperator, FilterExpression};
use proximadb_multimodel_plan::{JoinCondition, JoinType, MultiModelPlan, Operator, PlanContext};
use proximadb_uql::{
    ast::DataModel,
    fusion::FusionStrategy,
    uql::{
        ComparisonOperator as UqlComparisonOperator, Condition, DataSource, JoinClause,
        JoinCondition as UqlJoinCondition, LogicOperator, MultiModalStatement, OrderByClause,
        SelectStatement, SortOrder, UQLStatement, Value, WhereClause,
    },
};
use tracing::{debug, trace, warn};

/// UQL to MultiModelPlan lowerer.
pub struct UQLLowerer {
    /// Current context for lowering.
    context: PlanContext,

    /// Enable plan optimization.
    enable_optimization: bool,
}

impl UQLLowerer {
    /// Create a new UQL lowerer.
    pub fn new(context: PlanContext) -> Self {
        Self {
            context,
            enable_optimization: true,
        }
    }

    /// Create a new UQL lowerer without optimization.
    pub fn new_no_optimization(context: PlanContext) -> Self {
        Self {
            context,
            enable_optimization: false,
        }
    }

    /// Lower a UQL statement to a multimodel plan.
    pub fn lower(&mut self, statement: &UQLStatement) -> Result<MultiModelPlan> {
        debug!("Lowering UQL statement to MultiModelPlan");

        let operators = match statement {
            UQLStatement::Select(select) => self.lower_select(select)?,
            UQLStatement::MultiModal(multimodal) => self.lower_multimodal(multimodal)?,
            UQLStatement::Explain(stmt) => {
                let inner_plan = self.lower(stmt)?;
                inner_plan.operators
            }
        };

        let mut plan = MultiModelPlan::new(operators, self.context.clone());

        if self.enable_optimization {
            let optimization_result = plan.optimize()?;
            debug!(
                "Plan optimization complete: {} optimizations applied",
                optimization_result.optimizations_applied.len()
            );
        }

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

    fn lower_select(&mut self, select: &SelectStatement) -> Result<Vec<Operator>> {
        let mut operators = Vec::new();

        trace!("Lowering SELECT statement");

        operators.push(self.lower_from(&select.from)?);

        for join in &select.joins {
            operators.push(self.lower_join(join)?);
        }

        if let Some(where_clause) = &select.where_clause {
            operators.push(self.lower_where(where_clause)?);
        }

        if !select.columns.is_empty() && select.columns.iter().any(|c| c != "*") {
            operators.push(self.lower_projection(&select.columns)?);
        }

        if let Some(order_by) = &select.order_by {
            operators.push(self.lower_order_by(order_by, select.limit)?);
        }

        if select.limit.is_some() && select.order_by.is_none() {
            operators.push(self.lower_limit(select.limit.unwrap_or_default(), None)?);
        }

        trace!("Lowered SELECT to {} operators", operators.len());
        Ok(operators)
    }

    /// Routes by data model to the correct service layer.
    /// Storage engine selection is deferred to execution time.
    fn lower_from(&self, from: &DataSource) -> Result<Operator> {
        trace!("Lowering FROM clause: {:?}", from);

        Ok(Operator::Scan {
            data_model: from.model.clone(),
            source: from.collection.clone(),
            columns: None,
            filter: None,
        })
    }

    fn lower_join(&self, join: &JoinClause) -> Result<Operator> {
        trace!("Lowering JOIN clause: {:?}", join);

        let join_type = match join.join_type {
            proximadb_uql::uql::JoinType::Inner => JoinType::Inner,
            proximadb_uql::uql::JoinType::Left => JoinType::Left,
            proximadb_uql::uql::JoinType::Right => JoinType::Right,
            proximadb_uql::uql::JoinType::Full => JoinType::Full,
            _ => JoinType::Inner,
        };

        let condition = match &join.condition {
            UqlJoinCondition::On { left, right } => JoinCondition::On(left.clone(), right.clone()),
            _ => {
                return Err(anyhow::anyhow!("Unsupported JOIN condition type"));
            }
        };

        let left_plan = MultiModelPlan::new(
            vec![Operator::Scan {
                data_model: DataModel::Vector,
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

    fn lower_where(&self, where_clause: &WhereClause) -> Result<Operator> {
        trace!("Lowering WHERE clause");

        let expression = if where_clause.conditions.len() == 1 {
            self.convert_condition_to_filter(&where_clause.conditions[0])?
        } else {
            let converted_exprs: Result<Vec<_>> = where_clause
                .conditions
                .iter()
                .map(|c| self.convert_condition_to_filter(c))
                .collect();
            match where_clause.logic {
                LogicOperator::And => FilterExpression::And(converted_exprs?),
                LogicOperator::Or => FilterExpression::Or(converted_exprs?),
            }
        };

        Ok(Operator::Filter { expression })
    }

    fn lower_projection(&self, columns: &[String]) -> Result<Operator> {
        trace!("Lowering projection: {} columns", columns.len());

        let columns: Vec<String> = columns
            .iter()
            .filter(|c| c.as_str() != "*")
            .cloned()
            .collect();

        Ok(Operator::Project { columns })
    }

    fn lower_order_by(&self, order_by: &OrderByClause, limit: Option<u32>) -> Result<Operator> {
        trace!("Lowering ORDER BY: {:?}", order_by.columns);

        let (column, sort_order) = order_by
            .columns
            .first()
            .ok_or_else(|| anyhow::anyhow!("ORDER BY clause has no columns"))?;

        let ascending = matches!(sort_order, SortOrder::Asc);

        Ok(Operator::Sort {
            column: column.clone(),
            ascending,
            limit: limit.map(|l| l as usize),
        })
    }

    fn lower_limit(&self, limit: u32, sort_column: Option<String>) -> Result<Operator> {
        trace!("Lowering LIMIT: {}", limit);

        Ok(Operator::TopK {
            k: limit as usize,
            sort_column: sort_column.unwrap_or_else(|| "_id".to_string()),
        })
    }

    fn lower_multimodal(&self, multimodal: &MultiModalStatement) -> Result<Vec<Operator>> {
        trace!(
            "Lowering MultiModal statement with {} components",
            multimodal.components.len()
        );

        let mut plans = Vec::new();

        for (data_model, _query_string) in &multimodal.components {
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

        let union_operator = Operator::Union {
            plans,
            distinct: matches!(multimodal.fusion, FusionStrategy::Intersection),
        };

        let mut operators = vec![union_operator];

        if let Some(limit) = multimodal.limit {
            operators.push(self.lower_limit(limit, None)?);
        }

        Ok(operators)
    }

    fn convert_condition_to_filter(&self, condition: &Condition) -> Result<FilterExpression> {
        match condition {
            Condition::Comparison {
                field,
                operator,
                value,
            } => {
                let core_operator = match operator {
                    UqlComparisonOperator::Eq => ComparisonOperator::Equals,
                    UqlComparisonOperator::Ne => ComparisonOperator::NotEquals,
                    UqlComparisonOperator::Gt => ComparisonOperator::GreaterThan,
                    UqlComparisonOperator::Gte => ComparisonOperator::GreaterThanOrEqual,
                    UqlComparisonOperator::Lt => ComparisonOperator::LessThan,
                    UqlComparisonOperator::Lte => ComparisonOperator::LessThanOrEqual,
                    _ => {
                        return Err(anyhow::anyhow!(
                            "Unsupported filter operator: {:?}",
                            operator
                        ));
                    }
                };

                let filter_value = match value {
                    Value::String(s) => serde_json::Value::String(s.clone()),
                    Value::Number(n) => serde_json::Value::Number(
                        serde_json::Number::from_f64(*n)
                            .unwrap_or_else(|| serde_json::Number::from(0)),
                    ),
                    Value::Boolean(b) => serde_json::Value::Bool(*b),
                    _ => {
                        return Err(anyhow::anyhow!(
                            "Unsupported filter value type: {:?}",
                            value
                        ));
                    }
                };

                Ok(FilterExpression::Comparison {
                    field: field.clone(),
                    operator: core_operator,
                    value: filter_value,
                })
            }
            _ => Err(anyhow::anyhow!(
                "Unsupported condition type: {:?}",
                condition
            )),
        }
    }
}

/// Convenience function to lower a UQL statement.
pub fn lower_uql_to_plan(statement: &UQLStatement, context: PlanContext) -> Result<MultiModelPlan> {
    let mut lowerer = UQLLowerer::new(context);
    lowerer.lower(statement)
}

/// Convenience function to lower a UQL statement without optimization.
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
    use proximadb_uql::uql::{
        Condition, DataSource, LogicOperator, OrderByClause, SelectStatement, SortOrder,
        UQLStatement, WhereClause,
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
        let plan = lowerer.lower(&statement).expect("lower simple select");

        assert_eq!(plan.len(), 2);
        assert!(plan.validate().expect("validate plan").is_valid);
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
                    operator: UqlComparisonOperator::Lt,
                    value: Value::Number(1000.0),
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
        let plan = lowerer.lower(&statement).expect("lower select with filter");

        assert_eq!(plan.len(), 3);
        assert!(plan.validate().expect("validate plan").is_valid);
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
        let plan = lowerer
            .lower(&statement)
            .expect("lower select with order by");

        assert_eq!(plan.len(), 3);
        assert!(plan.validate().expect("validate plan").is_valid);
    }

    #[test]
    fn test_lowerer_uses_data_model_not_engine() {
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
        let plan = lowerer.lower(&statement).expect("lower document select");

        assert!(plan.validate().expect("validate plan").is_valid);
    }

    #[test]
    fn test_condition_to_filter_conversion() {
        let context = PlanContext::default();
        let lowerer = UQLLowerer::new(context);

        let condition = Condition::Comparison {
            field: "status".to_string(),
            operator: UqlComparisonOperator::Eq,
            value: Value::String("active".to_string()),
        };

        let core_filter = lowerer
            .convert_condition_to_filter(&condition)
            .expect("convert condition");

        match core_filter {
            FilterExpression::Comparison {
                field,
                operator,
                value,
            } => {
                assert_eq!(field, "status");
                assert_eq!(operator, ComparisonOperator::Equals);
                assert_eq!(value, serde_json::json!("active"));
            }
            _ => panic!("Expected Comparison expression"),
        }
    }
}
