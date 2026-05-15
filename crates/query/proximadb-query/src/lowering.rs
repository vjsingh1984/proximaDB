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
            data_model: from.model,
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
                data_model: join.source.model,
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

        for data_model in multimodal.components.keys() {
            let component_plan = MultiModelPlan::new(
                vec![Operator::Scan {
                    data_model: *data_model,
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

// =========== Phase 4: Engine-level RLS predicate pushdown ===========
//
// Per spec §8: `tenant_id` and `permitted_principals` are record fields.
// RLS predicates MUST be pushed into every Scan and VectorTopK at planning
// time — not applied post-hoc at the application layer.

/// Row-level security context carried by each query request.
///
/// The planner calls `push_rls_predicates` to inject these predicates into
/// every scan/projection operator before plan execution so the storage engine
/// never returns records outside the caller's security boundary.
#[derive(Debug, Clone)]
pub struct RlsContext {
    /// Tenant identifier. Non-empty string injects `tenant_id = value` into scans.
    pub tenant_id: String,
    /// Principals the caller belongs to. Non-empty vec injects
    /// `permitted_principals IN [p1, p2, ...]` into scans.
    pub permitted_principals: Vec<String>,
}

impl RlsContext {
    /// Create a tenant-only RLS context (no principal list).
    pub fn for_tenant(tenant_id: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            permitted_principals: vec![],
        }
    }

    /// Create an RLS context with both tenant and principal list.
    pub fn with_principals(tenant_id: impl Into<String>, principals: Vec<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            permitted_principals: principals,
        }
    }

    /// Returns `true` if this context would inject any predicate.
    pub fn is_active(&self) -> bool {
        !self.tenant_id.is_empty() || !self.permitted_principals.is_empty()
    }
}

/// Build the `FilterExpression` that represents the RLS predicates in `ctx`.
///
/// Returns `None` when the context is empty (unauthenticated/system bypass).
pub fn build_rls_filter(ctx: &RlsContext) -> Option<FilterExpression> {
    let mut parts = Vec::new();

    if !ctx.tenant_id.is_empty() {
        parts.push(FilterExpression::Comparison {
            field: "tenant_id".to_string(),
            operator: ComparisonOperator::Equals,
            value: serde_json::Value::String(ctx.tenant_id.clone()),
        });
    }

    if !ctx.permitted_principals.is_empty() {
        let principals: Vec<serde_json::Value> = ctx
            .permitted_principals
            .iter()
            .map(|p| serde_json::Value::String(p.clone()))
            .collect();
        parts.push(FilterExpression::Comparison {
            field: "permitted_principals".to_string(),
            operator: ComparisonOperator::In,
            value: serde_json::Value::Array(principals),
        });
    }

    match parts.len() {
        0 => None,
        1 => Some(parts.into_iter().next().unwrap()),
        _ => Some(FilterExpression::And(parts)),
    }
}

/// Inject RLS predicates into every `Scan` and `VectorTopK` operator in `plan`.
///
/// For `Scan`: the RLS predicate is ANDed with any existing `filter` field so
/// the storage engine enforces tenant isolation during the scan.
/// For `VectorTopK`: the RLS predicate is ANDed with the `predicate` field so
/// the HNSW navigator excludes cross-tenant vectors.
///
/// This is a no-op when `ctx.is_active()` returns `false`.
pub fn push_rls_predicates(plan: &mut MultiModelPlan, ctx: &RlsContext) {
    if !ctx.is_active() {
        return;
    }

    let Some(rls_filter) = build_rls_filter(ctx) else {
        return;
    };

    for op in &mut plan.operators {
        match op {
            Operator::Scan { filter, .. } => {
                *filter = Some(and_with_existing(filter.take(), rls_filter.clone()));
            }
            Operator::VectorTopK { predicate, .. } => {
                *predicate = Some(and_with_existing(predicate.take(), rls_filter.clone()));
            }
            _ => {}
        }
    }
}

/// AND `new_expr` with `existing`, or return `new_expr` if `existing` is `None`.
fn and_with_existing(
    existing: Option<FilterExpression>,
    new_expr: FilterExpression,
) -> FilterExpression {
    match existing {
        None => new_expr,
        Some(FilterExpression::And(mut parts)) => {
            parts.push(new_expr);
            FilterExpression::And(parts)
        }
        Some(other) => FilterExpression::And(vec![other, new_expr]),
    }
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

    // ── Phase 4: RLS predicate pushdown tests ────────────────────────────────

    use proximadb_multimodel_plan::VectorMetric;

    fn scan_op(with_filter: bool) -> Operator {
        Operator::Scan {
            data_model: DataModel::Vector,
            source: "embeddings".to_string(),
            columns: None,
            filter: if with_filter {
                Some(FilterExpression::Comparison {
                    field: "category".to_string(),
                    operator: ComparisonOperator::Equals,
                    value: serde_json::json!("ai"),
                })
            } else {
                None
            },
        }
    }

    fn vector_topk_op(with_predicate: bool) -> Operator {
        Operator::VectorTopK {
            query_vector: vec![0.1, 0.2],
            k: 10,
            metric: VectorMetric::Cosine,
            predicate: if with_predicate {
                Some(FilterExpression::Comparison {
                    field: "lang".to_string(),
                    operator: ComparisonOperator::Equals,
                    value: serde_json::json!("en"),
                })
            } else {
                None
            },
        }
    }

    #[test]
    fn rls_injects_tenant_id_into_scan_with_no_existing_filter() {
        let mut plan = MultiModelPlan::new(vec![scan_op(false)], PlanContext::default());
        push_rls_predicates(&mut plan, &RlsContext::for_tenant("acme"));

        match &plan.operators[0] {
            Operator::Scan {
                filter: Some(f), ..
            } => match f {
                FilterExpression::Comparison {
                    field,
                    operator,
                    value,
                } => {
                    assert_eq!(field, "tenant_id");
                    assert_eq!(*operator, ComparisonOperator::Equals);
                    assert_eq!(*value, serde_json::json!("acme"));
                }
                other => panic!("expected Comparison, got {:?}", other),
            },
            other => panic!("expected Scan with filter, got {:?}", other),
        }
    }

    #[test]
    fn rls_ands_tenant_id_with_existing_scan_filter() {
        let mut plan = MultiModelPlan::new(vec![scan_op(true)], PlanContext::default());
        push_rls_predicates(&mut plan, &RlsContext::for_tenant("acme"));

        match &plan.operators[0] {
            Operator::Scan {
                filter: Some(FilterExpression::And(parts)),
                ..
            } => {
                assert_eq!(parts.len(), 2);
                // One part should be the existing category filter
                // One part should be the tenant_id filter
                let has_category = parts.iter().any(|p| {
                    matches!(p, FilterExpression::Comparison { field, .. } if field == "category")
                });
                let has_tenant = parts.iter().any(|p| {
                    matches!(p, FilterExpression::Comparison { field, .. } if field == "tenant_id")
                });
                assert!(has_category, "existing filter must be preserved");
                assert!(has_tenant, "tenant_id must be injected");
            }
            other => panic!("expected Scan with And filter, got {:?}", other),
        }
    }

    #[test]
    fn rls_injects_tenant_id_into_vector_topk_predicate() {
        let mut plan = MultiModelPlan::new(vec![vector_topk_op(false)], PlanContext::default());
        push_rls_predicates(&mut plan, &RlsContext::for_tenant("acme"));

        match &plan.operators[0] {
            Operator::VectorTopK {
                predicate: Some(FilterExpression::Comparison { field, .. }),
                ..
            } => {
                assert_eq!(field, "tenant_id");
            }
            other => panic!("expected VectorTopK with tenant predicate, got {:?}", other),
        }
    }

    #[test]
    fn rls_ands_with_existing_vector_topk_predicate() {
        let mut plan = MultiModelPlan::new(vec![vector_topk_op(true)], PlanContext::default());
        push_rls_predicates(&mut plan, &RlsContext::for_tenant("acme"));

        match &plan.operators[0] {
            Operator::VectorTopK {
                predicate: Some(FilterExpression::And(parts)),
                ..
            } => {
                assert_eq!(parts.len(), 2);
            }
            other => panic!("expected VectorTopK with And predicate, got {:?}", other),
        }
    }

    #[test]
    fn rls_with_principals_injects_in_predicate() {
        let ctx =
            RlsContext::with_principals("acme", vec!["admin".to_string(), "editor".to_string()]);
        let mut plan = MultiModelPlan::new(vec![scan_op(false)], PlanContext::default());
        push_rls_predicates(&mut plan, &ctx);

        // tenant_id + permitted_principals → And([tenant_eq, principals_in])
        match &plan.operators[0] {
            Operator::Scan {
                filter: Some(FilterExpression::And(parts)),
                ..
            } => {
                assert_eq!(parts.len(), 2);
                let has_principals = parts.iter().any(|p| {
                    matches!(p, FilterExpression::Comparison { field, operator, .. }
                        if field == "permitted_principals" && *operator == ComparisonOperator::In)
                });
                assert!(has_principals, "permitted_principals IN predicate expected");
            }
            other => panic!("expected And filter, got {:?}", other),
        }
    }

    #[test]
    fn rls_noop_for_empty_context() {
        let ctx = RlsContext::for_tenant("");
        let mut plan = MultiModelPlan::new(vec![scan_op(false)], PlanContext::default());
        push_rls_predicates(&mut plan, &ctx);

        // No filter injected
        match &plan.operators[0] {
            Operator::Scan { filter: None, .. } => {}
            other => panic!(
                "expected Scan with no filter for empty ctx, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn rls_does_not_touch_non_scan_operators() {
        let mut plan = MultiModelPlan::new(
            vec![
                scan_op(false),
                Operator::Filter {
                    expression: FilterExpression::Comparison {
                        field: "score".to_string(),
                        operator: ComparisonOperator::GreaterThan,
                        value: serde_json::json!(0.5),
                    },
                },
            ],
            PlanContext::default(),
        );
        push_rls_predicates(&mut plan, &RlsContext::for_tenant("acme"));

        // Filter operator must remain unchanged
        match &plan.operators[1] {
            Operator::Filter {
                expression: FilterExpression::Comparison { field, .. },
            } => {
                assert_eq!(field, "score", "Filter operator must not be modified");
            }
            other => panic!("Filter operator unexpectedly changed: {:?}", other),
        }
    }
}
