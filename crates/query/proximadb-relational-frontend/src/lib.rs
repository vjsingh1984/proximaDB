//! SQL → LogicalNode frontend for ProximaDB's relational query path
//! (ADR-019 L1 / S5a). Parses SQL with sqlparser, walks the AST,
//! and emits a [`LogicalNode`] from `proximadb-relational-algebra`.
//! The planner and executor crates take it from there.
//!
//! Scope (MVP):
//!
//! - `SELECT [DISTINCT] <projection> FROM <table>|<join> [WHERE …]
//!   [GROUP BY …] [HAVING …] [ORDER BY …] [LIMIT n OFFSET m]`
//! - `INNER JOIN` and `LEFT JOIN`
//! - `UNION [ALL]` set operations
//! - Top-level `VALUES`
//! - Expressions: Identifier / CompoundIdentifier (table.col),
//!   numeric / string / boolean / NULL literals, BinaryOp,
//!   UnaryOp, IsNull / IsNotNull, Between, InList, Like / ILike,
//!   Aggregate calls (COUNT/SUM/AVG/MIN/MAX), wildcard `*`.
//!
//! Out of scope for this slice — explicitly returned as
//! [`FrontendError::Unsupported`]:
//!
//! - Subqueries (need `AlgebraExpr::Sub` lowering).
//! - CTEs (parsed but rejected — Phase 3).
//! - Window functions, CASE/COALESCE/NULLIF expressions.
//! - DML (INSERT/UPDATE/DELETE) — separate write-path slice.
//! - DDL (CREATE TABLE / ALTER TABLE) — separate.
//!
//! Schema resolution: callers provide a [`CatalogLookup`] so we
//! don't depend on the runtime catalog crate from here.

use proximadb_data_model::{ProximaType, ProximaValue};
use proximadb_relational_algebra::{
    AggregateExpr, JoinKind, JoinStrategy, LogicalNode, NamedAggregate, NamedExpr, SortKey, TableId,
};
use proximadb_relational_types::{
    BinaryOp, ColumnInfo, ColumnRef, Expr, RelationalSchema, UnaryOp,
};
use sqlparser::ast::{
    BinaryOperator, Distinct, Expr as SqlExpr, FunctionArg, FunctionArgExpr, FunctionArguments,
    GroupByExpr, JoinConstraint, JoinOperator, LimitClause, ObjectName, OrderByKind,
    Query as SqlQuery, Select as SqlSelect, SelectItem, SetExpr, SetOperator, SetQuantifier,
    Statement, TableFactor, UnaryOperator, Value as SqlValue, ValueWithSpan,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;
use thiserror::Error;

// =========================================================================
// Errors
// =========================================================================

#[derive(Debug, Error, Clone, PartialEq)]
pub enum FrontendError {
    #[error("SQL parse error: {0}")]
    Parse(String),

    #[error("table not found: {0}")]
    TableNotFound(String),

    #[error("column not found: {0}")]
    ColumnNotFound(String),

    #[error("ambiguous column reference: {0}")]
    AmbiguousColumn(String),

    #[error("unsupported SQL feature: {0}")]
    Unsupported(String),

    #[error("invalid literal: {0}")]
    InvalidLiteral(String),

    #[error("expected exactly one statement, got {0}")]
    StatementCount(usize),

    #[error("type error: {0}")]
    Type(String),
}

// =========================================================================
// Catalog lookup trait
// =========================================================================

/// Resolve a table name to its schema. The frontend stays pure
/// — no catalog crate dependency — by taking this trait object.
pub trait CatalogLookup {
    fn lookup_table(&self, name: &str) -> Option<RelationalSchema>;
}

/// Simple in-memory catalog for tests and standalone use.
#[derive(Debug, Default, Clone)]
pub struct InMemoryCatalog {
    tables: std::collections::HashMap<String, RelationalSchema>,
}

impl InMemoryCatalog {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register(&mut self, name: impl Into<String>, schema: RelationalSchema) {
        self.tables.insert(name.into(), schema);
    }
}

impl CatalogLookup for InMemoryCatalog {
    fn lookup_table(&self, name: &str) -> Option<RelationalSchema> {
        self.tables.get(name).cloned()
    }
}

// =========================================================================
// Entry point
// =========================================================================

/// Parse one SQL statement and lower it to a [`LogicalNode`].
/// Errors if the input has zero or multiple statements.
pub fn lower_sql(sql: &str, catalog: &dyn CatalogLookup) -> Result<LogicalNode, FrontendError> {
    let statements = Parser::parse_sql(&GenericDialect {}, sql)
        .map_err(|e| FrontendError::Parse(e.to_string()))?;
    if statements.len() != 1 {
        return Err(FrontendError::StatementCount(statements.len()));
    }
    lower_statement(&statements[0], catalog)
}

fn lower_statement(
    stmt: &Statement,
    catalog: &dyn CatalogLookup,
) -> Result<LogicalNode, FrontendError> {
    match stmt {
        Statement::Query(q) => lower_query(q, catalog),
        Statement::Insert(_) | Statement::Update { .. } | Statement::Delete(_) => Err(
            FrontendError::Unsupported("DML (INSERT/UPDATE/DELETE)".into()),
        ),
        other => Err(FrontendError::Unsupported(format!(
            "statement kind {:?}",
            std::mem::discriminant(other)
        ))),
    }
}

// =========================================================================
// Query (top-level): SELECT / VALUES / UNION
// =========================================================================

fn lower_query(
    query: &SqlQuery,
    catalog: &dyn CatalogLookup,
) -> Result<LogicalNode, FrontendError> {
    if query.with.is_some() {
        return Err(FrontendError::Unsupported("WITH (CTE)".into()));
    }
    let mut body = lower_set_expr(&query.body, catalog)?;
    // ORDER BY (only at top of the query)
    if let Some(order_by) = &query.order_by {
        body = apply_order_by(body, order_by, catalog)?;
    }
    // LIMIT / OFFSET
    if let Some(limit_clause) = &query.limit_clause {
        body = apply_limit(body, limit_clause)?;
    }
    Ok(body)
}

fn lower_set_expr(
    set: &SetExpr,
    catalog: &dyn CatalogLookup,
) -> Result<LogicalNode, FrontendError> {
    match set {
        SetExpr::Select(s) => lower_select(s, catalog),
        SetExpr::Query(q) => lower_query(q, catalog),
        SetExpr::Values(v) => lower_values(v),
        SetExpr::SetOperation {
            op,
            set_quantifier,
            left,
            right,
        } => {
            if !matches!(op, SetOperator::Union) {
                return Err(FrontendError::Unsupported(format!("set operator {:?}", op)));
            }
            let l = lower_set_expr(left, catalog)?;
            let r = lower_set_expr(right, catalog)?;
            let all = matches!(set_quantifier, SetQuantifier::All);
            Ok(LogicalNode::Union {
                inputs: vec![l, r],
                all,
            })
        }
        other => Err(FrontendError::Unsupported(format!(
            "set-expr {:?}",
            std::mem::discriminant(other)
        ))),
    }
}

fn lower_values(values: &sqlparser::ast::Values) -> Result<LogicalNode, FrontendError> {
    let mut rows: Vec<Vec<Expr>> = Vec::with_capacity(values.rows.len());
    // VALUES rows live in an empty-schema scope; expressions must
    // be self-contained (literal-only at MVP).
    let empty_scope = Scope::empty();
    let mut col_types: Vec<ProximaType> = Vec::new();
    for (i, row) in values.rows.iter().enumerate() {
        let mut lowered = Vec::with_capacity(row.len());
        for (j, e) in row.iter().enumerate() {
            let expr = lower_expr(e, &empty_scope)?;
            if i == 0 {
                col_types.push(expr.result_type());
            }
            lowered.push(expr);
            let _ = j;
        }
        rows.push(lowered);
    }
    let cols: Vec<ColumnInfo> = col_types
        .into_iter()
        .enumerate()
        .map(|(i, ty)| ColumnInfo {
            name: format!("column{}", i + 1),
            ty,
            nullable: true,
        })
        .collect();
    Ok(LogicalNode::Values {
        rows,
        output_schema: RelationalSchema::new(cols),
    })
}

// =========================================================================
// SELECT
// =========================================================================

fn lower_select(
    select: &SqlSelect,
    catalog: &dyn CatalogLookup,
) -> Result<LogicalNode, FrontendError> {
    // 1) FROM (single or with joins). Empty FROM is unsupported.
    if select.from.is_empty() {
        return Err(FrontendError::Unsupported("SELECT without FROM".into()));
    }
    if select.from.len() > 1 {
        return Err(FrontendError::Unsupported(
            "comma-separated FROM (use explicit JOIN)".into(),
        ));
    }
    let twj = &select.from[0];
    let (scan, scope) = lower_table_with_joins(twj, catalog)?;
    let mut plan = scan;
    let mut scope = scope;

    // 2) WHERE — lift uncorrelated IN / EXISTS / NOT EXISTS subqueries that appear
    //    as top-level AND-conjuncts into Semi/Anti joins; the remaining conjuncts
    //    become a Filter. A subquery that can't be lifted (correlated, NOT IN,
    //    multi-column, or nested under OR/NOT) stays in the filter predicate, where
    //    `lower_expr` rejects it — so the whole query falls through to the legacy
    //    path rather than silently mis-evaluating.
    if let Some(where_expr) = &select.selection {
        let mut filter_conjuncts: Vec<&SqlExpr> = Vec::new();
        for conj in flatten_sql_and(where_expr) {
            match lower_subquery_join_parts(conj, &scope, catalog)? {
                Some((kind, on, right)) => {
                    // Semi/Anti emit the LEFT schema only, so `scope` is unchanged.
                    plan = LogicalNode::Join {
                        left: Box::new(plan),
                        right: Box::new(right),
                        kind,
                        on,
                        strategy: JoinStrategy::Auto,
                    };
                }
                None => filter_conjuncts.push(conj),
            }
        }
        if let Some((first, rest)) = filter_conjuncts.split_first() {
            let mut predicate = lower_expr(first, &scope)?;
            for conj in rest {
                predicate = Expr::bin(BinaryOp::And, predicate, lower_expr(conj, &scope)?);
            }
            plan = LogicalNode::Filter {
                input: Box::new(plan),
                predicate,
            };
        }
    }

    // 3) GROUP BY + aggregates (split projection into group_by
    //    and aggregates, leaving non-aggregate columns to be
    //    treated as group keys).
    let group_keys = match &select.group_by {
        GroupByExpr::All(_) => {
            return Err(FrontendError::Unsupported("GROUP BY ALL".into()));
        }
        GroupByExpr::Expressions(exprs, _) => exprs.clone(),
    };
    let has_group_by = !group_keys.is_empty();
    let has_aggregate_in_projection = select.projection.iter().any(projection_contains_aggregate);
    let has_having = select.having.is_some();
    let needs_aggregate = has_group_by || has_aggregate_in_projection || has_having;

    if needs_aggregate {
        // 4a) Group-by expressions become NamedExprs (their
        //     pre-aggregate ordinals are their position in the
        //     group_by Vec; the post-aggregate scope places them
        //     first, followed by aggregate slots).
        let group_by: Vec<NamedExpr> = group_keys
            .iter()
            .map(|g| -> Result<NamedExpr, FrontendError> {
                let expr = lower_expr(g, &scope)?;
                let name = projection_alias_for_expr(g);
                Ok(NamedExpr {
                    name: name.unwrap_or_else(|| "group_key".into()),
                    expr,
                })
            })
            .collect::<Result<_, _>>()?;
        // 4b) Project + aggregate extraction.
        let (post_agg_outputs, aggregates) =
            lower_projection_with_aggregates(&select.projection, &scope, &group_by)?;
        // 4c) HAVING — for MVP we only support HAVING
        //     expressions that reference group_by columns. Bare
        //     aggregates inside HAVING are Phase 3 (would need a
        //     unified aggregate-extraction pass over both
        //     projection AND HAVING).
        let having = match &select.having {
            Some(expr) if expr_contains_aggregate(expr) => {
                return Err(FrontendError::Unsupported(
                    "HAVING with aggregate calls (use a sub-select for now)".into(),
                ));
            }
            Some(expr) => {
                let post_agg_scope = post_aggregate_scope(&group_by, &aggregates);
                Some(lower_expr(expr, &post_agg_scope)?)
            }
            None => None,
        };
        plan = LogicalNode::Aggregate {
            input: Box::new(plan),
            group_by,
            aggregates,
            having,
        };
        plan = LogicalNode::Project {
            input: Box::new(plan),
            outputs: post_agg_outputs,
        };
    } else {
        let projection_items = lower_projection_items(&select.projection, &scope)?;
        plan = LogicalNode::Project {
            input: Box::new(plan),
            outputs: projection_items,
        };
    }

    // 5) DISTINCT
    if let Some(distinct) = select.distinct.as_ref() {
        match distinct {
            Distinct::Distinct => {
                plan = LogicalNode::Distinct {
                    input: Box::new(plan),
                };
            }
            Distinct::On(_) => {
                return Err(FrontendError::Unsupported("DISTINCT ON".into()));
            }
        }
    }

    // 6) Update scope to post-projection schema (subsequent
    //    ORDER BY references use the projected schema, mirroring
    //    SQL semantics).
    scope = Scope::from_schema(&plan.output_schema());
    let _ = scope; // ORDER BY is applied at the Query layer.

    Ok(plan)
}

/// Flatten a WHERE expression into its top-level `AND` conjuncts (descending through
/// parenthesised `Nested` wrappers). Disjunctions / other operators are returned whole
/// — only conjuncts at the AND-top-level are individually liftable into Semi/Anti joins.
fn flatten_sql_and(expr: &SqlExpr) -> Vec<&SqlExpr> {
    fn rec<'a>(e: &'a SqlExpr, out: &mut Vec<&'a SqlExpr>) {
        match e {
            SqlExpr::BinaryOp {
                left,
                op: BinaryOperator::And,
                right,
            } => {
                rec(left, out);
                rec(right, out);
            }
            SqlExpr::Nested(inner) => rec(inner, out),
            other => out.push(other),
        }
    }
    let mut out = Vec::new();
    rec(expr, &mut out);
    out
}

