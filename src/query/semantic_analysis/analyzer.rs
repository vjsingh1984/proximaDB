//! Contains the main logic for the semantic analysis pass.

use crate::query::ast::{Query, Select, Expr, BinaryOp, UnaryOp, Literal, ProjectionItem, TableRef};
use crate::query::semantic_analysis::scope::{Scope, Symbol, Column, DataType};
use crate::services::collection::manager::CollectionService;
use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::sync::Arc;

/// The semantic analyzer.
pub struct Analyzer {
    collection_service: Arc<CollectionService>,
}

impl Analyzer {
    /// Creates a new analyzer.
    pub fn new(collection_service: Arc<CollectionService>) -> Self {
        Self { collection_service }
    }

    /// Analyzes a query.
    pub async fn analyze(&self, query: &Query) -> Result<Scope> {
        let mut scope = Scope::new();
        self.analyze_query(query, &mut scope).await?;
        Ok(scope)
    }

    fn analyze_query<'a>(&'a self, query: &'a Query, scope: &'a mut Scope) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
        match query {
            Query::Select(select) => self.analyze_select(select, scope).await,
            Query::With { ctes, query } => {
                // Analyze CTEs and add them to the scope
                for cte in ctes {
                    let mut cte_scope = Scope::new_with_parent(scope.clone());
                    self.analyze_query(&cte.query, &mut cte_scope).await?;
                    // For now, just add the CTE name as a table symbol without columns
                    // TODO: Extract columns from CTE query for proper type checking
                    scope.insert(&cte.name, Symbol::Table { name: cte.name.clone(), columns: HashMap::new() });
                }
                self.analyze_query(query, scope).await
            }
            Query::Set { .. } => Err(anyhow!("SET operations are not yet supported")),
        }
        })
    }

    async fn analyze_select(&self, select: &Select, scope: &mut Scope) -> Result<()> {
        // Analyze the FROM clause and populate the scope with tables and columns.
        for table_ref in &select.from {
            self.analyze_table_ref(table_ref, scope).await?;
        }

        // Analyze JOINs
        for join in &select.joins {
            self.analyze_table_ref(&join.right_table, scope).await?;
            if let Some(on_condition) = &join.on_condition {
                self.analyze_expr(on_condition, scope).await?;
            }
        }

        // Analyze the WHERE clause.
        if let Some(selection) = &select.selection {
            self.analyze_expr(selection, scope).await?;
        }

        // Analyze the GROUP BY clause.
        for expr in &select.group_by {
            self.analyze_expr(expr, scope).await?;
        }

        // Analyze the HAVING clause.
        if let Some(having) = &select.having {
            self.analyze_expr(having, scope).await?;
        }

        // Analyze the projection.
        for item in &select.projection {
            self.analyze_projection_item(item, scope).await?;
        }

        // Analyze the ORDER BY clause.
        for order_expr in &select.order_by {
            self.analyze_expr(&order_expr.expr, scope).await?;
        }

        Ok(())
    }

    async fn analyze_table_ref(&self, table_ref: &TableRef, scope: &mut Scope) -> Result<()> {
        if let Some(table_name) = &table_ref.name {
            let collection = self.collection_service.collection(table_name).await?;
            if let Some(collection) = collection {
                let mut columns = HashMap::new();
                if let Some(config) = collection.config {
                    // Use filterable_columns instead of schema which doesn't exist
                    for field in config.filterable_columns {
                        let data_type = match field.data_type() {
                                crate::proto::proximadb_v1::FilterableDataType::FilterableString => DataType::String,
                                crate::proto::proximadb_v1::FilterableDataType::FilterableInteger => DataType::Int64,
                                crate::proto::proximadb_v1::FilterableDataType::FilterableFloat => DataType::Float64,
                                crate::proto::proximadb_v1::FilterableDataType::FilterableBoolean => DataType::Boolean,
                                _ => DataType::Unknown,
                            };
                            columns.insert(field.name.clone(), Column { name: field.name.clone(), data_type });
                        }
                }
                scope.insert(table_name, Symbol::Table { name: table_name.clone(), columns });
            } else {
                return Err(anyhow!("Table not found: {}", table_name));
            }
        } else if let Some(subquery) = &table_ref.subquery {
            // Analyze subquery and add its projected columns to the scope
            let mut subquery_scope = Scope::new_with_parent(scope.clone());
            self.analyze_query(subquery, &mut subquery_scope).await?;
            // TODO: Extract projected columns from subquery for proper type checking
            // For now, just add a placeholder table symbol
            if let Some(alias) = &table_ref.alias {
                scope.insert(alias, Symbol::Table { name: alias.clone(), columns: HashMap::new() });
            }
        }
        Ok(())
    }

    async fn analyze_projection_item(&self, item: &ProjectionItem, scope: &mut Scope) -> Result<()> {
        self.analyze_expr(&item.expr, scope).await?;
        // If there's an alias, add it to the current scope
        if let Some(alias) = &item.alias {
            scope.insert(alias, Symbol::Alias(item.expr.clone()));
        }
        Ok(())
    }

    fn analyze_expr<'a>(&'a self, expr: &'a Expr, scope: &'a mut Scope) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<DataType>> + Send + 'a>> {
        Box::pin(async move {
        match expr {
            Expr::Identifier(ident) => {
                // Handle qualified identifiers (e.g., table.column)
                let parts: Vec<&str> = ident.split('.').collect();
                if parts.len() == 2 {
                    let table_name = parts[0];
                    let column_name = parts[1];
                    // Clone the column data to avoid borrowing conflicts
                    if let Some(Symbol::Table { columns, .. }) = scope.lookup(table_name) {
                        if let Some(column) = columns.get(column_name) {
                            let column_clone = column.clone();
                            scope.insert(ident, Symbol::Column(column_clone.clone()));
                            Ok(column_clone.data_type)
                        } else {
                            Err(anyhow!("Column '{}' not found in table '{}'", column_name, table_name))
                        }
                    } else {
                        Err(anyhow!("Table '{}' not found in scope", table_name))
                    }
                } else if parts.len() == 1 {
                    // Unqualified identifier (e.g., column)
                    if let Some(symbol) = scope.lookup(ident) {
                        match symbol {
                            Symbol::Column(column) => Ok(column.data_type.clone()),
                            Symbol::Alias(aliased_expr) => {
                                let expr_clone = aliased_expr.clone();
                                self.analyze_expr(&expr_clone, scope).await
                            },
                            _ => Err(anyhow!("Identifier '{}' is not a column or alias", ident)),
                        }
                    } else {
                        // Try to find in any table if only one table is in scope or column is unambiguous
                        let mut found_column: Option<Column> = None;
                        let ambiguous = false;

                        // Need to iterate through all tables in scope to find the column
                        // This is a simplified approach - the scope should provide a proper API
                        if let Some(Symbol::Table { columns, .. }) = scope.lookup(ident) {
                            if let Some(column) = columns.get(ident) {
                                found_column = Some(column.clone());
                            }
                        }

                        if ambiguous {
                            Err(anyhow!("Ambiguous column reference: '{}'. Please qualify with table name.", ident))
                        } else if let Some(column) = found_column {
                            scope.insert(ident, Symbol::Column(column.clone()));
                            Ok(column.data_type.clone())
                        } else {
                            Err(anyhow!("Identifier not found: {}", ident))
                        }
                    }
                } else {
                    Err(anyhow!("Invalid identifier format: {}", ident))
                }
            }
            Expr::Literal(literal) => self.analyze_literal(literal),
            Expr::Binary { left, op, right } => {
                let left_type = self.analyze_expr(left, scope).await?;
                let right_type = self.analyze_expr(right, scope).await?;
                self.analyze_binary_op(op, left_type, right_type)
            }
            Expr::Unary { op, expr } => {
                let expr_type = self.analyze_expr(expr, scope).await?;
                self.analyze_unary_op(op, expr_type)
            }
            Expr::FuncCall { name, args } => {
                let mut arg_types = Vec::new();
                for arg in args {
                    arg_types.push(self.analyze_expr(arg, scope).await?);
                }
                self.analyze_function_call(name, arg_types)
            }
            Expr::Case {
                operand,
                conditions,
                else_expr,
            } => {
                let mut result_type = DataType::Unknown;
                if let Some(op) = operand {
                    self.analyze_expr(op, scope).await?;
                }
                for (when_expr, then_expr) in conditions {
                    let when_type = self.analyze_expr(when_expr, scope).await?;
                    if when_type != DataType::Boolean {
                        return Err(anyhow!("CASE WHEN clause must be boolean, found {:?}", when_type));
                    }
                    let then_type = self.analyze_expr(then_expr, scope).await?;
                    if result_type == DataType::Unknown {
                        result_type = then_type;
                    } else if result_type != then_type {
                        // TODO: Implement type coercion rules
                        return Err(anyhow!("CASE THEN clauses must have compatible types"));
                    }
                }
                if let Some(el) = else_expr {
                    let else_type = self.analyze_expr(el, scope).await?;
                    if result_type == DataType::Unknown {
                        result_type = else_type;
                    } else if result_type != else_type {
                        // TODO: Implement type coercion rules
                        return Err(anyhow!("CASE ELSE clause must have compatible type"));
                    }
                }
                Ok(result_type)
            }
            Expr::Subquery(subquery) => {
                let mut subquery_scope = Scope::new_with_parent(scope.clone());
                self.analyze_query(subquery, &mut subquery_scope).await?;
                // TODO: Determine the return type of the subquery (e.g., single column, single row)
                // For now, assume it returns a single value of unknown type.
                // A more robust implementation would analyze the subquery's projection.
                Ok(DataType::Unknown) // Placeholder
            }
            Expr::SksSimilar { field, query, metric, threshold } => {
                // Semantic analysis for SKS_SIMILAR
                // field is a String (field name), query is an Expr
                let query_type = self.analyze_expr(query, scope).await?;

                // TODO: Look up field type from scope/schema
                // For now, assume vector field type
                let field_type = DataType::Vector(1536); // Default assumption
                if !matches!(query_type, DataType::Vector(_)) && !matches!(query_type, DataType::String) {
                    return Err(anyhow!("SIMILAR query must be a vector or string literal, found {:?}", query_type));
                }
                // TODO: Validate metric and threshold types
                Ok(DataType::Float64) // Returns a similarity score
            }
            Expr::SksFollow { start, edge, max_depth } => {
                // Semantic analysis for SKS_FOLLOW
                let start_type = self.analyze_expr(start, scope).await?;
                if !matches!(start_type, DataType::String) && !matches!(start_type, DataType::Int64) {
                    return Err(anyhow!("FOLLOW start node must be a string or integer ID, found {:?}", start_type));
                }
                // TODO: Validate edge type and max_depth
                Ok(DataType::Unknown) // Returns graph traversal results
            }
            Expr::SksAssemble { context_items, strategy, max_size } => {
                // Semantic analysis for SKS_ASSEMBLE
                for item in context_items {
                    self.analyze_expr(item, scope).await?;
                }
                // TODO: Validate strategy and max_size
                Ok(DataType::String) // Returns assembled text
            }
            _ => Err(anyhow!("Unsupported expression: {:?}", expr)),
        }
        })
    }

    fn analyze_literal(&self, literal: &Literal) -> Result<DataType> {
        match literal {
            Literal::String(_) => Ok(DataType::String),
            Literal::Number(_) => Ok(DataType::Float64), // Treat all numbers as float64 for now
            Literal::Bool(_) => Ok(DataType::Boolean),
            Literal::Null => Ok(DataType::Unknown),
        }
    }

    fn analyze_binary_op(&self, op: &BinaryOp, left: DataType, right: DataType) -> Result<DataType> {
        match op {
            BinaryOp::Eq | BinaryOp::Ne => {
                // Equality can compare compatible types
                if left == right || left == DataType::Unknown || right == DataType::Unknown {
                    Ok(DataType::Boolean)
                } else {
                    Err(anyhow!("Type mismatch in equality operation: {:?} vs {:?}", left, right))
                }
            }
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                // Comparison operators require numeric types
                if (left == DataType::Int64 || left == DataType::Float64) && 
                   (right == DataType::Int64 || right == DataType::Float64) {
                    Ok(DataType::Boolean)
                } else {
                    Err(anyhow!("Comparison operations require numeric operands: {:?} vs {:?}", left, right))
                }
            }
            BinaryOp::And | BinaryOp::Or => {
                // Logical operators require boolean types
                if left == DataType::Boolean && right == DataType::Boolean {
                    Ok(DataType::Boolean)
                } else {
                    Err(anyhow!("Logical operations require boolean operands"))
                }
            }
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div => {
                // Arithmetic operators require numeric types and return numeric
                if (left == DataType::Int64 || left == DataType::Float64) && 
                   (right == DataType::Int64 || right == DataType::Float64) {
                    // Promote to Float64 if either is Float64
                    if left == DataType::Float64 || right == DataType::Float64 {
                        Ok(DataType::Float64)
                    } else {
                        Ok(DataType::Int64)
                    }
                } else {
                    Err(anyhow!("Arithmetic operations require numeric operands: {:?} vs {:?}", left, right))
                }
            }
            _ => Err(anyhow!("Unsupported binary operator: {:?}", op)),
        }
    }

    fn analyze_unary_op(&self, op: &UnaryOp, expr_type: DataType) -> Result<DataType> {
        match op {
            UnaryOp::Not => {
                if expr_type == DataType::Boolean {
                    Ok(DataType::Boolean)
                } else {
                    Err(anyhow!("NOT operator requires a boolean operand"))
                }
            }
            UnaryOp::Neg => {
                if expr_type == DataType::Float64 || expr_type == DataType::Int64 {
                    Ok(expr_type)
                } else {
                    Err(anyhow!("Negation operator requires a numeric operand"))
                }
            }
        }
    }

    fn analyze_function_call(&self, name: &str, args: Vec<DataType>) -> Result<DataType> {
        // This is a placeholder for a proper function registry.
        match name.to_lowercase().as_str() {
            "cosine_distance" | "vector_similarity" => {
                if args.len() == 2 && matches!(args[0], DataType::Vector(_)) && matches!(args[1], DataType::Vector(_)) {
                    Ok(DataType::Float64)
                } else {
                    Err(anyhow!("Invalid arguments for vector similarity function. Expected (Vector, Vector)"))
                }
            }
            "count" => Ok(DataType::Int64),
            "sum" | "avg" | "min" | "max" => {
                if args.len() == 1 && (matches!(args[0], DataType::Int64) || matches!(args[0], DataType::Float64)) {
                    Ok(DataType::Float64) // Aggregates often return float
                } else {
                    Err(anyhow!("Invalid arguments for aggregate function. Expected (Numeric)"))
                }
            }
            _ => Err(anyhow!("Unknown function: {}", name)),
        }
    }
}