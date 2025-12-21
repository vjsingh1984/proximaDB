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
            // BinaryOperator::Like => Ok(BinaryOp::Like), // Removed in newer sqlparser
            BinaryOperator::Plus => Ok(BinaryOp::Add),
            BinaryOperator::Minus => Ok(BinaryOp::Sub),
            BinaryOperator::Multiply => Ok(BinaryOp::Mul),
            BinaryOperator::Divide => Ok(BinaryOp::Div),
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
}

impl Default for SqlFrontendParser {
    fn default() -> Self {
        Self::new()
    }
}