/// If `conj` is a liftable uncorrelated subquery predicate, lower it to the parts of a
/// Semi/Anti join `(kind, on, right_plan)` to wrap the current plan. Returns `Ok(None)`
/// when `conj` is not a liftable subquery (a normal predicate, `NOT IN`, a multi-column
/// `IN`, or a subquery that fails to lower in isolation — i.e. correlated/unsupported);
/// the caller then leaves it in the `Filter` predicate (where `lower_expr` decides).
///
/// Uncorrelated is enforced for free: `lower_query` builds the subquery's scope from its
/// OWN `FROM` only, so a reference to an outer column fails to resolve → `Err` → not
/// lifted. Supported: `expr IN (single-column SELECT)` → Semi; `EXISTS` → Semi;
/// `NOT EXISTS` → Anti. (`scope` resolves the outer operand of `IN`.)
fn lower_subquery_join_parts(
    conj: &SqlExpr,
    scope: &Scope,
    catalog: &dyn CatalogLookup,
) -> Result<Option<(JoinKind, Option<Expr>, LogicalNode)>, FrontendError> {
    match conj {
        SqlExpr::InSubquery {
            expr,
            subquery,
            negated: false,
        } => {
            // Lower the subquery in isolation; correlated/unsupported → decline (None).
            let Ok(sub) = lower_query(subquery, catalog) else {
                return Ok(None);
            };
            let sub_schema = sub.output_schema();
            // `x IN (subquery)` requires exactly one output column.
            let [sub_col] = sub_schema.columns.as_slice() else {
                return Ok(None);
            };
            let outer = lower_expr(expr, scope)?;
            // The subquery's lone column sits at `outer_width` in the combined
            // left++right row the executor evaluates `on` against (Scope::concat
            // offsets right ordinals by the left width).
            let sub_ref = ColumnRef {
                name: sub_col.name.clone(),
                ordinal: scope.columns.len(),
                ty: sub_col.ty.clone(),
                nullable: sub_col.nullable,
            };
            let on = Expr::bin(BinaryOp::Eq, outer, Expr::column(sub_ref));
            Ok(Some((JoinKind::Semi, Some(on), sub)))
        }
        SqlExpr::Exists { subquery, negated } => {
            let Ok(sub) = lower_query(subquery, catalog) else {
                return Ok(None);
            };
            // EXISTS has no join key (any matching row qualifies); the planner routes
            // the keyless Semi/Anti join to NestedLoop.
            let kind = if *negated {
                JoinKind::Anti
            } else {
                JoinKind::Semi
            };
            Ok(Some((kind, None, sub)))
        }
        // NOT IN (negated InSubquery) is deferred (three-valued-NULL semantics);
        // everything else is a normal predicate handled by the Filter.
        _ => Ok(None),
    }
}

