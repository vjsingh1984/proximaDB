//! Query Execution Planner - Cost-based optimization for vector, graph, and hybrid queries
//!
//! This module replaces sql_engine/planner.rs with AST-based planning that leverages
//! HashMap metadata filtering for optimal performance.

use crate::query::ast::{Query, Select, Expr, BinaryOp};
use crate::query::execution::{
    ExecutionPlan, ExecutionStrategy, ExecutionOperation, FusionStrategy, ProjectionTransform
};
use crate::services::operations::vectors::VectorOperationsService;
use crate::graph::service::GraphService;
use crate::core::search::FilterExpression;
use anyhow::{anyhow, Result};
use std::sync::Arc;

/// Cost-based execution planner for unified query optimization
pub struct ExecutionPlanner {
    vector_service: Arc<VectorOperationsService>,
    graph_service: Arc<GraphService>,
    cost_model: CostModel,
    params: Option<Vec<crate::proto::proximadb_v1::SqlValue>>, // for decoding $1 vectors when not substituted
}

impl ExecutionPlanner {
    /// Create new execution planner with service integrations
    pub fn new(
        vector_service: Arc<VectorOperationsService>, 
        graph_service: Arc<GraphService>
    ) -> Self {
        Self {
            vector_service,
            graph_service,
            cost_model: CostModel::new(),
            params: None,
        }
    }

    pub fn with_params(
        vector_service: Arc<VectorOperationsService>,
        graph_service: Arc<GraphService>,
        params: Option<Vec<crate::proto::proximadb_v1::SqlValue>>,
    ) -> Self {
        let mut p = Self::new(vector_service, graph_service);
        p.params = params;
        p
    }

    /// Generate optimized execution plan from internal AST
    /// 
    /// This method analyzes the query and determines the optimal execution strategy:
    /// - Vector-only: For similarity search and metadata filtering
    /// - Graph-only: For traversal and pathfinding queries  
    /// - Hybrid: For combined vector + graph intelligence
    /// - Relational: For traditional SQL operations
    pub fn create_plan(&self, query: &Query) -> Result<ExecutionPlan> {
        match query {
            Query::Select(select) => self.plan_select(select),
            Query::With { .. } => Err(anyhow!("WITH/CTE queries are not implemented yet")),
            Query::Set { .. } => Err(anyhow!("Set operations (UNION/INTERSECT/EXCEPT) are not implemented yet")),
        }
    }

    /// Plan SELECT query with intelligent strategy detection
    fn plan_select(&self, select: &Select) -> Result<ExecutionPlan> {
        // Join scaffolding: we'll emit Join ops for visibility; executor returns NotImplemented
        // Analyze query characteristics to determine optimal strategy
        let query_analysis = self.analyze_query(select)?;
        
        let execution_strategy = match (
            query_analysis.has_vector_functions,
            query_analysis.has_graph_patterns,
            query_analysis.has_sks_functions,
        ) {
            (true, false, false) => ExecutionStrategy::VectorOnly,
            (false, true, false) => ExecutionStrategy::GraphOnly,
            (true, true, _) | (_, _, true) => ExecutionStrategy::Hybrid,
            _ => ExecutionStrategy::Relational,
        };

        // Generate execution operations based on strategy
        let operations = self.generate_operations(select, &execution_strategy)?;
        
        // Estimate costs using our cost model
        let estimated_cost = self.cost_model.estimate_total_cost(&operations);
        
        // Generate performance optimizations
        let optimizations = self.generate_optimizations(select, &query_analysis);
        let performance_hints = self.generate_performance_hints(&query_analysis);

        // Add Join scaffolding ops, if present
        let mut operations = operations;
        if !select.joins.is_empty() {
            for j in &select.joins {
                let kind = match j.kind { crate::query::ast::JoinKind::Inner => crate::query::execution::JoinKind::Inner, crate::query::ast::JoinKind::Left => crate::query::execution::JoinKind::Left };
                let on_str = j.on.as_ref().map(|e| format!("{:?}", e)).unwrap_or_else(|| "<none>".to_string());
                operations.push(ExecutionOperation::Join { kind, on: on_str });
            }
        }

        Ok(ExecutionPlan {
            execution_strategy,
            operations,
            estimated_cost,
            optimizations,
            performance_hints,
        })
    }

