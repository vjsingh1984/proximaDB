//! # P4 — Shared Logical Lowering: `LogicalNode` → DataFusion `LogicalPlan`
//!
//! Lowers ProximaDB's relational algebra (`proximadb_relational_algebra::LogicalNode`,
//! produced by the shared frontend that also feeds the Volcano executor) into a DataFusion
//! `LogicalPlan`. This discharges the course-correction §5 "shared logical plane, split
//! physical plane" guarantee in code: SQL and (later) the PySpark-style DataFrame API lower
//! to ONE algebra, and the router picks the physical engine — Volcano runs the `LogicalNode`
//! directly, DataFusion runs the lowered `LogicalPlan`.
//!
//! ## Scope
//! Covers the OLAP shapes the P1 route targets:
//! `Scan / Filter / Project / Aggregate / Sort / Limit` and
//! `Join` (Inner/Left/Right/Full/Cross via `join_on`/`cross_join`), with `Expr`
//! translation for `Column / Literal / BinaryOp / UnaryOp / IsNull`. Anything else
//! (Semi/Anti join, Distinct, Union, Values, CTEs;
//! `Cast/Between/In/Like/Case/Coalesce/NullIf/FuncCall`; `StringAgg/Custom` aggregates)
//! returns [`DataFusionError::NotImplemented`] so the caller keeps the existing
//! `ctx.sql(...)` path for those — additive, never wrong.

use datafusion::error::{DataFusionError, Result as DFResult};
use datafusion::logical_expr::{
    Expr, JoinType, LogicalPlan, LogicalPlanBuilder, Operator, binary_expr, col, lit,
};
use datafusion::prelude::SessionContext;
use datafusion::scalar::ScalarValue;

use proximadb_data_model::ProximaValue;
use proximadb_relational_algebra::{AggregateExpr, JoinKind, LogicalNode, NamedAggregate};
use proximadb_relational_types::{BinaryOp as RBinOp, Expr as RExpr, UnaryOp as RUnOp};

fn unsupported(what: impl Into<String>) -> DataFusionError {
    DataFusionError::NotImplemented(format!("logical_lowering: {}", what.into()))
}

/// Lower a relational `LogicalNode` to a DataFusion `LogicalPlan`, resolving `Scan` leaves
/// against the tables registered in `ctx`.
pub async fn lower_logical_node(ctx: &SessionContext, node: &LogicalNode) -> DFResult<LogicalPlan> {
    lower(ctx, node).await
}

// Boxed recursion: an `async fn` calling itself needs an explicit boxed future.
fn lower<'a>(
    ctx: &'a SessionContext,
    node: &'a LogicalNode,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = DFResult<LogicalPlan>> + Send + 'a>> {
    Box::pin(async move {
        match node {
            LogicalNode::Scan { table, .. } => {
                // Resolve the registered table provider by name and build a scan over it.
                let provider = ctx.table_provider(table.name.as_str()).await?;
                let source = datafusion::datasource::provider_as_source(provider);
                LogicalPlanBuilder::scan(table.name.as_str(), source, None)?.build()
            }
            LogicalNode::Filter { input, predicate } => {
                let input = lower(ctx, input).await?;
                LogicalPlanBuilder::from(input)
                    .filter(lower_expr(predicate)?)?
                    .build()
            }
            LogicalNode::Project { input, outputs } => {
                let input = lower(ctx, input).await?;
                let exprs = outputs
                    .iter()
                    .map(|o| Ok(lower_expr(&o.expr)?.alias(&o.name)))
                    .collect::<DFResult<Vec<Expr>>>()?;
                LogicalPlanBuilder::from(input).project(exprs)?.build()
            }
            LogicalNode::Aggregate {
                input,
                group_by,
                aggregates,
                having,
            } => {
                let input = lower(ctx, input).await?;
                let groups = group_by
                    .iter()
                    .map(|g| lower_expr(&g.expr))
                    .collect::<DFResult<Vec<Expr>>>()?;
                let aggs = aggregates
                    .iter()
                    .map(lower_aggregate)
                    .collect::<DFResult<Vec<Expr>>>()?;
                let mut builder = LogicalPlanBuilder::from(input).aggregate(groups, aggs)?;
                if let Some(h) = having {
                    builder = builder.filter(lower_expr(h)?)?;
                }
                builder.build()
            }
            LogicalNode::Sort { input, keys } => {
                let input = lower(ctx, input).await?;
                let sorts = keys
                    .iter()
                    .map(|k| Ok(lower_expr(&k.expr)?.sort(!k.descending, k.nulls_first)))
                    .collect::<DFResult<Vec<_>>>()?;
                LogicalPlanBuilder::from(input).sort(sorts)?.build()
            }
            LogicalNode::Limit {
                input,
                limit,
                offset,
            } => {
                let input = lower(ctx, input).await?;
                let fetch = limit.map(|l| l as usize);
                LogicalPlanBuilder::from(input)
                    .limit(*offset as usize, fetch)?
                    .build()
            }
            LogicalNode::Join {
                left,
                right,
                kind,
                on,
                strategy: _, // physical hint; DataFusion picks its own join algorithm
            } => {
                let left_plan = lower(ctx, left).await?;
                let right_plan = lower(ctx, right).await?;
                let join_type = match kind {
                    JoinKind::Inner => JoinType::Inner,
                    JoinKind::Left => JoinType::Left,
                    JoinKind::Right => JoinType::Right,
                    JoinKind::Full => JoinType::Full,
                    // CROSS JOIN carries no ON predicate.
                    JoinKind::Cross => {
                        return LogicalPlanBuilder::from(left_plan)
                            .cross_join(right_plan)?
                            .build();
                    }
                    // Semi/Anti come from IN / NOT IN / EXISTS subqueries — leave to
                    // the `ctx.sql` fallback until subquery lowering exists.
                    JoinKind::Semi | JoinKind::Anti => {
                        return Err(unsupported("Semi/Anti join (use ctx.sql path)"));
                    }
                };
                match on {
                    // A join condition the lowering can't translate errors out → the
                    // caller falls back to `ctx.sql` (additive, never wrong).
                    Some(predicate) => LogicalPlanBuilder::from(left_plan)
                        .join_on(right_plan, join_type, [lower_expr(predicate)?])?
                        .build(),
                    // No ON predicate on an inner-family join == a cross join.
                    None => LogicalPlanBuilder::from(left_plan)
                        .cross_join(right_plan)?
                        .build(),
                }
            }
            LogicalNode::Distinct { .. } => Err(unsupported("Distinct")),
            LogicalNode::Union { .. } => Err(unsupported("Union")),
            LogicalNode::Values { .. } => Err(unsupported("Values")),
            LogicalNode::CteBind { .. } => Err(unsupported("CteBind")),
            LogicalNode::CteRef { .. } => Err(unsupported("CteRef")),
        }
    })
}