fn projection_contains_aggregate(item: &SelectItem) -> bool {
    match item {
        SelectItem::UnnamedExpr(e) | SelectItem::ExprWithAlias { expr: e, .. } => {
            expr_contains_aggregate(e)
        }
        _ => false,
    }
}

fn expr_contains_aggregate(e: &SqlExpr) -> bool {
    match e {
        SqlExpr::Function(f) => is_aggregate_function_name(&f.name),
        SqlExpr::BinaryOp { left, right, .. } => {
            expr_contains_aggregate(left) || expr_contains_aggregate(right)
        }
        SqlExpr::UnaryOp { expr, .. } => expr_contains_aggregate(expr),
        SqlExpr::IsNull(e)
        | SqlExpr::IsNotNull(e)
        | SqlExpr::Nested(e)
        | SqlExpr::IsTrue(e)
        | SqlExpr::IsFalse(e)
        | SqlExpr::IsNotTrue(e)
        | SqlExpr::IsNotFalse(e) => expr_contains_aggregate(e),
        _ => false,
    }
}

fn is_aggregate_function_name(name: &ObjectName) -> bool {
    let n = name.to_string().to_uppercase();
    // Builtin aggregates, plus any aggregate registered in the shared function registry
    // (so a custom UDAF like `product` is routed to the aggregate path, not scalar lowering).
    matches!(n.as_str(), "COUNT" | "SUM" | "AVG" | "MIN" | "MAX")
        || proximadb_functions::builtins().lookup_aggregate(&n).is_some()
}

fn projection_alias_for_expr(e: &SqlExpr) -> Option<String> {
    match e {
        SqlExpr::Identifier(id) => Some(id.value.clone()),
        SqlExpr::CompoundIdentifier(parts) => parts.last().map(|i| i.value.clone()),
        _ => None,
    }
}

// =========================================================================
// FROM clause + joins
// =========================================================================

fn lower_table_with_joins(
    twj: &sqlparser::ast::TableWithJoins,
    catalog: &dyn CatalogLookup,
) -> Result<(LogicalNode, Scope), FrontendError> {
    let (mut plan, mut scope) = lower_table_factor(&twj.relation, catalog)?;
    for j in &twj.joins {
        let (right, right_scope) = lower_table_factor(&j.relation, catalog)?;
        let kind = match &j.join_operator {
            // The `OUTER` keyword is optional SQL noise: sqlparser parses bare
            // `JOIN`/`LEFT JOIN`/`RIGHT JOIN` as `Join`/`Left`/`Right`, distinct
            // from the `INNER`/`LEFT OUTER`/`RIGHT OUTER` spellings — both map to
            // the same `JoinKind`. (`FULL JOIN` is folded into `FullOuter`.)
            JoinOperator::Inner(_) | JoinOperator::Join(_) => JoinKind::Inner,
            JoinOperator::LeftOuter(_) | JoinOperator::Left(_) => JoinKind::Left,
            JoinOperator::RightOuter(_) | JoinOperator::Right(_) => JoinKind::Right,
            JoinOperator::FullOuter(_) => JoinKind::Full,
            JoinOperator::CrossJoin(_) => JoinKind::Cross,
            other => {
                return Err(FrontendError::Unsupported(format!(
                    "join operator {:?}",
                    std::mem::discriminant(other)
                )));
            }
        };
        // Merge scopes: the combined relation has columns from
        // left followed by columns from right.
        let combined = scope.concat(&right_scope);
        let on = match &j.join_operator {
            JoinOperator::Inner(c)
            | JoinOperator::Join(c)
            | JoinOperator::LeftOuter(c)
            | JoinOperator::Left(c)
            | JoinOperator::RightOuter(c)
            | JoinOperator::Right(c)
            | JoinOperator::FullOuter(c) => match c {
                JoinConstraint::On(expr) => Some(lower_expr(expr, &combined)?),
                JoinConstraint::Using(_) => {
                    return Err(FrontendError::Unsupported("USING constraint".into()));
                }
                JoinConstraint::Natural => {
                    return Err(FrontendError::Unsupported("NATURAL JOIN".into()));
                }
                JoinConstraint::None => None,
            },
            JoinOperator::CrossJoin(_) => None,
            _ => None,
        };
        plan = LogicalNode::Join {
            left: Box::new(plan),
            right: Box::new(right),
            kind,
            on,
            strategy: JoinStrategy::Auto,
        };
        scope = combined;
    }
    Ok((plan, scope))
}

fn lower_table_factor(
    tf: &TableFactor,
    catalog: &dyn CatalogLookup,
) -> Result<(LogicalNode, Scope), FrontendError> {
    match tf {
        TableFactor::Table { name, alias, .. } => {
            let table_name = name.to_string();
            let schema = catalog
                .lookup_table(&table_name)
                .ok_or_else(|| FrontendError::TableNotFound(table_name.clone()))?;
            let scope_name = alias
                .as_ref()
                .map(|a| a.name.value.clone())
                .unwrap_or_else(|| table_name.clone());
            let scope = Scope::from_table(&scope_name, &schema);
            let plan = LogicalNode::Scan {
                table: TableId::new(table_name),
                table_schema: schema,
                projected_columns: None,
                predicate: None,
            };
            Ok((plan, scope))
        }
        TableFactor::Derived {
            subquery, alias, ..
        } => {
            let inner = lower_query(subquery, catalog)?;
            let schema = inner.output_schema();
            let scope_name = alias
                .as_ref()
                .map(|a| a.name.value.clone())
                .unwrap_or_else(|| "subquery".into());
            let scope = Scope::from_table(&scope_name, &schema);
            Ok((inner, scope))
        }
        other => Err(FrontendError::Unsupported(format!(
            "table factor {:?}",
            std::mem::discriminant(other)
        ))),
    }
}

// =========================================================================
// Projection lowering
// =========================================================================

fn lower_projection_items(
    items: &[SelectItem],
    scope: &Scope,
) -> Result<Vec<NamedExpr>, FrontendError> {
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            SelectItem::UnnamedExpr(e) => {
                let expr = lower_expr(e, scope)?;
                let name =
                    projection_alias_for_expr(e).unwrap_or_else(|| auto_column_name(out.len()));
                out.push(NamedExpr { name, expr });
            }
            SelectItem::ExprWithAlias { expr, alias } => {
                let e = lower_expr(expr, scope)?;
                out.push(NamedExpr {
                    name: alias.value.clone(),
                    expr: e,
                });
            }
            SelectItem::Wildcard(_) => {
                // SELECT * → expand to every column in scope.
                for col in scope.all_columns() {
                    out.push(NamedExpr {
                        name: col.name.clone(),
                        expr: Expr::column(col.clone()),
                    });
                }
            }
            SelectItem::QualifiedWildcard(_, _) => {
                return Err(FrontendError::Unsupported("qualified wildcard".into()));
            }
        }
    }
    Ok(out)
}

fn auto_column_name(idx: usize) -> String {
    format!("col{}", idx + 1)
}

// =========================================================================
// Expression lowering
// =========================================================================

