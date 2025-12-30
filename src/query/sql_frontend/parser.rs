//! SQL frontend parser: wraps sqlparser-rs and produces the internal AST.

use anyhow::{Result, anyhow};
use sqlparser::ast::{
    BinaryOperator, Cte as SqlCte, Expr as SqlExpr, FunctionArg, FunctionArgExpr, Join as SqlJoin,
    JoinConstraint, JoinOperator, OrderByExpr as SqlOrderByExpr, Query as SqlQuery,
    Select as SqlSelect, SelectItem, SetExpr, SetOperator as SqlSetOperator, Statement,
    TableFactor, TableWithJoins, UnaryOperator, Value, With as SqlWith,
};
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

// DML types for INSERT/UPDATE/DELETE
use crate::services::dml::{
    DmlStatement, SqlValueLiteral, WhereClause, Condition,
    ComparisonOperator as DmlComparisonOperator, LogicalOperator,
};

use crate::query::ast::{
    BinaryOp, Cte, Expr, Join, JoinType, Literal, OrderByExpr, ProjectionItem, Query, Select,
    SetOp, TableRef, UnaryOp,
};

pub struct SqlFrontendParser {
    dialect: GenericDialect,
}

impl SqlFrontendParser {
    pub fn new() -> Self {
        Self {
            dialect: GenericDialect {},
        }
    }

    /// Parse SQL text into the internal AST.
    pub fn parse(&self, sql: &str) -> Result<Query> {
        // Parse SQL using sqlparser-rs
        let statements = Parser::parse_sql(&self.dialect, sql)
            .map_err(|e| anyhow!("SQL parsing failed: {}", e))?;

        if statements.is_empty() {
            return Err(anyhow!("No SQL statements found"));
        }

        if statements.len() > 1 {
            return Err(anyhow!(
                "Multiple statements not supported, found {}",
                statements.len()
            ));
        }

        // Convert the first statement to internal AST
        let statement = &statements[0];
        self.convert_statement(statement)
    }

    fn convert_statement(&self, statement: &Statement) -> Result<Query> {
        match statement {
            Statement::Query(query) => self.convert_query(query),
            Statement::Insert { .. } => {
                Err(anyhow!("INSERT statements are parsed but not yet executed. Use REST/gRPC API for insertions."))
            }
            Statement::Update { .. } => {
                Err(anyhow!("UPDATE statements are parsed but not yet executed. Use REST/gRPC API for updates."))
            }
            Statement::Delete { .. } => {
                Err(anyhow!("DELETE statements are parsed but not yet executed. Use REST/gRPC API for deletions."))
            }
            _ => Err(anyhow!("Only SELECT queries are currently supported. For DML operations (INSERT/UPDATE/DELETE), use REST/gRPC API.")),
        }
    }

    fn convert_query(&self, query: &SqlQuery) -> Result<Query> {
        // Handle WITH (CTE)
        if let Some(with) = &query.with {
            let ctes = self.convert_with(with)?;
            let inner = self.convert_query_no_with(query)?; // convert body
            return Ok(Query::With {
                ctes,
                query: Box::new(inner),
            });
        }
        self.convert_query_no_with(query)
    }

    /// Convert query without handling WITH (handled by caller)
    fn convert_query_no_with(&self, query: &SqlQuery) -> Result<Query> {
        match &*query.body {
            SetExpr::Select(select) => Ok(Query::Select(self.convert_select(select, query)?)),
            SetExpr::SetOperation {
                left,
                op,
                right,
                set_quantifier,
            } => {
                let (set_op, all) = match op {
                    SqlSetOperator::Union => (
                        SetOp::Union,
                        matches!(set_quantifier, sqlparser::ast::SetQuantifier::All),
                    ),
                    SqlSetOperator::Intersect => (
                        SetOp::Intersect,
                        matches!(set_quantifier, sqlparser::ast::SetQuantifier::All),
                    ),
                    SqlSetOperator::Except | SqlSetOperator::Minus => (
                        SetOp::Except,
                        matches!(set_quantifier, sqlparser::ast::SetQuantifier::All),
                    ),
                };
                let left_q = self.convert_setexpr(left)?;
                let right_q = self.convert_setexpr(right)?;
                Ok(Query::Set {
                    left: Box::new(left_q),
                    op: set_op,
                    all,
                    right: Box::new(right_q),
                })
            }
            other => Err(anyhow!("Unsupported query body: {:?}", other)),
        }
    }

    fn convert_setexpr(&self, expr: &SetExpr) -> Result<Query> {
        match expr {
            SetExpr::Select(sel) => {
                // Minimal wrapper Query for nested select
                Ok(Query::Select(self.convert_select(
                    sel,
                    &SqlQuery {
                        with: None,
                        body: Box::new(SetExpr::Select(sel.clone())),
                        order_by: None,
                        limit_clause: None,
                        fetch: None,
                        locks: vec![],
                        for_clause: None,
                        settings: None,
                        format_clause: None,
                        pipe_operators: vec![],
                    },
                )?))
            }
            SetExpr::SetOperation {
                left,
                op,
                right,
                set_quantifier,
            } => {
                let (set_op, all) = match op {
                    SqlSetOperator::Union => (
                        SetOp::Union,
                        matches!(set_quantifier, sqlparser::ast::SetQuantifier::All),
                    ),
                    SqlSetOperator::Intersect => (
                        SetOp::Intersect,
                        matches!(set_quantifier, sqlparser::ast::SetQuantifier::All),
                    ),
                    SqlSetOperator::Except | SqlSetOperator::Minus => (
                        SetOp::Except,
                        matches!(set_quantifier, sqlparser::ast::SetQuantifier::All),
                    ),
                };
                let left_q = self.convert_setexpr(left)?;
                let right_q = self.convert_setexpr(right)?;
                Ok(Query::Set {
                    left: Box::new(left_q),
                    op: set_op,
                    all,
                    right: Box::new(right_q),
                })
            }
            _ => Err(anyhow!("Unsupported set expression: {:?}", expr)),
        }
    }

    fn convert_with(&self, with: &SqlWith) -> Result<Vec<Cte>> {
        let mut ctes = Vec::new();
        for SqlCte { alias, query, .. } in &with.cte_tables {
            let name = alias.name.value.clone();
            let q = self.convert_query(query)?;
            ctes.push(Cte {
                name,
                query: Box::new(q),
            });
        }
        Ok(ctes)
    }

