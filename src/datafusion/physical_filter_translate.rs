//! Translate a resolved DataFusion [`PhysicalExpr`] into the engine-neutral
//! split-pruning vocabulary (TD-OLAP-3 slice B).
//!
//! DataFusion 54's join runtime filter (`DynamicFilterPhysicalExpr`) resolves
//! at stream-open time to a conjunction of per-column min/max bounds and/or an
//! IN-list over the HashJoin build keys. This module walks that resolved
//! expression and extracts `(column, ScalarPredicate)` pairs that
//! `FileSplit::can_prune_scalar` understands — turning the join's build-side
//! knowledge into row-group skips *before* fetch.
//!
//! **Conservative by construction**: any expression shape this walker does not
//! recognize (OR trees, casts, non-column operands, negated lists, …)
//! contributes NO predicates — a miss can only cost bytes, never correctness.
//! The pruning consumer additionally re-applies exact filtering above the scan.

use std::sync::Arc;

use datafusion::common::ScalarValue as DfScalarValue;
use datafusion::logical_expr::Operator;
use datafusion::physical_expr::PhysicalExpr;
use datafusion::physical_expr::expressions::{BinaryExpr, Column, InListExpr, Literal};
use proximadb_storage_common::format_splits::{ScalarPredicate, ScalarValue};

/// Extract conjunctive `(column, predicate)` pruning pairs from a resolved
/// physical expression. Unrecognized shapes yield nothing (never a wrong skip).
pub(crate) fn pruning_predicates(expr: &Arc<dyn PhysicalExpr>) -> Vec<(String, ScalarPredicate)> {
    let mut out = Vec::new();
    collect(expr.as_ref(), &mut out);
    out
}

fn collect(expr: &dyn PhysicalExpr, out: &mut Vec<(String, ScalarPredicate)>) {
    if let Some(binary) = expr.downcast_ref::<BinaryExpr>() {
        match binary.op() {
            // A conjunction prunes if ANY side prunes — recurse both.
            Operator::And => {
                collect(binary.left().as_ref(), out);
                collect(binary.right().as_ref(), out);
            }
            // A disjunction cannot be decomposed into independent per-column
            // predicates without risking a wrong skip — contribute nothing.
            Operator::Or => {}
            op => {
                if let Some(pair) = comparison(binary.left(), *op, binary.right()) {
                    out.push(pair);
                }
            }
        }
        return;
    }
    if let Some(in_list) = expr.downcast_ref::<InListExpr>()
        && !in_list.negated()
        && let Some(column) = column_name(in_list.expr())
    {
        let values: Vec<ScalarValue> = in_list.list().iter().filter_map(literal_value).collect();
        // Only prune when EVERY list element translated — a partial list
        // would wrongly skip splits containing the untranslated values.
        if !values.is_empty() && values.len() == in_list.list().len() {
            out.push((column, ScalarPredicate::In(values)));
        }
    }
    // `lit(true)` (the dynamic filter's pre-build placeholder), columns,
    // casts, and anything else: no contribution.
}

fn comparison(
    left: &Arc<dyn PhysicalExpr>,
    op: Operator,
    right: &Arc<dyn PhysicalExpr>,
) -> Option<(String, ScalarPredicate)> {
    if let (Some(column), Some(value)) = (column_name(left), literal_value(right)) {
        return scalar_predicate(op, value).map(|p| (column, p));
    }
    if let (Some(value), Some(column)) = (literal_value(left), column_name(right)) {
        return scalar_predicate(reverse_operator(op), value).map(|p| (column, p));
    }
    None
}

fn scalar_predicate(op: Operator, value: ScalarValue) -> Option<ScalarPredicate> {
    match op {
        Operator::Eq => Some(ScalarPredicate::Equal(value)),
        Operator::NotEq => Some(ScalarPredicate::NotEqual(value)),
        Operator::Lt => Some(ScalarPredicate::LessThan(value)),
        Operator::LtEq => Some(ScalarPredicate::LessThanOrEqual(value)),
        Operator::Gt => Some(ScalarPredicate::GreaterThan(value)),
        Operator::GtEq => Some(ScalarPredicate::GreaterThanOrEqual(value)),
        _ => None,
    }
}