fn lower_expr(expr: &SqlExpr, scope: &Scope) -> Result<Expr, FrontendError> {
    match expr {
        SqlExpr::Nested(inner) => lower_expr(inner, scope),
        SqlExpr::Identifier(id) => scope.resolve_unqualified(&id.value).map(Expr::column),
        SqlExpr::CompoundIdentifier(parts) => {
            if parts.len() < 2 {
                return Err(FrontendError::ColumnNotFound(
                    parts
                        .iter()
                        .map(|i| i.value.clone())
                        .collect::<Vec<_>>()
                        .join("."),
                ));
            }
            let table = &parts[0].value;
            let column = &parts[1].value;
            scope.resolve_qualified(table, column).map(Expr::column)
        }
        SqlExpr::Value(v) => Ok(Expr::literal(lower_value(&v.value)?)),
        SqlExpr::TypedString { .. } => {
            Err(FrontendError::Unsupported("typed string literal".into()))
        }
        SqlExpr::BinaryOp { left, op, right } => {
            let l = lower_expr(left, scope)?;
            let r = lower_expr(right, scope)?;
            Ok(Expr::bin(lower_binary_op(op)?, l, r))
        }
        SqlExpr::UnaryOp { op, expr } => {
            let inner = lower_expr(expr, scope)?;
            match lower_unary_op(op)? {
                Some(uop) => Ok(Expr::unary(uop, inner)),
                None => Ok(inner), // unary +
            }
        }
        SqlExpr::IsNull(e) => Ok(Expr::IsNull {
            expr: Box::new(lower_expr(e, scope)?),
            not: false,
        }),
        SqlExpr::IsNotNull(e) => Ok(Expr::IsNull {
            expr: Box::new(lower_expr(e, scope)?),
            not: true,
        }),
        SqlExpr::Between {
            expr,
            low,
            high,
            negated,
        } => Ok(Expr::Between {
            expr: Box::new(lower_expr(expr, scope)?),
            low: Box::new(lower_expr(low, scope)?),
            high: Box::new(lower_expr(high, scope)?),
            not: *negated,
        }),
        SqlExpr::InList {
            expr,
            list,
            negated,
        } => Ok(Expr::In {
            expr: Box::new(lower_expr(expr, scope)?),
            list: list
                .iter()
                .map(|e| lower_expr(e, scope))
                .collect::<Result<Vec<_>, _>>()?,
            not: *negated,
        }),
        SqlExpr::Like {
            expr,
            pattern,
            negated,
            ..
        } => Ok(Expr::Like {
            expr: Box::new(lower_expr(expr, scope)?),
            pattern: Box::new(lower_expr(pattern, scope)?),
            not: *negated,
            case_insensitive: false,
        }),
        SqlExpr::ILike {
            expr,
            pattern,
            negated,
            ..
        } => Ok(Expr::Like {
            expr: Box::new(lower_expr(expr, scope)?),
            pattern: Box::new(lower_expr(pattern, scope)?),
            not: *negated,
            case_insensitive: true,
        }),
        SqlExpr::Function(f) => lower_scalar_function(f, scope),
        SqlExpr::Subquery(_) | SqlExpr::Exists { .. } | SqlExpr::InSubquery { .. } => {
            Err(FrontendError::Unsupported("subqueries".into()))
        }
        other => Err(FrontendError::Unsupported(format!(
            "expression {:?}",
            std::mem::discriminant(other)
        ))),
    }
}

fn lower_binary_op(op: &BinaryOperator) -> Result<BinaryOp, FrontendError> {
    Ok(match op {
        BinaryOperator::Plus => BinaryOp::Plus,
        BinaryOperator::Minus => BinaryOp::Minus,
        BinaryOperator::Multiply => BinaryOp::Mul,
        BinaryOperator::Divide => BinaryOp::Div,
        BinaryOperator::Modulo => BinaryOp::Mod,
        BinaryOperator::Eq => BinaryOp::Eq,
        BinaryOperator::NotEq => BinaryOp::NotEq,
        BinaryOperator::Lt => BinaryOp::Lt,
        BinaryOperator::LtEq => BinaryOp::LtEq,
        BinaryOperator::Gt => BinaryOp::Gt,
        BinaryOperator::GtEq => BinaryOp::GtEq,
        BinaryOperator::And => BinaryOp::And,
        BinaryOperator::Or => BinaryOp::Or,
        BinaryOperator::StringConcat => BinaryOp::Concat,
        other => {
            return Err(FrontendError::Unsupported(format!("binary op {:?}", other)));
        }
    })
}

/// Lower a unary operator. SQL `+` is a no-op (returns
/// `Ok(None)`); callers strip it. Unsupported operators return
/// the matching error.
fn lower_unary_op(op: &UnaryOperator) -> Result<Option<UnaryOp>, FrontendError> {
    Ok(match op {
        UnaryOperator::Not => Some(UnaryOp::Not),
        UnaryOperator::Minus => Some(UnaryOp::Neg),
        UnaryOperator::Plus => None, // identity
        other => {
            return Err(FrontendError::Unsupported(format!("unary op {:?}", other)));
        }
    })
}

fn lower_value(v: &SqlValue) -> Result<ProximaValue, FrontendError> {
    match v {
        SqlValue::Number(s, _) => {
            // Try i64 first, then f64.
            if let Ok(i) = s.parse::<i64>() {
                return Ok(ProximaValue::Int64(i));
            }
            if let Ok(f) = s.parse::<f64>() {
                return Ok(ProximaValue::Float64(f));
            }
            Err(FrontendError::InvalidLiteral(s.clone()))
        }
        SqlValue::SingleQuotedString(s) | SqlValue::DoubleQuotedString(s) => {
            Ok(ProximaValue::String(s.clone()))
        }
        SqlValue::Boolean(b) => Ok(ProximaValue::Boolean(*b)),
        SqlValue::Null => Ok(ProximaValue::Null),
        other => Err(FrontendError::Unsupported(format!("literal {:?}", other))),
    }
}

// =========================================================================
// Projection with aggregates
// =========================================================================
//
// To avoid carrying aggregate placeholders in `Expr` (which would
// create a cycle with the algebra crate), we lower the projection
// in a single pass that recognises aggregate function calls at
// the *projection level* (i.e. as the top-level node of a
// SelectItem), produces a NamedAggregate, and substitutes the
// projection's Expr with a column reference into the aggregate
// slot. This is the SQL standard's pattern for simple aggregate
// projections (`SELECT col, COUNT(*), SUM(x) FROM …`). Complex
// nested cases (aggregates inside arithmetic) are Phase 3.
fn lower_projection_with_aggregates(
    items: &[SelectItem],
    scope: &Scope,
    group_by: &[NamedExpr],
) -> Result<(Vec<NamedExpr>, Vec<NamedAggregate>), FrontendError> {
    let group_count = group_by.len();
    let mut outputs = Vec::new();
    let mut aggregates = Vec::new();
    for item in items {
        let (sql_expr, alias) = match item {
            SelectItem::UnnamedExpr(e) => (e.clone(), None),
            SelectItem::ExprWithAlias { expr, alias } => (expr.clone(), Some(alias.value.clone())),
            SelectItem::Wildcard(_) => {
                for col in scope.all_columns() {
                    outputs.push(NamedExpr {
                        name: col.name.clone(),
                        expr: Expr::column(col.clone()),
                    });
                }
                continue;
            }
            SelectItem::QualifiedWildcard(_, _) => {
                return Err(FrontendError::Unsupported("qualified wildcard".into()));
            }
        };
        match &sql_expr {
            SqlExpr::Function(f) if is_aggregate_function_name(&f.name) => {
                let agg = lower_aggregate_call(f, scope)?;
                let slot = group_count + aggregates.len();
                let name = alias.unwrap_or_else(|| f.name.to_string().to_lowercase());
                let result_ty = agg.result_type();
                aggregates.push(NamedAggregate {
                    name: name.clone(),
                    agg,
                });
                // ColumnRef references the post-aggregate slot.
                // Crucially, the ref's `name` matches the slot's
                // declared name so name-based projection-pushdown
                // rebinding (planner Phase 3 rule) works.
                outputs.push(NamedExpr {
                    name: name.clone(),
                    expr: Expr::column(ColumnRef {
                        name,
                        ordinal: slot,
                        ty: result_ty,
                        nullable: true,
                    }),
                });
            }
            _ => {
                let expr = lower_expr(&sql_expr, scope)?;
                let name = alias
                    .or_else(|| projection_alias_for_expr(&sql_expr))
                    .unwrap_or_else(|| auto_column_name(outputs.len()));
                // A non-aggregate projected column must be a GROUP BY key. The
                // Aggregate node places group keys FIRST (ordinals
                // 0..group_count) ahead of the aggregate slots, so rebind the
                // reference to its group-key slot instead of leaving the
                // pre-aggregate ordinal — which would otherwise point into an
                // aggregate column (wrong type/value) once the grouped column
                // isn't the table's first column.
                let ty = expr.result_type();
                let output_expr = match group_by.iter().position(|g| g.expr == expr) {
                    Some(slot) => Expr::column(ColumnRef {
                        name: name.clone(),
                        ordinal: slot,
                        ty,
                        nullable: true,
                    }),
                    None => expr,
                };
                outputs.push(NamedExpr {
                    name,
                    expr: output_expr,
                });
            }
        }
    }
    Ok((outputs, aggregates))
}

/// Build a [`Scope`] representing the columns visible AFTER an
/// Aggregate node: group_by columns first (in declared order),
/// followed by aggregate-result slots. Used to lower HAVING
/// expressions over the post-aggregate schema.
fn post_aggregate_scope(group_by: &[NamedExpr], aggregates: &[NamedAggregate]) -> Scope {
    let mut columns = Vec::with_capacity(group_by.len() + aggregates.len());
    let mut ordinal = 0;
    for g in group_by {
        columns.push(ScopeColumn {
            table: String::new(),
            column: ColumnInfo {
                name: g.name.clone(),
                ty: g.expr.result_type(),
                nullable: true,
            },
            ordinal,
        });
        ordinal += 1;
    }
    for a in aggregates {
        columns.push(ScopeColumn {
            table: String::new(),
            column: ColumnInfo {
                name: a.name.clone(),
                ty: a.agg.result_type(),
                nullable: !matches!(a.agg, AggregateExpr::Count { .. }),
            },
            ordinal,
        });
        ordinal += 1;
    }
    Scope { columns }
}

