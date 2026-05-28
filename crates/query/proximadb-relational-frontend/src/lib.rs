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

    // 2) WHERE
    if let Some(where_expr) = &select.selection {
        let predicate = lower_expr(where_expr, &scope)?;
        plan = LogicalNode::Filter {
            input: Box::new(plan),
            predicate,
        };
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
    let has_aggregate_in_projection = select
        .projection
        .iter()
        .any(projection_contains_aggregate);
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
            lower_projection_with_aggregates(&select.projection, &scope, group_by.len())?;
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
    matches!(n.as_str(), "COUNT" | "SUM" | "AVG" | "MIN" | "MAX")
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
            JoinOperator::Inner(_) => JoinKind::Inner,
            JoinOperator::LeftOuter(_) => JoinKind::Left,
            JoinOperator::RightOuter(_) => JoinKind::Right,
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
            | JoinOperator::LeftOuter(c)
            | JoinOperator::RightOuter(c)
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
        SqlExpr::Function(_) => Err(FrontendError::Unsupported(
            "aggregate function in non-aggregate position".into(),
        )),
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
    group_count: usize,
) -> Result<(Vec<NamedExpr>, Vec<NamedAggregate>), FrontendError> {
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
                outputs.push(NamedExpr { name, expr });
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
        other => {
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
    fn subquery_in_where_is_rejected() {
        let err = lower_sql(
            "SELECT id FROM users WHERE id IN (SELECT uid FROM orders)",
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
}