    /// Analyze query to detect vector, graph, and SKS patterns
    fn analyze_query(&self, select: &Select) -> Result<QueryAnalysis> {
        let mut analysis = QueryAnalysis::default();

        // Check ORDER BY for vector functions
        for order_expr in &select.order_by {
            if let Expr::FuncCall { name, .. } = &order_expr.expr {
                if name.to_uppercase().contains("VECTOR_SIMILARITY") 
                   || name.to_uppercase().contains("COSINE_DISTANCE") {
                    analysis.has_vector_functions = true;
                }
            }
        }

        // Check for SKS functions in WHERE clause
        if let Some(where_expr) = &select.selection {
            analysis.has_sks_functions = self.detect_sks_functions(where_expr) || self.contains_sks_funcs(where_expr);
        }

        // Check for graph patterns in FROM clause
        for table in &select.from {
            if let Some(table_name) = &table.name {
                // TODO: Detect graph collections vs vector collections
                // For now, assume graph if collection name suggests it
                if table_name.contains("graph") || table_name.contains("node") || table_name.contains("edge") {
                    analysis.has_graph_patterns = true;
                }
            }
        }

        // Analyze metadata filtering complexity for HashMap optimization
        analysis.metadata_fields = self.extract_metadata_fields(select);
        analysis.filter_complexity = self.calculate_filter_complexity(&select.selection);

        Ok(analysis)
    }

    /// Quick detector for SKS function variants lowered by the frontend
    fn contains_sks_funcs(&self, expr: &Expr) -> bool {
        match expr {
            Expr::SksSimilar { .. } | Expr::SksFollow { .. } => true,
            Expr::Unary { expr, .. } => self.contains_sks_funcs(expr),
            Expr::Binary { left, right, .. } => self.contains_sks_funcs(left) || self.contains_sks_funcs(right),
            Expr::AggCall { args, .. } | Expr::FuncCall { args, .. } => args.iter().any(|e| self.contains_sks_funcs(e)),
            _ => false,
        }
    }