/// Translate a named aggregate to a DataFusion aggregate `Expr`, aliased to its output name.
fn lower_aggregate(named: &NamedAggregate) -> DFResult<Expr> {
    use datafusion::functions_aggregate::expr_fn::{avg, count, count_distinct, max, min, sum};
    let expr = match &named.agg {
        AggregateExpr::Count { arg, distinct } => match (arg, distinct) {
            (None, _) => count(lit(1i64)), // COUNT(*) — count of a constant
            (Some(a), true) => count_distinct(lower_expr(a)?),
            (Some(a), false) => count(lower_expr(a)?),
        },
        AggregateExpr::Sum { arg, distinct } => {
            let e = lower_expr(arg)?;
            if *distinct {
                return Err(unsupported("SUM(DISTINCT)"));
            }
            sum(e)
        }
        AggregateExpr::Avg { arg, distinct } => {
            let e = lower_expr(arg)?;
            if *distinct {
                return Err(unsupported("AVG(DISTINCT)"));
            }
            avg(e)
        }
        AggregateExpr::Min { arg } => min(lower_expr(arg)?),
        AggregateExpr::Max { arg } => max(lower_expr(arg)?),
        AggregateExpr::StringAgg { .. } => return Err(unsupported("StringAgg")),
        AggregateExpr::Custom { name, .. } => {
            return Err(unsupported(format!("custom aggregate {name}")));
        }
    };
    Ok(expr.alias(&named.name))
}

/// Translate a relational `Expr` to a DataFusion `Expr` (first-slice subset).
fn lower_expr(e: &RExpr) -> DFResult<Expr> {
    Ok(match e {
        RExpr::Column(c) => col(c.name.as_str()),
        RExpr::Literal { value, .. } => lit(proxima_value_to_scalar(value)?),
        RExpr::BinaryOp { op, left, right } => {
            let l = lower_expr(left)?;
            let r = lower_expr(right)?;
            binary_expr(l, lower_binary_op(op)?, r)
        }
        RExpr::UnaryOp { op, expr } => {
            let inner = lower_expr(expr)?;
            match op {
                RUnOp::Not => !inner,
                RUnOp::Neg => Expr::Negative(Box::new(inner)),
            }
        }
        RExpr::IsNull { expr, not } => {
            let inner = lower_expr(expr)?;
            if *not {
                inner.is_not_null()
            } else {
                inner.is_null()
            }
        }
        RExpr::Cast { .. } => return Err(unsupported("Cast")),
        RExpr::Between { .. } => return Err(unsupported("Between")),
        RExpr::In { .. } => return Err(unsupported("In")),
        RExpr::Like { .. } => return Err(unsupported("Like")),
        RExpr::Case { .. } => return Err(unsupported("Case")),
        RExpr::Coalesce(_) => return Err(unsupported("Coalesce")),
        RExpr::NullIf { .. } => return Err(unsupported("NullIf")),
        RExpr::FuncCall { name, .. } => return Err(unsupported(format!("function {name}"))),
    })
}