/// Lower a non-aggregate SQL function call `f(args)` to [`Expr::FuncCall`].
///
/// Reaching here means the function is in **scalar position** (aggregate calls are
/// handled by [`lower_aggregate_call`] in the SELECT/HAVING aggregate path), so a bare
/// aggregate name here is a misuse and is rejected. Scalar functions resolve their
/// `return_ty` from the shared builtin registry — the *same* registry the Volcano
/// executor dispatches through, so definition and lowering stay single-sourced. Unknown
/// names still lower (the DataFusion path may serve them via its own function set, and the
/// Volcano path raises a clear `UnknownFunction` at execution rather than at parse).
fn lower_scalar_function(
    f: &sqlparser::ast::Function,
    scope: &Scope,
) -> Result<Expr, FrontendError> {
    let raw_name = f.name.to_string();
    if matches!(
        raw_name.to_uppercase().as_str(),
        "COUNT" | "SUM" | "AVG" | "MIN" | "MAX"
    ) {
        return Err(FrontendError::Unsupported(
            "aggregate function in non-aggregate position".into(),
        ));
    }

    let args = match &f.args {
        FunctionArguments::List(list) => list
            .args
            .iter()
            .map(|fa| match fa {
                FunctionArg::Unnamed(FunctionArgExpr::Expr(e)) => lower_expr(e, scope),
                _ => Err(FrontendError::Unsupported(
                    "unsupported scalar function argument".into(),
                )),
            })
            .collect::<Result<Vec<_>, _>>()?,
        FunctionArguments::None => Vec::new(),
        FunctionArguments::Subquery(_) => {
            return Err(FrontendError::Unsupported(
                "subquery as function argument".into(),
            ));
        }
    };

    // Resolve the declared return type from the shared registry; unknown functions get a
    // permissive placeholder (the evaluator ignores `return_ty`, and the DataFusion path
    // re-derives types from Arrow — see `Expr::FuncCall` eval).
    let return_ty = proximadb_functions::builtins()
        .lookup_scalar(&raw_name)
        .map(|d| d.signature.return_ty.clone())
        .unwrap_or(ProximaType::String);

    Ok(Expr::FuncCall {
        name: raw_name.to_ascii_lowercase(),
        args,
        return_ty,
    })
}