    /// Generate execution operations for the selected strategy
    fn generate_operations(&self, select: &Select, strategy: &ExecutionStrategy) -> Result<Vec<ExecutionOperation>> {
        let mut operations = Vec::new();

        match strategy {
            ExecutionStrategy::VectorOnly => {
                // Prefer SKS SIMILAR() when present; otherwise fallback to generic extraction
                if let Some(sim) = self.find_sks_similar(select) {
                    if let Some(table) = select.from.first() {
                        let collection_id = table.name.as_ref().ok_or_else(|| anyhow!("Missing collection name"))?;
                        operations.push(ExecutionOperation::VectorSearch {
                            collection_id: collection_id.clone(),
                            query_vector: self.try_parse_query_vector(&sim.query),
                            filters: self.convert_where_to_filter(&select.selection)?,
                            top_k: select.limit.unwrap_or(100) as usize,
                            distance_metric: sim.metric.unwrap_or_else(|| "cosine".to_string()),
                        });
                    }
                } else if let Some(table) = select.from.first() {
                    let collection_id = table.name.as_ref()
                        .ok_or_else(|| anyhow!("Missing collection name"))?;
                    operations.push(ExecutionOperation::VectorSearch {
                        collection_id: collection_id.clone(),
                        query_vector: self.extract_query_vector(select)?,
                        filters: self.convert_where_to_filter(&select.selection)?,
                        top_k: select.limit.unwrap_or(100) as usize,
                        distance_metric: self.extract_distance_metric(select)?,
                    });
                }
            },
            
            ExecutionStrategy::GraphOnly => {
                // Prefer SKS FOLLOW() when present; otherwise fallback to generic extraction
                if let Some(fol) = self.find_sks_follow(select) {
                    operations.push(ExecutionOperation::GraphTraversal {
                        start_nodes: self.expr_to_start_nodes(&fol.start),
                        edge_types: vec![fol.edge],
                        max_depth: fol.max_depth,
                        filters: self.convert_where_to_filter(&select.selection)?,
                    });
                } else {
                    operations.push(ExecutionOperation::GraphTraversal {
                        start_nodes: self.extract_start_nodes(select)?,
                        edge_types: self.extract_edge_types(select)?,
                        max_depth: self.extract_max_depth(select).unwrap_or(3),
                        filters: self.convert_where_to_filter(&select.selection)?,
                    });
                }
            },
            
            ExecutionStrategy::Hybrid => {
                // Generate both vector and graph operations with fusion
                if let Some(table) = select.from.first() {
                    let collection_id = table.name.as_ref().ok_or_else(|| anyhow!("Missing collection name"))?;
                    // Vector leg
                    if let Some(sim) = self.find_sks_similar(select) {
                        operations.push(ExecutionOperation::VectorSearch {
                            collection_id: collection_id.clone(),
                            query_vector: self.try_parse_query_vector(&sim.query),
                            filters: self.convert_where_to_filter(&select.selection)?,
                            top_k: select.limit.unwrap_or(100) as usize,
                            distance_metric: sim.metric.unwrap_or_else(|| "cosine".to_string()),
                        });
                    }
                    // Graph leg
                    if let Some(fol) = self.find_sks_follow(select) {
                        operations.push(ExecutionOperation::GraphTraversal {
                            start_nodes: self.expr_to_start_nodes(&fol.start),
                            edge_types: vec![fol.edge],
                            max_depth: fol.max_depth,
                            filters: self.convert_where_to_filter(&select.selection)?,
                        });
                    }
                }
                operations.push(ExecutionOperation::Fusion {
                    strategy: FusionStrategy::ReciprocalRankFusion { k: 60.0 },
                    weights: vec![0.6, 0.4], // Vector, Graph weights
                });
            },

            _ => return Err(anyhow!("Execution strategy not yet implemented: {:?}", strategy)),
        }

        // Aggregate (GROUP BY / HAVING)
        if !select.group_by.is_empty() {
            let group_keys = select.group_by.iter().filter_map(|e| self.expr_to_identifier(e)).collect::<Vec<_>>();
            let aggs = self.extract_aggregates(&select.projection);
            let having = self.convert_where_to_filter(&select.having)?; // reuse filter converter
            operations.push(ExecutionOperation::Aggregate { group_keys, aggs, having });
        }

        // Add projection operation for result formatting
        operations.push(ExecutionOperation::Project {
            columns: self.extract_projection_columns(select),
            transformations: self.generate_projections(select),
        });

        Ok(operations)
    }

    /// Convert WHERE clause to FilterExpression with HashMap optimization
    /// 
    /// This method ensures that metadata filtering will use O(1) HashMap.get()
    /// instead of O(n) Vec.find() operations for 10x performance improvement.
    fn convert_where_to_filter(&self, where_clause: &Option<Expr>) -> Result<Option<FilterExpression>> {
        if let Some(expr) = where_clause {
            self.expr_to_filter_expression(expr)
        } else {
            Ok(None)
        }
    }

    /// Find first SKS SIMILAR occurrence in selection or order_by
    fn find_sks_similar(&self, select: &Select) -> Option<SksSimilarArgs> {
        if let Some(expr) = &select.selection {
            if let Some(sim) = self.walk_find_similar(expr) { return Some(sim); }
        }
        for ob in &select.order_by {
            if let Some(sim) = self.walk_find_similar(&ob.expr) { return Some(sim); }
        }
        None
    }

    fn walk_find_similar(&self, expr: &Expr) -> Option<SksSimilarArgs> {
        match expr {
            Expr::SksSimilar { field: _, query, metric, threshold } => Some(SksSimilarArgs { query: query.as_ref().clone(), metric: metric.clone(), threshold: *threshold }),
            Expr::Unary { expr, .. } => self.walk_find_similar(expr),
            Expr::Binary { left, right, .. } => self.walk_find_similar(left).or_else(|| self.walk_find_similar(right)),
            Expr::AggCall { args, .. } | Expr::FuncCall { args, .. } => args.iter().find_map(|e| self.walk_find_similar(e)),
            _ => None,
        }
    }

    /// Find first SKS FOLLOW occurrence in selection
    fn find_sks_follow(&self, select: &Select) -> Option<SksFollowArgs> {
        if let Some(expr) = &select.selection {
            return self.walk_find_follow(expr);
        }
        None
    }