fn reverse_operator(op: Operator) -> Operator {
    match op {
        Operator::Lt => Operator::Gt,
        Operator::LtEq => Operator::GtEq,
        Operator::Gt => Operator::Lt,
        Operator::GtEq => Operator::LtEq,
        other => other,
    }
}

fn column_name(expr: &Arc<dyn PhysicalExpr>) -> Option<String> {
    expr.downcast_ref::<Column>().map(|c| c.name().to_string())
}

fn literal_value(expr: &Arc<dyn PhysicalExpr>) -> Option<ScalarValue> {
    expr.downcast_ref::<Literal>()
        .and_then(|l| df_scalar_value(l.value()))
}

/// Convert a DataFusion scalar into the engine-neutral pruning scalar.
/// Shared with the logical-side pruning in `object_store_parquet_reader`.
pub(crate) fn df_scalar_value(value: &DfScalarValue) -> Option<ScalarValue> {
    match value {
        DfScalarValue::Null => Some(ScalarValue::Null),
        DfScalarValue::Boolean(Some(v)) => Some(ScalarValue::Bool(*v)),
        DfScalarValue::Int8(Some(v)) => Some(ScalarValue::Int64(*v as i64)),
        DfScalarValue::Int16(Some(v)) => Some(ScalarValue::Int64(*v as i64)),
        DfScalarValue::Int32(Some(v)) => Some(ScalarValue::Int64(*v as i64)),
        DfScalarValue::Int64(Some(v)) => Some(ScalarValue::Int64(*v)),
        DfScalarValue::UInt8(Some(v)) => Some(ScalarValue::Int64(*v as i64)),
        DfScalarValue::UInt16(Some(v)) => Some(ScalarValue::Int64(*v as i64)),
        DfScalarValue::UInt32(Some(v)) => Some(ScalarValue::Int64(*v as i64)),
        DfScalarValue::UInt64(Some(v)) => i64::try_from(*v).ok().map(ScalarValue::Int64),
        DfScalarValue::Float32(Some(v)) => Some(ScalarValue::Float64(*v as f64)),
        DfScalarValue::Float64(Some(v)) => Some(ScalarValue::Float64(*v)),
        DfScalarValue::Utf8(Some(v))
        | DfScalarValue::Utf8View(Some(v))
        | DfScalarValue::LargeUtf8(Some(v)) => Some(ScalarValue::String(v.clone())),
        // Temporal literals prune against the Parquet footer's physical-integer
        // bounds (Date32 = days-since-epoch Int32 stats; timestamps = Int64
        // ticks in the column's stored unit). TPC-H filters are almost all
        // date-ranged — without these arms no date predicate ever prunes.
        DfScalarValue::Date32(Some(days)) => Some(ScalarValue::Int64(*days as i64)),
        DfScalarValue::Date64(Some(ms)) => Some(ScalarValue::Int64(*ms / 86_400_000)),
        DfScalarValue::TimestampSecond(Some(v), _)
        | DfScalarValue::TimestampMillisecond(Some(v), _)
        | DfScalarValue::TimestampMicrosecond(Some(v), _)
        | DfScalarValue::TimestampNanosecond(Some(v), _) => Some(ScalarValue::Int64(*v)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_schema::{DataType, Field, Schema};
    use datafusion::logical_expr::Operator;
    use datafusion::physical_expr::expressions::{col, lit};

    fn schema() -> Schema {
        Schema::new(vec![
            Field::new("k", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
        ])
    }

    fn binary(
        l: Arc<dyn PhysicalExpr>,
        op: Operator,
        r: Arc<dyn PhysicalExpr>,
    ) -> Arc<dyn PhysicalExpr> {
        Arc::new(BinaryExpr::new(l, op, r))
    }

    #[test]
    fn bounds_conjunction_translates_to_range_predicates() {
        let s = schema();
        // k >= 5 AND k <= 10 — the shape DF's join dynamic filter resolves to.
        let expr = binary(
            binary(col("k", &s).unwrap(), Operator::GtEq, lit(5i64)),
            Operator::And,
            binary(col("k", &s).unwrap(), Operator::LtEq, lit(10i64)),
        );
        let preds = pruning_predicates(&expr);
        assert_eq!(preds.len(), 2);
        assert_eq!(preds[0].0, "k");
        assert!(matches!(
            preds[0].1,
            ScalarPredicate::GreaterThanOrEqual(ScalarValue::Int64(5))
        ));
        assert!(matches!(
            preds[1].1,
            ScalarPredicate::LessThanOrEqual(ScalarValue::Int64(10))
        ));
    }

    #[test]
    fn in_list_translates_and_reversed_comparison_flips() {
        use datafusion::physical_expr::expressions::in_list;

        let s = schema();
        let in_expr = in_list(
            col("name", &s).unwrap(),
            vec![lit("a"), lit("b")],
            &false,
            &s,
        )
        .unwrap();
        let preds = pruning_predicates(&in_expr);
        assert_eq!(preds.len(), 1);
        match &preds[0].1 {
            ScalarPredicate::In(values) => {
                assert_eq!(values.len(), 2);
                assert!(matches!(&values[0], ScalarValue::String(v) if v == "a"));
                assert!(matches!(&values[1], ScalarValue::String(v) if v == "b"));
            }
            other => panic!("expected In predicate, got {other:?}"),
        }

        // 5 < k (literal on the left) flips to k > 5.
        let flipped = binary(lit(5i64), Operator::Lt, col("k", &s).unwrap());
        let preds = pruning_predicates(&flipped);
        assert!(matches!(
            preds[0].1,
            ScalarPredicate::GreaterThan(ScalarValue::Int64(5))
        ));
    }

    /// The EXACT post-build shape DF 54's join dynamic filter resolves to
    /// (observed live): `col >= lit AND col <= lit AND col IN (SET)(...)`
    /// with INT32 literals (the column's arrow type), left-associated ANDs.
    #[test]
    fn live_join_filter_shape_translates() {
        use datafusion::physical_expr::expressions::in_list;
        let s = Schema::new(vec![Field::new("ss_sold_date_sk", DataType::Int32, true)]);
        let c = || col("ss_sold_date_sk", &s).unwrap();
        let i32lit = |v: i32| lit(DfScalarValue::Int32(Some(v)));
        let bounds = binary(
            binary(c(), Operator::GtEq, i32lit(23617)),
            Operator::And,
            binary(c(), Operator::LtEq, i32lit(23644)),
        );
        let inset = in_list(c(), (23617..=23644).map(i32lit).collect(), &false, &s).unwrap();
        let expr = binary(bounds, Operator::And, inset);

        let preds = pruning_predicates(&expr);
        assert!(
            preds.len() >= 2,
            "bounds must translate from the live shape, got {preds:?}"
        );
        // And they must actually prune a disjoint row group.
        use proximadb_storage_common::format_splits::ColumnBounds;
        let bounds = ColumnBounds {
            min: Some(serde_json::json!(23001)),
            max: Some(serde_json::json!(23085)),
            null_count: 0,
            distinct_count: None,
        };
        assert!(
            preds
                .iter()
                .any(|(col, p)| col == "ss_sold_date_sk" && bounds.can_prune(p)),
            "a [23001,23085] group must prune under >= 23617: {preds:?}"
        );
    }

    #[test]
    fn unrecognized_shapes_contribute_nothing() {
        use datafusion::physical_expr::expressions::in_list;

        let s = schema();
        // OR cannot be decomposed conjunctively.
        let or_expr = binary(
            binary(col("k", &s).unwrap(), Operator::Eq, lit(1i64)),
            Operator::Or,
            binary(col("k", &s).unwrap(), Operator::Eq, lit(2i64)),
        );
        assert!(pruning_predicates(&or_expr).is_empty());
        // The dynamic filter's pre-build placeholder.
        let placeholder: Arc<dyn PhysicalExpr> = lit(true);
        assert!(pruning_predicates(&placeholder).is_empty());
        // Negated IN-list must not prune.
        let negated = in_list(col("k", &s).unwrap(), vec![lit(1i64)], &true, &s).unwrap();
        assert!(pruning_predicates(&negated).is_empty());
    }
}