    fn convert_select(&self, select: &SqlSelect, query: &SqlQuery) -> Result<Select> {
        // Convert projection
        let projection = select
            .projection
            .iter()
            .map(|item| self.convert_select_item_with_alias(item))
            .collect::<Result<Vec<ProjectionItem>>>()?;

        // Convert FROM clause
        let from = select
            .from
            .iter()
            .map(|table_with_joins| self.convert_table_with_joins(table_with_joins))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect();

        // Convert JOINs (handled in convert_table_with_joins)
        let joins = select
            .from
            .iter()
            .flat_map(|table_with_joins| &table_with_joins.joins)
            .map(|join| self.convert_join(join))
            .collect::<Result<Vec<_>>>()?;

        // Convert WHERE clause
        let selection = match &select.selection {
            Some(expr) => Some(self.convert_expr(expr)?),
            None => None,
        };

        // Convert GROUP BY
        let group_by = match &select.group_by {
            sqlparser::ast::GroupByExpr::All(_) => vec![], // Handle GROUP BY ALL (PostgreSQL extension)
            sqlparser::ast::GroupByExpr::Expressions(exprs, _modifiers) => exprs
                .iter()
                .map(|expr| self.convert_expr(expr))
                .collect::<Result<Vec<_>>>()?,
        };

        // Convert HAVING
        let having = match &select.having {
            Some(expr) => Some(self.convert_expr(expr)?),
            None => None,
        };

        // Convert ORDER BY
        let order_by = if let Some(order_by_clause) = &query.order_by {
            match &order_by_clause.kind {
                sqlparser::ast::OrderByKind::Expressions(exprs) => exprs
                    .iter()
                    .map(|order_expr| self.convert_order_by_expr(order_expr))
                    .collect::<Result<Vec<_>>>()?,
                sqlparser::ast::OrderByKind::All(_) => vec![],
            }
        } else {
            vec![]
        };

        // Convert LIMIT and OFFSET (sqlparser 0.59 uses LimitClause enum)
        let (limit, offset) = if let Some(lc) = &query.limit_clause {
            match lc {
                sqlparser::ast::LimitClause::LimitOffset { limit: lim, offset: off, .. } => {
                    let limit_val = lim.as_ref().and_then(|expr| {
                        if let SqlExpr::Value(value_with_span) = expr {
                            if let Value::Number(n, _) = &value_with_span.value {
                                return n.parse::<u64>().ok();
                            }
                        }
                        None
                    });
                    let offset_val = off.as_ref().and_then(|off_expr| {
                        if let SqlExpr::Value(value_with_span) = &off_expr.value {
                            if let Value::Number(n, _) = &value_with_span.value {
                                return n.parse::<u64>().ok();
                            }
                        }
                        None
                    });
                    (limit_val, offset_val)
                }
                sqlparser::ast::LimitClause::OffsetCommaLimit { offset: off, limit: lim } => {
                    let limit_val = if let SqlExpr::Value(value_with_span) = lim {
                        if let Value::Number(n, _) = &value_with_span.value {
                            n.parse::<u64>().ok()
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let offset_val = if let SqlExpr::Value(value_with_span) = off {
                        if let Value::Number(n, _) = &value_with_span.value {
                            n.parse::<u64>().ok()
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    (limit_val, offset_val)
                }
            }
        } else {
            (None, None)
        };

        Ok(Select {
            projection,
            from,
            joins,
            selection,
            group_by,
            having,
            order_by,
            limit,
            offset,
        })
    }

    fn convert_select_item_with_alias(&self, item: &SelectItem) -> Result<ProjectionItem> {
        match item {
            SelectItem::UnnamedExpr(expr) => Ok(ProjectionItem {
                expr: self.convert_expr(expr)?,
                alias: None,
            }),
            SelectItem::ExprWithAlias { expr, alias } => Ok(ProjectionItem {
                expr: self.convert_expr(expr)?,
                alias: Some(alias.value.clone()),
            }),
            SelectItem::Wildcard(_) => Ok(ProjectionItem {
                expr: Expr::Identifier("*".to_string()),
                alias: None,
            }),
            _ => Err(anyhow!("Unsupported select item: {:?}", item)),
        }
    }

    fn convert_table_with_joins(&self, table_with_joins: &TableWithJoins) -> Result<Vec<TableRef>> {
        let tables = vec![self.convert_table_factor(&table_with_joins.relation)?];

        // Note: Joins are handled separately in convert_select
        // This just returns the main table references

        Ok(tables)
    }

    fn convert_table_factor(&self, table_factor: &TableFactor) -> Result<TableRef> {
        match table_factor {
            TableFactor::Table { name, alias, .. } => {
                let table_name = name.to_string();
                let alias_name = alias.as_ref().map(|a| a.name.value.clone());

                Ok(TableRef {
                    name: Some(table_name),
                    subquery: None,
                    alias: alias_name,
                })
            }
            TableFactor::Derived {
                subquery, alias, ..
            } => {
                let converted_subquery = self.convert_query(subquery)?;
                let alias_name = alias.as_ref().map(|a| a.name.value.clone());

                Ok(TableRef {
                    name: None,
                    subquery: Some(Box::new(converted_subquery)),
                    alias: alias_name,
                })
            }
            _ => Err(anyhow!("Unsupported table factor: {:?}", table_factor)),
        }
    }

    fn convert_join(&self, join: &SqlJoin) -> Result<Join> {
        let kind = match &join.join_operator {
            JoinOperator::Join(_) => JoinType::Inner, // Default JOIN is treated as INNER
            JoinOperator::Inner(_) => JoinType::Inner,
            JoinOperator::Left(_) | JoinOperator::LeftOuter(_) => JoinType::LeftOuter,
            JoinOperator::Right(_) | JoinOperator::RightOuter(_) => JoinType::RightOuter,
            JoinOperator::FullOuter(_) => JoinType::FullOuter,
            JoinOperator::CrossJoin(_) => JoinType::Cross,
            _ => return Err(anyhow!("Unsupported join type: {:?}", join.join_operator)),
        };

        let right = self.convert_table_factor(&join.relation)?;

        let on = match &join.join_operator {
            JoinOperator::Join(constraint)
            | JoinOperator::Inner(constraint)
            | JoinOperator::Left(constraint)
            | JoinOperator::LeftOuter(constraint)
            | JoinOperator::Right(constraint)
            | JoinOperator::RightOuter(constraint)
            | JoinOperator::FullOuter(constraint) => match constraint {
                JoinConstraint::On(expr) => Some(self.convert_expr(expr)?),
                JoinConstraint::Using(_) => return Err(anyhow!("USING clause not supported")),
                JoinConstraint::Natural => None,
                JoinConstraint::None => None,
            },
            JoinOperator::CrossJoin(_) => None, // Cross join has no ON condition
            _ => None,
        };

        Ok(Join {
            join_type: kind,
            right_table: right,
            on_condition: on,
        })
    }

    fn convert_expr(&self, expr: &SqlExpr) -> Result<Expr> {
        match expr {
            SqlExpr::Identifier(ident) => Ok(Expr::Identifier(ident.value.clone())),

            SqlExpr::Value(value_with_span) => match &value_with_span.value {
                Value::Placeholder(ph) => Ok(Expr::Param(ph.clone())),
                _ => Ok(Expr::Literal(self.convert_value(&value_with_span.value)?)),
            },

            SqlExpr::BinaryOp { left, op, right } => {
                let left_expr = Box::new(self.convert_expr(left)?);
                let right_expr = Box::new(self.convert_expr(right)?);
                let binary_op = self.convert_binary_op(op)?;

                Ok(Expr::Binary {
                    left: left_expr,
                    op: binary_op,
                    right: right_expr,
                })
            }

            SqlExpr::UnaryOp { op, expr } => {
                let converted_expr = Box::new(self.convert_expr(expr)?);
                let unary_op = self.convert_unary_op(op)?;

                Ok(Expr::Unary {
                    op: unary_op,
                    expr: converted_expr,
                })
            }

            SqlExpr::Function(func) => {
                let name = func.name.to_string();
                // In sqlparser 0.59, args is FunctionArguments enum
                let arg_list = match &func.args {
                    sqlparser::ast::FunctionArguments::List(func_arg_list) => &func_arg_list.args,
                    _ => return Err(anyhow!("Unsupported function argument type")),
                };
                let args = arg_list
                    .iter()
                    .map(|arg| self.convert_function_arg(arg))
                    .collect::<Result<Vec<_>>>()?;

                // Check if it's an aggregate function
                if self.is_aggregate_function(&name) {
                    Ok(Expr::AggCall { name, args })
                } else {
                    // Normalize function name for SKS detection
                    let upper = name.to_uppercase();
                    match upper.as_str() {
                        "SIMILAR" => {
                            // Expected: SIMILAR(field, query, metric?, threshold?)
                            if args.len() < 2 {
                                return Err(anyhow!(
                                    "SIMILAR(field, query, metric?, threshold?) requires at least 2 arguments"
                                ));
                            }

                            // Validate field identifier
                            let field = match &args[0] {
                                Expr::Identifier(s) => s.clone(),
                                Expr::FuncCall { .. } | Expr::AggCall { .. } => {
                                    return Err(anyhow!(
                                        "SIMILAR: first argument must be a column identifier (e.g., embedding)"
                                    ));
                                }
                                _ => {
                                    return Err(anyhow!(
                                        "SIMILAR: unsupported first argument; expected identifier (e.g., embedding)"
                                    ));
                                }
                            };

                            // Validate query expression is present (allow literal/param/identifier)
                            let query_expr = Box::new(args[1].clone());
                            if matches!(args[1], Expr::Literal(Literal::Null)) {
                                return Err(anyhow!("SIMILAR: query argument cannot be NULL"));
                            }

                            // Optional metric validation
                            let metric = if args.len() >= 3 {
                                match &args[2] {
                                    Expr::Literal(Literal::String(s)) => {
                                        let m = s.to_lowercase();
                                        match m.as_str() {
                                            "cosine" | "dot" | "euclidean" => Some(m),
                                            _ => {
                                                return Err(anyhow!(
                                                    "SIMILAR: unsupported metric '{}'. Use 'cosine', 'dot', or 'euclidean'",
                                                    s
                                                ));
                                            }
                                        }
                                    }
                                    _ => {
                                        return Err(anyhow!(
                                            "SIMILAR: metric must be a string literal (e.g., 'cosine')"
                                        ));
                                    }
                                }
                            } else {
                                None
                            };

                            // Optional threshold validation
                            let threshold = if args.len() >= 4 {
                                match &args[3] {
                                    Expr::Literal(Literal::Number(n)) => {
                                        if n < &0.0 {
                                            return Err(anyhow!("SIMILAR: threshold must be ≥ 0"));
                                        }
                                        Some(*n)
                                    }
                                    _ => {
                                        return Err(anyhow!(
                                            "SIMILAR: threshold must be numeric (e.g., 0.75)"
                                        ));
                                    }
                                }
                            } else {
                                None
                            };

                            // Extra arguments guard
                            if args.len() > 4 {
                                return Err(anyhow!(
                                    "SIMILAR: too many arguments; expected up to 4"
                                ));
                            }

                            Ok(Expr::SksSimilar {
                                field,
                                query: query_expr,
                                metric,
                                threshold,
                            })
                        }
                        "FOLLOW" => {
                            // Expected: FOLLOW(start, edge_type, max_depth)
                            if args.len() != 3 {
                                return Err(anyhow!(
                                    "FOLLOW(start, edge, max_depth) requires exactly 3 arguments"
                                ));
                            }

                            // start node id (string or identifier)
                            let start = match &args[0] {
                                Expr::Literal(Literal::String(_)) | Expr::Identifier(_) => {
                                    Box::new(args[0].clone())
                                }
                                _ => {
                                    return Err(anyhow!(
                                        "FOLLOW: start must be an identifier or string literal (node id)"
                                    ));
                                }
                            };

                            // edge type (string or identifier)
                            let edge = match &args[1] {
                                Expr::Literal(Literal::String(s)) => s.clone(),
                                Expr::Identifier(s) => s.clone(),
                                _ => {
                                    return Err(anyhow!(
                                        "FOLLOW: edge must be a string literal or identifier (edge type)"
                                    ));
                                }
                            };

                            // depth must be positive integer
                            let max_depth = match &args[2] {
                                Expr::Literal(Literal::Number(n)) => {
                                    if n < &1.0 {
                                        return Err(anyhow!("FOLLOW: max_depth must be ≥ 1"));
                                    }
                                    *n as u32
                                }
                                _ => {
                                    return Err(anyhow!(
                                        "FOLLOW: max_depth must be a number (e.g., 2)"
                                    ));
                                }
                            };

                            Ok(Expr::SksFollow {
                                start,
                                edge,
                                max_depth,
                            })
                        }
                        "GEO_DISTANCE" => {
                            // GEO_DISTANCE(lat1, lon1, lat2, lon2) returns distance in km
                            if args.len() != 4 {
                                return Err(anyhow!(
                                    "GEO_DISTANCE(lat1, lon1, lat2, lon2) requires exactly 4 arguments"
                                ));
                            }
                            Ok(Expr::GeoDistance {
                                lat1: Box::new(args[0].clone()),
                                lon1: Box::new(args[1].clone()),
                                lat2: Box::new(args[2].clone()),
                                lon2: Box::new(args[3].clone()),
                            })
                        }
                        "GEO_WITHIN_DISTANCE" | "GEO_NEAR" => {
                            // GEO_WITHIN_DISTANCE(lat, lon, center_lat, center_lon, radius, unit?)
                            if args.len() < 5 || args.len() > 6 {
                                return Err(anyhow!(
                                    "GEO_WITHIN_DISTANCE(lat, lon, center_lat, center_lon, radius, unit?) requires 5-6 arguments"
                                ));
                            }
                            let unit = if args.len() == 6 {
                                match &args[5] {
                                    Expr::Literal(Literal::String(s)) => {
                                        let u = s.to_lowercase();
                                        match u.as_str() {
                                            "km" | "kilometers" => Some("km".to_string()),
                                            "mi" | "miles" => Some("mi".to_string()),
                                            "m" | "meters" => Some("m".to_string()),
                                            _ => return Err(anyhow!(
                                                "GEO_WITHIN_DISTANCE: unit must be 'km', 'mi', or 'm'"
                                            )),
                                        }
                                    }
                                    _ => return Err(anyhow!(
                                        "GEO_WITHIN_DISTANCE: unit must be a string literal"
                                    )),
                                }
                            } else {
                                Some("km".to_string()) // Default to km
                            };
                            Ok(Expr::GeoWithinDistance {
                                lat: Box::new(args[0].clone()),
                                lon: Box::new(args[1].clone()),
                                center_lat: Box::new(args[2].clone()),
                                center_lon: Box::new(args[3].clone()),
                                radius: Box::new(args[4].clone()),
                                unit,
                            })
                        }
                        "GEO_WITHIN_BOX" | "GEO_BBOX" => {
                            // GEO_WITHIN_BOX(lat, lon, sw_lat, sw_lon, ne_lat, ne_lon)
                            if args.len() != 6 {
                                return Err(anyhow!(
                                    "GEO_WITHIN_BOX(lat, lon, sw_lat, sw_lon, ne_lat, ne_lon) requires exactly 6 arguments"
                                ));
                            }
                            Ok(Expr::GeoWithinBox {
                                lat: Box::new(args[0].clone()),
                                lon: Box::new(args[1].clone()),
                                sw_lat: Box::new(args[2].clone()),
                                sw_lon: Box::new(args[3].clone()),
                                ne_lat: Box::new(args[4].clone()),
                                ne_lon: Box::new(args[5].clone()),
                            })
                        }
                        "GEO_POINT" => {
                            // GEO_POINT(lat, lon) creates a geo point
                            if args.len() != 2 {
                                return Err(anyhow!(
                                    "GEO_POINT(lat, lon) requires exactly 2 arguments"
                                ));
                            }
                            Ok(Expr::GeoPoint {
                                lat: Box::new(args[0].clone()),
                                lon: Box::new(args[1].clone()),
                            })
                        }
                        _ => Ok(Expr::FuncCall { name, args }),
                    }
                }
            }

            SqlExpr::CompoundIdentifier(idents) => {
                let combined = idents
                    .iter()
                    .map(|i| i.value.as_str())
                    .collect::<Vec<_>>()
                    .join(".");
                Ok(Expr::Identifier(combined))
            }

            SqlExpr::Case {
                operand,
                conditions,
                else_result,
                case_token: _,
                end_token: _,
            } => {
                // Convert CASE expression
                let lowered_operand = if let Some(op) = operand {
                    Some(Box::new(self.convert_expr(op)?))
                } else {
                    None
                };

                let mut lowered_conditions = Vec::new();
                // In sqlparser 0.59, conditions is Vec<CaseWhen>
                for case_when in conditions {
                    let when_expr = self.convert_expr(&case_when.condition)?;
                    let then_expr = self.convert_expr(&case_when.result)?;
                    lowered_conditions.push((when_expr, then_expr));
                }

                let lowered_else_expr = if let Some(el) = else_result {
                    Some(Box::new(self.convert_expr(el)?))
                } else {
                    None
                };

                Ok(Expr::Case {
                    operand: lowered_operand,
                    conditions: lowered_conditions,
                    else_expr: lowered_else_expr,
                })
            }

            SqlExpr::Subquery(subquery) => {
                // Convert subquery expression
                let converted_subquery = self.convert_query(subquery)?;
                Ok(Expr::Subquery(Box::new(converted_subquery)))
            }

            SqlExpr::InSubquery {
                expr,
                subquery,
                negated,
            } => {
                // Convert IN (SELECT ...) expression as a binary operation
                let left_expr = Box::new(self.convert_expr(expr)?);
                let subquery_expr =
                    Box::new(Expr::Subquery(Box::new(self.convert_query(subquery)?)));

                // Create a binary expression with IN operator
                Ok(Expr::Binary {
                    left: left_expr,
                    op: if *negated {
                        BinaryOp::NotIn
                    } else {
                        BinaryOp::In
                    },
                    right: subquery_expr,
                })
            }

            SqlExpr::Array(sqlparser::ast::Array { elem, named }) => {
                // Convert array literal [0.1, 0.2, ...] to Expr::Array
                let converted_elements: Result<Vec<Expr>> =
                    elem.iter().map(|e| self.convert_expr(e)).collect();

                Ok(Expr::Array {
                    elem: converted_elements?,
                    named: *named,
                })
            }

            // EXISTS (SELECT ...) / NOT EXISTS (SELECT ...)
            SqlExpr::Exists { subquery, negated } => {
                let converted_subquery = self.convert_query(subquery)?;
                Ok(Expr::Exists {
                    subquery: Box::new(converted_subquery),
                    negated: *negated,
                })
            }

            // expr LIKE pattern / expr NOT LIKE pattern
            SqlExpr::Like { negated, expr, pattern, .. } => {
                let left_expr = Box::new(self.convert_expr(expr)?);
                let right_expr = Box::new(self.convert_expr(pattern)?);
                Ok(Expr::Binary {
                    left: left_expr,
                    op: if *negated { BinaryOp::NotLike } else { BinaryOp::Like },
                    right: right_expr,
                })
            }

            // Case-insensitive LIKE (ILIKE) - treat as regular LIKE for now
            SqlExpr::ILike { negated, expr, pattern, .. } => {
                let left_expr = Box::new(self.convert_expr(expr)?);
                let right_expr = Box::new(self.convert_expr(pattern)?);
                Ok(Expr::Binary {
                    left: left_expr,
                    op: if *negated { BinaryOp::NotLike } else { BinaryOp::Like },
                    right: right_expr,
                })
            }

            // expr BETWEEN low AND high
            SqlExpr::Between { expr, negated, low, high } => {
                let converted_expr = Box::new(self.convert_expr(expr)?);
                let converted_low = Box::new(self.convert_expr(low)?);
                let converted_high = Box::new(self.convert_expr(high)?);
                Ok(Expr::Between {
                    expr: converted_expr,
                    low: converted_low,
                    high: converted_high,
                    negated: *negated,
                })
            }

            // expr IS NULL
            SqlExpr::IsNull(expr) => {
                let converted_expr = Box::new(self.convert_expr(expr)?);
                Ok(Expr::IsNull {
                    expr: converted_expr,
                    negated: false,
                })
            }

            // expr IS NOT NULL
            SqlExpr::IsNotNull(expr) => {
                let converted_expr = Box::new(self.convert_expr(expr)?);
                Ok(Expr::IsNull {
                    expr: converted_expr,
                    negated: true,
                })
            }

            // expr IN (val1, val2, ...)
            SqlExpr::InList { expr, list, negated } => {
                let converted_expr = Box::new(self.convert_expr(expr)?);
                let converted_list: Result<Vec<Expr>> =
                    list.iter().map(|e| self.convert_expr(e)).collect();
                Ok(Expr::InList {
                    expr: converted_expr,
                    list: converted_list?,
                    negated: *negated,
                })
            }

            // Parenthesized expression (nested)
            SqlExpr::Nested(inner) => {
                self.convert_expr(inner)
            }

            _ => Err(anyhow!("Unsupported expression: {:?}", expr)),
        }
    }

    fn convert_value(&self, value: &Value) -> Result<Literal> {
        match value {
            Value::Number(n, _) => {
                // Try to parse as integer first, then as float
                if let Ok(int_val) = n.parse::<i64>() {
                    Ok(Literal::Number(int_val as f64))
                } else if let Ok(float_val) = n.parse::<f64>() {
                    Ok(Literal::Number(float_val))
                } else {
                    Err(anyhow!("Invalid number: {}", n))
                }
            }
            Value::SingleQuotedString(s) | Value::DoubleQuotedString(s) => {
                Ok(Literal::String(s.clone()))
            }
            Value::Boolean(b) => Ok(Literal::Bool(*b)),
            Value::Null => Ok(Literal::Null),
            _ => Err(anyhow!("Unsupported literal value: {:?}", value)),
        }
    }

    fn convert_binary_op(&self, op: &BinaryOperator) -> Result<BinaryOp> {
        match op {
            BinaryOperator::Eq => Ok(BinaryOp::Eq),
            BinaryOperator::NotEq => Ok(BinaryOp::Ne),
            BinaryOperator::Lt => Ok(BinaryOp::Lt),
            BinaryOperator::LtEq => Ok(BinaryOp::Le),
            BinaryOperator::Gt => Ok(BinaryOp::Gt),
            BinaryOperator::GtEq => Ok(BinaryOp::Ge),
            BinaryOperator::And => Ok(BinaryOp::And),
            BinaryOperator::Or => Ok(BinaryOp::Or),
            BinaryOperator::Plus => Ok(BinaryOp::Add),
            BinaryOperator::Minus => Ok(BinaryOp::Sub),
            BinaryOperator::Multiply => Ok(BinaryOp::Mul),
            BinaryOperator::Divide => Ok(BinaryOp::Div),
            BinaryOperator::Modulo => Ok(BinaryOp::Mod),
            _ => Err(anyhow!("Unsupported binary operator: {:?}", op)),
        }
    }

    fn convert_unary_op(&self, op: &UnaryOperator) -> Result<UnaryOp> {
        match op {
            UnaryOperator::Not => Ok(UnaryOp::Not),
            UnaryOperator::Minus => Ok(UnaryOp::Neg),
            _ => Err(anyhow!("Unsupported unary operator: {:?}", op)),
        }
    }

    fn convert_function_arg(&self, arg: &FunctionArg) -> Result<Expr> {
        match arg {
            FunctionArg::Named { .. } => Err(anyhow!("Named function arguments not supported")),
            FunctionArg::Unnamed(FunctionArgExpr::Expr(expr)) => self.convert_expr(expr),
            FunctionArg::Unnamed(FunctionArgExpr::Wildcard) => {
                Ok(Expr::Identifier("*".to_string()))
            }
            _ => Err(anyhow!("Unsupported function argument: {:?}", arg)),
        }
    }

    fn convert_order_by_expr(&self, order_expr: &SqlOrderByExpr) -> Result<OrderByExpr> {
        let expr = self.convert_expr(&order_expr.expr)?;
        // In sqlparser 0.59, asc is in options.asc
        let asc = order_expr.options.asc.unwrap_or(true); // Default to ascending

        Ok(OrderByExpr { expr, asc })
    }

    fn is_aggregate_function(&self, name: &str) -> bool {
        matches!(
            name.to_lowercase().as_str(),
            "count" | "sum" | "avg" | "min" | "max" | "stddev" | "variance"
        )
    }

    // ========================
    // DML Statement Parsing
    // ========================

    /// Parse SQL text and return a DML statement if it's INSERT/UPDATE/DELETE
    pub fn parse_dml(&self, sql: &str) -> Result<Option<DmlStatement>> {
        let statements = Parser::parse_sql(&self.dialect, sql)
            .map_err(|e| anyhow!("SQL parsing failed: {}", e))?;

        if statements.is_empty() {
            return Err(anyhow!("No SQL statements found"));
        }

        if statements.len() > 1 {
            return Err(anyhow!(
                "Multiple statements not supported, found {}",
                statements.len()
            ));
        }

        let statement = &statements[0];
        self.try_convert_dml(statement)
    }

    /// Try to convert a statement to DML, returning None for SELECT queries
    fn try_convert_dml(&self, statement: &Statement) -> Result<Option<DmlStatement>> {
        match statement {
            Statement::Insert(insert) => {
                Ok(Some(self.convert_insert(insert)?))
            }
            Statement::Update { table, assignments, selection, .. } => {
                Ok(Some(self.convert_update(table, assignments, selection)?))
            }
            Statement::Delete(delete) => {
                Ok(Some(self.convert_delete(delete)?))
            }
            Statement::Query(_) => Ok(None), // SELECT query, not DML
            _ => Err(anyhow!("Unsupported statement type for DML")),
        }
    }

    /// Convert INSERT statement to DmlStatement
    fn convert_insert(&self, insert: &sqlparser::ast::Insert) -> Result<DmlStatement> {
        // Get table name
        let table_name = insert.table.to_string();

        // Get column names
        let columns: Vec<String> = insert.columns.iter()
            .map(|c| c.value.clone())
            .collect();

        // Get values from source
        let values = match &insert.source {
            Some(source) => self.extract_values_from_source(source)?,
            None => return Err(anyhow!("INSERT requires VALUES clause")),
        };

        // Check for ON CONFLICT (UPSERT) - simplified, return basic insert for now
        if insert.on.is_some() {
            // For now, treat ON CONFLICT as upsert with empty conflict handling
            return Ok(DmlStatement::Upsert {
                table_name,
                columns,
                values,
                conflict_columns: Vec::new(),
                update_assignments: Vec::new(),
            });
        }

        Ok(DmlStatement::Insert {
            table_name,
            columns,
            values,
        })
    }

    /// Extract values from INSERT source (VALUES clause)
    fn extract_values_from_source(&self, source: &sqlparser::ast::Query) -> Result<Vec<Vec<SqlValueLiteral>>> {
        match &*source.body {
            SetExpr::Values(values) => {
                values.rows.iter()
                    .map(|row| {
                        row.iter()
                            .map(|expr| self.convert_expr_to_dml_literal(expr))
                            .collect()
                    })
                    .collect()
            }
            _ => Err(anyhow!("INSERT source must be VALUES clause")),
        }
    }

    /// Convert expression to SQL value literal (for DML)
    fn convert_expr_to_dml_literal(&self, expr: &SqlExpr) -> Result<SqlValueLiteral> {
        match expr {
            SqlExpr::Value(value_with_span) => {
                // In sqlparser 0.59, Value is wrapped in ValueWithSpan
                self.convert_value_to_dml_literal(&value_with_span.value)
            }
            SqlExpr::Array(arr) => {
                let elements: Result<Vec<SqlValueLiteral>> = arr.elem.iter()
                    .map(|e| self.convert_expr_to_dml_literal(e))
                    .collect();
                Ok(SqlValueLiteral::Array(elements?))
            }
            SqlExpr::UnaryOp { op: UnaryOperator::Minus, expr } => {
                // Handle negative numbers
                match self.convert_expr_to_dml_literal(expr)? {
                    SqlValueLiteral::Integer(i) => Ok(SqlValueLiteral::Integer(-i)),
                    SqlValueLiteral::Float(f) => Ok(SqlValueLiteral::Float(-f)),
                    _ => Err(anyhow!("Unary minus only supported for numbers")),
                }
            }
            SqlExpr::Function(func) => {
                let name = func.name.to_string();
                // Extract function args based on sqlparser 0.59 API
                let args = self.extract_function_args_dml(&func.args)?;
                Ok(SqlValueLiteral::Function { name, args })
            }
            SqlExpr::Identifier(ident) => {
                // Could be DEFAULT or a column reference
                if ident.value.eq_ignore_ascii_case("DEFAULT") {
                    Ok(SqlValueLiteral::Default)
                } else {
                    Ok(SqlValueLiteral::Column(ident.value.clone()))
                }
            }
            _ => Err(anyhow!("Unsupported expression in VALUES: {:?}", expr)),
        }
    }

    /// Extract function arguments for DML
    fn extract_function_args_dml(&self, args: &sqlparser::ast::FunctionArguments) -> Result<Vec<SqlValueLiteral>> {
        use sqlparser::ast::FunctionArguments;
        match args {
            FunctionArguments::List(list) => {
                list.args.iter()
                    .filter_map(|arg| {
                        match arg {
                            FunctionArg::Unnamed(FunctionArgExpr::Expr(e)) => {
                                self.convert_expr_to_dml_literal(e).ok()
                            }
                            _ => None,
                        }
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .map(Ok)
                    .collect()
            }
            FunctionArguments::None => Ok(Vec::new()),
            FunctionArguments::Subquery(_) => Err(anyhow!("Subquery function args not supported")),
        }
    }

    /// Convert sqlparser Value to SqlValueLiteral (for DML)
    fn convert_value_to_dml_literal(&self, value: &Value) -> Result<SqlValueLiteral> {
        match value {
            Value::Null => Ok(SqlValueLiteral::Null),
            Value::Boolean(b) => Ok(SqlValueLiteral::Boolean(*b)),
            Value::Number(n, _) => {
                // Try to parse as integer first, then float
                if let Ok(i) = n.parse::<i64>() {
                    Ok(SqlValueLiteral::Integer(i))
                } else if let Ok(f) = n.parse::<f64>() {
                    Ok(SqlValueLiteral::Float(f))
                } else {
                    Err(anyhow!("Invalid number: {}", n))
                }
            }
            Value::SingleQuotedString(s) | Value::DoubleQuotedString(s) => {
                Ok(SqlValueLiteral::String(s.clone()))
            }
            Value::HexStringLiteral(h) => {
                // Simple hex decode
                let bytes: Vec<u8> = h.as_bytes().chunks(2)
                    .filter_map(|chunk| {
                        if chunk.len() == 2 {
                            u8::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()
                        } else {
                            None
                        }
                    })
                    .collect();
                Ok(SqlValueLiteral::Binary(bytes))
            }
            _ => Err(anyhow!("Unsupported value type: {:?}", value)),
        }
    }

    /// Convert UPDATE statement to DmlStatement
    fn convert_update(
        &self,
        table: &sqlparser::ast::TableWithJoins,
        assignments: &[sqlparser::ast::Assignment],
        selection: &Option<SqlExpr>,
    ) -> Result<DmlStatement> {
        // Get table name
        let table_name = match &table.relation {
            TableFactor::Table { name, .. } => name.to_string(),
            _ => return Err(anyhow!("UPDATE requires a table name")),
        };

        // Convert assignments
        let mut update_assignments = Vec::new();
        for assignment in assignments {
            let column = self.assignment_target_to_string(&assignment.target)?;
            let value = self.convert_expr_to_dml_literal(&assignment.value)?;
            update_assignments.push((column, value));
        }

        // Convert WHERE clause
        let where_clause = if let Some(selection) = selection {
            Some(self.convert_where_clause_dml(selection)?)
        } else {
            None
        };

        Ok(DmlStatement::Update {
            table_name,
            assignments: update_assignments,
            where_clause,
        })
    }

    /// Convert assignment target to string
    fn assignment_target_to_string(&self, target: &sqlparser::ast::AssignmentTarget) -> Result<String> {
        use sqlparser::ast::AssignmentTarget;
        match target {
            AssignmentTarget::ColumnName(names) => {
                // ObjectName is a newtype around Vec<Ident>, access inner with .0
                Ok(names.0.iter().map(|n| n.to_string()).collect::<Vec<_>>().join("."))
            }
            AssignmentTarget::Tuple(cols) => {
                // For tuple assignment, join column names
                Ok(cols.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", "))
            }
        }
    }

    /// Convert DELETE statement to DmlStatement
    fn convert_delete(&self, delete: &sqlparser::ast::Delete) -> Result<DmlStatement> {
        use sqlparser::ast::FromTable;

        // Get table name from FROM clause
        let table_name = match &delete.from {
            FromTable::WithFromKeyword(tables) => {
                if let Some(first) = tables.first() {
                    match &first.relation {
                        TableFactor::Table { name, .. } => name.to_string(),
                        _ => return Err(anyhow!("DELETE requires a table name")),
                    }
                } else {
                    return Err(anyhow!("DELETE requires FROM clause"));
                }
            }
            FromTable::WithoutKeyword(tables) => {
                if let Some(first) = tables.first() {
                    match &first.relation {
                        TableFactor::Table { name, .. } => name.to_string(),
                        _ => return Err(anyhow!("DELETE requires a table name")),
                    }
                } else {
                    return Err(anyhow!("DELETE requires FROM clause"));
                }
            }
        };

        // Convert WHERE clause
        let where_clause = if let Some(selection) = &delete.selection {
            Some(self.convert_where_clause_dml(selection)?)
        } else {
            None
        };

        Ok(DmlStatement::Delete {
            table_name,
            where_clause,
        })
    }

    /// Convert WHERE clause expression to WhereClause (for DML)
    fn convert_where_clause_dml(&self, expr: &SqlExpr) -> Result<WhereClause> {
        let conditions = self.extract_conditions_dml(expr)?;
        let operator = self.determine_logical_operator_dml(expr);

        Ok(WhereClause {
            conditions,
            operator,
        })
    }

    /// Extract conditions from a WHERE expression (for DML)
    fn extract_conditions_dml(&self, expr: &SqlExpr) -> Result<Vec<Condition>> {
        match expr {
            SqlExpr::BinaryOp { left, op, right } => {
                match op {
                    BinaryOperator::And | BinaryOperator::Or => {
                        // Recursively extract conditions from both sides
                        let mut conditions = self.extract_conditions_dml(left)?;
                        conditions.extend(self.extract_conditions_dml(right)?);
                        Ok(conditions)
                    }
                    BinaryOperator::Eq | BinaryOperator::NotEq |
                    BinaryOperator::Lt | BinaryOperator::LtEq |
                    BinaryOperator::Gt | BinaryOperator::GtEq => {
                        // Simple comparison condition
                        let column = self.expr_to_column_name_dml(left)?;
                        let operator = self.convert_comparison_op_dml(op)?;
                        let value = self.convert_expr_to_dml_literal(right)?;
                        Ok(vec![Condition::Comparison { column, operator, value }])
                    }
                    _ => Err(anyhow!("Unsupported operator in WHERE: {:?}", op)),
                }
            }
            SqlExpr::InList { expr, list, negated } => {
                let column = self.expr_to_column_name_dml(expr)?;
                let values: Result<Vec<SqlValueLiteral>> = list.iter()
                    .map(|e| self.convert_expr_to_dml_literal(e))
                    .collect();
                Ok(vec![Condition::In { column, values: values?, negated: *negated }])
            }
            SqlExpr::Between { expr, negated, low, high } => {
                let column = self.expr_to_column_name_dml(expr)?;
                let low_val = self.convert_expr_to_dml_literal(low)?;
                let high_val = self.convert_expr_to_dml_literal(high)?;
                Ok(vec![Condition::Between {
                    column,
                    low: low_val,
                    high: high_val,
                    negated: *negated,
                }])
            }
            SqlExpr::IsNull(expr) => {
                let column = self.expr_to_column_name_dml(expr)?;
                Ok(vec![Condition::IsNull { column, negated: false }])
            }
            SqlExpr::IsNotNull(expr) => {
                let column = self.expr_to_column_name_dml(expr)?;
                Ok(vec![Condition::IsNull { column, negated: true }])
            }
            SqlExpr::Like { expr, pattern, negated, .. } => {
                let column = self.expr_to_column_name_dml(expr)?;
                let pattern_str = self.extract_like_pattern(pattern)?;
                Ok(vec![Condition::Like { column, pattern: pattern_str, negated: *negated }])
            }
            SqlExpr::Nested(inner) => {
                self.extract_conditions_dml(inner)
            }
            _ => Err(anyhow!("Unsupported WHERE expression: {:?}", expr)),
        }
    }

    /// Extract LIKE pattern string
    fn extract_like_pattern(&self, pattern: &SqlExpr) -> Result<String> {
        match pattern {
            SqlExpr::Value(value_with_span) => {
                match &value_with_span.value {
                    Value::SingleQuotedString(s) => Ok(s.clone()),
                    Value::DoubleQuotedString(s) => Ok(s.clone()),
                    _ => Err(anyhow!("LIKE pattern must be a string")),
                }
            }
            _ => Err(anyhow!("LIKE pattern must be a string literal")),
        }
    }

    /// Extract column name from expression (for DML)
    fn expr_to_column_name_dml(&self, expr: &SqlExpr) -> Result<String> {
        match expr {
            SqlExpr::Identifier(ident) => Ok(ident.value.clone()),
            SqlExpr::CompoundIdentifier(parts) => {
                Ok(parts.iter().map(|p| p.value.clone()).collect::<Vec<_>>().join("."))
            }
            _ => Err(anyhow!("Expected column name, got {:?}", expr)),
        }
    }

    /// Convert comparison operator (for DML)
    fn convert_comparison_op_dml(&self, op: &BinaryOperator) -> Result<DmlComparisonOperator> {
        match op {
            BinaryOperator::Eq => Ok(DmlComparisonOperator::Equal),
            BinaryOperator::NotEq => Ok(DmlComparisonOperator::NotEqual),
            BinaryOperator::Lt => Ok(DmlComparisonOperator::LessThan),
            BinaryOperator::LtEq => Ok(DmlComparisonOperator::LessThanOrEqual),
            BinaryOperator::Gt => Ok(DmlComparisonOperator::GreaterThan),
            BinaryOperator::GtEq => Ok(DmlComparisonOperator::GreaterThanOrEqual),
            _ => Err(anyhow!("Not a comparison operator: {:?}", op)),
        }
    }

    /// Determine the logical operator from an expression (for DML)
    fn determine_logical_operator_dml(&self, expr: &SqlExpr) -> LogicalOperator {
        match expr {
            SqlExpr::BinaryOp { op: BinaryOperator::Or, .. } => LogicalOperator::Or,
            _ => LogicalOperator::And, // Default to AND
        }
    }
}

impl Default for SqlFrontendParser {
    fn default() -> Self {
        Self::new()
    }
}

// ========================
// DDL Statement Parsing
// ========================

use crate::services::ddl::{
    AlterTableChange, ColumnDefinition, ColumnPosition, DdlStatement,
    SqlDataType, TableConstraint,
};

impl SqlFrontendParser {
    /// Parse SQL text and return a DDL statement if it's CREATE/ALTER/DROP
    pub fn parse_ddl(&self, sql: &str) -> Result<Option<DdlStatement>> {
        let statements = Parser::parse_sql(&self.dialect, sql)
            .map_err(|e| anyhow!("SQL parsing failed: {}", e))?;

        if statements.is_empty() {
            return Err(anyhow!("No SQL statements found"));
        }

        if statements.len() > 1 {
            return Err(anyhow!(
                "Multiple statements not supported, found {}",
                statements.len()
            ));
        }

        let statement = &statements[0];
        self.try_convert_ddl(statement)
    }

    /// Try to convert a statement to DDL, returning None for non-DDL statements
    fn try_convert_ddl(&self, statement: &Statement) -> Result<Option<DdlStatement>> {
        use sqlparser::ast::{AlterTableOperation, ColumnOption};

        match statement {
            Statement::AlterTable {
                name,
                if_exists: _,
                only: _,
                operations,
                ..
            } => {
                let table_name = name.to_string();
                let mut changes = Vec::new();

                for op in operations {
                    match op {
                        AlterTableOperation::AddColumn { column_def, .. } => {
                            let col = self.convert_column_def(column_def)?;
                            changes.push(AlterTableChange::AddColumn(col));
                        }
                        AlterTableOperation::DropColumn { column_names, .. } => {
                            // Take the first column name (most ALTER DROP COLUMN has single column)
                            if let Some(col_name) = column_names.first() {
                                changes.push(AlterTableChange::DropColumn(col_name.to_string()));
                            }
                        }
                        AlterTableOperation::RenameColumn { old_column_name, new_column_name } => {
                            changes.push(AlterTableChange::RenameColumn {
                                old_name: old_column_name.to_string(),
                                new_name: new_column_name.to_string(),
                            });
                        }
                        AlterTableOperation::AlterColumn { column_name, op } => {
                            use sqlparser::ast::AlterColumnOperation;
                            match op {
                                AlterColumnOperation::SetNotNull => {
                                    changes.push(AlterTableChange::SetNullable {
                                        column_name: column_name.to_string(),
                                        nullable: false,
                                    });
                                }
                                AlterColumnOperation::DropNotNull => {
                                    changes.push(AlterTableChange::SetNullable {
                                        column_name: column_name.to_string(),
                                        nullable: true,
                                    });
                                }
                                AlterColumnOperation::SetDefault { value } => {
                                    changes.push(AlterTableChange::SetDefault {
                                        column_name: column_name.to_string(),
                                        default_value: Some(format!("{}", value)),
                                    });
                                }
                                AlterColumnOperation::DropDefault => {
                                    changes.push(AlterTableChange::SetDefault {
                                        column_name: column_name.to_string(),
                                        default_value: None,
                                    });
                                }
                                AlterColumnOperation::SetDataType { data_type, .. } => {
                                    let new_type = self.convert_data_type(data_type)?;
                                    changes.push(AlterTableChange::ChangeType {
                                        column_name: column_name.to_string(),
                                        new_type,
                                    });
                                }
                                _ => {
                                    return Err(anyhow!("Unsupported ALTER COLUMN operation: {:?}", op));
                                }
                            }
                        }
                        AlterTableOperation::AddConstraint { constraint, .. } => {
                            use sqlparser::ast::TableConstraint as SqlConstraint;
                            match constraint {
                                SqlConstraint::Unique { name, columns, .. } => {
                                    let cols: Vec<String> = columns.iter().map(|c| c.to_string()).collect();
                                    changes.push(AlterTableChange::AddConstraint {
                                        constraint_name: name.as_ref().map(|n| n.to_string()),
                                        constraint: TableConstraint::Unique { columns: cols },
                                    });
                                }
                                SqlConstraint::Check { name, expr, .. } => {
                                    changes.push(AlterTableChange::AddConstraint {
                                        constraint_name: name.as_ref().map(|n| n.to_string()),
                                        constraint: TableConstraint::Check {
                                            expression: format!("{}", expr),
                                        },
                                    });
                                }
                                SqlConstraint::ForeignKey { name, columns, foreign_table, referred_columns, .. } => {
                                    let cols: Vec<String> = columns.iter().map(|c| c.to_string()).collect();
                                    let ref_cols: Vec<String> = referred_columns.iter().map(|c| c.to_string()).collect();
                                    changes.push(AlterTableChange::AddConstraint {
                                        constraint_name: name.as_ref().map(|n| n.to_string()),
                                        constraint: TableConstraint::ForeignKey {
                                            columns: cols,
                                            references_table: foreign_table.to_string(),
                                            references_columns: ref_cols,
                                        },
                                    });
                                }
                                _ => {
                                    return Err(anyhow!("Unsupported constraint type"));
                                }
                            }
                        }
                        AlterTableOperation::DropConstraint { name, .. } => {
                            changes.push(AlterTableChange::DropConstraint {
                                constraint_name: name.to_string(),
                            });
                        }
                        AlterTableOperation::ChangeColumn { old_name, new_name, data_type, options, .. } => {
                            // MySQL-style CHANGE COLUMN (rename + type change)
                            if old_name.to_string() != new_name.to_string() {
                                changes.push(AlterTableChange::RenameColumn {
                                    old_name: old_name.to_string(),
                                    new_name: new_name.to_string(),
                                });
                            }
                            let new_type = self.convert_data_type(data_type)?;
                            changes.push(AlterTableChange::ChangeType {
                                column_name: new_name.to_string(),
                                new_type,
                            });
                            // Handle FIRST/AFTER positioning
                            for opt in options {
                                if let ColumnOption::CharacterSet(cs) = opt {
                                    // Check for FIRST/AFTER in the raw string
                                    let cs_str = cs.to_string().to_uppercase();
                                    if cs_str == "FIRST" {
                                        changes.push(AlterTableChange::MoveColumn {
                                            column_name: new_name.to_string(),
                                            position: ColumnPosition::First,
                                        });
                                    }
                                }
                            }
                        }
                        _ => {
                            return Err(anyhow!("Unsupported ALTER TABLE operation: {:?}", op));
                        }
                    }
                }

                Ok(Some(DdlStatement::AlterTable { table_name, changes }))
            }
            Statement::CreateTable(_) | Statement::CreateIndex(_) | Statement::Drop { .. } => {
                // These can be added later if needed
                Err(anyhow!("CREATE/DROP DDL statements should use the DDL service directly"))
            }
            Statement::Query(_) => Ok(None), // SELECT query, not DDL
            Statement::Insert(_) | Statement::Update { .. } | Statement::Delete(_) => Ok(None), // DML
            _ => Ok(None),
        }
    }

    /// Convert sqlparser column definition to DDL ColumnDefinition
    fn convert_column_def(&self, col_def: &sqlparser::ast::ColumnDef) -> Result<ColumnDefinition> {
        use sqlparser::ast::ColumnOption;

        let name = col_def.name.to_string();
        let data_type = self.convert_data_type(&col_def.data_type)?;
        let mut nullable = true;
        let mut default_value = None;
        let mut comment = None;
        let mut primary_key = false;

        for option in &col_def.options {
            match &option.option {
                ColumnOption::NotNull => {
                    nullable = false;
                }
                ColumnOption::Null => {
                    nullable = true;
                }
                ColumnOption::Default(expr) => {
                    default_value = Some(format!("{}", expr));
                }
                ColumnOption::Comment(c) => {
                    comment = Some(c.clone());
                }
                ColumnOption::Unique { is_primary, .. } => {
                    if *is_primary {
                        primary_key = true;
                        nullable = false;
                    }
                }
                _ => {}
            }
        }

        Ok(ColumnDefinition {
            name,
            data_type,
            nullable,
            default_value,
            comment,
            primary_key,
        })
    }

    /// Convert sqlparser DataType to DDL SqlDataType
    fn convert_data_type(&self, dt: &sqlparser::ast::DataType) -> Result<SqlDataType> {
        use sqlparser::ast::DataType as SqlDt;

        match dt {
            SqlDt::Boolean | SqlDt::Bool => Ok(SqlDataType::Boolean),
            SqlDt::TinyInt(_) => Ok(SqlDataType::TinyInt),
            SqlDt::SmallInt(_) => Ok(SqlDataType::SmallInt),
            SqlDt::Int(_) | SqlDt::Integer(_) => Ok(SqlDataType::Int),
            SqlDt::BigInt(_) => Ok(SqlDataType::BigInt),
            SqlDt::Real | SqlDt::Float(_) => Ok(SqlDataType::Float),
            SqlDt::Double(_) | SqlDt::DoublePrecision => Ok(SqlDataType::Double),
            SqlDt::Decimal(info) | SqlDt::Numeric(info) => {
                use sqlparser::ast::ExactNumberInfo;
                match info {
                    ExactNumberInfo::PrecisionAndScale(p, s) => {
                        Ok(SqlDataType::Decimal { precision: *p as u32, scale: *s as u32 })
                    }
                    ExactNumberInfo::Precision(p) => {
                        Ok(SqlDataType::Decimal { precision: *p as u32, scale: 0 })
                    }
                    ExactNumberInfo::None => {
                        Ok(SqlDataType::Decimal { precision: 38, scale: 9 })
                    }
                }
            }
            SqlDt::Varchar(info) | SqlDt::CharVarying(info) => {
                use sqlparser::ast::CharacterLength;
                let max_length = match info {
                    Some(CharacterLength::IntegerLength { length, .. }) => Some(*length as u32),
                    _ => None,
                };
                Ok(SqlDataType::Varchar { max_length })
            }
            SqlDt::Text => Ok(SqlDataType::Text),
            SqlDt::Binary(_) | SqlDt::Varbinary(_) => Ok(SqlDataType::Binary),
            SqlDt::Blob(_) => Ok(SqlDataType::Blob),
            SqlDt::Date => Ok(SqlDataType::Date),
            SqlDt::Time(_, _) => Ok(SqlDataType::Time),
            SqlDt::Timestamp(_, tz_info) => {
                use sqlparser::ast::TimezoneInfo;
                match tz_info {
                    TimezoneInfo::WithTimeZone | TimezoneInfo::Tz => Ok(SqlDataType::TimestampTz),
                    _ => Ok(SqlDataType::Timestamp),
                }
            }
            SqlDt::Uuid => Ok(SqlDataType::Uuid),
            SqlDt::JSON | SqlDt::JSONB => Ok(SqlDataType::Json),
            SqlDt::Custom(name, modifiers) => {
                let type_name = name.to_string().to_uppercase();
                match type_name.as_str() {
                    "VECTOR" => {
                        // Parse dimension from modifiers: VECTOR(768)
                        let dimension = modifiers.first()
                            .and_then(|m| m.to_string().parse::<u32>().ok())
                            .unwrap_or(0);
                        Ok(SqlDataType::Vector { dimension })
                    }
                    "SPARSE_VECTOR" => {
                        let dimension = modifiers.first()
                            .and_then(|m| m.to_string().parse::<u32>().ok())
                            .unwrap_or(0);
                        Ok(SqlDataType::SparseVector { dimension })
                    }
                    "BINARY_VECTOR" => {
                        let dimension = modifiers.first()
                            .and_then(|m| m.to_string().parse::<u32>().ok())
                            .unwrap_or(0);
                        Ok(SqlDataType::BinaryVector { dimension })
                    }
                    _ => Err(anyhow!("Unsupported custom type: {}", type_name)),
                }
            }
            _ => Err(anyhow!("Unsupported data type: {:?}", dt)),
        }
    }
}