fn lower_binary_op(op: &RBinOp) -> DFResult<Operator> {
    Ok(match op {
        RBinOp::Plus => Operator::Plus,
        RBinOp::Minus => Operator::Minus,
        RBinOp::Mul => Operator::Multiply,
        RBinOp::Div => Operator::Divide,
        RBinOp::Mod => Operator::Modulo,
        RBinOp::Eq => Operator::Eq,
        RBinOp::NotEq => Operator::NotEq,
        RBinOp::Lt => Operator::Lt,
        RBinOp::LtEq => Operator::LtEq,
        RBinOp::Gt => Operator::Gt,
        RBinOp::GtEq => Operator::GtEq,
        RBinOp::And => Operator::And,
        RBinOp::Or => Operator::Or,
        RBinOp::Concat => return Err(unsupported("|| (concat)")),
    })
}

/// Convert a `ProximaValue` literal to a DataFusion `ScalarValue` (common scalar types).
fn proxima_value_to_scalar(v: &ProximaValue) -> DFResult<ScalarValue> {
    Ok(match v {
        ProximaValue::Null => ScalarValue::Null,
        ProximaValue::Boolean(b) => ScalarValue::Boolean(Some(*b)),
        ProximaValue::Int8(x) => ScalarValue::Int8(Some(*x)),
        ProximaValue::Int16(x) => ScalarValue::Int16(Some(*x)),
        ProximaValue::Int32(x) => ScalarValue::Int32(Some(*x)),
        ProximaValue::Int64(x) => ScalarValue::Int64(Some(*x)),
        ProximaValue::UInt8(x) => ScalarValue::UInt8(Some(*x)),
        ProximaValue::UInt16(x) => ScalarValue::UInt16(Some(*x)),
        ProximaValue::UInt32(x) => ScalarValue::UInt32(Some(*x)),
        ProximaValue::UInt64(x) => ScalarValue::UInt64(Some(*x)),
        ProximaValue::Float32(x) => ScalarValue::Float32(Some(*x)),
        ProximaValue::Float64(x) => ScalarValue::Float64(Some(*x)),
        ProximaValue::String(s) | ProximaValue::Symbol(s) => ScalarValue::Utf8(Some(s.clone())),
        other => return Err(unsupported(format!("literal {other:?}"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Float64Array, RecordBatch, StringArray};
    use arrow_schema::{DataType, Field, Schema};
    use datafusion::datasource::MemTable;
    use proximadb_data_model::ProximaType;
    use proximadb_relational_algebra::{
        JoinKind, JoinStrategy, LogicalNode, NamedExpr, SortKey, TableId,
    };
    use proximadb_relational_types::{ColumnInfo, ColumnRef, RelationalSchema};
    use std::sync::Arc;

    fn colref(name: &str, ordinal: usize, ty: ProximaType) -> ColumnRef {
        ColumnRef {
            name: name.to_string(),
            ordinal,
            ty,
            nullable: false,
        }
    }

    async fn ctx_with_t() -> SessionContext {
        let schema = Arc::new(Schema::new(vec![
            Field::new("k", DataType::Utf8, false),
            Field::new("x", DataType::Float64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a", "a", "b"])),
                Arc::new(Float64Array::from(vec![1.0, 3.0, 10.0])),
            ],
        )
        .unwrap();
        let mem = MemTable::try_new(schema, vec![vec![batch]]).unwrap();
        let ctx = SessionContext::new();
        ctx.register_table("t", Arc::new(mem)).unwrap();
        ctx
    }

    fn scan_t() -> LogicalNode {
        LogicalNode::Scan {
            table: TableId::new("t"),
            table_schema: RelationalSchema::new(vec![
                ColumnInfo::new("k", ProximaType::String, false),
                ColumnInfo::new("x", ProximaType::Float64, false),
            ]),
            projected_columns: None,
            predicate: None,
        }
    }

    #[tokio::test]
    async fn lowers_scan_filter_project() {
        let ctx = ctx_with_t().await;
        // SELECT k FROM t WHERE x > 2.0
        let node = LogicalNode::Project {
            input: Box::new(LogicalNode::Filter {
                input: Box::new(scan_t()),
                predicate: RExpr::BinaryOp {
                    op: RBinOp::Gt,
                    left: Box::new(RExpr::Column(colref("x", 1, ProximaType::Float64))),
                    right: Box::new(RExpr::Literal {
                        value: ProximaValue::Float64(2.0),
                        ty: ProximaType::Float64,
                    }),
                },
            }),
            outputs: vec![NamedExpr {
                name: "k".to_string(),
                expr: RExpr::Column(colref("k", 0, ProximaType::String)),
            }],
        };
        let plan = lower_logical_node(&ctx, &node).await.unwrap();
        let batches = ctx
            .execute_logical_plan(plan)
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        let total: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total, 2); // x=3.0 (a) and x=10.0 (b)
    }

    #[tokio::test]
    async fn lowers_inner_join_on() {
        // t(k,x) ⋈ u(j,y) ON k = j  (distinct join-key names avoid ambiguity).
        let ctx = SessionContext::new();
        let t_schema = Arc::new(Schema::new(vec![
            Field::new("k", DataType::Utf8, false),
            Field::new("x", DataType::Float64, false),
        ]));
        let t_batch = RecordBatch::try_new(
            t_schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a", "a", "b"])),
                Arc::new(Float64Array::from(vec![1.0, 3.0, 10.0])),
            ],
        )
        .unwrap();
        ctx.register_table(
            "t",
            Arc::new(MemTable::try_new(t_schema, vec![vec![t_batch]]).unwrap()),
        )
        .unwrap();
        let u_schema = Arc::new(Schema::new(vec![
            Field::new("j", DataType::Utf8, false),
            Field::new("y", DataType::Float64, false),
        ]));
        let u_batch = RecordBatch::try_new(
            u_schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a", "b", "b"])),
                Arc::new(Float64Array::from(vec![100.0, 200.0, 300.0])),
            ],
        )
        .unwrap();
        ctx.register_table(
            "u",
            Arc::new(MemTable::try_new(u_schema, vec![vec![u_batch]]).unwrap()),
        )
        .unwrap();

        let scan_u = LogicalNode::Scan {
            table: TableId::new("u"),
            table_schema: RelationalSchema::new(vec![
                ColumnInfo::new("j", ProximaType::String, false),
                ColumnInfo::new("y", ProximaType::Float64, false),
            ]),
            projected_columns: None,
            predicate: None,
        };
        let node = LogicalNode::Join {
            left: Box::new(scan_t()),
            right: Box::new(scan_u),
            kind: JoinKind::Inner,
            on: Some(RExpr::BinaryOp {
                op: RBinOp::Eq,
                left: Box::new(RExpr::Column(colref("k", 0, ProximaType::String))),
                right: Box::new(RExpr::Column(colref("j", 0, ProximaType::String))),
            }),
            strategy: JoinStrategy::Auto,
        };
        let plan = lower_logical_node(&ctx, &node).await.unwrap();
        let batches = ctx
            .execute_logical_plan(plan)
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        let total: usize = batches.iter().map(|b| b.num_rows()).sum();
        // a⋈a (t has 2 a-rows × u has 1 a-row = 2) + b⋈b (1 × 2 = 2) = 4.
        assert_eq!(total, 4);
    }

    #[tokio::test]
    async fn lowers_aggregate_sort() {
        let ctx = ctx_with_t().await;
        // SELECT k, sum(x) AS s FROM t GROUP BY k ORDER BY k
        let agg = LogicalNode::Aggregate {
            input: Box::new(scan_t()),
            group_by: vec![NamedExpr {
                name: "k".to_string(),
                expr: RExpr::Column(colref("k", 0, ProximaType::String)),
            }],
            aggregates: vec![NamedAggregate {
                name: "s".to_string(),
                agg: AggregateExpr::Sum {
                    arg: RExpr::Column(colref("x", 1, ProximaType::Float64)),
                    distinct: false,
                },
            }],
            having: None,
        };
        let node = LogicalNode::Sort {
            input: Box::new(agg),
            keys: vec![SortKey {
                expr: RExpr::Column(colref("k", 0, ProximaType::String)),
                descending: false,
                nulls_first: false,
            }],
        };
        let plan = lower_logical_node(&ctx, &node).await.unwrap();
        let batches = ctx
            .execute_logical_plan(plan)
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        let total: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total, 2); // groups: a, b
    }

    #[tokio::test]
    async fn distinct_is_unsupported_for_now() {
        // (Inner/Left/Right/Full/Cross joins now lower; Distinct still falls back.)
        let ctx = ctx_with_t().await;
        let node = LogicalNode::Distinct {
            input: Box::new(scan_t()),
        };
        assert!(lower_logical_node(&ctx, &node).await.is_err());
    }
}