fn lower_aggregate_call(
    f: &sqlparser::ast::Function,
    scope: &Scope,
) -> Result<AggregateExpr, FrontendError> {
    let name = f.name.to_string().to_uppercase();
    // Detect DISTINCT and the argument(s).
    let (args, distinct) = match &f.args {
        FunctionArguments::List(list) => {
            let distinct = matches!(
                list.duplicate_treatment,
                Some(sqlparser::ast::DuplicateTreatment::Distinct)
            );
            let args = list
                .args
                .iter()
                .map(|fa| match fa {
                    FunctionArg::Unnamed(FunctionArgExpr::Expr(e)) => {
                        Ok(Some(lower_expr(e, scope)?))
                    }
                    FunctionArg::Unnamed(FunctionArgExpr::Wildcard) => Ok(None),
                    FunctionArg::Unnamed(FunctionArgExpr::QualifiedWildcard(_)) => Ok(None),
                    FunctionArg::Named { .. } | FunctionArg::ExprNamed { .. } => {
                        Err(FrontendError::Unsupported("named function arg".into()))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            (args, distinct)
        }
        FunctionArguments::None => (Vec::new(), false),
        FunctionArguments::Subquery(_) => {
            return Err(FrontendError::Unsupported("subquery in aggregate".into()));
        }
    };
    Ok(match name.as_str() {
        "COUNT" => match args.as_slice() {
            [None] => AggregateExpr::Count {
                arg: None,
                distinct: false,
            },
            [Some(e)] => AggregateExpr::Count {
                arg: Some(e.clone()),
                distinct,
            },
            [] => AggregateExpr::Count {
                arg: None,
                distinct: false,
            },
            _ => {
                return Err(FrontendError::Unsupported(
                    "COUNT with multiple args".into(),
                ));
            }
        },
        "SUM" => match args.as_slice() {
            [Some(e)] => AggregateExpr::Sum {
                arg: e.clone(),
                distinct,
            },
            _ => {
                return Err(FrontendError::Unsupported(
                    "SUM with wrong arg count".into(),
                ));
            }
        },
        "AVG" => match args.as_slice() {
            [Some(e)] => AggregateExpr::Avg {
                arg: e.clone(),
                distinct,
            },
            _ => {
                return Err(FrontendError::Unsupported(
                    "AVG with wrong arg count".into(),
                ));
            }
        },
        "MIN" => match args.as_slice() {
            [Some(e)] => AggregateExpr::Min { arg: e.clone() },
            _ => {
                return Err(FrontendError::Unsupported(
                    "MIN with wrong arg count".into(),
                ));
            }
        },
        "MAX" => match args.as_slice() {
            [Some(e)] => AggregateExpr::Max { arg: e.clone() },
            _ => {
                return Err(FrontendError::Unsupported(
                    "MAX with wrong arg count".into(),
                ));
            }
        },
        // A non-builtin aggregate: resolve it against the shared registry (the same one the
        // Volcano executor accumulates through, and the DataFusion AggregateUDF adapter binds).
        // Unknown names still error clearly.
        other => {
            if let Some(def) = proximadb_functions::builtins().lookup_aggregate(other) {
                let lowered = args
                    .into_iter()
                    .map(|a| {
                        a.ok_or_else(|| {
                            FrontendError::Unsupported(
                                "wildcard argument in custom aggregate".into(),
                            )
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok(AggregateExpr::Custom {
                    name: other.to_ascii_lowercase(),
                    args: lowered,
                    distinct,
                    return_ty: def.signature.return_ty.clone(),
                });
            }
            return Err(FrontendError::Unsupported(format!(
                "aggregate function {}",
                other
            )));
        }
    })
}

// =========================================================================
// ORDER BY + LIMIT
// =========================================================================

fn apply_order_by(
    plan: LogicalNode,
    order_by: &sqlparser::ast::OrderBy,
    _catalog: &dyn CatalogLookup,
) -> Result<LogicalNode, FrontendError> {
    let exprs = match &order_by.kind {
        OrderByKind::Expressions(e) => e,
        OrderByKind::All(_) => {
            return Err(FrontendError::Unsupported("ORDER BY ALL".into()));
        }
    };
    let scope = Scope::from_schema(&plan.output_schema());
    let mut keys = Vec::with_capacity(exprs.len());
    for o in exprs {
        let expr = lower_expr(&o.expr, &scope)?;
        let descending = matches!(o.options.asc, Some(false));
        let nulls_first = match o.options.nulls_first {
            Some(b) => b,
            None => descending, // ASC → NULLS LAST, DESC → NULLS FIRST
        };
        keys.push(SortKey {
            expr,
            descending,
            nulls_first,
        });
    }
    Ok(LogicalNode::Sort {
        input: Box::new(plan),
        keys,
    })
}

fn apply_limit(plan: LogicalNode, lc: &LimitClause) -> Result<LogicalNode, FrontendError> {
    match lc {
        LimitClause::LimitOffset {
            limit,
            offset,
            limit_by,
        } => {
            if !limit_by.is_empty() {
                return Err(FrontendError::Unsupported("LIMIT BY".into()));
            }
            let limit_val = match limit {
                Some(SqlExpr::Value(ValueWithSpan {
                    value: SqlValue::Number(s, _),
                    ..
                })) => Some(
                    s.parse::<u64>()
                        .map_err(|e| FrontendError::InvalidLiteral(format!("LIMIT: {e}")))?,
                ),
                Some(_) => {
                    return Err(FrontendError::Unsupported("LIMIT with non-literal".into()));
                }
                None => None,
            };
            let offset_val = match offset {
                Some(off) => match &off.value {
                    SqlExpr::Value(ValueWithSpan {
                        value: SqlValue::Number(s, _),
                        ..
                    }) => s
                        .parse::<u64>()
                        .map_err(|e| FrontendError::InvalidLiteral(format!("OFFSET: {e}")))?,
                    _ => {
                        return Err(FrontendError::Unsupported("OFFSET with non-literal".into()));
                    }
                },
                None => 0,
            };
            Ok(LogicalNode::Limit {
                input: Box::new(plan),
                limit: limit_val,
                offset: offset_val,
            })
        }
        LimitClause::OffsetCommaLimit { offset, limit } => {
            let offset_val = parse_literal_u64(offset)?;
            let limit_val = parse_literal_u64(limit)?;
            Ok(LogicalNode::Limit {
                input: Box::new(plan),
                limit: Some(limit_val),
                offset: offset_val,
            })
        }
    }
}

fn parse_literal_u64(e: &SqlExpr) -> Result<u64, FrontendError> {
    match e {
        SqlExpr::Value(ValueWithSpan {
            value: SqlValue::Number(s, _),
            ..
        }) => s
            .parse::<u64>()
            .map_err(|err| FrontendError::InvalidLiteral(err.to_string())),
        _ => Err(FrontendError::Unsupported(
            "LIMIT/OFFSET with non-literal".into(),
        )),
    }
}

// =========================================================================
// Scope: resolves identifier references to column ordinals
// =========================================================================

/// Tracks the columns in the current relation. Builds an ordinal
/// index from `(table_alias?, column_name)` to `ColumnRef`.
#[derive(Debug, Clone)]
struct Scope {
    columns: Vec<ScopeColumn>,
}

#[derive(Debug, Clone)]
struct ScopeColumn {
    table: String,
    column: ColumnInfo,
    /// Position in the combined relation (used as the `ordinal`
    /// in resulting `ColumnRef`s).
    ordinal: usize,
}

impl Scope {
    fn empty() -> Self {
        Self {
            columns: Vec::new(),
        }
    }

    fn from_table(table_alias: &str, schema: &RelationalSchema) -> Self {
        Self {
            columns: schema
                .columns
                .iter()
                .enumerate()
                .map(|(i, c)| ScopeColumn {
                    table: table_alias.to_string(),
                    column: c.clone(),
                    ordinal: i,
                })
                .collect(),
        }
    }

    fn from_schema(schema: &RelationalSchema) -> Self {
        // No table alias context (e.g. post-projection scope).
        Self {
            columns: schema
                .columns
                .iter()
                .enumerate()
                .map(|(i, c)| ScopeColumn {
                    table: String::new(),
                    column: c.clone(),
                    ordinal: i,
                })
                .collect(),
        }
    }

    fn concat(&self, other: &Scope) -> Scope {
        let mut combined = self.clone();
        let offset = combined.columns.len();
        for sc in &other.columns {
            combined.columns.push(ScopeColumn {
                table: sc.table.clone(),
                column: sc.column.clone(),
                ordinal: sc.ordinal + offset,
            });
        }
        combined
    }

    fn all_columns(&self) -> Vec<ColumnRef> {
        self.columns
            .iter()
            .map(|sc| ColumnRef {
                name: sc.column.name.clone(),
                ordinal: sc.ordinal,
                ty: sc.column.ty.clone(),
                nullable: sc.column.nullable,
            })
            .collect()
    }

    fn resolve_unqualified(&self, name: &str) -> Result<ColumnRef, FrontendError> {
        let mut hits: Vec<&ScopeColumn> = self
            .columns
            .iter()
            .filter(|c| c.column.name == name)
            .collect();
        match hits.len() {
            0 => Err(FrontendError::ColumnNotFound(name.into())),
            1 => Ok(ColumnRef {
                name: hits[0].column.name.clone(),
                ordinal: hits[0].ordinal,
                ty: hits[0].column.ty.clone(),
                nullable: hits[0].column.nullable,
            }),
            _ => {
                // Multiple hits — ambiguous only if the names AND
                // tables differ; if they all come from the same
                // table, return the first (post-projection scope).
                hits.sort_by_key(|c| &c.table);
                hits.dedup_by_key(|c| c.table.clone());
                if hits.len() == 1 {
                    Ok(ColumnRef {
                        name: hits[0].column.name.clone(),
                        ordinal: hits[0].ordinal,
                        ty: hits[0].column.ty.clone(),
                        nullable: hits[0].column.nullable,
                    })
                } else {
                    Err(FrontendError::AmbiguousColumn(name.into()))
                }
            }
        }
    }

    fn resolve_qualified(&self, table: &str, column: &str) -> Result<ColumnRef, FrontendError> {
        self.columns
            .iter()
            .find(|c| c.table == table && c.column.name == column)
            .map(|c| ColumnRef {
                name: c.column.name.clone(),
                ordinal: c.ordinal,
                ty: c.column.ty.clone(),
                nullable: c.column.nullable,
            })
            .ok_or_else(|| FrontendError::ColumnNotFound(format!("{table}.{column}")))
    }
}

// =========================================================================
// CREATE FUNCTION body lowering (F5)
// =========================================================================

/// Lower a SQL-language function body — a scalar expression over the function's parameters —
/// into an engine-neutral [`Expr`]. Each parameter is exposed as a column (parameter `i` →
/// ordinal `i`) so the body references it by name; the resulting `Expr` feeds
/// `proximadb_functions::sql_bodied_scalar` to register a `CREATE FUNCTION` as a registry
/// function that runs on both engines (F5).
///
/// Uses the SAME `lower_expr` the SELECT path uses, so a function body supports the full scalar
/// grammar (arithmetic, builtins, CASE, …). Parse errors and references to unknown parameters
/// return a clear [`FrontendError`].
pub fn lower_function_body(
    body_sql: &str,
    params: &[(String, ProximaType)],
) -> Result<Expr, FrontendError> {
    let columns: Vec<ColumnInfo> = params
        .iter()
        .map(|(name, ty)| ColumnInfo::new(name.clone(), ty.clone(), true))
        .collect();
    let schema = RelationalSchema::new(columns);
    let scope = Scope::from_schema(&schema);

    let mut parser = Parser::new(&GenericDialect {})
        .try_with_sql(body_sql)
        .map_err(|e| FrontendError::Parse(format!("function body: {e}")))?;
    let expr = parser
        .parse_expr()
        .map_err(|e| FrontendError::Parse(format!("function body: {e}")))?;
    lower_expr(&expr, &scope)
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn users_schema() -> RelationalSchema {
        RelationalSchema::new(vec![
            ColumnInfo::new("id", ProximaType::Int64, false),
            ColumnInfo::new("name", ProximaType::String, true),
            ColumnInfo::new("age", ProximaType::Int32, true),
        ])
    }

    fn orders_schema() -> RelationalSchema {
        RelationalSchema::new(vec![
            ColumnInfo::new("oid", ProximaType::Int64, false),
            ColumnInfo::new("uid", ProximaType::Int64, false),
            ColumnInfo::new("total", ProximaType::Float64, false),
        ])
    }

    fn catalog() -> InMemoryCatalog {
        let mut c = InMemoryCatalog::new();
        c.register("users", users_schema());
        c.register("orders", orders_schema());
        c
    }

    fn lower(sql: &str) -> LogicalNode {
        lower_sql(sql, &catalog()).unwrap_or_else(|e| panic!("lower failed: {e:?}"))
    }

    // --- Semi/Anti via uncorrelated IN / EXISTS / NOT EXISTS subqueries --------

    /// Strip the outer `Project` to reach the join/filter the WHERE produced.
    fn under_project(plan: LogicalNode) -> LogicalNode {
        match plan {
            LogicalNode::Project { input, .. } => *input,
            other => other,
        }
    }

    #[test]
    fn in_subquery_lowers_to_semi_join_with_equi_on() {
        // users.id(0) IN (SELECT uid FROM orders) → Semi join, ON id = orders.uid.
        // The subquery's lone column sits at ordinal 3 (users width = 3).
        let plan = under_project(lower(
            "SELECT name FROM users WHERE id IN (SELECT uid FROM orders)",
        ));
        match plan {
            LogicalNode::Join {
                kind: JoinKind::Semi,
                on: Some(Expr::BinaryOp { left, op, right }),
                ..
            } => {
                assert_eq!(op, BinaryOp::Eq);
                assert!(matches!(*left, Expr::Column(c) if c.ordinal == 0));
                assert!(matches!(*right, Expr::Column(c) if c.ordinal == 3));
            }
            other => panic!("expected Semi join with equi ON, got {other:?}"),
        }
    }

    #[test]
    fn exists_lowers_to_keyless_semi_join() {
        let plan = under_project(lower(
            "SELECT name FROM users WHERE EXISTS (SELECT oid FROM orders)",
        ));
        assert!(
            matches!(
                plan,
                LogicalNode::Join {
                    kind: JoinKind::Semi,
                    on: None,
                    ..
                }
            ),
            "EXISTS → keyless Semi join: {plan:?}"
        );
    }

    #[test]
    fn not_exists_lowers_to_keyless_anti_join() {
        let plan = under_project(lower(
            "SELECT name FROM users WHERE NOT EXISTS (SELECT oid FROM orders)",
        ));
        assert!(
            matches!(
                plan,
                LogicalNode::Join {
                    kind: JoinKind::Anti,
                    on: None,
                    ..
                }
            ),
            "NOT EXISTS → keyless Anti join: {plan:?}"
        );
    }

    #[test]
    fn where_subquery_and_plain_predicate_compose() {
        // `age > 25 AND id IN (subquery)` → Filter(age>25) over Semi join.
        let plan = under_project(lower(
            "SELECT name FROM users WHERE age > 25 AND id IN (SELECT uid FROM orders)",
        ));
        match plan {
            LogicalNode::Filter { input, predicate } => {
                assert!(matches!(predicate, Expr::BinaryOp { op: BinaryOp::Gt, .. }));
                assert!(matches!(
                    *input,
                    LogicalNode::Join {
                        kind: JoinKind::Semi,
                        ..
                    }
                ));
            }
            other => panic!("expected Filter over Semi join, got {other:?}"),
        }
    }

    #[test]
    fn not_in_subquery_is_declined() {
        // NOT IN is deferred (three-valued NULL semantics) → falls through to the
        // Filter path where lower_expr rejects the subquery.
        assert!(lower_sql(
            "SELECT name FROM users WHERE id NOT IN (SELECT uid FROM orders)",
            &catalog()
        )
        .is_err());
    }

    #[test]
    fn correlated_subquery_is_declined() {
        // The subquery references the outer table (users.age) → fails to lower in
        // isolation → not lifted → rejected (correlated subqueries unsupported).
        assert!(lower_sql(
            "SELECT name FROM users WHERE id IN (SELECT uid FROM orders WHERE total = users.age)",
            &catalog()
        )
        .is_err());
    }

    #[test]
    fn multi_column_in_subquery_is_declined() {
        assert!(lower_sql(
            "SELECT name FROM users WHERE id IN (SELECT oid, uid FROM orders)",
            &catalog()
        )
        .is_err());
    }

    #[test]
    fn select_all_from_table() {
        let plan = lower("SELECT * FROM users");
        // Project(Scan)
        match plan {
            LogicalNode::Project { input, outputs } => {
                assert_eq!(outputs.len(), 3);
                assert!(matches!(*input, LogicalNode::Scan { .. }));
            }
            _ => panic!("expected Project, got {plan:?}"),
        }
    }

    #[test]
    fn select_with_where() {
        let plan = lower("SELECT id FROM users WHERE age > 25");
        // Project(Filter(Scan))
        match plan {
            LogicalNode::Project { input, .. } => match *input {
                LogicalNode::Filter {
                    input: _,
                    predicate,
                } => {
                    assert!(matches!(
                        predicate,
                        Expr::BinaryOp {
                            op: BinaryOp::Gt,
                            ..
                        }
                    ));
                }
                other => panic!("expected Filter, got {other:?}"),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn select_count_star() {
        // COUNT(*) with no GROUP BY → Project(Aggregate(Scan))
        let plan = lower("SELECT COUNT(*) FROM users");
        match plan {
            LogicalNode::Project { input, outputs } => {
                assert_eq!(outputs.len(), 1);
                match *input {
                    LogicalNode::Aggregate {
                        aggregates,
                        group_by,
                        ..
                    } => {
                        assert!(group_by.is_empty());
                        assert_eq!(aggregates.len(), 1);
                        assert!(matches!(
                            aggregates[0].agg,
                            AggregateExpr::Count {
                                arg: None,
                                distinct: false
                            }
                        ));
                    }
                    other => panic!("expected Aggregate, got {other:?}"),
                }
            }
            _ => panic!(),
        }
    }

    #[test]
    fn select_sum_with_group_by() {
        let plan = lower("SELECT age, SUM(id) FROM users GROUP BY age");
        match plan {
            LogicalNode::Project { input, .. } => match *input {
                LogicalNode::Aggregate {
                    group_by,
                    aggregates,
                    ..
                } => {
                    assert_eq!(group_by.len(), 1);
                    assert_eq!(aggregates.len(), 1);
                    assert!(matches!(aggregates[0].agg, AggregateExpr::Sum { .. }));
                }
                other => panic!("expected Aggregate, got {other:?}"),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn select_group_by() {
        let plan = lower("SELECT age FROM users GROUP BY age");
        match plan {
            LogicalNode::Project { input, .. } => match *input {
                LogicalNode::Aggregate { group_by, .. } => {
                    assert_eq!(group_by.len(), 1);
                }
                other => panic!("expected Aggregate, got {other:?}"),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn select_group_key_projection_rebinds_to_post_aggregate_ordinal() {
        // `name` is column ordinal 1 in `users`. After GROUP BY, the Aggregate
        // node places group keys FIRST, so the projected `name` must reference
        // the post-aggregate group slot (ordinal 0) — NOT its pre-aggregate
        // ordinal 1, which now holds the COUNT result (Int64). Regression for a
        // type-mismatch that surfaced whenever the grouped column isn't the
        // table's first column.
        let plan = lower("SELECT name, COUNT(*) FROM users GROUP BY name");
        let LogicalNode::Project { outputs, .. } = plan else {
            panic!("expected Project");
        };
        assert_eq!(outputs.len(), 2);
        match &outputs[0].expr {
            Expr::Column(c) => {
                assert_eq!(c.ordinal, 0, "group key rebinds to post-agg slot 0");
                assert_eq!(c.ty, ProximaType::String);
            }
            other => panic!("expected group-key column ref, got {other:?}"),
        }
        match &outputs[1].expr {
            Expr::Column(c) => assert_eq!(c.ordinal, 1, "COUNT slot follows the group keys"),
            other => panic!("expected aggregate column ref, got {other:?}"),
        }
    }

    #[test]
    fn select_with_inner_join() {
        let plan = lower("SELECT users.id FROM users INNER JOIN orders ON users.id = orders.uid");
        match plan {
            LogicalNode::Project { input, .. } => match *input {
                LogicalNode::Join { kind, .. } => {
                    assert_eq!(kind, JoinKind::Inner);
                }
                other => panic!("expected Join, got {other:?}"),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn select_unqualified_columns_across_bare_join() {
        // Bare `JOIN` (no INNER keyword) must lower as INNER JOIN, and
        // unqualified column refs resolve against the merged scope: `name` is
        // unique to users (ordinal 1), `total` unique to orders (combined
        // ordinal 3+2=5). Regression: bare `JOIN` was previously rejected as an
        // unsupported join operator, forcing a fall-through to the legacy path.
        let plan = lower("SELECT name, total FROM users JOIN orders ON users.id = orders.uid");
        let LogicalNode::Project { outputs, .. } = plan else {
            panic!("expected Project");
        };
        let ords: Vec<(String, usize)> = outputs
            .iter()
            .map(|o| match &o.expr {
                Expr::Column(c) => (o.name.clone(), c.ordinal),
                other => panic!("expected column, got {other:?}"),
            })
            .collect();
        // users = [id@0, name@1, age@2]; orders = [oid@3, uid@4, total@5].
        assert_eq!(
            ords,
            vec![("name".to_string(), 1), ("total".to_string(), 5)]
        );
    }

    #[test]
    fn bare_left_and_right_join_lower_to_outer_join_kinds() {
        // Bare `LEFT JOIN` / `RIGHT JOIN` (no `OUTER`) parse to sqlparser's
        // `Left`/`Right` variants (distinct from `LeftOuter`/`RightOuter`) and
        // must lower to the same JoinKind. Regression: the bare forms were
        // previously rejected as unsupported → PATH B fell through to legacy.
        for (sql, expected) in [
            (
                "SELECT name, total FROM users LEFT JOIN orders ON users.id = orders.uid",
                JoinKind::Left,
            ),
            (
                "SELECT name, total FROM users RIGHT JOIN orders ON users.id = orders.uid",
                JoinKind::Right,
            ),
        ] {
            let plan = lower(sql);
            let LogicalNode::Project { input, .. } = plan else {
                panic!("expected Project for `{sql}`");
            };
            match *input {
                LogicalNode::Join { kind, .. } => assert_eq!(kind, expected, "for `{sql}`"),
                other => panic!("expected Join for `{sql}`, got {other:?}"),
            }
        }
    }

    #[test]
    fn select_distinct() {
        let plan = lower("SELECT DISTINCT id FROM users");
        match plan {
            LogicalNode::Distinct { .. } => {}
            _ => panic!("expected Distinct, got {plan:?}"),
        }
    }

    #[test]
    fn select_order_by_limit_offset() {
        // NB: MVP limit — ORDER BY resolves against the
        // post-projection schema, so the sort key must appear in
        // the projection list. ORDER BY on non-projected columns
        // is a Phase 3 enhancement.
        let plan = lower("SELECT id FROM users ORDER BY id DESC LIMIT 10 OFFSET 5");
        match plan {
            LogicalNode::Limit { limit, offset, .. } => {
                assert_eq!(limit, Some(10));
                assert_eq!(offset, 5);
            }
            _ => panic!("expected Limit, got {plan:?}"),
        }
    }

    #[test]
    fn union_all_combines_selects() {
        let plan = lower("SELECT id FROM users UNION ALL SELECT oid FROM orders");
        match plan {
            LogicalNode::Union { inputs, all } => {
                assert!(all);
                assert_eq!(inputs.len(), 2);
            }
            _ => panic!("expected Union, got {plan:?}"),
        }
    }

    #[test]
    fn values_emits_values_node() {
        let plan = lower("VALUES (1, 'a'), (2, 'b')");
        match plan {
            LogicalNode::Values { rows, .. } => {
                assert_eq!(rows.len(), 2);
            }
            _ => panic!("expected Values, got {plan:?}"),
        }
    }

    #[test]
    fn unknown_table_errors() {
        let err = lower_sql("SELECT * FROM nope", &catalog()).unwrap_err();
        assert!(matches!(err, FrontendError::TableNotFound(_)));
    }

    #[test]
    fn unknown_column_errors() {
        let err = lower_sql("SELECT bogus FROM users", &catalog()).unwrap_err();
        assert!(matches!(err, FrontendError::ColumnNotFound(_)));
    }

    #[test]
    fn dml_is_rejected() {
        let err = lower_sql("INSERT INTO users VALUES (1, 'x', 30)", &catalog()).unwrap_err();
        assert!(matches!(err, FrontendError::Unsupported(_)));
    }

    #[test]
    fn subquery_under_or_is_rejected() {
        // Only TOP-LEVEL AND-conjuncts lift into Semi/Anti joins. A subquery under
        // OR isn't a standalone conjunct → not lifted → lower_expr rejects it (a
        // disjunctive membership test can't be expressed as a filtering join), so
        // the query falls through to the legacy path.
        let err = lower_sql(
            "SELECT id FROM users WHERE age > 1 OR id IN (SELECT uid FROM orders)",
            &catalog(),
        )
        .unwrap_err();
        assert!(matches!(err, FrontendError::Unsupported(_)));
    }

    #[test]
    fn is_null_lowers_correctly() {
        let plan = lower("SELECT id FROM users WHERE name IS NULL");
        match plan {
            LogicalNode::Project { input, .. } => match *input {
                LogicalNode::Filter { predicate, .. } => {
                    assert!(matches!(predicate, Expr::IsNull { not: false, .. }));
                }
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn between_lowers_correctly() {
        let plan = lower("SELECT id FROM users WHERE age BETWEEN 18 AND 65");
        match plan {
            LogicalNode::Project { input, .. } => match *input {
                LogicalNode::Filter { predicate, .. } => {
                    assert!(matches!(predicate, Expr::Between { not: false, .. }));
                }
                _ => panic!(),
            },
            _ => panic!(),
        }
    }

    #[test]
    fn scalar_function_lowers_to_funccall_with_registry_return_type() {
        // F1b: a scalar function in projection position now lowers to `Expr::FuncCall`
        // (previously rejected as "aggregate function in non-aggregate position"). The
        // `return_ty` is resolved from the shared builtin registry: `upper`→String,
        // `abs`→Float64.
        let plan = lower("SELECT upper(name), abs(age) FROM users");
        let LogicalNode::Project { outputs, .. } = plan else {
            panic!("expected Project");
        };
        assert_eq!(outputs.len(), 2);
        match &outputs[0].expr {
            Expr::FuncCall {
                name,
                args,
                return_ty,
            } => {
                assert_eq!(name, "upper");
                assert_eq!(args.len(), 1);
                assert_eq!(*return_ty, ProximaType::String);
            }
            other => panic!("expected FuncCall upper, got {other:?}"),
        }
        match &outputs[1].expr {
            Expr::FuncCall {
                name, return_ty, ..
            } => {
                assert_eq!(name, "abs");
                assert_eq!(*return_ty, ProximaType::Float64);
            }
            other => panic!("expected FuncCall abs, got {other:?}"),
        }
    }

    #[test]
    fn aggregate_in_scalar_position_is_rejected() {
        // A bare aggregate name in scalar (non-GROUP-BY) position is still a misuse.
        let err = lower_sql("SELECT sum(age) + 1 FROM users", &catalog()).unwrap_err();
        assert!(matches!(err, FrontendError::Unsupported(_)));
    }

    #[test]
    fn unknown_scalar_function_still_lowers() {
        // Unknown functions lower permissively (the DataFusion path may serve them; the
        // Volcano path raises UnknownFunction at execution, not at parse). Placeholder
        // return type is String.
        let plan = lower("SELECT some_udf(name) FROM users");
        let LogicalNode::Project { outputs, .. } = plan else {
            panic!("expected Project");
        };
        match &outputs[0].expr {
            Expr::FuncCall { name, .. } => assert_eq!(name, "some_udf"),
            other => panic!("expected FuncCall, got {other:?}"),
        }
    }

    #[test]
    fn custom_aggregate_lowers_to_aggregate_expr_custom() {
        // F3: a non-builtin aggregate registered in the shared registry (`product`) lowers to
        // AggregateExpr::Custom carrying the registry's return type, instead of being rejected.
        let plan = lower("SELECT product(age) FROM users");
        let LogicalNode::Project { input, .. } = plan else {
            panic!("expected Project");
        };
        let LogicalNode::Aggregate {
            aggregates,
            group_by,
            ..
        } = *input
        else {
            panic!("expected Aggregate");
        };
        assert!(group_by.is_empty());
        assert_eq!(aggregates.len(), 1);
        match &aggregates[0].agg {
            AggregateExpr::Custom {
                name,
                args,
                return_ty,
                ..
            } => {
                assert_eq!(name, "product");
                assert_eq!(args.len(), 1);
                assert_eq!(*return_ty, ProximaType::Float64);
            }
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn ambiguous_column_errors_in_join() {
        // both tables have a column named `id`? users has id, orders has oid.
        // Construct an actual ambiguity: users.id vs another table with id.
        let mut c = catalog();
        c.register(
            "people",
            RelationalSchema::new(vec![ColumnInfo::new("id", ProximaType::Int64, false)]),
        );
        let err = lower_sql(
            "SELECT id FROM users INNER JOIN people ON users.id = people.id",
            &c,
        )
        .unwrap_err();
        assert!(matches!(err, FrontendError::AmbiguousColumn(_)));
    }

    #[test]
    fn lower_function_body_lowers_param_expression() {
        // F5: `CREATE FUNCTION double(x BIGINT) AS 'x * 2'` body → BinaryOp(Mul, Col(0), Lit).
        let body =
            lower_function_body("x * 2", &[("x".to_string(), ProximaType::Int64)]).unwrap();
        match body {
            Expr::BinaryOp { op, left, right } => {
                assert_eq!(op, BinaryOp::Mul);
                assert!(matches!(*left, Expr::Column(ColumnRef { ordinal: 0, .. })));
                assert!(matches!(*right, Expr::Literal { .. }));
            }
            other => panic!("expected BinaryOp, got {other:?}"),
        }
    }

    #[test]
    fn lower_function_body_supports_builtins_and_rejects_unknown_param() {
        // body may call builtins (resolved at execution against the registry) ...
        assert!(
            lower_function_body("abs(x)", &[("x".to_string(), ProximaType::Float64)]).is_ok()
        );
        // ... and a reference to an undeclared parameter errors clearly.
        let err = lower_function_body("y + 1", &[("x".to_string(), ProximaType::Int64)])
            .unwrap_err();
        assert!(matches!(err, FrontendError::ColumnNotFound(_)));
    }
}