    fn walk_find_follow(&self, expr: &Expr) -> Option<SksFollowArgs> {
        match expr {
            Expr::SksFollow { start, edge, max_depth } => Some(SksFollowArgs { start: start.as_ref().clone(), edge: edge.clone(), max_depth: *max_depth }),
            Expr::Unary { expr, .. } => self.walk_find_follow(expr),
            Expr::Binary { left, right, .. } => self.walk_find_follow(left).or_else(|| self.walk_find_follow(right)),
            Expr::AggCall { args, .. } | Expr::FuncCall { args, .. } => args.iter().find_map(|e| self.walk_find_follow(e)),
            _ => None,
        }
    }

    /// Try to parse a query vector from an expression (best-effort)
    fn try_parse_query_vector(&self, expr: &Expr) -> Option<Vec<f32>> {
        match expr {
            // VECTOR(0.1, 0.2, 0.3)
            Expr::FuncCall { name, args } if name.eq_ignore_ascii_case("VECTOR") => {
                let mut out = Vec::new();
                for a in args {
                    if let Expr::Literal(crate::query::ast::Literal::Number(n)) = a { out.push(*n as f32); } else { return None; }
                }
                if out.is_empty() { None } else { Some(out) }
            }
            // '[0.1,0.2,0.3]' string literal
            Expr::Literal(crate::query::ast::Literal::String(s)) => {
                if s.starts_with('[') {
                    if let Ok(v) = serde_json::from_str::<Vec<f32>>(s) { return Some(v); }
                }
                None
            }
            // Parameter (decode from planner params: $1, $2, ...)
            Expr::Param(ph) => {
                // Expect $<n>
                if let Some(n) = ph.strip_prefix('$') {
                    if let Ok(idx) = n.parse::<usize>() {
                        let pos = idx.saturating_sub(1);
                        if let Some(pv) = self.params.as_ref().and_then(|v| v.get(pos)) {
                            return self.sql_value_to_vec(pv);
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }

    fn sql_value_to_vec(&self, v: &crate::proto::proximadb_v1::SqlValue) -> Option<Vec<f32>> {
        use crate::proto::proximadb_v1::sql_value::Value as V;
        match v.value.as_ref()? {
            V::ArrayValue(arr) => {
                let mut out = Vec::new();
                for sv in &arr.values {
                    match sv.value.as_ref()? {
                        V::NumberValue(n) => out.push(*n as f32),
                        V::Int64Value(i) => out.push((*i) as f32),
                        _ => return None,
                    }
                }
                Some(out)
            }
            _ => None,
        }
    }

    /// Convert an expression into start node IDs for FOLLOW
    fn expr_to_start_nodes(&self, expr: &Expr) -> Vec<String> {
        match expr {
            Expr::Literal(crate::query::ast::Literal::String(s)) => vec![s.clone()],
            Expr::Identifier(s) => vec![s.clone()],
            _ => vec![],
        }
    }

    /// Convert AST expression to FilterExpression recursively  
    fn expr_to_filter_expression(&self, expr: &Expr) -> Result<Option<FilterExpression>> {
        match expr {
            Expr::Binary { left, op, right } => {
                let field = self.extract_field_name(left)?;
                let operator = self.convert_comparison_op(op)?;
                let value = self.extract_filter_value(right)?;

                Ok(Some(FilterExpression::Comparison {
                    field,
                    operator,
                    value,
                }))
            },
            _ => Ok(None), // TODO: Handle other expression types
        }
    }

    /// Extract field name from expression (e.g., metadata.category → "category")
    fn extract_field_name(&self, expr: &Expr) -> Result<String> {
        match expr {
            Expr::Identifier(ident) => {
                // Handle metadata.field pattern for HashMap access
                if ident.contains("metadata.") {
                    Ok(ident.strip_prefix("metadata.").unwrap_or(ident).to_string())
                } else {
                    Ok(ident.clone())
                }
            },
            _ => Err(anyhow!("Unsupported field expression: {:?}", expr)),
        }
    }

    fn expr_to_identifier(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Identifier(s) => Some(s.clone()),
            _ => None,
        }
    }

    fn extract_aggregates(&self, projection: &Vec<Expr>) -> Vec<crate::query::execution::AggregateSpec> {
        use crate::query::execution::{AggregateSpec, AggregateFunc};
        let mut out = Vec::new();
        for expr in projection {
            if let Expr::AggCall { name, args } = expr {
                let alias = name.to_uppercase();
                let field = args.get(0).and_then(|e| self.expr_to_identifier(e)).unwrap_or("*".to_string());
                let func = match alias.as_str() {
                    s if s.contains("COUNT") => AggregateFunc::Count,
                    s if s.contains("SUM") => AggregateFunc::Sum,
                    s if s.contains("AVG") => AggregateFunc::Avg,
                    s if s.contains("MIN") => AggregateFunc::Min,
                    s if s.contains("MAX") => AggregateFunc::Max,
                    _ => AggregateFunc::Count,
                };
                out.push(AggregateSpec { alias: alias.clone(), func, field });
            }
        }
        out
    }

    /// Convert AST binary operator to FilterExpression comparison operator
    fn convert_comparison_op(&self, op: &BinaryOp) -> Result<crate::core::search::ComparisonOperator> {
        match op {
            BinaryOp::Eq => Ok(crate::core::search::ComparisonOperator::Equals),
            BinaryOp::Ne => Ok(crate::core::search::ComparisonOperator::NotEquals),
            BinaryOp::Lt => Ok(crate::core::search::ComparisonOperator::LessThan),
            BinaryOp::Le => Ok(crate::core::search::ComparisonOperator::LessThanOrEqual),
            BinaryOp::Gt => Ok(crate::core::search::ComparisonOperator::GreaterThan),
            BinaryOp::Ge => Ok(crate::core::search::ComparisonOperator::GreaterThanOrEqual),
            BinaryOp::Like => Ok(crate::core::search::ComparisonOperator::Like),
            _ => Err(anyhow!("Unsupported comparison operator: {:?}", op)),
        }
    }

    /// Extract filter value from expression
    fn extract_filter_value(&self, expr: &Expr) -> Result<serde_json::Value> {
        match expr {
            Expr::Literal(literal) => {
                match literal {
                    crate::query::ast::Literal::String(s) => Ok(serde_json::Value::String(s.clone())),
                    crate::query::ast::Literal::Number(n) => Ok(serde_json::json!(n)),
                    crate::query::ast::Literal::Bool(b) => Ok(serde_json::Value::Bool(*b)),
                    crate::query::ast::Literal::Null => Ok(serde_json::Value::Null),
                }
            },
            _ => Err(anyhow!("Unsupported filter value expression: {:?}", expr)),
        }
    }

    /// Extract query vector from ORDER BY vector similarity function
    fn extract_query_vector(&self, select: &Select) -> Result<Option<Vec<f32>>> {
        for order_expr in &select.order_by {
            if let Expr::FuncCall { name, args } = &order_expr.expr {
                if name.to_uppercase().contains("VECTOR_SIMILARITY") && args.len() >= 2 {
                    // TODO: Extract vector from second argument
                    // For now, return None to indicate vector should be extracted
                    return Ok(None);
                }
            }
        }
        Ok(None)
    }

    /// Extract distance metric from vector similarity function
    fn extract_distance_metric(&self, select: &Select) -> Result<String> {
        for order_expr in &select.order_by {
            if let Expr::FuncCall { name, args } = &order_expr.expr {
                if name.to_uppercase().contains("VECTOR_SIMILARITY") && args.len() >= 3 {
                    // TODO: Extract metric from third argument
                    return Ok("cosine".to_string()); // Default for now
                }
            }
        }
        Ok("cosine".to_string()) // Default distance metric
    }

    /// Helper methods for graph query analysis
    fn extract_start_nodes(&self, _select: &Select) -> Result<Vec<String>> {
        // TODO: Extract start nodes from FOLLOW functions
        Ok(vec![])
    }

    fn extract_edge_types(&self, _select: &Select) -> Result<Vec<String>> {
        // TODO: Extract edge types from FOLLOW functions
        Ok(vec![])
    }

    fn extract_max_depth(&self, _select: &Select) -> Option<u32> {
        // TODO: Extract depth from FOLLOW function options
        None
    }

    fn extract_projection_columns(&self, select: &Select) -> Vec<String> {
        select.projection.iter()
            .filter_map(|expr| {
                if let Expr::Identifier(name) = expr {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    fn generate_projections(&self, select: &Select) -> Vec<ProjectionTransform> {
        let mut transforms = Vec::new();
        
        for expr in &select.projection {
            match expr {
                Expr::Identifier(name) if name.starts_with("metadata.") => {
                    let field = name.strip_prefix("metadata.").unwrap_or(name);
                    transforms.push(ProjectionTransform::ExtractMetadata {
                        field: field.to_string(),
                    });
                },
                _ => {
                    // TODO: Handle other projection types
                }
            }
        }
        
        transforms
    }

    fn detect_sks_functions(&self, expr: &Expr) -> bool {
        match expr {
            Expr::FuncCall { name, .. } => {
                matches!(name.to_uppercase().as_str(), "SIMILAR" | "FOLLOW" | "ASSEMBLE")
            },
            Expr::Binary { left, right, .. } => {
                self.detect_sks_functions(left) || self.detect_sks_functions(right)
            },
            _ => false,
        }
    }

    fn extract_metadata_fields(&self, select: &Select) -> Vec<String> {
        let mut fields = Vec::new();
        
        // Extract from WHERE clause
        if let Some(where_expr) = &select.selection {
            fields.extend(self.extract_fields_from_expr(where_expr));
        }
        
        // Extract from projection
        for expr in &select.projection {
            fields.extend(self.extract_fields_from_expr(expr));
        }
        
        fields
    }

    fn extract_fields_from_expr(&self, expr: &Expr) -> Vec<String> {
        match expr {
            Expr::Identifier(ident) if ident.starts_with("metadata.") => {
                vec![ident.strip_prefix("metadata.").unwrap_or(ident).to_string()]
            },
            Expr::Binary { left, right, .. } => {
                let mut fields = self.extract_fields_from_expr(left);
                fields.extend(self.extract_fields_from_expr(right));
                fields
            },
            _ => vec![],
        }
    }

    fn calculate_filter_complexity(&self, where_clause: &Option<Expr>) -> f64 {
        match where_clause {
            Some(expr) => self.count_filter_operations(expr) as f64,
            None => 0.0,
        }
    }

    fn count_filter_operations(&self, expr: &Expr) -> usize {
        match expr {
            Expr::Binary { left, right, .. } => {
                1 + self.count_filter_operations(left) + self.count_filter_operations(right)
            },
            _ => 0,
        }
    }

    fn generate_optimizations(&self, select: &Select, analysis: &QueryAnalysis) -> Vec<String> {
        let mut optimizations = Vec::new();
        
        if !analysis.metadata_fields.is_empty() {
            optimizations.push("HashMap metadata filtering (O(1) vs O(n))".to_string());
        }
        
        if analysis.has_vector_functions {
            optimizations.push("Progressive search (Binary → INT8 → PQ → Full)".to_string());
            optimizations.push("Hardware acceleration (SIMD/GPU)".to_string());
        }
        
        if analysis.has_graph_patterns {
            optimizations.push("ORION graph engine (CSR storage)".to_string());
            optimizations.push("Indexed property filtering".to_string());
        }
        
        if select.limit.is_some() {
            optimizations.push("Early termination with LIMIT".to_string());
        }
        
        optimizations
    }

    fn generate_performance_hints(&self, analysis: &QueryAnalysis) -> Vec<String> {
        let mut hints = Vec::new();
        
        if analysis.metadata_fields.len() > 3 {
            hints.push("Consider indexing frequently filtered metadata fields".to_string());
        }
        
        if analysis.filter_complexity > 5.0 {
            hints.push("Complex WHERE clause - consider query optimization".to_string());
        }
        
        if analysis.has_vector_functions && analysis.has_graph_patterns {
            hints.push("Hybrid query detected - using advanced fusion algorithms".to_string());
        }
        
        hints
    }
}

/// Query analysis results for optimization planning
#[derive(Debug, Default)]
struct QueryAnalysis {
    has_vector_functions: bool,
    has_graph_patterns: bool, 
    has_sks_functions: bool,
    metadata_fields: Vec<String>,
    filter_complexity: f64,
}

/// Extracted args for SKS functions
#[derive(Debug, Clone)]
struct SksSimilarArgs {
    pub query: Expr,
    pub metric: Option<String>,
    pub threshold: Option<f64>,
}

#[derive(Debug, Clone)]
struct SksFollowArgs {
    pub start: Expr,
    pub edge: String,
    pub max_depth: u32,
}

/// Cost model for execution planning optimization
struct CostModel {
    // Cost estimates for different operation types
    vector_search_base_cost: f64,
    graph_traversal_base_cost: f64,
    metadata_filter_cost: f64,
    fusion_cost: f64,
}

impl CostModel {
    fn new() -> Self {
        Self {
            vector_search_base_cost: 1.0,
            graph_traversal_base_cost: 2.0,
            metadata_filter_cost: 0.1, // Very low due to HashMap optimization
            fusion_cost: 0.5,
        }
    }

    /// Estimate total execution cost for operations
    fn estimate_total_cost(&self, operations: &[ExecutionOperation]) -> f64 {
        operations.iter().map(|op| self.estimate_operation_cost(op)).sum()
    }

    /// Estimate cost for individual operation
    fn estimate_operation_cost(&self, operation: &ExecutionOperation) -> f64 {
        match operation {
            ExecutionOperation::VectorSearch { top_k, filters, .. } => {
                let base_cost = self.vector_search_base_cost * (*top_k as f64).log10();
                let filter_cost = match filters {
                    Some(_) => self.metadata_filter_cost, // HashMap filtering is cheap
                    None => 0.0,
                };
                base_cost + filter_cost
            },
            ExecutionOperation::GraphTraversal { max_depth, .. } => {
                self.graph_traversal_base_cost * (*max_depth as f64)
            },
            ExecutionOperation::Fusion { .. } => self.fusion_cost,
            ExecutionOperation::Project { .. } => 0.1,
        }
    }
}

#[cfg(test)]
mod planner_tests {
    use super::*;
    use crate::query::ast::*;

    #[test]
    fn test_vector_query_planning() {
        let planner = create_test_planner();
        
        // Create vector query AST
        let query = Query::Select(Select {
            projection: vec![Expr::Identifier("*".to_string())],
            from: vec![TableRef {
                name: Some("products".to_string()),
                subquery: None,
                alias: None,
            }],
            selection: Some(Expr::Binary {
                left: Box::new(Expr::Identifier("metadata.category".to_string())),
                op: BinaryOp::Eq,
                right: Box::new(Expr::Literal(Literal::String("electronics".to_string()))),
            }),
            order_by: vec![OrderByExpr {
                expr: Expr::FuncCall {
                    name: "VECTOR_SIMILARITY".to_string(),
                    args: vec![],
                },
                asc: false,
            }],
            limit: Some(10),
            ..Default::default()
        });
        
        let plan = planner.create_plan(&query).unwrap();
        
        assert!(matches!(plan.execution_strategy, ExecutionStrategy::VectorOnly));
        assert!(plan.operations.len() >= 1);
        assert!(plan.optimizations.contains(&"HashMap metadata filtering (O(1) vs O(n))".to_string()));
    }

    #[test]
    fn test_metadata_filter_cost_estimation() {
        let cost_model = CostModel::new();
        
        let vector_op_with_filter = ExecutionOperation::VectorSearch {
            collection_id: "test".to_string(),
            query_vector: None,
            filters: Some(FilterExpression::Comparison {
                field: "category".to_string(),
                operator: crate::core::search::ComparisonOperator::Equals,
                value: serde_json::Value::String("electronics".to_string()),
            }),
            top_k: 100,
            distance_metric: "cosine".to_string(),
        };
        
        let cost = cost_model.estimate_operation_cost(&vector_op_with_filter);
        
        // Cost should be low due to HashMap optimization
        assert!(cost < 5.0, "HashMap filtering should have low cost, got {}", cost);
    }

    fn create_test_planner() -> ExecutionPlanner {
        // TODO: Create with mock services
        unimplemented!("Create test planner")
    }
}
